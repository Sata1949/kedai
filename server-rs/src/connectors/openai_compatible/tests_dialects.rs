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
        response_format: None,
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
            images: Vec::new(),
        },
        LlmMessage {
            role: "tool".into(),
            content: "result".into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("call_1".into()),
            images: Vec::new(),
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
            images: Vec::new(),
        },
        LlmMessage {
            role: "tool".into(),
            content: "r1".into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("toolu_1".into()),
            images: Vec::new(),
        },
        LlmMessage {
            role: "tool".into(),
            content: "r2".into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("toolu_1".into()),
            images: Vec::new(),
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
    let body = build_anthropic_body(
        "m",
        &[LlmMessage::plain("user", "hi")],
        &params,
        Default::default(),
    );
    assert!(
        body.get("tools").is_none(),
        "none 档应整体隐去 tools: {body}"
    );
    params.tool_choice = crate::models::types::ToolChoice::Required;
    let body = build_anthropic_body(
        "m",
        &[LlmMessage::plain("user", "hi")],
        &params,
        Default::default(),
    );
    assert_eq!(body["tool_choice"], json!({"type": "any"}));
    params.tool_choice = crate::models::types::ToolChoice::Function("read".into());
    let body = build_anthropic_body(
        "m",
        &[LlmMessage::plain("user", "hi")],
        &params,
        Default::default(),
    );
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

// ===== 视觉能力包 D2:图像映射(chat image_url / responses input_image / anthropic image)=====

/// 一条带图像的 user 消息(下发音以 data URL 就绪;id/name 仅引用元数据)
fn image_message() -> LlmMessage {
    let mut m = LlmMessage::plain("user", "看图");
    m.images = vec![crate::models::types::ImageRef {
        id: "a.png".into(),
        name: "a.png".into(),
        mime: "image/png".into(),
        data_url: "data:image/png;base64,AAAA".into(),
        label: None,
    }];
    m
}

fn vision_caps() -> crate::connectors::ConnectorCapabilities {
    crate::connectors::ConnectorCapabilities {
        supports_vision: true,
        image_auto_split: false,
        supports_structured_output: false,
        supports_prefix_completion: false,
        supports_mid_conversation_system: false,
    }
}

/// chat:能力位关闭时与纯文本路径逐字节一致(images 被忽略);
/// 打开且有 data_url 时 content 变 parts 数组(text + image_url)。
#[test]
fn chat_body_serializes_images_only_when_vision_enabled() {
    let params = test_params();
    let msgs = vec![image_message()];
    // 关闭:content 仍是纯字符串(无图路径逐字节不变)
    let body = build_chat_body("m", &msgs, &params, Default::default());
    assert_eq!(body["messages"][0]["content"], "看图");
    // 打开:text + image_url data URL 两段
    let body = build_chat_body("m", &msgs, &params, vision_caps());
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({"type": "text", "text": "看图"})
    );
    assert_eq!(
        body["messages"][0]["content"][1],
        json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}})
    );
    // 纯图消息(正文为空):parts 只有 image_url 一段
    let mut only_img = image_message();
    only_img.content = String::new();
    let body = build_chat_body("m", &[only_img], &params, vision_caps());
    assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 1);
    assert_eq!(body["messages"][0]["content"][0]["type"], "image_url");
}

/// responses:user 图像映射为 input_text + input_image 两个 input 项;
/// 能力位关闭时保持纯字符串 content(旧行为)。
#[test]
fn responses_body_maps_user_images_to_input_parts() {
    let params = test_params();
    let msgs = vec![image_message()];
    let body = build_responses_body("m", &msgs, &params, Default::default());
    assert_eq!(body["input"][0]["content"], "看图");
    let body = build_responses_body("m", &msgs, &params, vision_caps());
    assert_eq!(
        body["input"][0]["content"][0],
        json!({"type": "input_text", "text": "看图"})
    );
    assert_eq!(
        body["input"][0]["content"][1],
        json!({"type": "input_image", "image_url": "data:image/png;base64,AAAA"})
    );
}

/// anthropic:user 图像变块数组 text + image 块;source.data 是**裸 base64**
/// (data URL 前缀必须剥掉,否则上游 400)。
#[test]
fn anthropic_body_strips_data_url_prefix_for_image_source() {
    let params = test_params();
    let msgs = vec![image_message()];
    let body = build_anthropic_body("m", &msgs, &params, Default::default());
    assert_eq!(body["messages"][0]["content"], "看图");
    let body = build_anthropic_body("m", &msgs, &params, vision_caps());
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({"type": "text", "text": "看图"})
    );
    assert_eq!(
        body["messages"][0]["content"][1],
        json!({
            "type": "image",
            "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}
        })
    );
}

// ===== 视觉能力包 D3:大图拆分标注(label 以 text 紧贴图像之前)=====

fn labeled_image_message() -> LlmMessage {
    let mut m = LlmMessage::plain("user", "看图");
    m.images = vec![
        crate::models::types::ImageRef {
            id: "o".into(),
            name: "x.png".into(),
            mime: "image/png".into(),
            data_url: "data:image/png;base64,AA".into(),
            label: Some("（原图总览:100×100）".into()),
        },
        crate::models::types::ImageRef {
            id: "b".into(),
            name: "x.png".into(),
            mime: "image/png".into(),
            data_url: "data:image/png;base64,BB".into(),
            label: Some("（大图拆分:第1行/第1列,共1行×2列）".into()),
        },
    ];
    m
}

/// chat:标注以 text part 紧贴各自图像之前输出(总览 → 块序保持)
#[test]
fn chat_labeled_images_emit_text_parts() {
    let body = build_chat_body(
        "m",
        &[labeled_image_message()],
        &test_params(),
        vision_caps(),
    );
    let content = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content[0], json!({"type": "text", "text": "看图"}));
    assert_eq!(
        content[1],
        json!({"type": "text", "text": "（原图总览:100×100）"})
    );
    assert_eq!(content[2]["type"], "image_url");
    assert_eq!(content[3]["text"], "（大图拆分:第1行/第1列,共1行×2列）");
    assert_eq!(content[4]["type"], "image_url");
}

/// anthropic:标注同为 text 块(与图像块交错)
#[test]
fn anthropic_labeled_images_emit_text_blocks() {
    let body = build_anthropic_body(
        "m",
        &[labeled_image_message()],
        &test_params(),
        vision_caps(),
    );
    let content = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content[0], json!({"type": "text", "text": "看图"}));
    assert_eq!(
        content[1],
        json!({"type": "text", "text": "（原图总览:100×100）"})
    );
    assert_eq!(content[2]["type"], "image");
    assert_eq!(content[3]["text"], "（大图拆分:第1行/第1列,共1行×2列）");
    assert_eq!(content[4]["type"], "image");
}

// ===== 修复批次(2026-10-02):工具图像下发落位 + Anthropic 连发 user 合并 =====
//
// 背景:此前工具图像统一挂在 role="tool" 消息上——chat 方言把 image_url part 塞进
// tool 消息(OpenAI 契约只允许文本 part,严格后端整轮 400),responses/anthropic
// 只取文本则静默丢图。本组用例锁定修复后的落位:
//   chat      → tool 消息纯文本;图像在本轮工具组末尾收口为一条 user 消息
//   responses → function_call_output.output 纯文本;图像随后以 user 输入项承载
//   anthropic → 图像内嵌 tool_result 的 content 块数组(官方支持,不新增消息)

/// assistant(tool_calls) 消息(空正文;与执行器产出的形态一致)
fn assistant_tool_call(id: &str) -> LlmMessage {
    let mut m = LlmMessage::plain("assistant", "");
    m.tool_calls = Some(vec![ToolCallArgs {
        id: id.into(),
        name: "screenshot".into(),
        arguments: "{}".into(),
    }]);
    m
}

/// tool 消息(带一张图像;label 为可选拆分标注)
fn tool_image_message(call_id: &str, label: Option<&str>) -> LlmMessage {
    let mut m = LlmMessage::plain("tool", "已截图:输出 100×100");
    m.tool_call_id = Some(call_id.into());
    m.images = vec![crate::models::types::ImageRef {
        id: "shot.png".into(),
        name: "shot.png".into(),
        mime: "image/png".into(),
        data_url: "data:image/png;base64,AAAA".into(),
        label: label.map(str::to_string),
    }];
    m
}

/// chat:tool 消息恒为纯文本字符串,图像落在组末尾的 user 消息(含来源说明);
/// 能力位关闭时图像整条不出现(tool 消息形状不变)
#[test]
fn chat_tool_images_land_in_trailing_user_message() {
    let msgs = vec![
        assistant_tool_call("c1"),
        tool_image_message("c1", Some("（大图拆分:第1行/第1列,共1行×1列）")),
    ];
    let body = build_chat_body("m", &msgs, &test_params(), vision_caps());
    let arr = body["messages"].as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[1]["role"], "tool");
    assert_eq!(arr[1]["content"], "已截图:输出 100×100");
    assert_eq!(arr[2]["role"], "user");
    assert_eq!(arr[2]["content"][0]["type"], "text"); // 来源说明行
    assert_eq!(
        arr[2]["content"][1],
        json!({"type": "text", "text": "（大图拆分:第1行/第1列,共1行×1列）"})
    );
    assert_eq!(
        arr[2]["content"][2],
        json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}})
    );
    // 能力位关闭:不新增 user 消息,tool 消息仍为纯文本
    let body = build_chat_body("m", &msgs, &test_params(), Default::default());
    let arr = body["messages"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[1]["content"], "已截图:输出 100×100");
}

/// chat:并行工具调用组内不得插其它角色——两条工具结果相邻,图像统一在本组末尾收口
#[test]
fn chat_parallel_tool_images_flush_after_whole_group() {
    let msgs = vec![
        assistant_tool_call("c1"),
        tool_image_message("c1", None),
        tool_image_message("c2", None),
    ];
    let body = build_chat_body("m", &msgs, &test_params(), vision_caps());
    let arr = body["messages"].as_array().unwrap();
    assert_eq!(arr.len(), 4);
    assert_eq!(arr[1]["role"], "tool");
    assert_eq!(arr[2]["role"], "tool");
    assert_eq!(arr[3]["role"], "user");
    assert_eq!(arr[3]["content"].as_array().unwrap().len(), 3); // 说明 + 两张图
}

/// responses:function_call_output.output 保持纯文本,图像以 user 输入项
/// (input_text/input_image 词表)追加在工具组之后
#[test]
fn responses_tool_images_land_in_trailing_user_item() {
    let msgs = vec![assistant_tool_call("c1"), tool_image_message("c1", None)];
    let body = build_responses_body("m", &msgs, &test_params(), vision_caps());
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[0]["type"], "function_call");
    assert_eq!(input[1]["type"], "function_call_output");
    assert_eq!(input[1]["output"], "已截图:输出 100×100");
    assert_eq!(input[2]["role"], "user");
    assert_eq!(input[2]["content"][0]["type"], "input_text");
    assert_eq!(
        input[2]["content"][1],
        json!({"type": "input_image", "image_url": "data:image/png;base64,AAAA"})
    );
}

/// anthropic:图像内嵌 tool_result 的 content 块数组(官方支持),不新增 user 消息;
/// 能力位关闭时 content 回退纯字符串(与改造前一致)
#[test]
fn anthropic_tool_images_embed_in_tool_result() {
    let msgs = vec![
        assistant_tool_call("c1"),
        tool_image_message("c1", Some("（原图总览:100×100）")),
    ];
    let body = build_anthropic_body("m", &msgs, &test_params(), vision_caps());
    let arr = body["messages"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[1]["role"], "user");
    let tr = &arr[1]["content"][0];
    assert_eq!(tr["type"], "tool_result");
    assert_eq!(tr["tool_use_id"], "c1");
    assert_eq!(
        tr["content"][0],
        json!({"type": "text", "text": "已截图:输出 100×100"})
    );
    assert_eq!(
        tr["content"][1],
        json!({"type": "text", "text": "（原图总览:100×100）"})
    );
    assert_eq!(tr["content"][2]["type"], "image");
    assert_eq!(tr["content"][2]["source"]["data"], "AAAA");
    let body = build_anthropic_body("m", &msgs, &test_params(), Default::default());
    assert_eq!(
        body["messages"][1]["content"][0]["content"],
        "已截图:输出 100×100"
    );
}

/// anthropic:上一条 user 是纯文本 + 本轮 user 带图 → 并入同一条(升级为块数组),
/// 不得产生连续 user 消息(Anthropic 要求角色交替,连发 user 会被 400)
#[test]
fn anthropic_image_message_merges_into_plain_text_user() {
    let first = LlmMessage::plain("user", "第一段");
    let body = build_anthropic_body(
        "m",
        &[first, image_message()],
        &test_params(),
        vision_caps(),
    );
    let arr = body["messages"].as_array().unwrap();
    assert_eq!(arr.len(), 1, "连续 user 必须合并为一条");
    assert_eq!(
        arr[0]["content"][0],
        json!({"type": "text", "text": "第一段"})
    );
    assert_eq!(
        arr[0]["content"][1],
        json!({"type": "text", "text": "看图"})
    );
    assert_eq!(arr[0]["content"][2]["type"], "image");
}

// ---------- 能力位消费(2026-10-03 VISION-L6 收口) ----------

fn json_intent_params() -> GenerationParams {
    let mut p = test_params();
    p.response_format = Some(crate::models::types::ResponseFormat::Json);
    p
}

fn structured_caps() -> crate::connectors::ConnectorCapabilities {
    crate::connectors::ConnectorCapabilities {
        supports_structured_output: true,
        ..Default::default()
    }
}

/// 结构化输出下发矩阵:意图 + 能力位共同决定;chat → response_format、
/// responses → text.format、anthropic 无等价参数恒不下发。
#[test]
fn structured_output_intent_gated_by_capability_and_dialect() {
    let msgs = vec![LlmMessage::plain("user", "输出 JSON")];
    // chat:位开 + 意图 → 下发;位关 / 无意图 → 不下发(请求体与既有路径一致)
    let body = build_chat_body("m", &msgs, &json_intent_params(), structured_caps());
    assert_eq!(body["response_format"]["type"], "json_object");
    let body = build_chat_body("m", &msgs, &json_intent_params(), Default::default());
    assert!(body.get("response_format").is_none(), "能力位关不得下发");
    let body = build_chat_body("m", &msgs, &test_params(), structured_caps());
    assert!(body.get("response_format").is_none(), "无意图不得下发");

    // responses:等价参数是 text.format
    let body = build_responses_body("m", &msgs, &json_intent_params(), structured_caps());
    assert_eq!(body["text"]["format"]["type"], "json_object");
    let body = build_responses_body("m", &msgs, &json_intent_params(), Default::default());
    assert!(body.get("text").is_none(), "能力位关不得下发");

    // anthropic:协议无等价参数,位开 + 意图也不下发
    let body = build_anthropic_body("m", &msgs, &json_intent_params(), structured_caps());
    assert!(body.get("response_format").is_none());
    assert!(body.get("text").is_none());
}

/// 中途系统插入(anthropic):位开时对话中途的 system 就地转 user 保序;
/// 头部 system 恒上提;位关路径逐字节维持既有行为(全部上提)。
#[test]
fn anthropic_mid_conversation_system_converts_to_user_when_enabled() {
    let msgs = vec![
        LlmMessage::plain("system", "头部系统"),
        LlmMessage::plain("user", "u1"),
        LlmMessage::plain("assistant", "a1"),
        LlmMessage::plain("system", "中途注入"),
        LlmMessage::plain("assistant", "a2"),
    ];
    let mut caps = crate::connectors::ConnectorCapabilities::default();
    caps.supports_mid_conversation_system = true;

    // 位开:头部上提;中途注入出现在 a1 与 a2 之间、角色为 user
    let body = build_anthropic_body("m", &msgs, &test_params(), caps);
    assert_eq!(body["system"], "头部系统", "头部 system 仍应上提");
    let arr = body["messages"].as_array().unwrap();
    assert_eq!(arr.len(), 4);
    assert_eq!(arr[0]["role"], "user");
    assert_eq!(arr[1]["role"], "assistant");
    assert_eq!(arr[2]["role"], "user", "中途 system 应转 user 保序");
    assert_eq!(arr[2]["content"], "中途注入");
    assert_eq!(arr[3]["role"], "assistant");

    // 位关:既有行为——全部上提拼顶,中途注入不得残留消息数组
    let body = build_anthropic_body("m", &msgs, &test_params(), Default::default());
    assert_eq!(body["system"], "头部系统\n\n中途注入");
    let arr = body["messages"].as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert!(
        !serde_json::to_string(&arr).unwrap().contains("中途注入"),
        "位关时中途注入只进顶部 system"
    );

    // 头部连续多条 system 恒属头部:即便位开也不转 user
    let head_only = vec![
        LlmMessage::plain("system", "首条"),
        LlmMessage::plain("system", "次条"),
        LlmMessage::plain("user", "u1"),
    ];
    let body = build_anthropic_body("m", &head_only, &test_params(), caps);
    assert_eq!(body["system"], "首条\n\n次条");
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
}

/// 中途系统插入(responses):位开时中途 system 转 user 输入项保序;位关上提 instructions。
#[test]
fn responses_mid_conversation_system_converts_to_user_when_enabled() {
    let msgs = vec![
        LlmMessage::plain("system", "头部系统"),
        LlmMessage::plain("user", "u1"),
        LlmMessage::plain("assistant", "a1"),
        LlmMessage::plain("system", "中途注入"),
        LlmMessage::plain("assistant", "a2"),
    ];
    let mut caps = crate::connectors::ConnectorCapabilities::default();
    caps.supports_mid_conversation_system = true;

    let body = build_responses_body("m", &msgs, &test_params(), caps);
    assert_eq!(body["instructions"], "头部系统");
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 4);
    assert_eq!(input[2]["role"], "user", "中途 system 应转 user 保序");
    assert_eq!(input[2]["content"], "中途注入");

    // 位关:既有行为——上提 instructions
    let body = build_responses_body("m", &msgs, &test_params(), Default::default());
    assert_eq!(body["instructions"], "头部系统\n\n中途注入");
    assert_eq!(body["input"].as_array().unwrap().len(), 3);
}

/// chat 方言对中途 system 天然透传:能力位开/关都逐字节保留原角色与位置(行为不变)。
#[test]
fn chat_dialect_passes_mid_system_through_regardless_of_flag() {
    let msgs = vec![
        LlmMessage::plain("system", "头部"),
        LlmMessage::plain("user", "u1"),
        LlmMessage::plain("system", "中途注入"),
        LlmMessage::plain("assistant", "a1"),
    ];
    let mut caps = crate::connectors::ConnectorCapabilities::default();
    caps.supports_mid_conversation_system = true;
    for (label, caps) in [("位开", caps), ("位关", Default::default())] {
        let body = build_chat_body("m", &msgs, &test_params(), caps);
        let arr = body["messages"].as_array().unwrap();
        assert_eq!(arr.len(), 4, "{label}:chat 应逐条透传");
        assert_eq!(arr[2]["role"], "system", "{label}:中途 system 角色不变");
        assert_eq!(arr[2]["content"], "中途注入");
    }
}
