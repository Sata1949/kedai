//! 数据根迁移与运行中操作的互斥协议。
//!
//! 请求进入时领取活动租约，后台任务和阻塞工作继承同一租约；最后一个持有者
//! 退出后才归还。迁移只在租约全部归还时开始，提交后旧实例保持维护态直到重启。
use std::future::Future;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Open,
    Moving,
    RestartRequired,
}

struct State {
    phase: Phase,
    active: usize,
}

pub struct DataActivity {
    state: Mutex<State>,
}

impl Default for DataActivity {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                phase: Phase::Open,
                active: 0,
            }),
        }
    }
}

impl DataActivity {
    pub fn enter(self: &Arc<Self>) -> Result<ActivityLease, &'static str> {
        let mut state = self.state.lock().expect("数据迁移活动锁已中毒");
        match state.phase {
            Phase::Moving => return Err("数据位置正在迁移，请等待迁移完成后重启"),
            Phase::RestartRequired => return Err("数据位置迁移已完成，请重启 Kedai 后继续操作"),
            Phase::Open => {}
        }
        state.active += 1;
        Ok(ActivityLease {
            _lease: Arc::new(Lease {
                activity: self.clone(),
            }),
        })
    }

    pub fn begin_move(self: &Arc<Self>) -> Result<DataMove, &'static str> {
        let mut state = self.state.lock().expect("数据迁移活动锁已中毒");
        match state.phase {
            Phase::Moving => return Err("已有数据位置迁移正在执行，请等待完成"),
            Phase::RestartRequired => return Err("数据位置迁移已完成，请先重启 Kedai"),
            Phase::Open => {}
        }
        if state.active > 0 {
            return Err("还有请求或后台操作在执行，请等待结束后再迁移数据位置");
        }
        state.phase = Phase::Moving;
        Ok(DataMove {
            activity: self.clone(),
            committed: false,
        })
    }
}

struct Lease {
    activity: Arc<DataActivity>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.activity
            .state
            .lock()
            .expect("数据迁移活动锁已中毒")
            .active -= 1;
    }
}

/// 克隆只延长同一操作的生命周期，不产生新的活动计数。
#[derive(Clone)]
pub struct ActivityLease {
    _lease: Arc<Lease>,
}

pub struct DataMove {
    activity: Arc<DataActivity>,
    committed: bool,
}

impl DataMove {
    pub fn commit(mut self) {
        self.activity
            .state
            .lock()
            .expect("数据迁移活动锁已中毒")
            .phase = Phase::RestartRequired;
        self.committed = true;
    }
}

impl Drop for DataMove {
    fn drop(&mut self) {
        if !self.committed {
            self.activity
                .state
                .lock()
                .expect("数据迁移活动锁已中毒")
                .phase = Phase::Open;
        }
    }
}

tokio::task_local! { static CURRENT: ActivityLease; }

pub async fn scope<F: Future>(lease: ActivityLease, future: F) -> F::Output {
    CURRENT.scope(lease, future).await
}

/// 在派发处捕获租约，供 JoinSet 等自己管理任务句柄的调用方使用。
pub fn scope_current<F: Future>(future: F) -> impl Future<Output = F::Output> {
    let inherited = CURRENT.try_with(Clone::clone).ok();
    async move {
        match inherited {
            Some(lease) => scope(lease, future).await,
            None => future.await,
        }
    }
}

pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(scope_current(future))
}

/// 阻塞任务即使其等待者被取消也会继续运行，租约必须随闭包而非等待者存活。
pub fn spawn_blocking<F, R>(work: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let inherited = CURRENT.try_with(Clone::clone).ok();
    tokio::task::spawn_blocking(move || match inherited {
        Some(lease) => CURRENT.sync_scope(lease, work),
        None => work(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_excludes_operations_and_other_migrations() {
        let activity = Arc::new(DataActivity::default());
        let lease = activity.enter().unwrap();
        let child = lease.clone();
        drop(lease);
        assert!(activity.begin_move().is_err());
        drop(child);
        let moving = activity.begin_move().unwrap();
        assert!(activity.enter().is_err());
        assert!(activity.begin_move().is_err());
        drop(moving);
        assert!(activity.enter().is_ok(), "迁移失败后恢复原实例");
        activity.begin_move().unwrap().commit();
        assert!(activity.enter().is_err());
        assert!(activity.begin_move().is_err());
    }

    #[tokio::test]
    async fn detached_child_keeps_source_active_after_request_returns() {
        let activity = Arc::new(DataActivity::default());
        let (release, wait) = tokio::sync::oneshot::channel();
        let (child,) = scope(activity.enter().unwrap(), async move {
            (spawn(async move {
                wait.await.unwrap();
            }),)
        })
        .await;
        assert!(activity.begin_move().is_err(), "请求结束后后台任务仍持租约");
        release.send(()).unwrap();
        child.await.unwrap();
        assert!(activity.begin_move().is_ok());
    }

    #[tokio::test]
    async fn blocking_dispatch_passes_lease_to_async_child() {
        let activity = Arc::new(DataActivity::default());
        let (release, wait) = tokio::sync::oneshot::channel();
        let (child,) = scope(activity.enter().unwrap(), async move {
            (spawn_blocking(move || {
                spawn(async move {
                    wait.await.unwrap();
                })
            })
            .await
            .unwrap(),)
        })
        .await;
        assert!(
            activity.begin_move().is_err(),
            "db_call 返回后派发的任务继续持有租约"
        );
        release.send(()).unwrap();
        child.await.unwrap();
        assert!(activity.begin_move().is_ok());
    }

    #[tokio::test]
    async fn abort_does_not_release_running_blocking_writer() {
        let activity = Arc::new(DataActivity::default());
        let (started, entered) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let lease = activity.enter().unwrap();
        let (blocking,) = scope(lease, async move {
            (spawn_blocking(move || {
                started.send(()).unwrap();
                wait.recv().unwrap();
            }),)
        })
        .await;
        entered.await.unwrap();
        blocking.abort();
        assert!(activity.begin_move().is_err());
        release.send(()).unwrap();
        blocking.await.unwrap();
        assert!(activity.begin_move().is_ok());
    }
}
