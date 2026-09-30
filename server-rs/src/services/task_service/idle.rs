// 任务级空闲看守(提交 3 · D7):客户端放弃 ≠ 任务永生。
//
// 背景(2026-09-26 真实模型实测):任务存活只由 `TaskService.cancels` 的 watch 通道决定,
// 与任何客户端连接无关——探针 15 分钟到点退出后任务仍在服务端跑(继续烧 token、与后续
// 轮次抢 provider),得人工 stop;`recover_orphan_tasks` 只在**服务重启**时兜底。
//
// 本模块提供三件事,判据与动作分离以便各自可测:
//   1. `is_idle`(纯函数):状态 × 取消 × 时间 × 阈值的唯一判定;
//   2. `scan_idle_tasks`(pub,单轮扫描,返回被收尾的 id):供看守与测试直调;
//   3. `spawn_idle_watchdog`(pub,常驻 tick):只在 `run_server` 挂载——
//      **刻意不挂 `build_test_app`**,免得每个测试 app 都带一个常驻定时器。
//
// 活动信号 = max(进程内心跳, 库内最近活动):
//   库侧 `task_llm_calls` / `tasks.updated_at` 都**不是**实时信号(工具循环期间不落行,
//   见 `db::running_tasks_activity` 的注释),只服务「没有心跳的任务」(重启前遗留、
//   测试直插行);真正常跑的任务靠心跳。两者取较晚者,宁可放过不可误杀。
use super::*;
use chrono::{DateTime, Utc};

/// 空闲判据(纯函数,便于组合表单测)。
///
/// 四条同时成立才算空闲:
///   - `timeout_secs != 0`(0 = 关,与其它闸门同款开关语义);
///   - 状态是 running/planning(planned = 等用户批准,是**合法静止**,不看守;
///     其余状态已是终态或未开始,不归看守);
///   - 取消信号未发出(已取消的任务由执行器自己收尾,看守不重复动手——幂等);
///   - 静默时长**严格大于**阈值(与轮次上限 `round >= max` 同族:边界值不算到点,
///     故 `timeout_secs = 900` 时「恰好 900 秒」仍放过,第 901 秒才收)。
///
/// `silent_secs` 由调用方按活动信号算(心跳优先、库兜底);负值(未来时间戳:时钟回拨
/// 或手改库)天然落到「不空闲」一侧——不猜、不误杀。
pub(super) fn is_idle(
    status: TaskStatus,
    cancel_signalled: bool,
    silent_secs: i64,
    timeout_secs: u64,
) -> bool {
    if timeout_secs == 0 || cancel_signalled {
        return false;
    }
    if !matches!(status, TaskStatus::Running | TaskStatus::Planning) {
        return false;
    }
    silent_secs > timeout_secs as i64
}

/// 空闲收尾原因文案(单一出处):落 `tasks.error` 与事件 detail。
fn idle_reason(silent_secs: i64, timeout_secs: u64) -> String {
    format!("空闲超时自动收尾:最近 {silent_secs}s 无模型调用或事件(阈值 {timeout_secs}s)")
}

impl TaskService {
    /// 刷新任务活动心跳(提交 3 · D7)。调用点三处:`register_cancel`(run 起点)、
    /// `emit_event` / `emit_llm_call`(全部事件,含工具调用/工具结果/步骤/Finish)。
    /// 单点小锁、无 await,不构成锁竞争面。
    pub(super) fn touch_activity(&self, id: &str) {
        self.activity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), Instant::now());
    }

    /// 当前执行是否已收到取消信号(判据用:已取消的任务不重复收尾)。
    /// 取 watch 发送端的当前值——`watch::Sender::borrow` 即「最后发出的值」。
    pub(super) fn cancel_signalled(&self, id: &str) -> bool {
        self.cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .map(|(tx, _)| *tx.borrow())
            .unwrap_or(false)
    }

    /// 扫描一轮空闲任务并逐一收尾,返回**被收尾**的任务 id(无则空)。
    ///
    /// `timeout_secs` 由调用方给(看守从设置读 `task_idle_timeout_secs`);`0` = 关闭,
    /// 直接返回空。`pub` 是**有意的测试缝**:`tests/` 是外部 crate,`pub(crate)` 不可达
    /// (先例:`TaskService::stop` 也是 pub),集成测试靠它直调短阈值验证机制——
    /// 真机无法把阈值调到秒级(下限按「单条命令上限 + 单次模型调用上限 + 1」派生,
    /// 见 `settings_service::params::task_idle_floor_secs`)。
    pub fn scan_idle_tasks(&self, timeout_secs: u64) -> Vec<String> {
        if timeout_secs == 0 {
            return Vec::new();
        }
        let now = Utc::now();
        let candidates = self.running_tasks_activity();
        // 心跳快照(Instant → 静默秒数):顺带把已不在候选集里的条目裁掉,
        // 表不随任务总数无限增长(任务结束时最后一轮会把它清出去)。
        let heartbeat_ages: HashMap<String, i64> = {
            let mut act = self.activity.lock().unwrap_or_else(|e| e.into_inner());
            act.retain(|id, _| candidates.iter().any(|(c, _)| c == id));
            act.iter()
                .map(|(id, t)| (id.clone(), t.elapsed().as_secs() as i64))
                .collect()
        };
        let mut reaped = Vec::new();
        for (id, db_iso) in &candidates {
            // 权威状态与存在性以最新一行为准(候选集来自上一次查询,可能已过期)
            let Some(task) = self.get(id) else {
                continue;
            };
            if self.cancel_signalled(id) {
                continue;
            }
            // 活动信号:有心跳就用心跳——每次 DB 写入后都会发事件(`record_llm_call`
            // 等),故心跳不早于同一次写入的落库时间,用它即最贴近真实;没有心跳的任务
            // (重启前遗留行、测试直插行)才回退库时间戳。
            let silent_secs = match heartbeat_ages.get(id).copied() {
                Some(secs) => secs,
                None => match db_iso.parse::<DateTime<Utc>>() {
                    Ok(ts) => now.signed_duration_since(ts).num_seconds(),
                    // 时间戳异常(手改库/格式漂移):不猜,放过本轮(下轮再看)
                    Err(_) => continue,
                },
            };
            if !is_idle(task.status, false, silent_secs, timeout_secs) {
                continue;
            }
            let reason = idle_reason(silent_secs, timeout_secs);
            tracing::warn!(
                task_id = id.as_str(),
                status = task.status.as_str(),
                silent_secs,
                timeout_secs,
                "任务空闲超时,自动收尾"
            );
            self.finish_ended(id, Some(&reason));
            reaped.push(id.clone());
        }
        reaped
    }

    /// 常驻看守(提交 3 · D7):每 `tick` 扫一轮,逐轮读设置(`0` = 关,不扫)。
    /// 只在 `run_server` 挂载——测试 app 不挂(见模块头注释)。
    pub fn spawn_idle_watchdog(self: &Arc<Self>, tick: Duration) {
        let svc = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(tick);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let timeout = svc.task_settings().task_idle_timeout_secs as u64;
                if timeout == 0 {
                    continue; // 关:本轮不扫(下一轮重读设置,改设置即生效)
                }
                svc.scan_idle_tasks(timeout);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判据组合表:状态 × 取消 × 阈值 × 边界。
    #[test]
    fn is_idle_covers_status_cancel_and_boundary() {
        // 运行中 + 静默超阈值 + 未取消 = 空闲
        assert!(is_idle(TaskStatus::Running, false, 901, 900));
        assert!(is_idle(TaskStatus::Planning, false, 901, 900));
        // 边界:严格大于才收(恰好等于阈值放过)
        assert!(!is_idle(TaskStatus::Running, false, 900, 900));
        assert!(is_idle(TaskStatus::Running, false, 900, 899));
        // 0 = 关
        assert!(!is_idle(TaskStatus::Running, false, 999_999, 0));
        // 已发出取消信号:执行器自己会收尾,不重复动手
        assert!(!is_idle(TaskStatus::Running, true, 999_999, 900));
        // 非法/无关状态
        for st in [
            TaskStatus::Pending,
            TaskStatus::Planned,
            TaskStatus::Done,
            TaskStatus::Partial,
            TaskStatus::Error,
            TaskStatus::Ended,
        ] {
            assert!(
                !is_idle(st, false, 999_999, 900),
                "{} 不在看守范围",
                st.as_str()
            );
        }
        // 未来时间戳(时钟回拨/手改库)= 负静默:必须放过,不得当成空闲
        assert!(!is_idle(TaskStatus::Running, false, -60, 900));
    }

    /// 原因文案:阈值与实测静默时长都要可见(用户得知道「凭什么给我停了」)。
    #[test]
    fn idle_reason_names_both_numbers() {
        let r = idle_reason(1234, 900);
        assert!(r.contains("空闲超时自动收尾"), "{r}");
        assert!(r.contains("1234") && r.contains("900"), "{r}");
    }

    /// **批次 2 的耦合钉子(把「抬 bash 上限必须同抬看守下限」变成机器可见事实)**:
    /// 一条跑满上限的合法长命令,期间**没有任何事件心跳**
    /// (工具循环不落库、不发事件,见模块头注释与 `touch_activity` 的三处调用点),
    /// 静默时长 = `exec::MAX_TIMEOUT_MS`;按下限阈值判定必须**不算空闲**。
    ///
    /// 为什么单独锁这一条:看守下限此前被硬编码成 601 三份(默认值注释 / PUT 校验文案 /
    /// load 钳制),而它的两个来源(命令上限、模型调用上限)在别的模块——
    /// 只改一处就会让 `cargo test` 级命令在跑到一半时被自动收尾,且现象伪装成「任务超时结束」。
    /// 现在下限由 `task_idle_floor_secs()` 派生,这条断言守住派生关系不被反向改坏。
    #[test]
    fn full_length_command_silence_is_not_idle() {
        use crate::services::exec::MAX_TIMEOUT_MS;
        use crate::services::settings_service::task_idle_floor_secs;
        let command_secs = (MAX_TIMEOUT_MS / 1_000) as i64;
        let floor = task_idle_floor_secs() as i64;
        assert!(
            floor > command_secs,
            "看守下限({floor}s)必须严格大于单条命令上限({command_secs}s),否则跑满上限的合法命令会被误杀"
        );
        assert!(
            !is_idle(TaskStatus::Running, false, command_secs, floor as u64),
            "静默 = 命令上限({command_secs}s)不得被判空闲(阈值 {floor}s)"
        );
        // 下限本身仍要在设置允许区间内(86400 上限,越界的下限会让任何取值都被拒)
        assert!(floor <= 86_400, "看守下限越界: {floor}");
    }
}
