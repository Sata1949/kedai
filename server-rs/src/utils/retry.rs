// 截断自愈(truncation heal)预算决策的公共实现(2026-09-13 批次 4.1)。
//
// 背景:上游 finish_reason=length(推理 token 吃光预算,JSON/正文腰斩)时,
// 「翻倍预算原样重发一次」在四条路径上各自实现过一遍,算法等价但常量与触发条件
// 抄了四份,任一路径改口径(例如调高封顶)不会同步,容易出现「同一现象四处语义漂移」:
//   1. api/chat.rs           卡片生成 generate-raw(封顶 32768,最多重发 2 次)
//   2. agents/engine/executor.rs 引擎单轮自愈(封顶 8192;触发条件由 truncation_heal_cause 判定)
//   3. services/task_engine/team.rs  team 审计/终审/汇总(封顶 65536,单次)
//   4. services/task_service/executor.rs 步骤/汇总空输出重试(封顶 65536,封顶后仍同预算重试)
// 本模块只提供两个纯函数,各路径保留各自薄封装(常量与场景注释留在原处),行为不变。

/// 预算翻倍并封顶;返回 None 表示「重发无意义」——结果未超过 current 时:
/// 已达/超过上限(cap)、或 current 为 0(0 翻倍不增长,重发只会用同一预算空烧一次)。
///
/// `saturating_mul` 保证 u32::MAX 附近不溢出(先饱和到 MAX 再与 cap 取小)。
pub fn doubled_heal_budget(current: u32, cap: u32) -> Option<u32> {
    let doubled = current.saturating_mul(2).min(cap);
    (doubled > current).then_some(doubled)
}

/// 截断自愈预算:`finish_reason == Some("length")` 且轮次未用尽(rounds < max_rounds)
/// 时按 cap 翻倍,其余情况(非截断原因、reason 缺失、轮次用尽、已封顶)返回 None。
pub fn truncation_heal_budget(
    finish_reason: Option<&str>,
    used: u32,
    rounds: u32,
    cap: u32,
    max_rounds: u32,
) -> Option<u32> {
    if finish_reason == Some("length") && rounds < max_rounds {
        doubled_heal_budget(used, cap)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 常规翻倍:未触及上限时返回两倍值
    #[test]
    fn doubled_grows_below_cap() {
        assert_eq!(doubled_heal_budget(1024, 32768), Some(2048));
        assert_eq!(doubled_heal_budget(4096, 65536), Some(8192));
    }

    /// 精确命中上限:翻倍结果恰好等于 cap 时仍返回 cap(相对 current 仍是增长)
    #[test]
    fn doubled_reaches_cap_exactly() {
        assert_eq!(doubled_heal_budget(16384, 32768), Some(32768));
        // 8192 翻倍即达 8192 封顶(cap 本身不大于 current*2)
        assert_eq!(doubled_heal_budget(4096, 8192), Some(8192));
    }

    /// 已在上限 / 超过上限:结果不再增长 → None(重发无意义)
    #[test]
    fn doubled_none_when_already_capped() {
        assert_eq!(doubled_heal_budget(32768, 32768), None, "已达封顶");
        assert_eq!(doubled_heal_budget(65536, 32768), None, "已超封顶");
    }

    /// 饱和相乘:u32::MAX 附近不得溢出,也不得把「无增长」误判成可重发
    #[test]
    fn doubled_saturates_without_overflow() {
        assert_eq!(doubled_heal_budget(u32::MAX, u32::MAX), None);
        assert_eq!(doubled_heal_budget(u32::MAX, 65536), None);
        assert_eq!(
            doubled_heal_budget(u32::MAX / 2, u32::MAX),
            Some(u32::MAX - 1),
            "饱和相乘不得回绕"
        );
    }

    /// 边界:cap=0 或 current=0 时都无法增长,一律 None
    #[test]
    fn doubled_none_on_zero_boundaries() {
        assert_eq!(doubled_heal_budget(1024, 0), None);
        assert_eq!(doubled_heal_budget(0, 0), None);
        assert_eq!(doubled_heal_budget(0, 32768), None);
    }

    /// 截断自愈:仅 finish_reason=length 触发;轮次用尽后不再重发
    #[test]
    fn truncation_heal_only_on_length_within_rounds() {
        assert_eq!(
            truncation_heal_budget(Some("length"), 8192, 0, 32768, 2),
            Some(16384)
        );
        assert_eq!(
            truncation_heal_budget(Some("length"), 8192, 1, 32768, 2),
            Some(16384)
        );
        assert_eq!(
            truncation_heal_budget(Some("length"), 8192, 2, 32768, 2),
            None,
            "轮次用尽"
        );
        assert_eq!(
            truncation_heal_budget(Some("stop"), 1024, 0, 32768, 2),
            None,
            "非截断原因不重发"
        );
        assert_eq!(
            truncation_heal_budget(None, 1024, 0, 32768, 2),
            None,
            "无 finish_reason 不重发"
        );
    }

    /// 单次重发路径(team):max_rounds=1 时首轮触发,第 1 轮起不再触发
    #[test]
    fn truncation_heal_single_round_limit() {
        assert_eq!(
            truncation_heal_budget(Some("length"), 40000, 0, 65536, 1),
            Some(65536)
        );
        assert_eq!(
            truncation_heal_budget(Some("length"), 40000, 1, 65536, 1),
            None
        );
    }
}
