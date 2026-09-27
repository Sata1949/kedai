// API 集成测试 · 工具权限派生与鉴权 / 轮次上限 / 并行调用 / custom 白名单与严格步。
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

/// 恢复流程配置为未启用
async fn reset_flow(app: &axum::Router) {
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": { "enabled": false, "steps": [] } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "重置流程配置应 200");
}

#[tokio::test]
async fn tool_permissions_are_derived_from_valid_session_and_registered_tool() {
    let app = test_app();
    let (_, first_char) = upload_character(app, "权限角色一.json").await;
    let (_, second_char) = upload_character(app, "权限角色二.json").await;
    let first_cid = first_char["id"].as_str().unwrap();
    let second_cid = second_char["id"].as_str().unwrap();
    let (_, first_session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": first_cid }),
    )
    .await;
    let (_, second_session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": second_cid }),
    )
    .await;
    let first_sid = first_session["id"].as_str().unwrap();
    let second_sid = second_session["id"].as_str().unwrap();

    let (status, _) = send_json(
        app,
        "POST",
        "/api/agent/tool-permissions",
        json!({
            "session_id": "missing-session", "tool": "write", "scope": "session"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send_json(
        app,
        "POST",
        "/api/agent/tool-permissions",
        json!({
            "session_id": first_sid, "tool": "not-registered", "scope": "session"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send_json(
        app,
        "POST",
        "/api/agent/tool-permissions",
        json!({
            "session_id": first_sid,
            "tool": "write",
            "scope": "role",
            "scope_id": second_cid
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, first_permissions) = send_json(
        app,
        "GET",
        &format!("/api/agent/tool-permissions?session_id={first_sid}&character_id={second_cid}"),
        json!({}),
    )
    .await;
    let first_write = first_permissions["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "write")
        .unwrap();
    assert_eq!(first_write["allowed"], json!(true));

    let (_, second_permissions) = send_json(
        app,
        "GET",
        &format!("/api/agent/tool-permissions?session_id={second_sid}&character_id={first_cid}"),
        json!({}),
    )
    .await;
    let second_write = second_permissions["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "write")
        .unwrap();
    assert_eq!(second_write["allowed"], json!(false));
}

#[tokio::test]
async fn tool_permission_api_authorizes_session_without_executing_tool() {
    let app = test_app();
    let (_, character) = upload_character(app, "权限 API 角色.json").await;
    let character_id = character["id"].as_str().unwrap();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": character_id }),
    )
    .await;
    let session_id = session["id"].as_str().unwrap();
    let (status, before) = send_json(
        app,
        "GET",
        &format!("/api/agent/tool-permissions?session_id={session_id}&character_id={character_id}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let write_before = before["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "write")
        .unwrap();
    assert_eq!(write_before["risk"], json!("dangerous"));
    assert_eq!(write_before["allowed"], json!(false));

    let (status, _) = send_json(
        app,
        "POST",
        "/api/agent/tool-permissions",
        json!({ "session_id": session_id, "tool": "write", "scope": "session" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, after) = send_json(
        app,
        "GET",
        &format!("/api/agent/tool-permissions?session_id={session_id}&character_id={character_id}"),
        json!({}),
    )
    .await;
    let write_after = after["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "write")
        .unwrap();
    assert_eq!(write_after["allowed"], json!(true));
}

/// M1:max_tool_rounds 设置读写与校验——默认 32、PUT 后回读一致、非法值(0/201)被拒绝。
#[tokio::test]
async fn settings_max_tool_rounds_roundtrip_and_validation() {
    let _guard = test_lock().await;
    let app = test_app();
    // 默认 32
    let (_, s) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(s["max_tool_rounds"], json!(32), "默认应为 32: {s}");
    // 合法值 5
    let (status, s) = send_json(app, "PUT", "/api/settings", json!({ "max_tool_rounds": 5 })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(s["settings"]["max_tool_rounds"], json!(5));
    // 非法值 0 → 拒绝并保持 5
    let (status, s) = send_json(app, "PUT", "/api/settings", json!({ "max_tool_rounds": 0 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "0 应返回 400: {s}");
    let (_, current) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(current["max_tool_rounds"], json!(5), "0 不得半提交");
    // 非法值 201(超上限 200)→ 拒绝并保持 5
    let (status, s) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "max_tool_rounds": 201 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "201 应返回 400: {s}");
    let (_, current) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(current["max_tool_rounds"], json!(5), "201 不得半提交");
    // 恢复默认
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "max_tool_rounds": 32 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "恢复应 200");
}

/// M2 回归:并行工具调用回填为 OpenAI 标准结构——一轮 2 个 tool_calls 时,
/// 下一轮消息只含 1 条 assistant(携带完整 tool_calls[2])+ 2 条 tool 结果。
/// 旧实现为每个 call 单独追加 assistant,违反标准,严格后端会 400。
#[tokio::test]
async fn parallel_tool_calls_echoed_as_single_assistant_message() {
    let app = test_app();
    let (_, char) = upload_character(app, "并行工具.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    // mock 钩子:首轮返回 2 个 role 工具调用;第 2 轮回显 LLM 消息结构
    let events = sse_events(
        app,
        &sid,
        &cid,
        r#"[[tool_echo:role {"sides":6}]]"#,
        "agent",
    )
    .await;
    // 两个调用都有结果终态
    let results = events.iter().filter(|e| e["type"] == "tool_result").count();
    assert_eq!(results, 2, "两个并行调用都应有 tool_result: {events:?}");
    let finish = events
        .iter()
        .find(|e| e["type"] == "finish")
        .expect("缺 finish");
    let content = finish["content"].as_str().unwrap_or("");
    // 回显中应恰好一条 assistant 消息携带 2 个 tool_calls(而非两条各 1 个)
    assert!(
        content.contains("[assistant [calls:2]]"),
        "应恰好一条 assistant 消息携带完整 tool_calls[2](旧实现为两条 [calls:1]):\n{content}"
    );
    assert!(
        !content.contains("[calls:1]"),
        "不应出现单调用 assistant 消息: {content}"
    );
    // 两条 tool 结果消息
    let tool_lines = content
        .lines()
        .filter(|l| l.starts_with("[tool]") || l.starts_with("[tool "))
        .count();
    assert_eq!(tool_lines, 2, "应回填 2 条 tool 结果消息: {content}");
}

/// M2 回归:Custom 白名单危险工具真实执行成功(白名单即授权语义)。
/// 旧实现外层 allowed=true 但 execute 内部二次权限裁决拒绝,白名单敏感/危险工具实际被拒。
#[tokio::test]
async fn custom_whitelist_dangerous_tool_executes_successfully() {
    let _guard = test_lock().await;
    let app = test_app();
    // 单步流程,步骤工具白名单 = ["write"](dangerous 工具,未在会话/角色授权)
    let flow = json!({
        "enabled": true,
        "steps": [
            {
                "id": "s1", "name": "白名单写入", "goal": "写入气泡",
                "action": "direct", "generates": true, "enabled": true,
                "system_prompt": "", "temperature": 0.7, "max_tokens": 1024,
                "tools": ["write"]
            }
        ]
    });
    let (status, _) = send_json(app, "PUT", "/api/agent-flows", json!({ "config": flow })).await;
    assert_eq!(status, StatusCode::OK, "配置流程应 200");
    let (_, char) = upload_character(app, "白名单写入.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    // mock [[tool:...]] 单轮工具调用:write 写气泡(target=bubble,无授权)
    let events = sse_events(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"bubble","content":"白名单放行"}]]#"#,
        "custom",
    )
    .await;
    // 白名单放行:无授权事件,直接执行成功
    assert!(
        !events
            .iter()
            .any(|e| e["type"] == "tool_authorization_required"),
        "白名单工具不应弹授权: {events:?}"
    );
    let result = events
        .iter()
        .find(|e| e["type"] == "tool_result")
        .expect("应有 tool_result");
    assert!(
        result["output"].get("error").is_none(),
        "白名单 write 应执行成功,而非二次权限裁决拒绝: {events:?}"
    );
    assert!(
        events.iter().any(|e| e["type"] == "finish"),
        "应有 finish: {events:?}"
    );
    // 恢复流程配置
    reset_flow(app).await;
}

/// 二维批次 6a:严格档在聊天侧同样生效——`step_params_for` 短路后严格节点不下发工具,
/// 模型即便索要工具也不会被**执行**(没有 tool 结果 → 没有第二轮)。
///
/// 两阶段**只改档位、其余完全相同**,故「严格阶段 0 个工具结果」不会被误读为钩子没生效:
/// 宽松阶段的同一份配置必须跑满工具循环。用 [[tool_loop_text:…]] 让「跑到第几轮」可读:
///   - 严格:停在「（第1轮说明）」(单次调用,工具请求被忽略);
///   - 宽松:推进到「（模拟回复）工具循环已完成,最终回复。」(两轮工具 + 完成轮)。
///
/// 注:`tool_call` **事件**在单次调用路径也会透出(`process_chunk` 对每个工具调用块都发事件,
/// 与是否执行无关,见 executor.rs),故记录**未执行**的判定点是 `tool_result` 与收尾正文。
#[tokio::test]
async fn custom_strict_step_does_not_dispatch_tools() {
    let _guard = test_lock().await;
    let app = test_app();
    let args = r#"{"queries":[{"type":"character_prompt"}]}"#;
    let flow_of = |kind: &str| {
        json!({
            "enabled": true,
            "steps": [{
                "id": "s1", "name": "原子步", "goal": "一句话产出",
                "action": "direct", "generates": true, "enabled": true,
                "tools": ["read"], "kind": kind
            }]
        })
    };
    let (_, char) = upload_character(app, "档位工具.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    let message = format!("[[tool_loop_text:read|2 {args}]]");

    // 阶段 1:严格档(声明了工具白名单也不下发 → 不会执行任何工具)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": flow_of("strict") }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "严格档流程应可保存(二维批次 6a 放宽校验)"
    );
    let events = sse_events(app, &sid, &cid, &message, "custom").await;
    assert_eq!(
        events.iter().filter(|e| e["type"] == "tool_result").count(),
        0,
        "严格节点不应执行任何工具(模型索要的工具无人执行): {events:?}"
    );
    let finish = events
        .iter()
        .find(|e| e["type"] == "finish")
        .expect("严格节点单次调用后应正常收尾");
    assert_eq!(
        finish["content"].as_str().unwrap_or(""),
        "（第1轮说明）",
        "严格节点正文应停在单次调用那一轮(若进了工具循环会推进到完成轮): {events:?}"
    );

    // 阶段 2:同一份流程切回宽松档 → 工具循环恢复(对照,证明钩子与工具配置确实生效)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": flow_of("loose") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "宽松档流程应可保存");
    let events = sse_events(app, &sid, &cid, &message, "custom").await;
    assert_eq!(
        events.iter().filter(|e| e["type"] == "tool_result").count(),
        2,
        "宽松节点应执行两轮工具调用: {events:?}"
    );
    let finish = events
        .iter()
        .find(|e| e["type"] == "finish")
        .expect("宽松节点应有 finish");
    assert!(
        finish["content"]
            .as_str()
            .unwrap_or("")
            .contains("工具循环已完成"),
        "宽松节点应推进到工具循环完成轮: {events:?}"
    );
    // 恢复流程配置
    reset_flow(app).await;
}
