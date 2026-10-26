// 单次调用总时长看门狗与任务模式传输重试（C-2F-2 自 executor.rs 纯搬运）。
use super::super::*;
use super::heal::is_truncated_tool_call_error;

/// 任务模式「单次 LLM 调用」总时长上限(第二次看门狗,2026-09-18)。
///
/// 为什么需要:姊妹路径 `TaskService::generate_text`(规划/步骤/汇总等纯生成)有 300s
/// 总时长看门狗(`task_service/mod.rs` 的 `TASK_LLM_TOTAL_TIMEOUT`),而工具循环
/// (`run_tool_loop` → `execute_generation`)此前只有连接器层的**空闲**看门狗与轮次上限,
/// 没有总时长上限。两个缺口叠加可让任务无限停在 running:
///   ① 引擎侧无总时长;② 空闲判定对「慢速滴流」无效——上游每 <120s 吐几个字节即可
///   每次重置空闲计时,单轮生成拖到无穷。
/// 本轮把任务模式补齐到与纯生成同档(300s/次),超时让任务进 error 终态而非静默卡死。
///
/// 常量与 `task_service` 的同名值口径一致但不跨层 import(L3 引擎不依赖 L2 服务;
/// 两处各自声明、注释互指,改动时须同步)。
///
/// 风险:会中断「极慢但最终能返回」的上游请求。取 300s 而非更小值,是因为推理模型
/// 单轮生成数十秒属正常;与 planner/step 既有 300s 口径一致,不对同类调用双标。
///
/// `pub(crate)` 的用途(2026-09-30 批次 2):任务空闲看守的**合法下限**按
/// 「单条命令上限 + 单次模型调用上限 + 1」推导,这里就是第二项的唯一出处
/// (消费点 `services::settings_service::params::task_idle_floor_secs`)——
/// 此前该下限被硬编码成 601 三份,抬 bash 上限时漏改任何一份都会误杀合法长命令。
pub(crate) const TASK_TOOL_LOOP_CALL_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(300);

/// 总时长看门狗包装:`watchdog=None` 时行为与裸调完全一致;`Some(limit)` 时超过
/// `limit` 未完成即放弃 future 并返回 `timeout_error()`。
///
/// 抽成泛型是为了让单测能用毫秒级阈值覆盖「超时/未超时」两条分支——生产阈值 300s
/// 不可能在测试里真等。
pub(crate) async fn with_call_watchdog<F, T>(
    fut: F,
    watchdog: Option<std::time::Duration>,
    timeout_error: impl FnOnce() -> EngineError,
) -> Result<T, EngineError>
where
    F: std::future::Future<Output = Result<T, EngineError>>,
{
    let Some(limit) = watchdog else {
        return fut.await;
    };
    match tokio::time::timeout(limit, fut).await {
        Ok(r) => r,
        Err(_) => Err(timeout_error()),
    }
}

/// 该会话是否启用工具循环总时长看门狗(任务模式虚拟 session 为 `task:` 前缀)。
///
/// 抽成纯函数以便单测锁死「只有任务模式启用」这条边界:聊天路径的长回复是正常形态,
/// 加总时长上限会把「模型慢慢写长文」误判为失败。
pub(crate) fn call_watchdog_for(session_id: &str) -> Option<std::time::Duration> {
    session_id
        .starts_with("task:")
        .then_some(TASK_TOOL_LOOP_CALL_TIMEOUT)
}

/// 单次调用看门狗的最终取值(A 批 A1):**节点级覆盖优先**,缺省回落上面那条既有判定。
///
/// 抽成纯函数的原因与 `call_watchdog_for` 相同——生产阈值是 300s(或节点配的 30~3600s),
/// 不可能在测试里真等,故把「取哪个值」与「等多久」分开锁:单测锁取值,引擎侧只按值包超时。
/// 覆盖**同时具备收紧与放宽两种用法**(60s 的严节点 / 900s 的慢节点),故不是 `min` 语义。
pub(crate) fn resolve_call_watchdog(
    session_id: &str,
    call_timeout: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    call_timeout.or_else(|| call_watchdog_for(session_id))
}

/// 任务模式单次调用遇到可重试上游失败(超时/限流/流中断)时的最大重试次数(TMR3-1,
/// 2026-10-22;单一出处)。
///
/// 为什么只重试**单次调用**而不是重启整个子目标:2026-10-21 实测主 Agent 2 跑完
/// 234 轮后上游错误,子目标级重启会丢掉全部工具历史并重跑已完成的副作用;单次
/// 重试保留 `llm_messages` 与工具结果,只把「这一次没有返回的调用」再做一遍。
///
/// 为什么上限是 1:任务模式的 300s 看门狗一次已很贵,重试一次最坏多花 300s;
/// 团队/单步墙钟预算与取消信号是外层兜底,不能让上游持续停滞把预算吃光。
pub(crate) const MAX_TRANSPORT_RETRIES: usize = 1;

/// 可重试失败与重试之间的固定退避(TMR3-1):只做一次,无需指数;给上游一个
/// 恢复窗口,同时不让测试等太久(连接器层重试另有 500ms×2^n 的退避)。
pub(crate) const TRANSPORT_RETRY_BACKOFF: std::time::Duration =
    std::time::Duration::from_millis(500);

/// 是否对本次失败发起单次调用重试(TMR3-1 的单一判定出处,纯函数便于单测):
///   - 只有任务模式启用(`call_watchdog.is_some()`;聊天路径的长回复是正常形态);
///   - 只重试已分类为可重试的上游失败(`Timeout`/`RateLimited`/`Upstream`);
///   - 已达到重试上限或用户已取消时不重试。
///
/// 内部错误、截断自愈失败、鉴权/参数类错误都不走这里:截断错误有自己的「翻倍预算」
/// 路径,传输重试只会原样重发同一截断;后两者重试无意义(与 `EngineError::retryable`
/// 的口径一致)。
pub(crate) fn should_retry_transport(
    err: &EngineError,
    call_watchdog: Option<std::time::Duration>,
    retries_used: usize,
    cancelled: bool,
) -> bool {
    !is_truncated_tool_call_error(err.message())
        && err.retryable()
        && call_watchdog.is_some()
        && retries_used < MAX_TRANSPORT_RETRIES
        && !cancelled
}
