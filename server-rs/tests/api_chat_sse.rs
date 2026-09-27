// API 集成测试 · 聊天发送与 SSE 帧（并发保留 / 重发校验 / finish_reason / token 计数 / 重试通知 / 多线程运行时）。
//
// 本文件由原 `tests/api_integration.rs`（137KB / 62 用例）按 `/api/*` 路由域拆分而来
// （QUALITY-FIX Q1-2,2026-09-27）：纯移动、不改行为。**拆分的实质收益是测试隔离**——
// `tests/` 下每个 `.rs` 是独立测试进程，`build_test_app()` 的 `DATA_DIR`
// （`%TEMP%\kedai-test-<pid>`）与 `OnceLock` 单例 app 随文件独立，原先「同进程共享
// settings.json / 设置覆盖层 / 流程库」的顺序耦合由此消失（既有 flake `TEST-ISO-1` 正是这一土壤）。
// **别再把它们合回一个文件。**
//
// 测试辅助函数按「谁用谁带」复制（仓库既有体例：40 个测试文件里 36 个各自持有 `test_app`），
// 刻意不建 `tests/common` 共享层：那会让 count-tests 的集成文件数口径虚高，也会给多会话并行
// 改测试制造新的争用点。`test_lock()` 是**本二进制内**的串行锁，随文件带走即可。
//
// 共用前提：与 Node 版 `server/tests/api.test.ts` 对齐；mock 连接器 + 临时数据目录 + 免鉴权。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

fn test_app() -> &'static axum::Router {
    static APP: OnceLock<axum::Router> = OnceLock::new();
    APP.get_or_init(|| build_test_app().expect("构建测试应用失败"))
}

async fn send_json(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn upload_character(app: &axum::Router, name: &str) -> (StatusCode, Value) {
    // 构造 multipart 表单:字段 file
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        json!({
            "spec": "chara_card_v2",
            "spec_version": "1.0",
            "name": "测试角色",
            "description": "测试描述",
            "first_mes": "你好,我是测试角色",
            "custom_field": { "unknown": true }
        })
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/characters/upload")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// 串行化修改全局配置(settings / agent-flows)的测试,避免同进程内相互污染
async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // 测试进程级:把运行时主提示词目录指向空目录,避免真实项目根的 AGENTS_RUNTIME.md
    // 被注入 mock 断言(与 build_test_app 临时数据目录的隔离语义一致)。Once 保证只设一次。
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // uuid 唯一名(避免多测试二进制并发共用同名目录);守卫随本闭包析构即回收。
        // 目录是否存在不影响语义:with_dir 关闭内置默认回退,读不到文件即视为「无提示词」,
        // 与「指向一个空目录」等价(全仓无测试写该文件)。
        let dir = kedai_server::utils::test_support::TempDataDir::new("test-runtime-prompt-empty");
        std::env::set_var("KEDAI_RUNTIME_PROMPT_DIR", dir.path());
    });
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

/// 以指定 agent_mode 发送消息,解析 SSE 事件列表
async fn sse_events(
    app: &axum::Router,
    sid: &str,
    cid: &str,
    message: &str,
    agent_mode: &str,
) -> Vec<Value> {
    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "session_id": sid,
                "character_id": cid,
                "message": message,
                "agent_mode": agent_mode,
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "chat/send 应 200");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    text.split("\n\n")
        .filter_map(|block| {
            let block = block.trim();
            if block.is_empty() {
                return None;
            }
            let data_line = block.lines().find(|l| l.starts_with("data: "))?;
            serde_json::from_str(&data_line[6..]).ok()
        })
        .collect()
}

#[tokio::test]
async fn chat_send_sse() {
    let app = test_app();
    let (_, char) = upload_character(app, "SSE测试.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "session_id": sid, "character_id": cid, "message": "你好" }).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("text/event-stream"));

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    // 解析 SSE 事件
    let events: Vec<Value> = text
        .split("\n\n")
        .filter_map(|block| {
            let block = block.trim();
            if block.is_empty() {
                return None;
            }
            let data_line = block.lines().find(|l| l.starts_with("data: "))?;
            serde_json::from_str(&data_line[6..]).ok()
        })
        .collect();
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    // 至少包含 step(计划中…) 与 finish
    assert!(types.contains(&"step"), "事件缺 step: {types:?}");
    assert!(types.contains(&"finish"), "事件缺 finish: {types:?}");
    let finish = events.iter().find(|e| e["type"] == "finish").unwrap();
    assert!(!finish["content"].as_str().unwrap().is_empty());
    assert!(finish["usage"]["context_tokens"].is_number());
}

#[tokio::test]
async fn chat_send_validation() {
    let app = test_app();
    // 空消息 → 400
    let (status, _) = send_json(
        app,
        "POST",
        "/api/chat/send",
        json!({ "message": "  ", "character_id": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // 缺 session_id 和 character_id → 400
    let (status, _) = send_json(app, "POST", "/api/chat/send", json!({ "message": "hi" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // 会话不存在 → 404
    let (status, _) = send_json(
        app,
        "POST",
        "/api/chat/send",
        json!({ "message": "hi", "session_id": "no-such-session" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn concurrent_send_reserves_session_before_message_insert() {
    let app = test_app();
    let (_, character) = upload_character(app, "并发占位.json").await;
    let cid = character["id"].as_str().unwrap();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap();
    let make = || {
        Request::builder()
            .method("POST")
            .uri("/api/chat/send")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "session_id": sid, "message": "并发消息 [[sleep:200]]" }).to_string(),
            ))
            .unwrap()
    };
    let (first, second) = tokio::join!(app.clone().oneshot(make()), app.clone().oneshot(make()));
    let statuses = [first.unwrap().status(), second.unwrap().status()];
    assert!(statuses.contains(&StatusCode::OK));
    assert!(statuses.contains(&StatusCode::CONFLICT));
}

#[tokio::test]
async fn resend_requires_current_matching_user_message() {
    let app = test_app();
    let (_, character) = upload_character(app, "安全重发.json").await;
    let cid = character["id"].as_str().unwrap();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap();
    let (_, _) = send_json(
        app,
        "POST",
        "/api/import/chat",
        json!({ "session_id": sid, "messages": [{ "role": "user", "content": "原文" }] }),
    )
    .await;
    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let id = history["messages"][0]["id"].as_i64().unwrap();
    let (status, _) = send_json(
        app,
        "POST",
        "/api/chat/send",
        json!({ "session_id": sid, "message": "篡改", "resend_message_id": id }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn token_calculator_sse() {
    let app = test_app();
    let (_, char) = upload_character(app, "计算测试.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "session_id": sid, "character_id": cid, "message": "帮我算一下 12*34" })
                .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let events: Vec<Value> = text
        .split("\n\n")
        .filter_map(|block| {
            let block = block.trim();
            if block.is_empty() {
                return None;
            }
            let data_line = block.lines().find(|l| l.starts_with("data: "))?;
            serde_json::from_str(&data_line[6..]).ok()
        })
        .collect();
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(types.contains(&"tool_call"), "缺 tool_call: {types:?}");
    assert!(types.contains(&"tool_result"), "缺 tool_result: {types:?}");
    let tool_result = events.iter().find(|e| e["type"] == "tool_result").unwrap();
    assert_eq!(tool_result["output"]["result"].as_f64(), Some(408.0));
}

/// 可观测性问题①(2026-09-15):聊天路径的 finish 事件须透出 finish_reason。
/// 此前 SseEvent::Finish 只有 usage/content,聊天被 max_tokens 截断时前端完全静默
/// (任务模式早有「截断」徽标)。此处经 mock [[finish:length|…]] 钩子直造截断终态。
#[tokio::test]
async fn chat_finish_event_exposes_finish_reason_on_truncation() {
    let _guard = test_lock().await;
    let app = test_app();
    let (_, character) = upload_character(app, "截断测试.json").await;
    let cid = character["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    // [[finish:length|半截正文]] → 产出该正文并带 finish_reason=length
    let events = sse_events(
        app,
        &sid,
        &cid,
        "[[finish:length|这段回复在输出上限处被截断]]",
        "fast",
    )
    .await;
    let finish = events
        .iter()
        .find(|e| e["type"] == "finish")
        .expect("缺 finish 事件");
    assert_eq!(
        finish["finish_reason"], "length",
        "聊天 finish 事件应透出 finish_reason: {finish}"
    );

    // 正常收尾(stop)不得被误标截断
    let (_, session2) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid2 = session2["id"].as_str().unwrap().to_string();
    let events2 = sse_events(app, &sid2, &cid, "[[finish:stop|完整回复]]", "fast").await;
    let finish2 = events2
        .iter()
        .find(|e| e["type"] == "finish")
        .expect("缺 finish 事件");
    assert_eq!(finish2["finish_reason"], "stop", "正常收尾应为 stop");

    // 落库的 assistant 消息带 extra.truncated=true(刷新后提示仍在)
    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let msgs = history["messages"]
        .as_array()
        .expect("history 应含 messages");
    let assistant = msgs
        .iter()
        .rev()
        .find(|m| m["role"] == "assistant")
        .expect("应有 assistant 消息");
    assert_eq!(
        assistant["extra"]["truncated"], true,
        "截断应落库 extra.truncated(刷新后提示仍在): {assistant}"
    );
}

/// M3:顶层模型失败产生 Error 终态(带 code/retryable),而非「空内容 finish 伪装正常结束」。
#[tokio::test]
async fn upstream_model_error_emits_error_terminal_event() {
    let app = test_app();
    let (_, char) = upload_character(app, "错误终态.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    // mock [[fail:文案@分类]]:模型请求返回错误(非中断)。
    // 分类由 mock 钩子显式声明(@timeout)——错误分类不再由引擎对文案做子串猜测,
    // 见 server-rs/src/models/llm_error.rs(批次 4.3 未类型化的字符串回退已删除)。
    let events = sse_events(
        app,
        &sid,
        &cid,
        "触发错误 [[fail:上游连接超时 504@timeout]]",
        "deep",
    )
    .await;
    let err = events
        .iter()
        .find(|e| e["type"] == "error")
        .expect("模型失败应发 error 终态事件: {events:?}");
    assert_eq!(err["code"], "request_timeout", "错误码分类: {err}");
    assert_eq!(err["retryable"], json!(true), "超时应可重试: {err}");
    assert!(
        err["message"].as_str().unwrap().contains("超时"),
        "应携带错误消息: {err}"
    );
    // 不应再有空 finish 伪装成功
    let finish = events.iter().find(|e| e["type"] == "finish");
    assert!(
        finish.is_none(),
        "模型失败不应发空 finish 伪装正常结束: {events:?}"
    );
}

/// 多线程 runtime 上的聊天生成 + 并发读不被阻塞(2026-09-16 性能批次 P-7)。
///
/// 覆盖缺口:本文件其余用例都是 `#[tokio::test]`(current_thread),而生产 chat 生成经
/// `tokio::spawn` 跑在 multi-thread 的 worker 上(api/chat.rs:329)——`finalize_messages`
/// 里的同步 IO 让出逻辑(`utils::blocking::park_worker`)在这两种 flavor 下走**不同分支**,
/// 只在 current_thread 下测等于没覆盖生产路径。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_send_on_multi_thread_runtime_keeps_reads_alive() {
    let app = test_app();
    let _guard = test_lock().await;
    let (_, char) = upload_character(app, "多线程聊天.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    // 生成任务:走完整 finalize_messages(含 session_vars 落库、运行时提示词读取、记忆读取)
    let gen_app = app.clone();
    let gen_cid = cid.clone();
    let gen_sid = sid.clone();
    let generator = tokio::spawn(async move {
        sse_events(
            &gen_app,
            &gen_sid,
            &gen_cid,
            "[[reply:并发读验证正文]]",
            "fast",
        )
        .await
    });

    // 生成期间持续并发读:任何失败都说明 worker 被同步 IO 卡住
    let mut reads = 0usize;
    while !generator.is_finished() {
        let mut handles = Vec::new();
        for _ in 0..4 {
            let app = app.clone();
            handles.push(tokio::spawn(async move {
                let (st, _) = send_json(&app, "GET", "/api/health", json!({})).await;
                st
            }));
        }
        for h in handles {
            assert_eq!(
                h.await.unwrap(),
                StatusCode::OK,
                "生成期间并发读必须成功(worker 不应被同步 IO 阻塞)"
            );
            reads += 1;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    // 生成本身必须正常完成并产出 finish 事件
    let events = generator.await.unwrap();
    let finish = events
        .iter()
        .find(|e| e["type"] == "finish")
        .expect("应收到 finish 事件(multi-thread 下 finalize_messages 让出后链路仍完整)");
    assert!(
        finish["finish_reason"].is_string(),
        "finish 事件应带 finish_reason: {finish}"
    );
    assert!(reads > 0, "应至少完成一轮并发读");
}

/// 重试提示透出(HB-4,2026-09-18):连接器重试时必须先给界面一个信号,
/// 而不是「停几十秒然后报错」的无解释等待。
///
/// 真实重试发生在 openai_compatible 连接器内(带真实 HTTP,由该模块的
/// retry_emits_notice_before_success 用例覆盖 429→重试→成功);mock 不经过那条路径,
/// 故由钩子 [[retry:attempt,max@原因]] 直接注入 LlmStreamChunk::Retry,
/// 本用例锁「引擎翻译 → SseEvent::Retry → SSE 序列位置」这一段。
///
/// 钩子分隔符是逗号不是斜杠:`a/b` 会被引擎的 looks_like_calculation 启发式
/// 当成算式,自动插入 calculator 调用并改写最后一条 user 消息,钩子随之失效。
#[tokio::test]
async fn retry_notice_reaches_sse_before_finish() {
    let app = test_app();
    let (_, char) = upload_character(app, "重试提示.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    let events = sse_events(
        app,
        &sid,
        &cid,
        // 用 fast 模式:deep 会先规划/反思,marker 落在哪一轮的 last_user 不确定
        "请在遇到上游限流时给出重试提示 [[retry:1,3@上游返回 429 Too Many Requests]]",
        "fast",
    )
    .await;
    let retry = events
        .iter()
        .find(|e| e["type"] == "retry")
        .unwrap_or_else(|| panic!("应有 retry 事件: {events:?}"));
    assert_eq!(retry["attempt"], json!(1), "attempt 字段: {retry}");
    assert_eq!(retry["max"], json!(3), "max 字段: {retry}");
    assert!(
        retry["reason"].as_str().unwrap().contains("429"),
        "reason 应透出上游状态: {retry}"
    );
    // 非终态:重试提示之后仍须正常产出正文与 finish
    let retry_at = events
        .iter()
        .position(|e| e["type"] == "retry")
        .expect("retry 位置");
    let finish_at = events
        .iter()
        .position(|e| e["type"] == "finish")
        .unwrap_or_else(|| panic!("重试后仍应有 finish: {events:?}"));
    assert!(
        retry_at < finish_at,
        "retry 必须早于 finish(否则用户仍看不到等待信号)"
    );
    // mock 逐字符流式,故断言 token 事件拼接后的正文
    let text: String = events
        .iter()
        .filter(|e| e["type"] == "token")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert!(text.contains("模拟回复"), "重试后应正常输出正文: {text:?}");
}
