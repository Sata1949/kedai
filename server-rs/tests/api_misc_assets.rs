// API 集成测试 · 其余端点与静态资源（健康检查 / 设置与 token / 提示词预览 / 计划 / slash / 音频 / 渲染 / 插件 / SPA）。
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

async fn send_empty(app: &axum::Router, method: &str, path: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    resp.status()
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

#[tokio::test]
async fn health() {
    let app = test_app();
    let (status, json) = send_json(app, "GET", "/api/health", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["ok"], json!(true));
    assert!(json["ts"].is_number());
    // 批次 2:依赖探测。`ok` 必须保持 liveness 语义(桌面壳 health_ok() 依赖它,
    // src-tauri/src/lib.rs),依赖态只经 dependencies 上报;正常库应为 "ok"。
    assert_eq!(json["dependencies"]["db"], json!("ok"));
}

#[tokio::test]
async fn settings_and_token() {
    let app = test_app();
    // settings/connect(mock)
    let (status, conn) = send_json(app, "POST", "/api/settings/connect", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(conn["ok"], json!(true));
    assert_eq!(conn["models"][0], json!("mock-demo"));
    // settings/info
    let (_, info) = send_json(app, "GET", "/api/settings/info", json!({})).await;
    assert_eq!(info["connector"], json!("mock"));
    assert!(info["availableConnectors"].as_array().unwrap().len() >= 2);
    // settings/model
    let (_, m) = send_json(app, "GET", "/api/settings/model", json!({})).await;
    assert!(m["model"].as_str().is_some());
    // 切换模型(mock 下不变更但返回 ok)
    let (status, sw) = send_json(
        app,
        "PUT",
        "/api/settings/model",
        json!({ "model": "gpt-4o" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sw["ok"], json!(true));
    // token/count
    let (status, tc) = send_json(
        app,
        "POST",
        "/api/token/count",
        json!({ "messages": [{ "role": "user", "content": "你好" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(tc["total"].as_i64().unwrap() >= 4 + 2);
}

/// 最终提示词预览按层输出且聊天历史仅返回角色/长度/哈希，不泄露正文或 API Key。
#[tokio::test]
async fn prompt_preview_is_layered_and_redacts_history() {
    let app = test_app();
    let (_, char) = upload_character(app, "预览角色.json").await;
    let cid = char["id"].as_str().unwrap();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap();
    let secret = "CHAT_BODY_MUST_NOT_LEAK_92a1";
    let (_, _) = send_json(
        app,
        "POST",
        "/api/import/chat",
        json!({ "session_id": sid, "messages": [{ "role": "user", "content": secret }] }),
    )
    .await;

    let (status, preview) = send_json(
        app,
        "GET",
        &format!("/api/settings/prompt-preview?session_id={sid}&character_id={cid}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "预览失败: {preview}");
    let layers = preview["layers"].as_array().expect("layers 应为数组");
    assert!(layers.iter().all(|v| v["source"].is_string()
        && v["role"].is_string()
        && v["layer"].is_number()
        && v["order"].is_number()));
    let history = layers
        .iter()
        .find(|v| v["source"] == "history_summary")
        .expect("应有历史摘要层");
    assert!(history["content"].as_str().unwrap().contains("role=user"));
    assert!(history["content"].as_str().unwrap().contains("hash="));
    let text = preview.to_string();
    assert!(!text.contains(secret), "预览不得泄露聊天正文: {text}");
    assert!(
        !text.to_lowercase().contains("api_key"),
        "预览不得包含 API key 字段: {text}"
    );
}

#[tokio::test]
async fn agent_execute_is_explicitly_not_implemented() {
    let app = test_app();
    let (status, body) = send_json(
        app,
        "POST",
        "/api/agent/execute",
        json!({ "session_id": "unused" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body["error"], json!("该接口尚未实现,请使用 /api/chat/send"));
}

#[tokio::test]
async fn agent_plan() {
    let app = test_app();
    let (status, plan) = send_json(
        app,
        "POST",
        "/api/agent/plan",
        json!({ "message": "你好", "agent_mode": "fast" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plan["summary"], json!("快速模式:直接生成回复"));
    assert!(plan["tools"]
        .as_array()
        .unwrap()
        .contains(&json!("calculator")));

    let (status, deep) = send_json(
        app,
        "POST",
        "/api/agent/plan",
        json!({ "message": "你好", "agent_mode": "deep" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // deep 模式:计划生成步(先写 ≤200 字计划再输出正文)+ 反思步(理解意图步已废弃)
    let steps = deep["plan"]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["generates"], json!(true));
    assert_eq!(steps[1]["action"], json!("reflect"));
    assert!(
        steps[0]["system_prompt"]
            .as_str()
            .unwrap_or("")
            .contains("200 字"),
        "deep 生成步应先写 ≤200 字计划: {:?}",
        steps[0]["system_prompt"]
    );

    // 缺少 message → 400
    let (status, _) = send_json(app, "POST", "/api/agent/plan", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn concurrent_partial_settings_updates_preserve_both_fields() {
    let app = test_app();
    let first = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "preset_tail_role": "assistant" }),
    );
    let second = send_json(app, "PUT", "/api/settings", json!({ "render_html": true }));
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.0, StatusCode::OK, "第一笔设置更新失败: {:?}", first.1);
    assert_eq!(
        second.0,
        StatusCode::OK,
        "第二笔设置更新失败: {:?}",
        second.1
    );

    let (status, current) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(current["preset_tail_role"], json!("assistant"));
    assert_eq!(current["render_html"], json!(true));

    // 避免共享测试应用污染后续用例。
    let _ = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "preset_tail_role": "user", "render_html": false }),
    )
    .await;
}

#[tokio::test]
async fn generation_limits_return_bad_request() {
    let app = test_app();
    let (_, character) = upload_character(app, "参数上限.json").await;
    let cid = character["id"].as_str().unwrap();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap();
    let (status, _) = send_json(
        app,
        "POST",
        "/api/chat/send",
        json!({ "session_id": sid, "message": "hi", "max_tokens": 65537 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "max_context_tokens": 1048577 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn slash_commands_list() {
    // 阶段四 4a:GET /api/slash/commands 返回内置命令清单(前端输入框联想)
    let app = test_app();
    let (status, json) = send_json(app, "GET", "/api/slash/commands", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let commands = json["commands"].as_array().expect("commands 应为数组");
    let names: Vec<&str> = commands.iter().filter_map(|c| c["name"].as_str()).collect();
    for expected in ["echo", "var", "setvar", "getvar", "addvar", "help"] {
        assert!(
            names.contains(&expected),
            "命令清单应包含 {expected}: {names:?}"
        );
    }
    // 每项含 name/description/params 元信息
    for c in commands {
        assert!(c["name"].is_string());
        assert!(c["description"].is_string());
        assert!(c["params"].is_string());
    }
}

#[tokio::test]
async fn audio_crud() {
    // 阶段五 5a:GET /api/audio 返回默认双通道;PUT settings 改 mode/volume;
    // PUT playlist 校验 URL 协议白名单(非法协议 400,校验失败不落盘不改内存)
    let app = test_app();
    let _guard = test_lock().await;

    // 1) 默认状态:bgm repeat_all / ambient play_one_and_stop / volume 50
    let (status, json) = send_json(app, "GET", "/api/audio", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["audio"]["bgm"]["mode"], "repeat_all");
    assert_eq!(json["audio"]["ambient"]["mode"], "play_one_and_stop");
    assert_eq!(json["audio"]["bgm"]["volume"], 50);
    assert!(json["audio"]["bgm"]["enabled"].as_bool().unwrap());

    // 2) PUT settings:改 ambient mode + volume(部分字段合并)
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/audio/settings",
        json!({ "type": "ambient", "settings": { "mode": "repeat_all", "volume": 77 } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["audio"]["ambient"]["mode"], "repeat_all");
    assert_eq!(json["audio"]["ambient"]["volume"], 77);
    // bgm 不受影响
    assert_eq!(json["audio"]["bgm"]["mode"], "repeat_all");

    // 3) 非法通道 → 400
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/audio/settings",
        json!({ "type": "music", "settings": { "muted": true } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 4) PUT playlist:合法 URL 替换成功
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/audio/playlist",
        json!({
            "type": "bgm",
            "tracks": [
                { "title": "开场曲", "url": "https://example.com/a.mp3" },
                { "title": "主城", "url": "http://example.com/b.ogg" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["audio"]["bgm"]["playlist"].as_array().unwrap().len(),
        2
    );

    // 5) 非法协议 → 400,且不落盘不改内存
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/audio/playlist",
        json!({
            "type": "bgm",
            "tracks": [{ "title": "本地", "url": "file:///C:/x.mp3" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("协议"));
    let (_, after) = send_json(app, "GET", "/api/audio", json!({})).await;
    assert_eq!(
        after["audio"]["bgm"]["playlist"].as_array().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn render_frame_document_is_served() {
    // 阶段五 5b:GET /render-frame.html 返回宿主文档(CSP 断掉网络出口,
    // frame-ancestors 'self' 允许本机页面框入)
    let app = test_app();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/render-frame.html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/html; charset=utf-8");
    let csp = resp.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        csp.contains("default-src 'none'"),
        "CSP 应为 default-src 'none': {csp}"
    );
    assert!(csp.contains("script-src 'unsafe-inline'"));
    assert!(csp.contains("connect-src 'none'"));
    assert!(csp.contains("frame-ancestors 'self'"));
    assert_eq!(resp.headers()["x-frame-options"], "SAMEORIGIN");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&bytes);
    assert!(
        html.contains("kedai-render-panel-v1"),
        "宿主文档应含面板 channel"
    );
    assert!(html.contains("booted"), "宿主文档应含 boot 闩锁");
}

// ===== 缓存诊断端点(缓存感知管线) =====

/// 上传入口:文件名经净化后与原始名不一致即拒绝(「..」路径穿越 / 非法字符);
/// 合法名通过并注册成功。与删除入口同一 sanitize_plugin_filename 判定。
#[tokio::test]
async fn plugin_upload_sanitizes_filename() {
    let app = test_app();
    let _guard = test_lock().await;
    let upload = |filename: &str| {
        let body = format!(
            "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
            json!({ "name": "b4_demo", "description": "B-4 测试插件", "script": "result = 1;" })
        );
        Request::builder()
            .method("POST")
            .uri("/api/plugins/tools/upload")
            .header("content-type", "multipart/form-data; boundary=BOUND")
            .body(Body::from(body))
            .unwrap()
    };

    // 「..」路径穿越:净化删掉 '/' 后与原名不一致 → 400,不落盘
    let resp = app.clone().oneshot(upload("../evil.json")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, ".. 路径穿越应被拒");
    // 含非法字符(空格):净化后与原名不一致 → 400
    let resp = app.clone().oneshot(upload("my tool.json")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "含空格文件名应被拒");

    // 合法名通过:201 + 落盘注册;用唯一名避免与并行用例互踩
    let resp = app
        .clone()
        .oneshot(upload("b4-demo_plugin.json"))
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::CREATED, "合法文件名应通过: {json}");
    assert_eq!(json["file"], json!("b4-demo_plugin.json"));
    // 收尾删除(兼覆盖删除入口合法名路径)
    let (status, _) = send_json(
        app,
        "DELETE",
        "/api/plugins/tools/b4-demo_plugin.json",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "删除合法文件名应通过");
}

/// 删除入口:「..」与含非法字符名被拒(与上传同一净化函数);
/// 净化允许的点号组合(如无分隔符的「..」开头)不在拒绝范围,故「..」用带分隔符形态构造。
#[tokio::test]
async fn plugin_delete_sanitizes_filename() {
    let app = test_app();
    let _guard = test_lock().await;
    // 「..」路径穿越(URL 编码 %2E%2E%2F = ../):路径段解码后与原名不一致 → 400
    let status = send_empty(app, "DELETE", "/api/plugins/tools/..%2Fevil.json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, ".. 路径穿越应被拒");
    // 含非法字符(空格 %20):净化后与原名不一致 → 400
    let status = send_empty(app, "DELETE", "/api/plugins/tools/my%20tool.json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "含空格文件名应被拒");
    // 合法名通过(文件不存在也走完整校验后 200,删除语义幂等)
    let status = send_empty(app, "DELETE", "/api/plugins/tools/nonexistent-b4.json").await;
    assert_eq!(status, StatusCode::OK, "合法文件名应通过校验");
}

/// SPA 回退边界(2026-09 修复「面板加载失败」):静态资源未命中必须 404,
/// 不能回退成 index.html——否则浏览器把 HTML 当 JS 解析,前端表现为
/// 「面板加载失败」且懒加载重试永远失败(典型触发:前端重建后 chunk hash 变化)。
#[tokio::test]
async fn spa_fallback_returns_404_for_missing_assets() {
    let app = test_app();
    let _guard = test_lock().await;

    // 不存在的 assets chunk → 404(不是 200 + text/html)
    let (status, _) = send_json(
        app,
        "GET",
        "/assets/EmbeddingSection-STALEHASH.js",
        json!({}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "缺失的 assets chunk 应 404,不能回退 index.html"
    );

    // 其它带扩展名的静态资源(如 favicon 缺失)同样 404
    let (status, _) = send_json(app, "GET", "/missing-icon.png", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "缺失的静态资源应 404");

    // 路由式路径(无点、非 assets)仍回退 index.html,保证前端路由可刷新
    let (status, _) = send_json(app, "GET", "/some-spa-route", json!({})).await;
    assert_eq!(status, StatusCode::OK, "路由路径应回退 index.html");
}

/// 静态资源缓存策略落地为真实响应头(2026-09-16 性能批次 P-2):
/// index.html 是版本指针必须恒禁缓存;这里同时断言它没被误改成可缓存。
#[tokio::test]
async fn index_html_is_served_no_store() {
    let app = test_app();
    let _guard = test_lock().await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/index.html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "index.html 应可取得");
    let cc = resp.headers()["cache-control"]
        .to_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        cc.contains("no-store"),
        "index.html 必须 no-store(版本指针,缓存会导致加载旧 hash 资源),实际: {cc}"
    );
}

// ==================== 批次 1:错误面收口(状态码 + code + 泄露守卫) ====================
//
// 约定(批次 7 先行的核心契约):错误响应必须断言**真实 HTTP 状态码 + JSON 的 code 字段**,
// 而不是仅断言「有 error 字段」;涉及内部失败的出口另加「泄露守卫」断言——
// 响应体不得回显 SQLite 报错片段、serde 内部类型名或盘符路径。
