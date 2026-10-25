// team 预算:主执行池的墙钟预留/份额,以及结构化阶段的输出预算与截断自愈决策。
// 由原 `team.rs` 整段搬来(C-2D,纯搬运)。
use super::{
    TEAM_POST_EXEC_RESERVE_MAX_SECS, TEAM_POST_EXEC_RESERVE_MIN_SECS, TEAM_RETRY_MAX_TOKENS_CAP,
    TEAM_STRUCTURED_MIN_TOKENS,
};
use std::time::{Duration, Instant};

/// 团队主执行阶段给审计/终审/汇总预留的墙钟(SKY-7,2026-10-24;单一出处)。
///
/// 为什么需要:团队总预算由各主共享且此前无预留,2026-10-24 全题复跑里主 2 吃掉
/// 全部 3600s,审计与汇总直接未执行,任务只能 partial。预留让确定性交付检查与
/// 质量闸门至少有一次运行窗口。
///
/// 口径:剩余 <120s 不预留(秒级集成测试与小预算行为不变);否则取 5%,
/// 夹在 `TEAM_POST_EXEC_RESERVE_MIN_SECS..=TEAM_POST_EXEC_RESERVE_MAX_SECS`。
pub(super) fn post_exec_reserve(remaining: Duration) -> Duration {
    let secs = remaining.as_secs();
    if secs < 120 {
        return Duration::ZERO;
    }
    Duration::from_secs((secs / 20).clamp(
        TEAM_POST_EXEC_RESERVE_MIN_SECS,
        TEAM_POST_EXEC_RESERVE_MAX_SECS,
    ))
}

/// 主执行阶段的预算池截止期 = 全局截止期 - 审计预留;无截止期 = `None`(不引入新闸)。
pub(super) fn main_pool_deadline(deadline: Option<Instant>) -> Option<Instant> {
    let d = deadline?;
    let remaining = d.saturating_duration_since(Instant::now());
    let reserve = post_exec_reserve(remaining);
    Some(d.checked_sub(reserve).unwrap_or(d))
}

/// 按「本主实际执行步数 / 剩余未执行步数」公平分配预算池(SKY-7;纯函数)。
///
/// - `pool_remaining` 或 `units` 为 0 → 0(调用方按即时截止处理,不启动新子目标);
/// - `remaining_units == 0` → 全部剩余(防御性兜底,正常调用不会命中);
/// - 池 >0 时至少给 1 秒,但绝不超过池本身(小池按池封顶,避免份额大于总预算)。
pub(super) fn fair_share(
    pool_remaining: Duration,
    units: usize,
    remaining_units: usize,
) -> Duration {
    if pool_remaining.is_zero() || units == 0 {
        return Duration::ZERO;
    }
    if remaining_units == 0 {
        return pool_remaining;
    }
    let pool_nanos = pool_remaining.as_nanos();
    let nanos = (pool_nanos * units as u128 / remaining_units as u128)
        .max(1_000_000_000)
        .min(pool_nanos);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// 审计/终审/汇总的实际输出预算:在用户设置之上保证 `TEAM_STRUCTURED_MIN_TOKENS`
/// 下限(结构化 JSON 与长文本汇总在小预算下必被 reasoning 挤断),并受封顶约束。
pub(super) fn structured_budget(setting: u32) -> u32 {
    // 下限与封顶都是常量,且 MIN <= CAP,故 clamp 不会 panic(见两常量定义处)
    setting.clamp(TEAM_STRUCTURED_MIN_TOKENS, TEAM_RETRY_MAX_TOKENS_CAP)
}

/// 截断自愈预算决策(纯函数):仅 finish_reason=length 触发,预算翻倍、不低于
/// `HEAL_BUDGET_FLOOR` 并封顶。对齐 solo/custom 逐步执行的自愈语义
///(2026-09-03 实测:team 审计/终审/汇总直调 generate_text 无自愈,推理模型
/// reasoning 烧光 1024 预算产出腰斩 JSON;2026-09-15 实录审计 10000 预算被
/// reasoning 吃掉 9894、正文仅 178 字符)。
/// 算法与触发条件收敛在 `utils::retry`(2026-09-13 批次 4.1 四路合一;
/// 2026-09-15 引入下限);本路径单次重发。
pub(super) fn trunc_heal_budget(finish_reason: Option<&str>, max_tokens: u32) -> Option<u32> {
    if finish_reason != Some("length") {
        return None;
    }
    crate::utils::retry::heal_budget_with_floor(
        max_tokens,
        TEAM_RETRY_MAX_TOKENS_CAP,
        crate::utils::retry::HEAL_BUDGET_FLOOR,
    )
}

/// 推理感知的截断自愈预算(纯函数,2026-09-15):在 `trunc_heal_budget` 之上,
/// 若该轮推理已挤占预算(有 reasoning 观测),按「已消耗 + 原预算」给足,让正文有
/// 与原预算等宽的空间,避免翻倍后仍被推理吃掉、再烧一个完整往返。
///
/// 只在这一形态加强:无 reasoning 观测(非思考模型)或推理未挤占时,维持既有翻倍语义。
pub(super) fn trunc_heal_budget_reasoning_aware(
    finish_reason: Option<&str>,
    max_tokens: u32,
    completion_tokens: i64,
    reasoning_tokens: i64,
) -> Option<u32> {
    let base = trunc_heal_budget(finish_reason, max_tokens)?;
    // 无推理观测(0 = 非思考模型或上游未下发)时不加强,保持既有语义
    if reasoning_tokens <= 0 || completion_tokens <= 0 {
        return Some(base);
    }
    // reasoning 与正文共用 completion 预算,推理占满时正文只能腰斩;
    // 目标 = 已消耗 + 原预算(正文至少还有与原预算等宽的空间)
    let target = (completion_tokens.max(0) as u32)
        .saturating_add(max_tokens)
        .min(TEAM_RETRY_MAX_TOKENS_CAP);
    Some(target.max(base))
}
