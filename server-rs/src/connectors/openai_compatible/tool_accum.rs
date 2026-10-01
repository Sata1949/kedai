// 工具调用增量聚合(三种接口方言共用:Chat Completions / Responses / Anthropic)。
//
// 三方言的增量事件形状不同(chat: delta.tool_calls[].index;responses: item_id +
// function_call_arguments.delta;anthropic: index + input_json_delta),但聚合语义与
// flush 校验完全一致——此前校验实现在 sse_parser 内,新增两方言若各抄一份,「非法 JSON
// 参数」「重复 id」两类完整性问题会三处漂移。本模块是唯一出处。
use crate::models::llm_error::LlmError;
use crate::models::types::{LlmStreamChunk, ToolCallArgs};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct PendingToolCalls {
    /// key 由调用方定(chat/anthropic 用 index 字符串,responses 用 item_id);
    /// 排序键随首次出现记录(flush 按它升序,输出顺序与上游声明顺序一致)
    calls: HashMap<String, (usize, ToolCallArgs)>,
}

impl PendingToolCalls {
    /// 取(或创建)某个调用的聚合槽。`default_id` 用于上游未给 id 的调用
    /// (chat 的 `call_{index}` 既有口径;responses 用 item_id;anthropic 用 `toolu_{index}`)。
    pub(super) fn entry(
        &mut self,
        key: &str,
        sort_key: usize,
        default_id: &str,
    ) -> &mut ToolCallArgs {
        &mut self
            .calls
            .entry(key.to_string())
            .or_insert_with(|| {
                (
                    sort_key,
                    ToolCallArgs {
                        id: default_id.to_string(),
                        name: String::new(),
                        arguments: String::new(),
                    },
                )
            })
            .1
    }

    /// flush 已聚合的调用。校验口径(自 sse_parser 原样迁移,三方言一致):
    /// - 缺 name 的调用过滤(无法执行);
    /// - 重复 id 报错(同一调用被上游声明两次,执行会串);
    /// - arguments 非空但非法 JSON 报错(截断/转义损坏必须暴露,不能交给执行器)。
    pub(super) fn flush(&mut self, out: &mut Vec<LlmStreamChunk>) -> Result<(), LlmError> {
        let mut calls: Vec<_> = self.calls.drain().collect();
        calls.sort_by_key(|(_, (sort_key, _))| *sort_key);
        let mut seen_ids: HashMap<String, ()> = HashMap::new();
        for (_, (_, call)) in &calls {
            if !call.name.is_empty() {
                if !call.id.is_empty() && seen_ids.contains_key(&call.id) {
                    return Err(LlmError::generation(format!(
                        "上游返回重复工具调用 id:{} (名称 {})",
                        call.id, call.name
                    )));
                }
                if !call.id.is_empty() {
                    seen_ids.insert(call.id.clone(), ());
                }
                if !call.arguments.trim().is_empty() {
                    // 保留 serde 原错(含行列位置),否则「参数不是合法 JSON」只剩半截
                    // 原文而无法判断是截断、转义还是编码问题
                    serde_json::from_str::<Value>(&call.arguments).map_err(|e| {
                        LlmError::generation(format!(
                            "工具 \"{}\" 的 arguments 不是合法 JSON({e}): {}",
                            call.name,
                            call.arguments.chars().take(200).collect::<String>()
                        ))
                    })?;
                }
            }
        }
        out.extend(calls.into_iter().filter_map(|(_, (_, call))| {
            (!call.name.is_empty()).then_some(LlmStreamChunk::ToolCall(call))
        }));
        Ok(())
    }
}
