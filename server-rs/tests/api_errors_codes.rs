// API 集成测试 · 统一错误面（形状闸门 / code / 400·404·405 / 不泄露上游细节）。
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

/// 断言错误响应:状态码匹配、body 为 JSON 且 code 符合预期
#[track_caller]
fn assert_error_shape(
    status: StatusCode,
    body: &Value,
    expect_status: StatusCode,
    expect_code: &str,
) {
    assert_eq!(
        status, expect_status,
        "状态码不符(期望 {expect_status}): {body}"
    );
    assert_eq!(
        body["code"], expect_code,
        "code 不符(期望 {expect_code}): {body}"
    );
    assert!(
        body["error"].as_str().is_some_and(|s| !s.is_empty()),
        "错误响应缺少可读 error 文案: {body}"
    );
}

/// 泄露守卫:响应体不得包含内部实现细节
#[track_caller]
fn assert_no_internal_leak(body: &Value) {
    let text = body.to_string();
    for needle in [
        "SQLite",
        "no such column",
        "no such table",
        "Failed to deserialize",
        "missing field",
        r"C:\",
        "D:\\",
        "rusqlite",
        "panicked at",
        "os error",
    ] {
        assert!(
            !text.contains(needle),
            "响应体泄露内部细节「{needle}」: {text}"
        );
    }
}

/// 发送原始字节请求体(用于构造畸形 JSON),返回 (状态码, content-type, 原始文本体)
async fn send_raw_body(
    app: &axum::Router,
    method: &str,
    path: &str,
    raw: &str,
) -> (StatusCode, String, String) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(raw.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, ct, String::from_utf8_lossy(&bytes).to_string())
}

/// 未知 /api 路径(资源式,含扩展名)必须 404 + JSON + code,不是空 body 也不是 HTML
#[tokio::test]
async fn unknown_api_path_is_json_404_with_code() {
    let app = test_app();
    let (status, body) = send_json(app, "GET", "/api/definitely-not-a-route.json", json!({})).await;
    assert_error_shape(status, &body, StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_no_internal_leak(&body);
}

/// 方法不允许(如 DELETE /api/health):405 必须是 JSON + code(此前是 405 空 body)
#[tokio::test]
async fn method_not_allowed_is_json_405_with_code() {
    let app = test_app();
    let (status, body) = send_json(app, "DELETE", "/api/health", json!({})).await;
    assert_error_shape(status, &body, StatusCode::METHOD_NOT_ALLOWED, "VALIDATION");
    assert_no_internal_leak(&body);
}

/// 头像服务:不存在的文件与带非法字符的名字都必须是真实 404 + code
/// (此前是 `200 + {"error":"Not Found"}` 的假成功,前端 http 层看不到失败)
#[tokio::test]
async fn avatar_missing_and_traversal_are_404_with_code() {
    let app = test_app();
    // 不存在(合法文件名)
    let (status, body) = send_json(app, "GET", "/api/avatars/no-such-avatar.png", json!({})).await;
    assert_error_shape(status, &body, StatusCode::NOT_FOUND, "NOT_FOUND");
    // 含非法字符(空格 URL 解码后与净化结果不一致 → 目录穿越拒绝分支)
    let (status, body) = send_json(app, "GET", "/api/avatars/my%20pic.png", json!({})).await;
    assert_error_shape(status, &body, StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_no_internal_leak(&body);
}

#[tokio::test]
async fn resource_proxy_rejects_invalid_urls() {
    let app = test_app();
    // 非 https 拒绝
    let (status, body) = send_json(
        app,
        "GET",
        "/api/resource/proxy?url=http%3A%2F%2Fexample.com%2Fpage.html",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "http 应拒绝: {body}");
    assert_eq!(body["code"], json!("VALIDATION"));
    // javascript: 拒绝
    let (status, body) = send_json(
        app,
        "GET",
        "/api/resource/proxy?url=javascript%3Aalert(1)",
        json!({}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "javascript: 应拒绝: {body}"
    );
    // 私网地址拒绝(SSRF):127.0.0.1
    let (status, body) = send_json(
        app,
        "GET",
        "/api/resource/proxy?url=https%3A%2F%2F127.0.0.1%2Fpage",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "私网应拒绝: {body}");
}

/// 资源代理:各类失败统一 `{error, code}` + 真实状态码,且不再回显上游/内部原文
#[tokio::test]
async fn resource_proxy_errors_carry_code_without_leaking_upstream_detail() {
    let app = test_app();
    // 非 https → 400 VALIDATION
    let (status, body) = send_json(
        app,
        "GET",
        "/api/resource/proxy?url=http%3A%2F%2Fexample.com%2Fpage.html",
        json!({}),
    )
    .await;
    assert_error_shape(status, &body, StatusCode::BAD_REQUEST, "VALIDATION");
    // SSRF 私网拒绝 → 400 VALIDATION
    let (status, body) = send_json(
        app,
        "GET",
        "/api/resource/proxy?url=https%3A%2F%2F127.0.0.1%2Fpage",
        json!({}),
    )
    .await;
    assert_error_shape(status, &body, StatusCode::BAD_REQUEST, "VALIDATION");
    // javascript: → 400 VALIDATION
    let (status, body) = send_json(
        app,
        "GET",
        "/api/resource/proxy?url=javascript%3Aalert(1)",
        json!({}),
    )
    .await;
    assert_error_shape(status, &body, StatusCode::BAD_REQUEST, "VALIDATION");

    // 上游主机无法解析(.invalid 保留域)→ 在 SSRF 解析阶段被拒:400 VALIDATION,
    // 且不回显 DNS/OS 原文(该分支此前会拼 `os error 11001` 与「搜索端点」错域措辞)
    let (status, body) = send_json(
        app,
        "GET",
        "/api/resource/proxy?url=https%3A%2F%2Fkedai-upstream-does-not-exist.invalid%2Fx.html",
        json!({}),
    )
    .await;
    assert_error_shape(status, &body, StatusCode::BAD_REQUEST, "VALIDATION");
    assert_eq!(
        body["error"],
        json!("资源地址不可用,请确认其为公开可访问的 https 地址")
    );
    assert!(
        !body.to_string().contains("搜索端点"),
        "不应把搜索工具的错域文案透给资源卡片: {body}"
    );
    assert!(
        !body.to_string().contains("os error"),
        "不应回显 DNS/OS 原文: {body}"
    );
    assert_no_internal_leak(&body);
}

/// 仓库索引不可用时的上报形态:`available:false` 语义保留,补 code,且不回显
/// serde 解析细节(此前 `reason` 里直接拼 `{e}`)
#[tokio::test]
async fn repo_index_unavailable_reports_code_without_parse_detail() {
    let app = test_app();
    let _guard = test_lock().await;
    // 指向一个存在但内容非法的索引文件,命中「解析失败」分支
    // (守卫目录:断言失败/panic 时也不在 TEMP 残留)
    let bad_dir = kedai_server::utils::test_support::TempDataDir::new("bad-index");
    let bad = bad_dir.join("index.json");
    std::fs::write(&bad, b"{ this is not json").unwrap();
    let prev = std::env::var_os("KEDAI_REPO_INDEX");
    std::env::set_var("KEDAI_REPO_INDEX", &bad);

    let (status, body) = send_json(app, "GET", "/api/repo-index", json!({})).await;

    // 恢复环境变量(避免污染同进程其它用例)
    match prev {
        Some(v) => std::env::set_var("KEDAI_REPO_INDEX", v),
        None => std::env::remove_var("KEDAI_REPO_INDEX"),
    }
    // 索引不可用属正常上报(HTTP 200 + available:false),不改状态码
    assert_eq!(
        status,
        StatusCode::OK,
        "索引不可用不上报为 HTTP 错误: {body}"
    );
    assert_eq!(body["available"], json!(false));
    assert_eq!(body["code"], json!("INTERNAL"));
    assert!(
        body["reason"]
            .as_str()
            .is_some_and(|s| s.contains("重新运行")),
        "应给出可操作的重新生成指引: {body}"
    );
    assert_no_internal_leak(&body);
}

/// 插件热重载:有失败项时保留 loaded/errors 业务数据,补 code 供前端统一分支
#[tokio::test]
async fn plugins_reload_error_keeps_payload_and_carries_code() {
    let app = test_app();
    let _guard = test_lock().await;
    // 往测试数据目录写一个非法插件文件,触发 parse_all 的逐项错误
    let dir = std::env::temp_dir()
        .join(format!("kedai-test-{}", std::process::id()))
        .join("plugins")
        .join("tools");
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("b1-broken_plugin.json");
    std::fs::write(&bad, b"{ broken").unwrap();

    let (status, body) = send_json(app, "POST", "/api/plugins/tools/reload", json!({})).await;

    let _ = std::fs::remove_file(&bad);
    let _ = std::fs::remove_dir_all(&dir);

    assert_error_shape(status, &body, StatusCode::BAD_REQUEST, "VALIDATION");
    // 业务数据必须保留:loaded 数量 + 逐项错误清单
    assert!(body["loaded"].is_number(), "loaded 计数不应丢失: {body}");
    assert!(
        body["errors"].as_array().is_some_and(|a| !a.is_empty()),
        "逐项错误清单不应丢失: {body}"
    );
}

/// 畸形 JSON 请求体统一为 JSON + code(批次 1 · JsonBody 接入):
/// 逐个热门 handler 断言「400 + application/json + code=VALIDATION」,而非
/// axum 默认的 `400 text/plain`(前端 request() 解析不出任何可读原因)。
#[tokio::test]
async fn malformed_json_body_is_uniform_json_400_on_hot_handlers() {
    let app = test_app();
    let _guard = test_lock().await;
    // 覆盖 chat / tasks / settings / characters 四域各自的 JSON 入口
    let routes: &[(&str, &str)] = &[
        ("POST", "/api/chat/send"),
        ("POST", "/api/chat/stop"),
        ("POST", "/api/chat/compact"),
        ("POST", "/api/tasks"),
        ("PUT", "/api/settings"),
        ("PUT", "/api/settings/model"),
        ("PUT", "/api/settings/agent-prompt"),
    ];
    for (method, path) in routes {
        let (status, ct, raw) = send_raw_body(app, method, path, "{这不是合法 JSON").await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{method} {path} 状态码: {raw}"
        );
        assert!(
            ct.starts_with("application/json"),
            "{method} {path} 应为 JSON content-type(而非 text/plain):{ct} / {raw}"
        );
        let body: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{method} {path} 响应不是 JSON({e}): {raw}"));
        assert_eq!(body["code"], "VALIDATION", "{method} {path}: {body}");
        assert!(
            !raw.contains("Failed to deserialize"),
            "{method} {path} 泄露 serde 解析原文: {raw}"
        );
    }
}

/// 结构合法但字段类型不符(缺失必填字段)同样收口为 JSON + VALIDATION
#[tokio::test]
async fn missing_required_field_is_uniform_json_400() {
    let app = test_app();
    let _guard = test_lock().await;
    // 任务标题必填:缺 title
    let (status, ct, raw) =
        send_raw_body(app, "POST", "/api/tasks", r#"{"character_id":null}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "缺 title: {raw}");
    assert!(ct.starts_with("application/json"), "content-type: {ct}");
    let body: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(body["code"], "VALIDATION", "{body}");
    // 发送字符串而非对象(类型不符)
    let (status, _, raw) = send_raw_body(app, "POST", "/api/chat/send", r#""just a string""#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "类型不符: {raw}");
}
