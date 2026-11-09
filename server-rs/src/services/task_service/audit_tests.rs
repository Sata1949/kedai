#![cfg(test)]

use crate::api::app_state::AppState;
use crate::models::types::{
    TaskApproveExecMode, TaskFollowupMode, TaskRunMode, TaskStatus, TaskStep,
};
use crate::services::task_core::terminal::TaskTerminal;
use crate::services::task_service::TaskService;
use crate::utils::test_support::TempDataDir;
use std::sync::{Arc, Barrier};

fn service() -> (TempDataDir, Arc<TaskService>) {
    let guard = TempDataDir::new("audit-task");
    let mut config = crate::config::test_config();
    config.data_dir = guard.path().to_path_buf();
    config.task_scratch_dir = guard.join("scratch");
    let state = AppState::new(config).unwrap();
    (guard, state.tasks.clone())
}

fn task(svc: &TaskService) -> String {
    svc.create(
        "并发回归",
        None,
        None,
        TaskRunMode::Solo,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap()
    .id
}

// 当前线程运行时在断言前不 poll 后台执行器，避免 mock 提前完成而允许合法重跑。
#[tokio::test(flavor = "current_thread")]
async fn audit_concurrent_starts_claim_exactly_one_run() {
    let (_guard, svc) = service();
    for operation in ["run", "approve", "followup"] {
        for _ in 0..8 {
            let id = task(&svc);
            match operation {
                "approve" => {
                    svc.set_plan(
                        &id,
                        &[TaskStep {
                            name: "步骤".into(),
                            goal: "测试".into(),
                            ..Default::default()
                        }],
                    );
                    svc.set_status(&id, TaskStatus::Planned);
                }
                "followup" => {
                    svc.set_status(&id, TaskStatus::Done);
                }
                _ => {}
            }
            let barrier = Arc::new(Barrier::new(16));
            let handle = tokio::runtime::Handle::current();
            let successes = std::thread::scope(|scope| {
                let joins: Vec<_> = (0..16)
                    .map(|_| {
                        let svc = svc.clone();
                        let id = id.clone();
                        let barrier = barrier.clone();
                        let handle = handle.clone();
                        scope.spawn(move || {
                            let _entered = handle.enter();
                            barrier.wait();
                            match operation {
                                "run" => svc.run(&id),
                                "approve" => {
                                    svc.approve(&id, None, TaskApproveExecMode::ApprovedPlan)
                                }
                                _ => svc.followup(&id, "继续", TaskFollowupMode::Append),
                            }
                            .is_ok()
                        })
                    })
                    .collect();
                joins
                    .into_iter()
                    .map(|j| usize::from(j.join().unwrap()))
                    .sum::<usize>()
            });
            assert_eq!(successes, 1, "{operation} 只能认领一次");
            if operation == "followup" {
                assert_eq!(svc.followup_count(&id), 1, "失败的认领不得追加用户消息");
            }
            svc.stop(&id);
        }
    }
}

#[test]
fn audit_cancelled_complete_keeps_ended_and_token_semantics() {
    let (_guard, svc) = service();
    for (status, cancel_snapshot) in [
        (TaskStatus::Done, true),
        (TaskStatus::Partial, true),
        (TaskStatus::Done, false),
        (TaskStatus::Partial, false),
    ] {
        let id = task(&svc);
        let (cancel, token) = svc.register_cancel(&id);
        svc.set_result(&id, "停止前成果", TaskStatus::Running);
        assert!(svc.stop(&id));
        assert!(svc.is_current_run(&id, token), "stop 不提前移除 token");
        assert!(*cancel.borrow());
        svc.finalize_terminal(
            &id,
            token,
            TaskTerminal::Complete {
                result: "延迟自检后的结果".into(),
                status,
                error: None,
            },
            cancel_snapshot,
        );
        let stored = svc.get(&id).unwrap();
        assert_eq!(stored.status, TaskStatus::Ended);
        assert_eq!(stored.result, "停止前成果");
        assert!(!svc.is_current_run(&id, token));
        assert!(!svc
            .list_task_messages(&id)
            .iter()
            .any(|m| m.kind == "result"));

        let (_, old) = svc.register_cancel(&id);
        let (_, current) = svc.register_cancel(&id);
        svc.set_status(&id, TaskStatus::Running);
        svc.finalize_terminal(
            &id,
            old,
            TaskTerminal::Complete {
                result: "旧轮".into(),
                status,
                error: None,
            },
            true,
        );
        assert_eq!(svc.get(&id).unwrap().status, TaskStatus::Running);
        assert!(svc.is_current_run(&id, current));
        svc.finalize_terminal(
            &id,
            current,
            TaskTerminal::Complete {
                result: "新轮".into(),
                status,
                error: None,
            },
            false,
        );
        assert_eq!(svc.get(&id).unwrap().status, status);
        assert_eq!(svc.get(&id).unwrap().result, "新轮");
    }
}

#[test]
fn audit_replacing_cancel_signals_old_receiver() {
    let (_guard, svc) = service();
    let id = task(&svc);
    let (old_rx, old) = svc.register_cancel(&id);
    let (new_rx, new) = svc.register_cancel(&id);
    assert!(*old_rx.borrow(), "旧轮必须收到取消，不能只丢弃 sender");
    assert!(!*new_rx.borrow());
    svc.remove_cancel_if(&id, old);
    assert!(svc.is_current_run(&id, new));
}

#[test]
fn runtime_probe_cannot_overwrite_a_new_run_or_a_stopped_run() {
    use crate::services::task_core::TaskStore;
    let (_guard, svc) = service();
    let id = task(&svc);
    let (_old_rx, old) = svc.register_cancel(&id);
    let (_new_rx, new) = svc.register_cancel(&id);
    assert!(TaskStore::set_runtime_issues(
        svc.as_ref(),
        &id,
        new,
        &["新轮".into()]
    ));
    assert!(!TaskStore::set_runtime_issues(
        svc.as_ref(),
        &id,
        old,
        &["旧探针".into()]
    ));
    assert_eq!(svc.runtime_issues(&id), vec!["新轮"]);
    svc.stop(&id);
    assert!(!TaskStore::set_runtime_issues(
        svc.as_ref(),
        &id,
        new,
        &["停止后的探针".into()]
    ));
    assert_eq!(svc.runtime_issues(&id), vec!["新轮"]);
}

#[test]
fn plan_chat_claim_waits_for_lifecycle_before_reading_planned() {
    let (_guard, svc) = service();
    let id = task(&svc);
    svc.set_status(&id, TaskStatus::Planned);
    let (_rx, token) = svc.register_cancel(&id);
    let claim = svc.lifecycle.lock().unwrap();
    let (started, ready) = std::sync::mpsc::channel();
    let (polled, progress) = std::sync::mpsc::channel();
    let worker_svc = svc.clone();
    let worker_id = id.clone();
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let mut revision = Box::pin(worker_svc.plan_chat(&worker_id, "修订计划"));
                started.send(()).unwrap();
                let first = futures::poll!(&mut revision);
                polled.send(()).ok();
                match first {
                    std::task::Poll::Ready(result) => result,
                    std::task::Poll::Pending => revision.await,
                }
            })
    });
    ready.recv().unwrap();
    let entered_while_locked = progress
        .recv_timeout(std::time::Duration::from_secs(1))
        .is_ok();
    // 模拟 approve 已在同一临界区完成状态更新；修订不能拿旧 planned 快照起跑。
    svc.set_status(&id, TaskStatus::Planning);
    let kept_token = svc.is_current_run(&id, token);
    drop(claim);
    let result = worker.join().unwrap();
    assert!(!entered_while_locked, "plan_chat 越过了启动认领临界区");
    assert!(kept_token, "plan_chat 替换了批准轮的取消通道");
    assert!(result.is_err());
    assert_eq!(svc.get(&id).unwrap().status, TaskStatus::Planning);
}
