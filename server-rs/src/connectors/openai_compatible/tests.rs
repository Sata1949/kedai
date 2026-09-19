// OpenAI 兼容连接器单测(自 openai_compatible.rs 尾部 #[cfg(test)] 迁入)
use super::sse_parser::{parse_usage, MAX_BAD_JSON_EVENTS};
use super::*;
use crate::models::llm_error::LlmErrorKind;
use crate::models::types::{ToolCallArgs, ToolDefinition};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};

fn test_params() -> GenerationParams {
    GenerationParams {
        temperature: 0.8,
        top_p: 0.9,
        max_tokens: 128,
        stop: None,
        tools: Vec::new(),
        max_tool_rounds: None,
        tool_choice: crate::models::types::ToolChoice::Auto,
        parallel_tool_calls: None,
    }
}

async fn slow_sse_server(delay: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let _ = socket.read(&mut request).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        let first = b"data: {\"choices\":[{\"delta\":{\"content\":\"first\"}}]}\n\n";
        socket
            .write_all(format!("{:X}\r\n", first.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(first).await.unwrap();
        socket.write_all(b"\r\n").await.unwrap();
        socket.flush().await.unwrap();
        tokio::time::sleep(delay).await;
        let done = b"data: [DONE]\n\n";
        socket
            .write_all(format!("{:X}\r\n", done.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(done).await.unwrap();
        socket.write_all(b"\r\n0\r\n\r\n").await.unwrap();
    });
    format!("http://{addr}")
}

/// 多轮工具调用回传:assistant 消息必须带 reasoning_content(DeepSeek 思考模式要求)
#[test]
fn tool_messages_echo_reasoning() {
    let msgs = vec![
        LlmMessage {
            role: "assistant".into(),
            content: String::new(),
            reasoning_content: Some("先掷骰再回答".into()),
            tool_calls: Some(vec![ToolCallArgs {
                id: "call_1".into(),
                name: "role".into(),
                arguments: r#"{"sides":6}"#.into(),
            }]),
            tool_call_id: None,
        },
        LlmMessage {
            role: "tool".into(),
            content: r#"{"rolls":[4]}"#.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some("call_1".into()),
        },
    ];
    let out = to_openai_messages(&msgs);
    // assistant:tool_calls 标准结构 + reasoning_content 回传
    assert_eq!(out[0]["role"], "assistant");
    assert_eq!(out[0]["tool_calls"][0]["type"], "function");
    assert_eq!(out[0]["tool_calls"][0]["function"]["name"], "role");
    assert_eq!(out[0]["reasoning_content"], "先掷骰再回答");
    // tool:tool_call_id + content
    assert_eq!(out[1]["role"], "tool");
    assert_eq!(out[1]["tool_call_id"], "call_1");
    assert_eq!(out[1]["content"], r#"{"rolls":[4]}"#);
}

/// SSE 数据可能在任意字节位置分块,只有完整事件才应产出 token。
#[test]
fn sse_parser_handles_arbitrary_chunk_boundaries() {
    let mut parser = SseParser::default();
    let mut out = Vec::new();
    for part in [
        b"data: {\"choices\":[{\"delta\":{\"content\":\"\xe4".as_slice(),
        b"\xbd\xa0\"}}]}\r".as_slice(),
        b"\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"hao\"}}]}\n\n".as_slice(),
        b"data: [DONE]\n\n".as_slice(),
    ] {
        parser.push(part, &mut out).unwrap();
    }
    assert!(parser.is_done());
    assert_eq!(out.len(), 2);
    assert!(matches!(&out[0], LlmStreamChunk::Token(text) if text == "你"));
    assert!(matches!(&out[1], LlmStreamChunk::Token(text) if text == "hao"));
}

/// 慢速上游的首块必须在流结束前交给调用方。
#[tokio::test]
async fn generate_stream_emits_before_response_finishes() {
    let base_url = slow_sse_server(Duration::from_secs(2)).await;
    let connector = OpenAiCompatibleConnector::new(&base_url, "key", "model");
    let (_abort_tx, abort_rx) = watch::channel(false);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        connector
            .generate_stream(
                &[LlmMessage::plain("user", "hi")],
                test_params(),
                abort_rx,
                tx,
            )
            .await
    });
    let chunk = timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(chunk, LlmStreamChunk::Token(text) if text == "first"));
    task.abort();
}

/// 等待慢速上游下一块时,abort 应立即结束读取而不是等网络返回。
#[tokio::test]
async fn generate_stream_aborts_while_waiting_for_next_chunk() {
    let base_url = slow_sse_server(Duration::from_secs(5)).await;
    let connector = OpenAiCompatibleConnector::new(&base_url, "key", "model");
    let (abort_tx, abort_rx) = watch::channel(false);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        connector
            .generate_stream(
                &[LlmMessage::plain("user", "hi")],
                test_params(),
                abort_rx,
                tx,
            )
            .await
    });
    let _ = timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap();
    abort_tx.send(true).unwrap();
    let result = timeout(Duration::from_millis(500), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err().message(), "生成已中断");
}

/// usage 解析:DeepSeek 格式带 prompt_cache_hit_tokens/prompt_cache_miss_tokens(命中率数据源)
#[test]
fn parse_usage_reads_cache_hit_tokens() {
    // DeepSeek 风格:prompt_tokens = hit + miss
    let u = serde_json::json!({
        "prompt_tokens": 1000,
        "completion_tokens": 200,
        "total_tokens": 1200,
        "prompt_cache_hit_tokens": 700,
        "prompt_cache_miss_tokens": 300,
    });
    assert_eq!(parse_usage(&u), (1000, 200, 1200, 700, 300, 0));

    // 无缓存字段的提供商(OpenAI 等)→ hit/miss 为 0,不 panic
    let plain = serde_json::json!({
        "prompt_tokens": 500,
        "completion_tokens": 50,
        "total_tokens": 550,
    });
    assert_eq!(parse_usage(&plain), (500, 50, 550, 0, 0, 0));

    // 字段缺失/类型异常 → 全部回退 0
    assert_eq!(parse_usage(&serde_json::json!({})), (0, 0, 0, 0, 0, 0));
}

/// usage 解析:OpenAI 风格 prompt_tokens_details.cached_tokens 兼容——
/// hit 取 cached_tokens,miss 由 prompt_tokens - cached 推导
#[test]
fn parse_usage_reads_openai_cached_tokens() {
    let u = serde_json::json!({
        "prompt_tokens": 1000,
        "completion_tokens": 50,
        "total_tokens": 1050,
        "prompt_tokens_details": { "cached_tokens": 600 },
    });
    assert_eq!(parse_usage(&u), (1000, 50, 1050, 600, 400, 0));

    // 两种风格并存时 DeepSeek 字段优先
    let both = serde_json::json!({
        "prompt_tokens": 1000,
        "completion_tokens": 50,
        "total_tokens": 1050,
        "prompt_cache_hit_tokens": 800,
        "prompt_cache_miss_tokens": 200,
        "prompt_tokens_details": { "cached_tokens": 600 },
    });
    assert_eq!(parse_usage(&both), (1000, 50, 1050, 800, 200, 0));
}

/// usage 解析:推理模型 completion_tokens_details.reasoning_tokens(诊断空输出的关键证据)
#[test]
fn parse_usage_reads_reasoning_tokens() {
    let u = serde_json::json!({
        "prompt_tokens": 800,
        "completion_tokens": 10000,
        "total_tokens": 10800,
        "completion_tokens_details": { "reasoning_tokens": 10000 },
    });
    assert_eq!(parse_usage(&u), (800, 10000, 10800, 0, 0, 10000));
}

/// finish_reason:stop/length 产出 Finish 块;tool_calls 同样产出
/// Finish{tool_calls}(F6,2026-09-10:带工具轮 finish 列此前落空串)。
#[test]
fn sse_parser_emits_finish_chunk() {
    let mut parser = SseParser::default();
    let mut out = Vec::new();
    parser
        .push(
            b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            &mut out,
        )
        .unwrap();
    assert!(
        matches!(&out[..], [crate::models::types::LlmStreamChunk::Finish { reason }] if reason == "length"),
        "length 应产出 Finish 块: {out:?}"
    );

    let mut parser2 = SseParser::default();
    let mut out2 = Vec::new();
    parser2
        .push(
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"tool_calls\"}]}\n\n",
            &mut out2,
        )
        .unwrap();
    assert!(
        out2
            .iter()
            .any(|c| matches!(c, crate::models::types::LlmStreamChunk::Finish { reason } if reason == "tool_calls")),
        "tool_calls 应产出 Finish{{tool_calls}} 块(F6): {out2:?}"
    );
}

/// 启动一个模拟上游:捕获请求体后返回固定 SSE 流;返回 (base_url, body 持有者)
async fn capture_request_server() -> (String, Arc<std::sync::Mutex<Option<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let holder = Arc::new(std::sync::Mutex::new(None::<String>));
    let holder2 = holder.clone();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        let n = socket.read(&mut request).await.unwrap();
        let raw = String::from_utf8_lossy(&request[..n]).to_string();
        if let Some(idx) = raw.find("\r\n\r\n") {
            *holder2.lock().unwrap_or_else(|e| e.into_inner()) = Some(raw[idx + 4..].to_string());
        }
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

/// 启动一个只返回固定 HTTP 错误的模拟上游(不重试的状态用);返回 base_url。
async fn fixed_status_server(status_line: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let _ = socket.read(&mut request).await;
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{{\"e\":\"r\"}}\n"
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.flush().await;
    });
    format!("http://{addr}")
}

/// 批次 4.3:流内错误对象的分类取自上游结构化 type/code 字段(非自由文案),
/// 保证 HTTP 200 流内上报的限流/鉴权仍得到正确错误码与可重试性。
#[test]
fn sse_parser_classifies_upstream_error_object_by_structured_type() {
    let cases: [(&[u8], LlmErrorKind, &str, bool); 5] = [
        (
            b"data: {\"error\":{\"message\":\"slow\",\"type\":\"request_timeout\"}}\n\n",
            LlmErrorKind::Timeout,
            "request_timeout",
            true,
        ),
        (
            b"data: {\"error\":{\"message\":\"rate limited\",\"type\":\"rate_limit_exceeded\"}}\n\n",
            LlmErrorKind::RateLimited,
            "rate_limited",
            true,
        ),
        (
            b"data: {\"error\":{\"message\":\"bad key\",\"type\":\"invalid_api_key\"}}\n\n",
            LlmErrorKind::AuthFailed,
            "auth_failed",
            false,
        ),
        (
            b"data: {\"error\":{\"message\":\"boom\",\"code\":\"server_error\"}}\n\n",
            LlmErrorKind::Upstream,
            "upstream_error",
            true,
        ),
        (
            b"data: {\"error\":{\"message\":\"over quota\",\"type\":\"insufficient_quota\"}}\n\n",
            LlmErrorKind::Upstream,
            "upstream_error",
            true,
        ),
    ];
    for (payload, kind, code, retryable) in cases {
        let mut parser = SseParser::default();
        let mut out = Vec::new();
        let err = parser.push(payload, &mut out).unwrap_err();
        assert_eq!(err.kind(), kind, "分类错误: {err}");
        assert_eq!(err.code(), code, "错误码错误: {err}");
        assert_eq!(err.retryable(), retryable, "可重试性错误: {err}");
    }
}

/// 批次 4.3:HTTP 状态在连接器边界被映射为分类(鉴权/参数错误不可重试),
/// 上游文案原样保留——上层不再解析文案判定 code/retryable。
#[tokio::test]
async fn http_status_maps_to_typed_error_kind() {
    for (status_line, kind, code, retryable) in [
        (
            "401 Unauthorized",
            crate::models::llm_error::LlmErrorKind::AuthFailed,
            "auth_failed",
            false,
        ),
        (
            "403 Forbidden",
            crate::models::llm_error::LlmErrorKind::AuthFailed,
            "auth_failed",
            false,
        ),
        (
            "400 Bad Request",
            crate::models::llm_error::LlmErrorKind::Generation,
            "generation_failed",
            false,
        ),
    ] {
        let base_url = fixed_status_server(status_line).await;
        let connector = OpenAiCompatibleConnector::new(&base_url, "key", "model");
        let (_abort_tx, abort_rx) = watch::channel(false);
        let (tx, _rx) = mpsc::unbounded_channel();
        let err = connector
            .generate_stream(
                &[LlmMessage::plain("user", "hi")],
                test_params(),
                abort_rx,
                tx,
            )
            .await
            .expect_err("非成功状态应报错");
        assert_eq!(err.kind(), kind, "{status_line} 分类错误");
        assert_eq!(err.code(), code, "{status_line} 错误码错误");
        assert_eq!(err.retryable(), retryable, "{status_line} 可重试性错误");
        assert!(
            err.message()
                .contains(status_line.split(' ').next().unwrap()),
            "应保留上游状态文案: {err}"
        );
    }
}

/// M2:tool_choice 与 parallel_tool_calls 按 GenerationParams 序列化进请求体;
/// 默认 Auto 时保持 "auto"(与旧行为一致)。
#[tokio::test]
async fn tool_choice_and_parallel_flag_serialized_in_request_body() {
    for (choice, expect) in [
        (crate::models::types::ToolChoice::Auto, json!("auto")),
        (
            crate::models::types::ToolChoice::Function("read".into()),
            json!({ "type": "function", "function": { "name": "read" } }),
        ),
    ] {
        let (base_url, body) = capture_request_server().await;
        let connector = OpenAiCompatibleConnector::new(&base_url, "key", "model");
        let (_abort_tx, abort_rx) = watch::channel(false);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut params = test_params();
        params.tools = vec![ToolDefinition {
            name: "read".into(),
            description: "read".into(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        params.tool_choice = choice;
        params.parallel_tool_calls = Some(false);
        connector
            .generate_stream(&[LlmMessage::plain("user", "hi")], params, abort_rx, tx)
            .await
            .unwrap();
        let raw = body
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["tool_choice"], expect, "tool_choice 序列化错误: {raw}");
        assert_eq!(v["parallel_tool_calls"], json!(false));
        assert!(v["tools"].is_array(), "tools 应下发: {raw}");
        assert_eq!(
            v["stream_options"],
            json!({ "include_usage": true }),
            "流式请求应声明 include_usage 以拿到 usage: {raw}"
        );
    }
}

/// M2:标准 OpenAI error 对象应暴露为错误,不再被静默吞掉(否则前端收到空成功 finish)
#[test]
fn sse_parser_surfaces_upstream_error_object() {
    let mut parser = SseParser::default();
    let mut out = Vec::new();
    let err = parser
        .push(
            b"data: {\"error\":{\"message\":\"insufficient_quota\",\"type\":\"insufficient_quota\"}}\n\n",
            &mut out,
        )
        .unwrap_err();
    assert!(
        err.contains("insufficient_quota"),
        "应暴露上游错误对象: {err}"
    );
}

/// M2:非 JSON data 行(keepalive/注释等)少量容忍,超过阈值视为协议损坏报错
#[test]
fn sse_parser_tolerates_bad_json_then_fails_at_threshold() {
    let mut parser = SseParser::default();
    let mut out = Vec::new();
    for _ in 0..5 {
        parser.push(b"data: ping\n\n", &mut out).unwrap();
    }
    assert!(out.is_empty(), "少量非 JSON 应静默忽略");

    let mut parser2 = SseParser::default();
    let mut out2 = Vec::new();
    let mut failed_at: Option<usize> = None;
    for i in 0..(MAX_BAD_JSON_EVENTS + 2) {
        let r = parser2.push(b"data: garbage\n\n", &mut out2);
        if r.is_err() {
            failed_at = Some(i);
            break;
        }
    }
    assert!(
        failed_at.is_some(),
        "连续非 JSON 达到阈值应报错(当前 {MAX_BAD_JSON_EVENTS})"
    );
    // bad_json_count 从 1 计数,达到阈值(20)即报错 → 第 20 个事件(0-indexed 19)失败
    assert_eq!(
        failed_at.unwrap(),
        MAX_BAD_JSON_EVENTS - 1,
        "应在恰好第 {MAX_BAD_JSON_EVENTS} 个坏事件时报错"
    );
}

/// M2:工具调用只在 finish_reason == "tool_calls" 时 flush;
/// stop/length 等其他 finish 不提前 flush,避免半截调用被当作完整调用发出。
/// F6(2026-09-10):tool_calls 完成时同时产出 Finish{tool_calls}——
/// 带工具轮此前 finish 无值,任务模式调用面板 finish 列空白。
#[test]
fn sse_parser_flushes_tool_calls_only_on_tool_calls_finish() {
    let mut parser = SseParser::default();
    let mut out = Vec::new();
    // 先聚合一个工具调用,但 finish_reason=stop → 不应 flush(stop 会产出 Finish 诊断块,但不产 ToolCall)
    parser
        .push(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"q\\\":1}\"}}]},\"finish_reason\":\"stop\"}]}\n\n",
            &mut out,
        )
        .unwrap();
    assert!(
        !out.iter().any(|c| matches!(c, LlmStreamChunk::ToolCall(_))),
        "非 tool_calls finish 不应 flush 工具调用: {out:?}"
    );
    // finish_reason=tool_calls → flush,且产出 Finish{tool_calls}(F6)
    parser
        .push(
            b"data: {\"choices\":[{\"finish_reason\":\"tool_calls\"}]}\n\n",
            &mut out,
        )
        .unwrap();
    assert!(
        out.iter()
            .any(|c| matches!(c, LlmStreamChunk::ToolCall(c) if c.name == "read")),
        "tool_calls finish 应 flush 已聚合调用: {out:?}"
    );
    assert!(
        out.iter()
            .any(|c| matches!(c, LlmStreamChunk::Finish { reason } if reason == "tool_calls")),
        "tool_calls finish 应产出 Finish{{tool_calls}} 供上层落库(F6): {out:?}"
    );
}

/// M2:工具 arguments 非法 JSON 时 flush 报错(完整性校验),不再把坏参数交给执行器
#[test]
fn sse_parser_rejects_invalid_tool_arguments() {
    let mut parser = SseParser::default();
    let mut out = Vec::new();
    let err = parser
        .push(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"read\",\"arguments\":\"not-json\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            &mut out,
        )
        .unwrap_err();
    assert!(
        err.contains("不是合法 JSON"),
        "非法 arguments 应在 flush 时暴露: {err}"
    );
}

/// M2:重复工具调用 id → flush 报错(完整性校验)
#[test]
fn sse_parser_rejects_duplicate_tool_call_ids() {
    let mut parser2 = SseParser::default();
    let mut out2 = Vec::new();
    let err2 = parser2
        .push(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"dup\",\"function\":{\"name\":\"read\",\"arguments\":\"{}\"}},{\"index\":1,\"id\":\"dup\",\"function\":{\"name\":\"role\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            &mut out2,
        )
        .unwrap_err();
    assert!(err2.contains("重复工具调用 id"), "重复 id 应报错: {err2}");
}

// ===== 空闲看门狗口径(2026-09-18 修正)=====
//
// 背景:此前按「收到任何字节」重置空闲计时,而上游可以在建流后只发 SSE 注释心跳
// (`: ping`)——字节在来、正文不出,看门狗永不触发,读取循环可无限挂起。
// 这组用例锁「只有有效数据事件才算推进」与「注释不续命」两条口径。

/// 注释心跳不算推进:只有合法 JSON data 行计 1,注释/空行计 0。
#[test]
fn sse_comment_heartbeat_is_not_progress() {
    let mut parser = SseParser::default();
    let mut out = Vec::new();

    // 仅注释行(SSE 心跳的标准形态),无 data 前缀 → 0
    let n = parser.push(b": ping\n\n", &mut out).unwrap();
    assert_eq!(n, 0, "注释行不得算作推进信号");
    assert!(out.is_empty(), "注释行不应产出任何块");

    // 合法数据事件 → 1
    let n = parser
        .push(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            &mut out,
        )
        .unwrap();
    assert_eq!(n, 1, "合法 JSON data 应记一次推进");
    assert!(matches!(&out[0], LlmStreamChunk::Token(t) if t == "hi"));

    // [DONE] 也是推进(流正常收尾,不是停滞)
    let n = parser.push(b"data: [DONE]\n\n", &mut out).unwrap();
    assert_eq!(n, 1, "[DONE] 应记一次推进");
    assert!(parser.is_done());
}

/// 非 JSON 的 data 行(部分网关的 keepalive 形态)容忍但不算推进。
#[test]
fn sse_non_json_data_line_is_not_progress() {
    let mut parser = SseParser::default();
    let mut out = Vec::new();
    let n = parser.push(b"data: keep-alive\n\n", &mut out).unwrap();
    assert_eq!(n, 0, "非 JSON data 行不构成推进");
}

/// 一次 push 含多个事件时累加计数(网络分块边界不影响判定)。
#[test]
fn sse_progress_counts_all_events_in_one_chunk() {
    let mut parser = SseParser::default();
    let mut out = Vec::new();
    let n = parser
        .push(
            b": ping\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n",
            &mut out,
        )
        .unwrap();
    assert_eq!(n, 2, "同批 2 个数据事件 + 1 个注释 → 计数为 2");
}

/// 核心回归:上游只发注释心跳(无任何数据事件)→ 空闲阈值到达后报超时,不再无限等待。
/// 用合成流 + 毫秒级阈值,不必真等 120s。
#[tokio::test]
async fn heartbeat_only_stream_times_out_instead_of_hanging() {
    use futures::stream;
    use std::time::Duration as StdDuration;

    // 注释心跳无限重复:模拟「上游活着但不产出正文」的停滞形态
    let heartbeats = stream::repeat_with(|| Ok::<_, std::io::Error>(b": ping\n\n".to_vec()));
    let (_abort_tx, abort_rx) = watch::channel(false);
    let (tx, mut rx) = mpsc::unbounded_channel();

    let started = tokio::time::Instant::now();
    let err = read_sse_stream(heartbeats, abort_rx, tx, StdDuration::from_millis(300))
        .await
        .expect_err("只发注释心跳必须超时报错,而非永久挂起");
    assert!(err.message().contains("停滞超时"), "实际:{}", err.message());
    // 断言真的在阈值量级返回(自动化用例不能等 120s)
    assert!(
        started.elapsed() < StdDuration::from_secs(5),
        "应在毫秒级阈值内返回,实际耗时 {:?}",
        started.elapsed()
    );
    assert!(rx.try_recv().is_err(), "注释不产出任何块");
}

/// 数据事件持续产出时不会被空闲阈值误杀(推进即续期)。
#[tokio::test]
async fn steady_data_events_do_not_trigger_idle_timeout() {
    use futures::stream;
    use std::time::Duration as StdDuration;

    let chunks: Vec<Result<Vec<u8>, std::io::Error>> = vec![
        Ok(b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n".to_vec()),
        Ok(b"data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n".to_vec()),
        Ok(b"data: [DONE]\n\n".to_vec()),
    ];
    // 每个事件之间隔 150ms,小于 400ms 空闲阈值:推进应不断续期至正常收尾
    let paced = stream::iter(chunks).then(|c| async move {
        tokio::time::sleep(StdDuration::from_millis(150)).await;
        c
    });
    let (_abort_tx, abort_rx) = watch::channel(false);
    let (tx, mut rx) = mpsc::unbounded_channel();

    read_sse_stream(paced, abort_rx, tx, StdDuration::from_millis(400))
        .await
        .expect("有数据推进时不应超时");

    let mut texts = Vec::new();
    while let Ok(c) = rx.try_recv() {
        if let LlmStreamChunk::Token(t) = c {
            texts.push(t);
        }
    }
    assert_eq!(texts, vec!["a".to_string(), "b".to_string()]);
}

/// 中断优先于读取:注释心跳不断时,abort 应立即结束(不被字节流拖住)。
#[tokio::test]
async fn abort_wins_over_continuous_heartbeats() {
    use futures::stream;
    use std::time::Duration as StdDuration;

    let heartbeats = stream::repeat_with(|| Ok::<_, std::io::Error>(b": ping\n\n".to_vec()));
    let (abort_tx, abort_rx) = watch::channel(false);
    let (tx, _rx) = mpsc::unbounded_channel();

    let task = tokio::spawn(async move {
        read_sse_stream(heartbeats, abort_rx, tx, StdDuration::from_secs(30)).await
    });
    tokio::time::sleep(StdDuration::from_millis(80)).await;
    abort_tx.send(true).unwrap();
    let err = tokio::time::timeout(StdDuration::from_secs(2), task)
        .await
        .expect("abort 应立即生效,不该等到 30s 空闲阈值")
        .unwrap()
        .expect_err("中断应返回错误");
    assert_eq!(err.message(), "生成已中断");
}

/// 退避序列与 Retry-After 上限(HB-4 补测):此前 retry_delay 无任何覆盖——
/// 上游 429 时的等待时长既影响体验也影响重试是否真能生效。
#[test]
fn retry_delay_backoff_sequence_and_retry_after_cap() {
    // 无 Retry-After:500ms 起步指数增长,封顶 5s;带 ≤250ms jitter(以系统时钟纳秒做,
    // 故断言区间而非定值)
    let d1 = retry_delay(1, None).as_millis();
    assert!((500..750).contains(&d1), "attempt=1 应为 500ms+jitter: {d1}");
    let d2 = retry_delay(2, None).as_millis();
    assert!((1000..1250).contains(&d2), "attempt=2 应为 1000ms+jitter: {d2}");
    let d3 = retry_delay(3, None).as_millis();
    assert!((2000..2250).contains(&d3), "attempt=3 应为 2000ms+jitter: {d3}");
    let d5 = retry_delay(5, None).as_millis();
    assert!((5000..5250).contains(&d5), "指数增长必须封顶 5s: {d5}");
    // Retry-After 以秒为准,并封顶 30s(上游误配 60 不得真等一分钟)
    assert_eq!(retry_delay(1, Some(5)), Duration::from_secs(5));
    assert_eq!(retry_delay(2, Some(60)), Duration::from_secs(30));
    assert_eq!(retry_delay(1, Some(0)), Duration::from_secs(0));
}

/// 上游 429 后重试成功:重试提示块必须先于正文产出(HB-4)——
/// 此前重试只写 tracing,界面表现为「停几十秒然后报错」。
/// 响应带 Retry-After: 0,用例无需真等退避。
#[tokio::test]
async fn retry_emits_notice_before_success() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        // 第一次:429 + Connection: close + Retry-After: 0
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let _ = socket.read(&mut request).await;
        socket
            .write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nConnection: close\r\nRetry-After: 0\r\nContent-Length: 2\r\n\r\n{}",
            )
            .await
            .unwrap();
        socket.flush().await.unwrap();
        drop(socket);
        // 第二次:200 + 正常 SSE 流
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let _ = socket.read(&mut request).await;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n",
            )
            .await
            .unwrap();
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
        socket
            .write_all(format!("{:X}\r\n", body.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(body).await.unwrap();
        socket.write_all(b"\r\n0\r\n\r\n").await.unwrap();
        socket.flush().await.unwrap();
    });

    let connector = OpenAiCompatibleConnector::new(&format!("http://{addr}"), "key", "model");
    let (_abort_tx, abort_rx) = watch::channel(false);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        connector
            .generate_stream(
                &[LlmMessage::plain("user", "hi")],
                test_params(),
                abort_rx,
                tx,
            )
            .await
    });

    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("重试提示应立即产出,不等退避")
        .unwrap();
    match first {
        LlmStreamChunk::Retry {
            attempt,
            max,
            reason,
        } => {
            assert_eq!((attempt, max), (1, MAX_ATTEMPTS), "首次重试应为 1/{MAX_ATTEMPTS}");
            assert!(reason.contains("429"), "原因应带上游状态码: {reason}");
        }
        other => panic!("首块应为重试提示: {other:?}"),
    }
    // 重试成功后照常产出正文
    let next = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(next, LlmStreamChunk::Token(t) if t == "ok"), "重试后应正常出块");
    assert!(task.await.unwrap().is_ok(), "重试后整体应成功");
}

/// 不可重试状态直接失败(不产生重试提示):401 鉴权类错误不得被当成瞬时故障
#[tokio::test]
async fn non_retryable_status_fails_without_retry_notice() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let _ = socket.read(&mut request).await;
        socket
            .write_all(b"HTTP/1.1 401 Unauthorized\r\nConnection: close\r\nContent-Length: 2\r\n\r\n{}")
            .await
            .unwrap();
        socket.flush().await.unwrap();
    });

    let connector = OpenAiCompatibleConnector::new(&format!("http://{addr}"), "key", "model");
    let (_abort_tx, abort_rx) = watch::channel(false);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let err = connector
        .generate_stream(
            &[LlmMessage::plain("user", "hi")],
            test_params(),
            abort_rx,
            tx,
        )
        .await
        .expect_err("401 应直接失败");
    assert_eq!(err.kind(), LlmErrorKind::AuthFailed);
    assert!(
        rx.try_recv().is_err(),
        "不可重试错误不得产出任何块(含重试提示)"
    );
}
