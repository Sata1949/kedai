// 上下文裁剪:trim_to_context(按 token 预算丢弃旧消息、极端情况截断 system)
// 与受保护头部长度(protected_head_len,供裁剪保护首条 system 与摘要槽/记忆槽)
// (自 messages.rs 拆分迁入,纯代码移动,逻辑不变)
// 可见性说明:trim_to_context 原 pub(super)(= 对 engine 可见)改为
// pub(in crate::agents::engine),供 messages/mod.rs 以相同可见性再导出,范围不变。
//
// **工具循环历史不在此裁剪(2026-11-09)**:原 R3b 的 `trim_tool_history` 已整块移除。
// 它每轮把「最老完整轮」的 tool 结果与参数原地摘要化——替换点落在历史**中段**,
// 而 prompt cache 靠共同前缀命中,中段一改该点之后全部失效。deepseek-v4.1-flash
// 实测(2026-11-09):带裁剪的请求命中率 33.7%、不带 85.9%;team 任务单会话
// 79 次请求触发 62 次裁剪后,命中率从 97% 掉到 25%。
// 本函数只在聊天初始消息装配时裁剪;工具轮追加的历史不在此裁剪。
use crate::models::types::LlmMessage;

/// 按上下文窗口上限裁剪:始终保留 system(角色设定)与摘要槽(独立 system 消息,
/// 缓存感知管线·改造 A),从最旧的 user/assistant 起丢弃,直到总 token 不超过预算;
/// 极端情况下(仅剩 system 仍超)截断 system 内容,并优先保留尾部注入文本
/// (protected_tail 字符),避免注入先于角色设定被切掉。
/// 返回 `(最终 token 总数, content 字符总数)`:两者在同一遍里算出,
/// 供 `LAST_SEND_TOKENS` / `LAST_SEND_CHARS` 直接消费(ALGO-6,2026-11-05)。
pub(in crate::agents::engine) fn trim_to_context(
    messages: &mut Vec<LlmMessage>,
    max_context: Option<u32>,
    token_service: &mut crate::services::token_service::TokenService,
    model: &str,
    protected_tail: usize,
) -> (i64, usize) {
    let Some(budget) = max_context else {
        return token_service.count_message_tokens_and_chars(messages, model);
    };
    if budget == 0 || messages.len() <= 1 {
        return token_service.count_message_tokens_and_chars(messages, model);
    }
    // 受保护头部:首条 system + 摘要槽(存在时)——裁剪从其后开始
    let head = protected_head_len(messages);
    let (mut total, mut chars): (i64, usize) =
        token_service.count_message_tokens_and_chars(messages, model);
    // 从最旧消息(head 起)丢弃,直到不超预算或仅剩受保护头部。
    // 先算出待删区间再一次 drain:旧实现逐条 remove,每删一条都要前移 O(n)。
    let mut end = head;
    while total > budget as i64 && end < messages.len() {
        // 与 count_message_tokens 同口径(含 tool_calls 的参数成本,2026-09-14 修复)
        total -= token_service.count_single_message_tokens(&messages[end], model);
        chars -= messages[end].content.chars().count();
        end += 1;
    }
    if end > head {
        messages.drain(head..end);
    }
    // 仍超预算(极长角色设定):按预算截断 system,保留尾部注入块
    if total > budget as i64 && messages.len() == head && head > 0 {
        let sys = &mut messages[0];
        let text = sys.content.clone();
        // 字符预算估计(2026-09-13 批次 3 修正量纲混用):此前写成 `budget * 0.8` 直接把
        // **token 数当字符数**用,对中文(约 1.5 字符/token)属过度截断。改为
        // 「1 token ≈ 2 字符」的保守估计起步(英文约 4、中文约 1.5,取偏小侧保证不超窗),
        // 再用真实 tokenizer 复测:仍超预算则按比例收缩一次,消除量纲混用。
        let mut n: usize = ((budget as usize).saturating_mul(2)).max(200);
        let probe: String = text.chars().take(n).collect();
        let measured = token_service.count_tokens(&probe, model);
        if measured > budget as i64 {
            let ratio = (budget as f64 / measured as f64).clamp(0.1, 0.95);
            n = ((n as f64 * ratio) as usize).max(200);
        }
        drop(probe);
        let total_chars = text.chars().count();
        let keep_tail = protected_tail.min(total_chars);
        if keep_tail > 0 {
            // 核心运行时契约位于 system 开头，注入/步骤约束位于末尾；两端都必须保留。
            // 即使 protected_tail 大于估算字符预算，也至少保留一段头部契约，宁可少裁一点，
            // 不得生成“只剩不可信素材/尾部要求、系统权限规则消失”的提示词。
            let min_head = (n / 3)
                .clamp(80, 800)
                .min(total_chars.saturating_sub(keep_tail));
            let max_head = n.saturating_sub(keep_tail).max(min_head);
            let head: String = text.chars().take(max_head).collect();
            let tail: String = text.chars().skip(total_chars - keep_tail).collect();
            sys.content = format!("{head}\n\n[上下文裁剪：中间非核心内容已省略]\n\n{tail}");
            tracing::warn!(
                budget = budget,
                protected_chars = keep_tail,
                "上下文超限:系统提示词被截断(保留尾部注入块)"
            );
        } else {
            sys.content = text.chars().take(n).collect();
            tracing::warn!(budget = budget, "上下文超限:系统提示词被截断");
        }
        // 截断后内容变了,返回的 total/chars 必须按新内容重算(仅极端分支付这一次)。
        let (new_total, new_chars) = token_service.count_message_tokens_and_chars(messages, model);
        total = new_total;
        chars = new_chars;
    }
    (total, chars)
}

/// 消息数组中受裁剪保护的头部条数:首条 system(恒保护)+ 开头连续的
/// 槽位 system 消息(摘要槽/记忆槽,存在则保护)。
/// trim_to_context 从其后开始丢弃旧消息,摘要槽与记忆槽不会被裁掉。
pub(super) fn protected_head_len(messages: &[LlmMessage]) -> usize {
    let mut n = usize::from(!messages.is_empty());
    for m in messages.iter().skip(1) {
        if m.role == "system" {
            n += 1;
        } else {
            break;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::engine::messages::inject::{MEMORY_SLOT_MARKER, SUMMARY_SLOT_MARKER};

    /// trim_to_context:超预算时 system 截断必须保留尾部注入块(注入不能先于角色设定被切掉)
    #[test]
    fn trim_keeps_protected_inject_tail() {
        let mut ts = crate::services::token_service::TokenService::new();
        // 超长角色设定 + 尾部注入文本(简单模式「请将回复控制在 99 字以内。」)
        let long_desc = "长".repeat(2000);
        let inject_text = "\n\n请将回复控制在 99 字以内。".to_string();
        let sys_content = format!(
            "你是角色「测试」的扮演者与文学创作者,与用户进行沉浸式角色扮演 / 文学创作。\n\n角色设定:\n{long_desc}\n\n要求:以\"能否被称为一段好小说\"为最低验收标准。{inject_text}"
        );
        let mut messages = vec![
            LlmMessage::plain("system", &sys_content),
            LlmMessage::plain("user", "你好"),
        ];
        let protected_tail = inject_text.chars().count();
        // 极小预算 → 丢弃历史后仍超 → 截断 system,但尾部注入必须保留
        trim_to_context(
            &mut messages,
            Some(50),
            &mut ts,
            "deepseek-v4-flash",
            protected_tail,
        );
        assert_eq!(messages.len(), 1, "历史消息应被丢弃,仅剩 system");
        assert!(
            messages[0].content.ends_with(inject_text.as_str()),
            "注入文本应保留在 system 尾部,实际: ...{}",
            messages[0]
                .content
                .chars()
                .rev()
                .take(40)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        );
        assert!(
            messages[0].content.chars().count() < sys_content.chars().count(),
            "system 应被截断压缩:原 {} 字符,截后 {} 字符",
            sys_content.chars().count(),
            messages[0].content.chars().count()
        );
    }
    /// trim_to_context:无注入时(protected_tail=0)行为与旧逻辑等价——从头部截断
    #[test]
    fn trim_without_inject_truncates_head() {
        let mut ts = crate::services::token_service::TokenService::new();
        let sys_content = format!(
            "你是角色「测试」,与用户进行沉浸式角色扮演对话。\n\n角色设定:\n{}",
            "长".repeat(2000)
        );
        let mut messages = vec![
            LlmMessage::plain("system", &sys_content),
            LlmMessage::plain("user", "你好"),
        ];
        trim_to_context(&mut messages, Some(50), &mut ts, "deepseek-v4-flash", 0);
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].content.starts_with("你是角色「测试」"),
            "无保护尾部时应从头部截断,保留开头,实际: {}",
            &messages[0].content[..20.min(messages[0].content.len())]
        );
    }

    /// ALGO-3/ALGO-6:裁剪返回的 (tokens, chars) 与全量口径一致
    /// (含头部丢弃与 system 截断两条路径)。
    #[test]
    fn trim_to_context_returns_final_token_total_and_chars() {
        let model = "deepseek-v4-flash";
        let mut ts = crate::services::token_service::TokenService::new();
        let mut messages = loop_messages(4, 10);
        let want = ts.count_message_tokens(&messages, model);
        let want_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        let (got, got_chars) = trim_to_context(&mut messages, Some(10_000), &mut ts, model, 0);
        assert_eq!(got, want, "未裁剪时返回值应等于全量计数");
        assert_eq!(got_chars, want_chars, "未裁剪时字符数应等于全量口径");

        let (got, got_chars) = trim_to_context(&mut messages, Some(5), &mut ts, model, 0);
        let want = ts.count_message_tokens(&messages, model);
        let want_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        assert_eq!(got, want, "裁剪(含 system 截断)后返回值应等于全量计数");
        assert_eq!(
            got_chars, want_chars,
            "裁剪(含 system 截断)后字符数应等于全量口径"
        );
    }

    /// 裁剪黄金测试：同时保留系统契约头、步骤/工具指南尾，丢弃中间非核心内容。
    #[test]
    fn trim_golden_keeps_contract_and_step_tool_tail() {
        let contract = "【核心运行时契约】系统权限规则不可修改。";
        let middle = "角色素材".repeat(3000);
        let tail = "【本步指令】完成当前步骤。\n\n【可用工具】仅按工具定义调用。";
        let mut messages = vec![
            LlmMessage::plain("system", &format!("{contract}\n{middle}\n{tail}")),
            LlmMessage::plain("user", "最新用户消息"),
        ];
        let mut ts = crate::services::token_service::TokenService::new();
        trim_to_context(
            &mut messages,
            Some(80),
            &mut ts,
            "deepseek-v4-flash",
            tail.chars().count(),
        );
        assert!(
            messages[0].content.contains(contract),
            "核心契约必须保留：{}",
            messages[0].content
        );
        assert!(
            messages[0].content.ends_with(tail),
            "步骤与工具指南尾必须保留：{}",
            messages[0].content
        );
        assert!(messages[0].content.contains("中间非核心内容已省略"));
    }

    /// trim_to_context:预算充足时不截断
    #[test]
    fn trim_does_nothing_when_within_budget() {
        let mut ts = crate::services::token_service::TokenService::new();
        let mut messages = vec![
            LlmMessage::plain(
                "system",
                "你是角色「测试」,请回复。\n\n请将回复控制在 99 字以内。",
            ),
            LlmMessage::plain("user", "你好"),
        ];
        let original = messages.clone();
        trim_to_context(
            &mut messages,
            Some(100000),
            &mut ts,
            "deepseek-v4-flash",
            12,
        );
        assert_eq!(messages.len(), original.len());
        assert_eq!(messages[0].content, original[0].content);
        assert_eq!(messages[1].content, original[1].content);
    }

    /// trim_to_context 必须保护摘要槽:裁剪从摘要槽之后开始,不得删掉摘要
    #[test]
    fn trim_never_removes_summary_slot() {
        let mut ts = crate::services::token_service::TokenService::new();
        let mut messages = vec![
            LlmMessage::plain("system", "系统提示"),
            LlmMessage::plain("system", "【早期对话摘要】\n很长的早期剧情摘要。"),
            LlmMessage::plain("user", "较早消息"),
            LlmMessage::plain("assistant", "较早回复"),
            LlmMessage::plain("user", "最新消息"),
        ];
        let head = protected_head_len(&messages);
        assert_eq!(head, 2, "system + 摘要槽都应受保护");
        trim_to_context(&mut messages, Some(30), &mut ts, "deepseek-v4-flash", 0);
        assert!(
            messages.iter().any(|m| m.content.contains("早期剧情摘要")),
            "极小预算下摘要槽也不得被裁掉: {:?}",
            messages
                .iter()
                .map(|m| m.content.chars().take(20).collect::<String>())
                .collect::<Vec<_>>()
        );
    }

    /// trim_to_context 必须保护记忆槽:极小预算下摘要槽与记忆槽都不得被裁掉
    #[test]
    fn trim_never_removes_memory_slot() {
        let mut ts = crate::services::token_service::TokenService::new();
        let mut messages = vec![
            LlmMessage::plain("system", "系统提示"),
            LlmMessage::plain("system", "【早期对话摘要】\n早期剧情摘要。"),
            LlmMessage::plain("system", "【角色长期记忆】\n- 用户害怕打雷"),
            LlmMessage::plain("user", "较早消息"),
            LlmMessage::plain("user", "最新消息"),
        ];
        assert_eq!(
            protected_head_len(&messages),
            3,
            "system+摘要槽+记忆槽都应受保护"
        );
        trim_to_context(&mut messages, Some(30), &mut ts, "deepseek-v4-flash", 0);
        assert!(
            messages
                .iter()
                .any(|m| m.content.starts_with(MEMORY_SLOT_MARKER)),
            "极小预算下记忆槽也不得被裁掉: {:?}",
            messages
                .iter()
                .map(|m| m.content.chars().take(15).collect::<String>())
                .collect::<Vec<_>>()
        );
        assert!(
            messages
                .iter()
                .any(|m| m.content.starts_with(SUMMARY_SLOT_MARKER)),
            "摘要槽保护不受记忆槽影响"
        );
    }

    /// 一轮工具循环消息:assistant(带 tool_calls + reasoning)+ 一条 tool 结果。
    /// 仅 `trim_to_context` 的计量用例用到(工具历史裁剪已于 2026-11-09 移除)。
    fn tool_round(name: &str, call_id: &str, output: &str) -> Vec<LlmMessage> {
        vec![
            LlmMessage {
                role: "assistant".into(),
                content: String::new(),
                reasoning_content: Some(format!("第{call_id}轮推理")),
                tool_calls: Some(vec![crate::models::types::ToolCallArgs {
                    id: call_id.into(),
                    name: name.into(),
                    arguments: "{}".into(),
                }]),
                tool_call_id: None,
                images: Vec::new(),
            },
            LlmMessage {
                role: "tool".into(),
                content: output.into(),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some(call_id.into()),
                images: Vec::new(),
            },
        ]
    }

    /// system + user + n 轮工具循环(每轮 tool 输出 output_len 字符)
    fn loop_messages(rounds: usize, output_len: usize) -> Vec<LlmMessage> {
        let mut msgs = vec![
            LlmMessage::plain("system", "系统提示"),
            LlmMessage::plain("user", "任务目标"),
        ];
        for i in 0..rounds {
            msgs.extend(tool_round(
                "read",
                &format!("call-{i}"),
                &format!("第{i}轮{}", "果".repeat(output_len)),
            ));
        }
        msgs
    }
}
