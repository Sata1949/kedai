// 截断自愈：翻倍预算重发与成因判定（C-2F-2 自 executor.rs 纯搬运）。
use super::ExecutorResult;
use serde_json::Value;

/// 截断自愈重发的 max_tokens 上限(问题①):与设置页 max_tokens 校验上限
/// (`1..=131072`)同口径。2026-09-15 修正:该封顶原为 8192,低于子任务输出上限的
/// 下限(16384),导致「下限之上的单轮一旦截断,翻倍结果仍被 8192 压回」——
/// `doubled_heal_budget` 见 `min(current*2, cap) <= current` 即判定重发无意义,
/// 子 agent 的自愈被静默放弃。封顶抬到 131072 后与子任务区间自洽。
pub(crate) const TRUNCATION_HEAL_MAX_TOKENS_CAP: u32 = 131_072;

/// 计算自愈重发的 max_tokens(翻倍+封顶);已封顶返回 None(重发无意义,走原错误路径)。
/// 算法收敛在 `utils::retry::doubled_heal_budget`(2026-09-13 批次 4.1 四路合一),
/// 此处仅绑定本路径专属封顶值;是否重发仍由外层 `truncation_heal_cause` 判定。
pub(crate) fn doubled_heal_budget(current: u32) -> Option<u32> {
    crate::utils::retry::doubled_heal_budget(current, TRUNCATION_HEAL_MAX_TOKENS_CAP)
}

/// Ok 形态的截断判定(问题①):finish_reason=length 且该轮产出不可用——
/// 任一 tool_call 的 arguments 非空但非法 JSON(参数被预算切成半截,执行必败),
/// 或空正文且无 tool_call(推理烧光预算,正文为零)。普通文本截断(非空正文、
/// 无/合法工具调用)不算:半截文本也是产出,维持既有「截断仍 done」语义
/// (task_llm_call_finish_reason_marks_length_truncation 锁定)。
/// 空正文但带合法 tool_call 也不算:模型本轮只想调工具,工具调用本身就是产出
/// (heal_cause_ignores_healthy_results 锁定),误触发自愈会白白重发一轮。
pub(crate) fn truncation_heal_cause(res: &ExecutorResult) -> Option<String> {
    if res.finish_reason.as_deref() != Some("length") {
        return None;
    }
    // 先查半截 tool_call(成因更具体:正文为空也可能是预算被工具参数烧光)
    if let Some(bad) = res.tool_calls.iter().find(|c| {
        !c.arguments.trim().is_empty() && serde_json::from_str::<Value>(&c.arguments).is_err()
    }) {
        return Some(format!("工具参数 JSON 截断(工具 \"{}\")", bad.name));
    }
    if res.content.trim().is_empty() && res.tool_calls.is_empty() {
        // 2026-09-14 细化:区分「推理烧光预算」与「输出预算本身不足」。
        // 思考模型(DeepSeek 系等)会把 max_tokens 全花在 reasoning_content 上,
        // 正文为空但推理非空——此时成因是推理挤占,单纯翻倍往往仍需再来一轮
        // (实测 agent 模式两次自愈吃掉 37 秒,占该次请求 54%)。
        // 识别出来才能给出可诊断的提示与更合理的提升幅度。
        if !res.reasoning.trim().is_empty() {
            return Some(format!(
                "推理耗尽输出预算(推理 {} 字符,正文为空)",
                res.reasoning.chars().count()
            ));
        }
        return Some("返回空内容(已达 token 上限)".into());
    }
    None
}

/// 自愈重发的 max_tokens 计算(2026-09-14 推理感知):
/// - 普通截断(输出预算不足):沿用翻倍 + 封顶(既有语义不变);
/// - 推理耗尽预算:按「已消耗推理 + 原预算」给足,使正文有与原预算等宽的空间。
///   实测中该形态下翻倍往往一次不够、要再自愈一轮(每次一个完整 LLM 往返),
///   一次给足可省掉后续轮次。仍受 CAP 封顶,不改变既有上限纪律。
///
/// 返回 None = 已无法再提升(重发无意义,走原错误路径)。
pub(crate) fn heal_budget_for(res: &ExecutorResult, current: u32) -> Option<u32> {
    let doubled = doubled_heal_budget(current)?;
    // 仅当本轮确认是「推理挤占」时做加强提升;其余形态维持翻倍语义。
    let reasoning_exhausted = res.finish_reason.as_deref() == Some("length")
        && res.content.trim().is_empty()
        && res.tool_calls.is_empty()
        && !res.reasoning.trim().is_empty();
    if !reasoning_exhausted {
        return Some(doubled);
    }
    // 推理消耗以 completion_tokens 近似(思考模型该值含推理);
    // 目标 = 已消耗 + 原预算(让正文有与原预算等宽的空间)
    let spent = res.usage.completion_tokens.max(0) as u32;
    let target = spent.saturating_add(current).max(doubled);
    let capped = target.min(TRUNCATION_HEAL_MAX_TOKENS_CAP);
    (capped > current).then_some(capped)
}

/// Err 形态的截断判定(问题①):真实连接器(openai_compatible)在 finish_reason=length
/// 时把半截 tool_call 留到流尾 flush 校验,报「工具 "X" 的 arguments 不是合法 JSON」,
/// finish_reason 随错误丢失,只能按该专属错误文案判定。
/// (跨层文案耦合点:connectors/openai_compatible/sse_parser.rs flush_tool_calls,
/// 该文案变更时此处须同步。)
pub(crate) fn is_truncated_tool_call_error(err: &str) -> bool {
    err.contains("arguments 不是合法 JSON")
}
