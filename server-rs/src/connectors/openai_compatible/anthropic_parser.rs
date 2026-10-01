// Anthropic Messages 流式解析(`/v1/messages` 端点;官方 api.anthropic.com 与兼容网关)。
//
// 事件序列(实测:百炼 claude-code-proxy,2026-10-01):
//   message_start → content_block_start → (ping)* → content_block_delta* → content_block_stop
//   → message_delta(stop_reason + usage)→ message_stop
// 关键口径:
// - **无 `[DONE]`**:流末以 message_stop 为终结;
// - usage 分散在两处(message_start 的 message.usage 与 message_delta 的 usage,
//   字段可能互有缺省)→ 各自记录、在 message_delta 合并成**一个** Usage 块产出。
//   引擎对 Usage 块是**累加**语义(executor.rs::process_chunk),分两次发会重复计数;
// - `ping` 是心跳不是产出:合法 JSON 但按「非推进」处理(与 2026-09-18 SSE 注释心跳
//   同一纪律——心跳不得为停滞的流续命,空闲看门狗以此收敛);
// - stop_reason → 统一 Finish 词汇:end_turn/stop_sequence → stop;max_tokens → length
//   (任务侧截断自愈只认 length);tool_use → tool_calls;refusal → content_filter;
// - 工具调用:content_block_start(type=tool_use) 声明 id/name,input_json_delta 增量拼参,
//   message_delta(stop_reason=tool_use) 统一 flush(完整性校验见 tool_accum)。
use super::sse_parser::{classify_upstream_error_object, find_event_end, MAX_BAD_JSON_EVENTS};
use super::tool_accum::PendingToolCalls;
use crate::models::llm_error::LlmError;
use crate::models::types::LlmStreamChunk;
use serde_json::Value;

#[derive(Default)]
pub(super) struct AnthropicParser {
    buffer: Vec<u8>,
    pending_tools: PendingToolCalls,
    done: bool,
    bad_json_count: usize,
    /// usage 计数累积(message_start 与 message_delta 各带一部分,产出时合并)
    input_tokens: i64,
    output_tokens: i64,
    cache_read: i64,
    cache_creation: i64,
    /// 是否见过任何 usage 对象(全缺时不产出零值 Usage 块)
    usage_seen: bool,
    /// 是否已产出 Usage(避免 message_stop 收尾时重复发)
    usage_emitted: bool,
}

impl AnthropicParser {
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
        self.pending_tools.flush(out)?;
        self.emit_usage(out);
        Ok(())
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
        self.bad_json_count = 0;
        let event_type = v.get("type").and_then(Value::as_str).unwrap_or("");
        match event_type {
            "message_start" => {
                if let Some(usage) = v.pointer("/message/usage") {
                    self.absorb_usage(usage);
                }
            }
            // 心跳:合法 JSON,但不是产出——不得算推进(见文件头)
            "ping" => return Ok(false),
            "content_block_start" => {
                if let Some(block) = v.get("content_block") {
                    if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                        let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                        let default_id = format!("toolu_{index}");
                        let id = block
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .unwrap_or(&default_id)
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let entry =
                            self.pending_tools
                                .entry(&index.to_string(), index as usize, &id);
                        entry.id = id;
                        entry.name = name;
                    }
                }
            }
            "content_block_delta" => {
                if let Some(delta) = v.get("delta") {
                    match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text_delta" => {
                            if let Some(t) = delta.get("text").and_then(Value::as_str) {
                                if !t.is_empty() {
                                    out.push(LlmStreamChunk::Token(t.to_string()));
                                }
                            }
                        }
                        "thinking_delta" => {
                            if let Some(t) = delta.get("thinking").and_then(Value::as_str) {
                                if !t.is_empty() {
                                    out.push(LlmStreamChunk::Reasoning(t.to_string()));
                                }
                            }
                        }
                        "input_json_delta" => {
                            if let Some(partial) = delta.get("partial_json").and_then(Value::as_str)
                            {
                                let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                                let default_id = format!("toolu_{index}");
                                self.pending_tools
                                    .entry(&index.to_string(), index as usize, &default_id)
                                    .arguments
                                    .push_str(partial);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "message_delta" => {
                if let Some(usage) = v.get("usage") {
                    self.absorb_usage(usage);
                }
                let stop_reason = v
                    .pointer("/delta/stop_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if stop_reason == "tool_use" {
                    self.pending_tools.flush(out)?;
                }
                self.emit_usage(out);
                if !stop_reason.is_empty() {
                    out.push(LlmStreamChunk::Finish {
                        reason: map_stop_reason(stop_reason).to_string(),
                    });
                }
            }
            "message_stop" => {
                self.pending_tools.flush(out)?;
                self.emit_usage(out);
                self.done = true;
            }
            "error" => {
                let err = v
                    .get("error")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"message": "上游返回 error 事件"}));
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
                return Err(LlmError::new(classify_upstream_error_object(&err), text));
            }
            // content_block_stop / 其余生命周期事件:合法数据事件、算推进,不产出块
            _ => {}
        }
        Ok(true)
    }

    fn absorb_usage(&mut self, usage: &Value) {
        self.usage_seen = true;
        if let Some(v) = usage.get("input_tokens").and_then(Value::as_i64) {
            if v > 0 {
                self.input_tokens = v;
            }
        }
        if let Some(v) = usage.get("output_tokens").and_then(Value::as_i64) {
            if v > 0 {
                self.output_tokens = v;
            }
        }
        if let Some(v) = usage.get("cache_read_input_tokens").and_then(Value::as_i64) {
            self.cache_read = v;
        }
        if let Some(v) = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_i64)
        {
            self.cache_creation = v;
        }
    }

    /// 合并产出唯一 Usage 块(引擎对 Usage 是累加语义,必须只发一次完整值)。
    /// Anthropic 语义:input_tokens **不含**缓存部分,总 prompt = input + cache_read + cache_creation;
    /// hit = cache_read,miss = input + cache_creation(与账单口径一致)。
    fn emit_usage(&mut self, out: &mut Vec<LlmStreamChunk>) {
        if !self.usage_seen || self.usage_emitted {
            return;
        }
        self.usage_emitted = true;
        let prompt = self.input_tokens + self.cache_read + self.cache_creation;
        let completion = self.output_tokens;
        out.push(LlmStreamChunk::Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            prompt_cache_hit_tokens: self.cache_read,
            prompt_cache_miss_tokens: self.input_tokens + self.cache_creation,
            reasoning_tokens: 0,
        });
    }
}

/// stop_reason → 统一 Finish 词汇(与 chat 方言对齐;任务侧截断自愈只认 length)。
fn map_stop_reason(reason: &str) -> &str {
    match reason {
        "end_turn" | "stop_sequence" => "stop",
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        "refusal" => "content_filter",
        other => other,
    }
}
