// Responses API 流式解析(OpenAI Responses 方言;`/responses` 端点)。
//
// 与 Chat 方言的差异:
// - 事件用 `type` 字段命名(response.output_text.delta / response.completed 等),
//   SSE 的 `event:` 行只是复述,解析一律以 JSON 内 `type` 为准(部分网关不发 event 行);
// - 文本事件是 `response.output_text.delta {delta}`;
// - 工具调用按 item 聚合:output_item.added 声明 function_call → function_call_arguments.delta
//   增量拼参 → completed 统一 flush(完整性校验见 tool_accum);
// - 流末是 `response.completed`(无 `[DONE]`;部分网关仍发 [DONE],两种都收);
// - usage 只出现在 completed 的 response.usage(input_tokens/output_tokens/total_tokens)。
//
// 网关实测(百炼 workspace,2026-10-01):事件间会插入 `:HTTP_STATUS/200` 注释行,
// 属于标准 SSE 注释,payload 提取只认 `data:` 行,天然兼容。
use super::sse_parser::{classify_upstream_error_object, find_event_end, MAX_BAD_JSON_EVENTS};
use super::tool_accum::PendingToolCalls;
use crate::models::llm_error::LlmError;
use crate::models::types::LlmStreamChunk;
use serde_json::Value;

#[derive(Default)]
pub(super) struct ResponsesParser {
    buffer: Vec<u8>,
    pending_tools: PendingToolCalls,
    /// function_call item 首见顺序(flush 排序键;item_id 本身无序)
    tool_seq: usize,
    /// 本轮是否产出过 function_call:completed 的 Finish 据此报 tool_calls(与 chat 方言口径一致)
    got_tool_call: bool,
    done: bool,
    bad_json_count: usize,
}

impl ResponsesParser {
    pub(super) fn push(
        &mut self,
        bytes: &[u8],
        out: &mut Vec<LlmStreamChunk>,
    ) -> Result<usize, LlmError> {
        self.buffer.extend_from_slice(bytes);
        let mut data_events = 0usize;
        while let Some((end, separator_len)) = find_event_end(&self.buffer) {
            let event = self.buffer.drain(..end).collect::<Vec<_>>();
            self.buffer.drain(..separator_len);
            if self.parse_event(&event, out)? {
                data_events += 1;
            }
        }
        Ok(data_events)
    }

    pub(super) fn finish(&mut self, out: &mut Vec<LlmStreamChunk>) -> Result<(), LlmError> {
        if !self.buffer.is_empty() {
            let event = std::mem::take(&mut self.buffer);
            self.parse_event(&event, out)?;
        }
        self.pending_tools.flush(out)
    }

    pub(super) fn is_done(&self) -> bool {
        self.done
    }

    fn parse_event(
        &mut self,
        event: &[u8],
        out: &mut Vec<LlmStreamChunk>,
    ) -> Result<bool, LlmError> {
        let text = std::str::from_utf8(event)
            .map_err(|e| LlmError::generation(format!("SSE 响应不是有效 UTF-8: {e}")))?;
        let payload = text
            .lines()
            .filter_map(|line| {
                line.strip_suffix('\r')
                    .unwrap_or(line)
                    .strip_prefix("data:")
            })
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if payload.is_empty() {
            return Ok(false);
        }
        if payload.trim() == "[DONE]" {
            self.done = true;
            self.pending_tools.flush(out)?;
            return Ok(true);
        }
        let v: Value = match serde_json::from_str(&payload) {
            Ok(value) => value,
            Err(_) => {
                self.bad_json_count += 1;
                if self.bad_json_count >= MAX_BAD_JSON_EVENTS {
                    let sample: String = payload.chars().take(200).collect();
                    return Err(LlmError::generation(format!(
                        "上游返回过多非 JSON data 事件({} 个),疑似协议损坏: {sample}",
                        self.bad_json_count
                    )));
                }
                return Ok(false);
            }
        };
        // 顶层错误对象(部分网关把错误放在流外层的 error 字段)
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            return Err(self.upstream_error(err));
        }
        self.bad_json_count = 0;
        let event_type = v.get("type").and_then(Value::as_str).unwrap_or("");
        match event_type {
            "response.output_text.delta" => {
                if let Some(delta) = v.get("delta").and_then(Value::as_str) {
                    if !delta.is_empty() {
                        out.push(LlmStreamChunk::Token(delta.to_string()));
                    }
                }
            }
            // 推理摘要/正文增量(推理模型经 Responses 方言输出时)
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = v.get("delta").and_then(Value::as_str) {
                    if !delta.is_empty() {
                        out.push(LlmStreamChunk::Reasoning(delta.to_string()));
                    }
                }
            }
            "response.output_item.added" => {
                if let Some(item) = v.get("item") {
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        self.got_tool_call = true;
                        let item_id = item.get("id").and_then(Value::as_str).unwrap_or("call");
                        let call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .unwrap_or(item_id)
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let arguments = item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        self.tool_seq += 1;
                        let seq = self.tool_seq;
                        let entry = self.pending_tools.entry(item_id, seq, &call_id);
                        entry.id = call_id;
                        entry.name = name;
                        if !arguments.is_empty() {
                            entry.arguments.push_str(&arguments);
                        }
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                if let (Some(item_id), Some(delta)) = (
                    v.get("item_id").and_then(Value::as_str),
                    v.get("delta").and_then(Value::as_str),
                ) {
                    self.tool_seq += 1;
                    let seq = self.tool_seq;
                    // 规范保证 output_item.added 先行;少数网关直发 delta,惰性建槽
                    // (name 缺失的残留调用由 flush 过滤,不会把半截调用交给执行器)
                    self.pending_tools
                        .entry(item_id, seq, item_id)
                        .arguments
                        .push_str(delta);
                }
            }
            "response.function_call_arguments.done" => {
                if let (Some(item_id), Some(arguments)) = (
                    v.get("item_id").and_then(Value::as_str),
                    v.get("arguments").and_then(Value::as_str),
                ) {
                    // done 携带全量参数:仅在增量阶段没收到任何字节时兜底(避免重复拼接)
                    self.tool_seq += 1;
                    let seq = self.tool_seq;
                    let entry = self.pending_tools.entry(item_id, seq, item_id);
                    if entry.arguments.is_empty() {
                        entry.arguments.push_str(arguments);
                    }
                }
            }
            "response.output_item.done" => {
                if let Some(item) = v.get("item") {
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        self.got_tool_call = true;
                        if let (Some(item_id), Some(arguments)) = (
                            item.get("id").and_then(Value::as_str),
                            item.get("arguments").and_then(Value::as_str),
                        ) {
                            self.tool_seq += 1;
                            let seq = self.tool_seq;
                            let entry = self.pending_tools.entry(item_id, seq, item_id);
                            if entry.arguments.is_empty() {
                                entry.arguments.push_str(arguments);
                            }
                        }
                    }
                }
            }
            "response.completed" | "response.incomplete" => {
                self.finish_response(&v, event_type == "response.incomplete", out)?;
            }
            "response.failed" => {
                let err = v
                    .pointer("/response/error")
                    .filter(|e| !e.is_null())
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"message": "上游返回 failed 状态"}));
                return Err(self.upstream_error(&err));
            }
            // created / in_progress / output_item.added(非工具)/ content_part.* 等
            // 生命周期事件:合法数据事件、算推进,但不产出块
            _ => {}
        }
        Ok(true)
    }

    /// completed / incomplete 收口:flush 工具调用 → usage → Finish。
    /// Finish 口径与 chat 方言对齐:有 function_call → `tool_calls`;
    /// incomplete 且原因是 max_output_tokens → `length`(任务侧截断自愈只认 length)。
    fn finish_response(
        &mut self,
        v: &Value,
        incomplete: bool,
        out: &mut Vec<LlmStreamChunk>,
    ) -> Result<(), LlmError> {
        self.pending_tools.flush(out)?;
        if let Some(usage) = v.pointer("/response/usage").filter(|u| !u.is_null()) {
            let (prompt, completion, total, hit, miss, reasoning) = parse_responses_usage(usage);
            out.push(LlmStreamChunk::Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: total,
                prompt_cache_hit_tokens: hit,
                prompt_cache_miss_tokens: miss,
                reasoning_tokens: reasoning,
            });
        }
        let reason = if incomplete {
            match v
                .pointer("/response/incomplete_details/reason")
                .and_then(Value::as_str)
            {
                Some("max_output_tokens") => "length".to_string(),
                Some(other) => other.to_string(),
                None => "length".to_string(),
            }
        } else if self.got_tool_call {
            "tool_calls".to_string()
        } else {
            "stop".to_string()
        };
        out.push(LlmStreamChunk::Finish { reason });
        self.done = true;
        Ok(())
    }

    fn upstream_error(&self, err: &Value) -> LlmError {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("未知上游错误");
        let err_type = err.get("type").and_then(Value::as_str).unwrap_or("");
        let text = if err_type.is_empty() {
            format!("上游返回错误: {message}")
        } else {
            format!("上游返回错误 [{err_type}]: {message}")
        };
        LlmError::new(classify_upstream_error_object(err), text)
    }
}

/// Responses usage → 计数:input_tokens/output_tokens/total_tokens;
/// 缓存命中取 input_tokens_details.cached_tokens(命中时 miss 由推导);
/// 推理 token 取 output_tokens_details.reasoning_tokens。
fn parse_responses_usage(u: &Value) -> (i64, i64, i64, i64, i64, i64) {
    let prompt = u.get("input_tokens").and_then(Value::as_i64).unwrap_or(0);
    let completion = u.get("output_tokens").and_then(Value::as_i64).unwrap_or(0);
    let total = u
        .get("total_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(prompt + completion);
    let hit = u
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let miss = if hit > 0 { (prompt - hit).max(0) } else { 0 };
    let reasoning = u
        .pointer("/output_tokens_details/reasoning_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    (prompt, completion, total, hit, miss, reasoning)
}
