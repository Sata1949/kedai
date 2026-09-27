//! 蒸馏:系统提示词、历史拼装、产出解析与行映射。

use crate::models::types::MessageRecord;

// 子模块内统一用 super::* 拿到 mod.rs 的公共词汇(类型/常量)与 re-export 的自由函数。
use super::*;

/// 蒸馏系统指令:取向沿用 compaction(人物关系与立场、关键事件与因果、未回收伏笔),
/// 输出改为「每行一条短记忆」以支持服务端按行拆分。
pub fn distill_system_prompt() -> &'static str {
    "你是对话记忆蒸馏助手。从下面的角色扮演对话中提取值得跨会话长期记住的记忆条目,\
     每条一行、独立成句、简洁平实(中文)。必须覆盖:人物关系与立场、已发生的关键事件\
     与因果、未回收的伏笔/承诺/约定。不要输出一次性的场景描写、寒暄或与长期记忆无关的\
     细节;不要编号、不要项目符号、不要评论或「记忆如下」等元文本,只输出记忆条目本身。"
}

/// 蒸馏输入文本:按「角色: 内容」逐条拼接(与 compaction_user_text 同格式)。
pub fn distill_user_text(history: &[MessageRecord]) -> String {
    history
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| {
            let speaker = if m.role == "user" { "用户" } else { "角色" };
            format!("{speaker}: {}", m.content)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 蒸馏输出按行拆分:去空白、跳过空行、剥一次行首项目符号(- / *),上限 MAX_DISTILLED_LINES。
pub fn parse_distilled_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw
            .trim()
            .trim_start_matches('-')
            .trim_start_matches('*')
            .trim();
        if line.is_empty() {
            continue;
        }
        if out.len() >= MAX_DISTILLED_LINES {
            break;
        }
        out.push(line.to_string());
    }
    out
}

pub(super) fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<MemoryEntry> {
    Ok(MemoryEntry {
        id: row.get(0)?,
        character_id: row.get(1)?,
        source_session_id: row.get(2)?,
        kind: row.get(3)?,
        content: row.get(4)?,
        usage_count: row.get(5)?,
        last_usage: row.get(6)?,
        selected: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        pinned: row.get::<_, i64>(10)? != 0,
    })
}
