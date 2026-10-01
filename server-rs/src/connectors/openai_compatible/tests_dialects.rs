// 接口方言批次(2026-10-01):responses / anthropic 的请求映射与流式解析单测。
//
// 夹具形态取自百炼 workspace 端点的真实流式实测(2026-10-01):
// - Responses:事件块内夹 `:HTTP_STATUS/200` 注释行、以 response.completed 收尾(无 [DONE]);
// - Anthropic:message_start → content_block_* → ping 心跳 → message_delta(stop_reason+usage)
//   → message_stop;usage 分两段下发。
use super::anthropic_parser::AnthropicParser;
use super::responses_parser::ResponsesParser;
use super::tests::capture_request_server;
use super::*;
use crate::models::llm_error::LlmErrorKind;
use crate::models::types::{ToolCallArgs, ToolDefinition};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 捕获整个请求原文(含请求行与头部);用于断言鉴权头与端点路径
async fn capture_raw_request_server() -> (String, Arc<std::sync::Mutex<Option<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let holder = Arc::new(std::sync::Mutex::new(None::<String>));
    let holder2 = holder.clone();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 16384];
        let n = socket.read(&mut request).await.unwrap();
        *holder2.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(String::from_utf8_lossy(&request[..n]).to_string());
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n",
            )
            .await
            .unwrap();
        let done = b"data: [DONE]\n\n";
        socket
            .write_all(format!("{:X}\r\n", done.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(done).await.unwrap();
        socket.write_all(b"\r\n0\r\n\r\n").await.unwrap();
    });
    (format!("http://{addr}"), holder)
}

fn test_params() -> GenerationParams {
    GenerationParams {
        temperature: 0.8,
        top_p: 0.9,
        max_tokens: 128,
        stop: None,
        tools: Vec::new(),
        max_tool_rounds: None,
        tool_choice: crate::models::types::ToolChoice::Auto,
        connection_id: None,
        parallel_tool_calls: None,
        step_budget: None,
        semantic_guard: None,
    }
}

/// 方言解析:未知值回退默认档(与连接器类型回退同纪律)
#[test]
fn api_style_from_str_lossy_falls_back_to_chat() {
    assert_eq!(ApiStyle::from_str_lossy("responses"), ApiStyle::Responses);
    assert_eq!(ApiStyle::from_str_lossy("anthropic"), ApiStyle::Anthropic);
    assert_eq!(
        ApiStyle::from_str_lossy("chat-completions"),
        ApiStyle::ChatCompletions
    );
    assert_eq!(ApiStyle::from_str_lossy(""), ApiStyle::ChatCompletions);
    assert_eq!(
        ApiStyle::from_str_lossy("ollama"),
        ApiStyle::ChatCompletions
    );
}

/// 各方言端点路径按厂商既定约定拼接:
/// responses = base + /responses;anthropic = base(以 /v1 结尾)+ /messages、否则 + /v1/messages
#[test]
fn style_endpoint_paths_follow_provider_conventions() {
    let c = OpenAiCompatibleConnector::new_with_style(
        "https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        "k",
        "m",
        ApiStyle::Responses,
    );
    assert_eq!(
        c.generate_url(),
        "https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/responses"
    );
    let c = OpenAiCompatibleConnector::new_with_style(
        "https://api.anthropic.com/v1",
        "k",
        "m",
        ApiStyle::Anthropic,
    );
    assert_eq!(c.generate_url(), "https://api.anthropic.com/v1/messages");
    let c = OpenAiCompatibleConnector::new_with_style(
        "https://api.anthropic.com",
        "k",
        "m",
        ApiStyle::Anthropic,
    );
    assert_eq!(c.generate_url(), "https://api.anthropic.com/v1/messages");
    let c = OpenAiCompatibleConnector::new_with_style(
        "https://ws-x/api/v2/apps/claude-code-proxy",
        "k",
        "m",
        ApiStyle::Anthropic,
    );
    assert_eq!(
        c.generate_url(),
        "https://ws-x/api/v2/apps/claude-code-proxy/v1/messages"
    );
    // 默认 chat 方言保持既有路径
    let c = OpenAiCompatibleConnector::new("https://api.deepseek.com/v1", "k", "m");
    assert_eq!(
        c.generate_url(),
        "https://api.deepseek.com/v1/chat/completions"
    );
}

/// responses 请求体映射:system → instructions;tool 结果 → function_call_output;
/// assistant 工具调用 → function_call 项;tools 为扁平格式;max_tokens → max_output_tokens
#[tokio::test]
async fn responses_body_maps_instructions_input_and_tools() {
    let (base_url, body) = capture_request_server().await;
    let connector =
        OpenAiCompatibleConnector::new_with_style(&base_url, "key", "model", ApiStyle::Responses);
    let (_abort_tx, abort_rx) = watch::channel(false);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut params = test_params();
    params.tools = vec![ToolDefinition {
        name: "read".into(),
        description: "读文件".into(),
        parameters: serde_json::json!({"type": "object"}),
    }];
    let msgs = vec![
        LlmMessage::plain("system", "sys1"),
        LlmMessage::plain("system", "sys2"),
        LlmMessage::plain("user", "hi"),
        LlmMessage {
            role: "assistant".into(),
            content: String::new(),
            reasoning_content: None,
            tool_calls: Some(vec![ToolCallArgs {
                id: "call_1".into(),
                name: "read".into(),
                arguments: r#"{"q":1}"#.into(),
            }]),
            tool_call_id: None,
        },
        LlmMessage {
            role: "tool".into(),
            content: "result".into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("call_1".into()),
        },
    ];
    connector
        .generate_stream(&msgs, params, abort_rx, tx)
        .await
        .unwrap();
    let raw = body
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["instructions"], "sys1\n\nsys2");
    assert_eq!(v["max_output_tokens"], 128);
    assert_eq!(v["stream"], true);
    assert_eq!(v["tools"][0]["type"], "function");
    assert_eq!(v["tools"][0]["name"], "read");
    assert_eq!(v["tool_choice"], "auto");
    let input = v["input"].as_array().unwrap();
    assert_eq!(input[0], json!({"role": "user", "content": "hi"}));
    assert_eq!(input[1]["type"], "function_call");
    assert_eq!(input[1]["call_id"], "call_1");
    assert_eq!(input[1]["name"], "read");
    assert_eq!(input[1]["arguments"], r#"{"q":1}"#);
    assert_eq!(input[2]["type"], "function_call_output");
    assert_eq!(input[2]["call_id"], "call_1");
    assert_eq!(input[2]["output"], "result");
}

/// responses 流式解析(真实形态夹具):注释行不推进;文本增量 / 工具增量聚合 /
/// completed 的 usage 与 Finish{tool_calls};is_done 收口
#[test]
fn responses_parser_text_tool_usage_and_finish() {
    let mut parser = ResponsesParser::default();
    let mut out = Vec::new();
    let n = parser.push(b": ping\n\n", &mut out).unwrap();
    assert_eq!(n, 0, "纯注释块不算推进");
    // 事件块内夹注释行(百炼实测形态):仍是 1 个数据事件
    let n = parser
        .push(
            b"id:1\nevent:response.output_text.delta\n:HTTP_STATUS/200\ndata:{\"delta\":\"Hi\",\"type\":\"response.output_text.delta\"}\n\n",
            &mut out,
        )
        .unwrap();
    assert_eq!(n, 1);
    parser
        .push(
            b"data:{\"delta\":\"!\",\"type\":\"response.output_text.delta\"}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(
            b"data:{\"item\":{\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"\",\"type\":\"function_call\"},\"type\":\"response.output_item.added\"}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(
            b"data:{\"delta\":\"{\\\"q\\\":\",\"item_id\":\"fc_1\",\"type\":\"response.function_call_arguments.delta\"}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(
            b"data:{\"delta\":\"1}\",\"item_id\":\"fc_1\",\"type\":\"response.function_call_arguments.delta\"}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(
            b"data:{\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":50,\"output_tokens\":24,\"total_tokens\":74,\"input_tokens_details\":{\"cached_tokens\":10},\"output_tokens_details\":{\"reasoning_tokens\":2}}},\"type\":\"response.completed\"}\n\n",
            &mut out,
        )
        .unwrap();
    assert!(parser.is_done(), "completed 应终结读取");
    let texts: Vec<&str> = out
        .iter()
        .filter_map(|c| match c {
            LlmStreamChunk::Token(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["Hi", "!"]);
    assert!(
        out.iter().any(|c| matches!(
            c,
            LlmStreamChunk::ToolCall(tc) if tc.id == "call_1" && tc.name == "read" && tc.arguments == r#"{"q":1}"#
        )),
        "工具调用应在 completed 时聚合完整: {out:?}"
    );
    assert!(
        out.iter().any(|c| matches!(
            c,
            LlmStreamChunk::Usage {
                prompt_tokens: 50,
                completion_tokens: 24,
                total_tokens: 74,
                prompt_cache_hit_tokens: 10,
                prompt_cache_miss_tokens: 40,
                reasoning_tokens: 2,
            }
        )),
        "usage 应映射缓存与推理字段: {out:?}"
    );
    assert!(
        out.iter()
            .any(|c| matches!(c, LlmStreamChunk::Finish { reason } if reason == "tool_calls")),
        "有 function_call 的 completed 应产出 Finish{{tool_calls}}: {out:?}"
    );
}

/// responses:incomplete(max_output_tokens)→ Finish{length}(任务侧截断自愈只认 length);
/// failed → 结构化错误分类(限流可重试)
#[test]
fn responses_incomplete_maps_length_and_failed_classifies() {
    let mut parser = ResponsesParser::default();
    let mut out = Vec::new();
    parser
        .push(
            b"data:{\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}},\"type\":\"response.incomplete\"}\n\n",
            &mut out,
        )
        .unwrap();
    assert!(
        out.iter()
            .any(|c| matches!(c, LlmStreamChunk::Finish { reason } if reason == "length")),
        "max_output_tokens 截断应映射 length: {out:?}"
    );
    assert!(parser.is_done());

    let mut parser2 = ResponsesParser::default();
    let mut out2 = Vec::new();
    let err = parser2
        .push(
            b"data:{\"response\":{\"error\":{\"message\":\"slow down\",\"type\":\"rate_limit_exceeded\"}},\"type\":\"response.failed\"}\n\n",
            &mut out2,
        )
        .unwrap_err();
    assert_eq!(err.kind(), LlmErrorKind::RateLimited);
    assert!(err.retryable(), "限流应可重试: {err}");
}

/// anthropic 鉴权头三件套:x-api-key + anthropic-version + Bearer(兼容网关两种鉴权都收);
/// 请求行落在 /v1/messages(base 无 /v1 时的既定约定)
#[tokio::test]
async fn anthropic_requests_carry_auth_headers_and_path() {
    let (base_url, holder) = capture_raw_request_server().await;
    let connector =
        OpenAiCompatibleConnector::new_with_style(&base_url, "sk-test", "m", ApiStyle::Anthropic);
    let (_abort_tx, abort_rx) = watch::channel(false);
    let (tx, _rx) = mpsc::unbounded_channel();
    connector
        .generate_stream(
            &[LlmMessage::plain("user", "hi")],
            test_params(),
            abort_rx,
            tx,
        )
        .await
        .unwrap();
    let raw = holder
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    let lower = raw.to_lowercase();
    assert!(lower.contains("x-api-key: sk-test"), "缺 x-api-key: {raw}");
    assert!(
        lower.contains("anthropic-version: 2023-06-01"),
        "缺 anthropic-version: {raw}"
    );
    assert!(
        lower.contains("authorization: bearer sk-test"),
        "缺 Bearer 兼容头: {raw}"
    );
    assert!(raw.contains("POST /v1/messages"), "端点路径错误: {raw}");
}

/// anthropic 请求体映射:system 顶层;tool_result 连续多条并入同一 user 消息;
/// tool_use 块 input 为解析后的 JSON;temperature 钳制 0..1;stop → stop_sequences
#[tokio::test]
async fn anthropic_body_maps_system_tool_blocks_and_clamps() {
    let (base_url, body) = capture_request_server().await;
    let connector =
        OpenAiCompatibleConnector::new_with_style(&base_url, "key", "model", ApiStyle::Anthropic);
    let (_abort_tx, abort_rx) = watch::channel(false);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut params = test_params();
    params.temperature = 1.6;
    params.stop = Some(vec!["END".into()]);
    params.tools = vec![ToolDefinition {
        name: "read".into(),
        description: "读文件".into(),
        parameters: serde_json::json!({"type": "object"}),
    }];
    let msgs = vec![
        LlmMessage::plain("system", "sys"),
        LlmMessage::plain("user", "hi"),
        LlmMessage {
            role: "assistant".into(),
            content: String::new(),
            reasoning_content: None,
            tool_calls: Some(vec![ToolCallArgs {
                id: "toolu_1".into(),
                name: "read".into(),
                arguments: r#"{"q":1}"#.into(),
            }]),
            tool_call_id: None,
        },
        LlmMessage {
            role: "tool".into(),
            content: "r1".into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("toolu_1".into()),
        },
        LlmMessage {
            role: "tool".into(),
            content: "r2".into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("toolu_1".into()),
        },
        LlmMessage::plain("user", "继续"),
    ];
    connector
        .generate_stream(&msgs, params, abort_rx, tx)
        .await
        .unwrap();
    let raw = body
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["system"], "sys");
    assert_eq!(v["temperature"], 1.0, "temperature 应钳制到 0..=1");
    assert_eq!(v["max_tokens"], 128);
    assert_eq!(v["stop_sequences"], json!(["END"]));
    assert_eq!(v["tools"][0]["name"], "read");
    assert_eq!(v["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(v["tool_choice"], json!({"type": "auto"}));
    let messages = v["messages"].as_array().unwrap();
    assert_eq!(messages[0], json!({"role": "user", "content": "hi"}));
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"][0]["type"], "tool_use");
    assert_eq!(messages[1]["content"][0]["id"], "toolu_1");
    assert_eq!(messages[1]["content"][0]["input"], json!({"q": 1}));
    assert_eq!(messages[2]["role"], "user");
    let blocks = messages[2]["content"].as_array().unwrap();
    assert_eq!(
        blocks.len(),
        3,
        "两条 tool_result 后并入新用户文本: {blocks:?}"
    );
    assert_eq!(blocks[0]["type"], "tool_result");
    assert_eq!(blocks[0]["tool_use_id"], "toolu_1");
    assert_eq!(blocks[1]["type"], "tool_result");
    assert_eq!(blocks[2]["type"], "text");
    assert_eq!(blocks[2]["text"], "继续");
}

/// anthropic:tool_choice=none 档无对应表达 → 不下发 tools(模型无从调用,语义等价);
/// required → any;function → tool(映射表锁定)
#[test]
fn anthropic_tool_choice_mapping() {
    let mut params = test_params();
    params.tools = vec![ToolDefinition {
        name: "read".into(),
        description: "读文件".into(),
        parameters: serde_json::json!({"type": "object"}),
    }];
    params.tool_choice = crate::models::types::ToolChoice::None;
    let body = build_anthropic_body("m", &[LlmMessage::plain("user", "hi")], &params);
    assert!(
        body.get("tools").is_none(),
        "none 档应整体隐去 tools: {body}"
    );
    params.tool_choice = crate::models::types::ToolChoice::Required;
    let body = build_anthropic_body("m", &[LlmMessage::plain("user", "hi")], &params);
    assert_eq!(body["tool_choice"], json!({"type": "any"}));
    params.tool_choice = crate::models::types::ToolChoice::Function("read".into());
    let body = build_anthropic_body("m", &[LlmMessage::plain("user", "hi")], &params);
    assert_eq!(body["tool_choice"], json!({"type": "tool", "name": "read"}));
}

/// anthropic 流式解析(真实形态夹具):ping 不推进;文本增量;usage 两段合并只发一次;
/// stop_reason 映射;message_stop 收口
#[test]
fn anthropic_parser_text_ping_usage_and_stop_reason() {
    let mut parser = AnthropicParser::default();
    let mut out = Vec::new();
    parser
        .push(
            b"data:{\"message\":{\"usage\":{\"input_tokens\":0,\"output_tokens\":0}},\"type\":\"message_start\"}\n\n",
            &mut out,
        )
        .unwrap();
    let n = parser
        .push(b"event:ping\ndata:{\"type\":\"ping\"}\n\n", &mut out)
        .unwrap();
    assert_eq!(n, 0, "ping 心跳不得算推进(停滞看门狗纪律)");
    parser
        .push(
            b"data:{\"content_block\":{\"type\":\"text\",\"text\":\"\"},\"index\":0,\"type\":\"content_block_start\"}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(
            b"data:{\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"},\"type\":\"content_block_delta\",\"index\":0}\n\n",
            &mut out,
        )
        .unwrap();
    // 空文本增量不产出(真实样例含前导/尾随空 delta)
    parser
        .push(
            b"data:{\"delta\":{\"type\":\"text_delta\",\"text\":\"\"},\"type\":\"content_block_delta\",\"index\":0}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(
            b"data:{\"delta\":{\"stop_reason\":\"end_turn\"},\"type\":\"message_delta\",\"usage\":{\"output_tokens\":13,\"input_tokens\":10,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(b"data:{\"type\":\"message_stop\"}\n\n", &mut out)
        .unwrap();
    assert!(parser.is_done(), "message_stop 应终结读取");
    let texts: Vec<&str> = out
        .iter()
        .filter_map(|c| match c {
            LlmStreamChunk::Token(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["Hi"]);
    let usages: Vec<_> = out
        .iter()
        .filter(|c| matches!(c, LlmStreamChunk::Usage { .. }))
        .collect();
    assert_eq!(
        usages.len(),
        1,
        "usage 两段必须合并成唯一一块(引擎累加语义)"
    );
    assert!(matches!(
        usages[0],
        LlmStreamChunk::Usage {
            prompt_tokens: 10,
            completion_tokens: 13,
            total_tokens: 23,
            prompt_cache_hit_tokens: 0,
            prompt_cache_miss_tokens: 10,
            reasoning_tokens: 0,
        }
    ));
    assert!(
        out.iter()
            .any(|c| matches!(c, LlmStreamChunk::Finish { reason } if reason == "stop")),
        "end_turn 应映射 stop: {out:?}"
    );
}

/// anthropic 工具流:input_json_delta 聚合 + tool_use 收尾 flush;
/// stop_reason 映射表;错误事件分类
#[test]
fn anthropic_parser_tool_flow_and_error_events() {
    let mut parser = AnthropicParser::default();
    let mut out = Vec::new();
    parser
        .push(
            b"data:{\"message\":{\"usage\":{\"input_tokens\":5}},\"type\":\"message_start\"}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(
            b"data:{\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"read\"},\"index\":1,\"type\":\"content_block_start\"}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(
            b"data:{\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"q\\\":\"},\"type\":\"content_block_delta\",\"index\":1}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(
            b"data:{\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"1}\"},\"type\":\"content_block_delta\",\"index\":1}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(
            b"data:{\"delta\":{\"stop_reason\":\"tool_use\"},\"type\":\"message_delta\",\"usage\":{\"output_tokens\":3}}\n\n",
            &mut out,
        )
        .unwrap();
    parser
        .push(b"data:{\"type\":\"message_stop\"}\n\n", &mut out)
        .unwrap();
    assert!(
        out.iter().any(|c| matches!(
            c,
            LlmStreamChunk::ToolCall(tc) if tc.id == "toolu_9" && tc.name == "read" && tc.arguments == r#"{"q":1}"#
        )),
        "tool_use 参数应聚合完整: {out:?}"
    );
    assert!(
        out.iter()
            .any(|c| matches!(c, LlmStreamChunk::Finish { reason } if reason == "tool_calls")),
        "tool_use 应映射 tool_calls: {out:?}"
    );
    // max_tokens → length
    let mut p2 = AnthropicParser::default();
    let mut o2 = Vec::new();
    p2.push(
        b"data:{\"delta\":{\"stop_reason\":\"max_tokens\"},\"type\":\"message_delta\"}\n\n",
        &mut o2,
    )
    .unwrap();
    assert!(
        o2.iter()
            .any(|c| matches!(c, LlmStreamChunk::Finish { reason } if reason == "length")),
        "max_tokens 应映射 length: {o2:?}"
    );
    // 错误事件分类:rate_limit / authentication
    let mut p3 = AnthropicParser::default();
    let mut o3 = Vec::new();
    let err = p3
        .push(
            b"data:{\"error\":{\"message\":\"busy\",\"type\":\"rate_limit_error\"},\"type\":\"error\"}\n\n",
            &mut o3,
        )
        .unwrap_err();
    assert_eq!(err.kind(), LlmErrorKind::RateLimited);
    assert!(err.retryable());
    let mut p4 = AnthropicParser::default();
    let mut o4 = Vec::new();
    let err = p4
        .push(
            b"data:{\"error\":{\"message\":\"bad key\",\"type\":\"authentication_error\"},\"type\":\"error\"}\n\n",
            &mut o4,
        )
        .unwrap_err();
    assert_eq!(err.kind(), LlmErrorKind::AuthFailed);
    assert!(!err.retryable(), "鉴权失败不得重试");
}
