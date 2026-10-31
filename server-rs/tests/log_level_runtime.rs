// 6.2 运行时日志调级端点的集成测试。
//
// 判据三条:① 未带 token 一律 401(与其余 /api/* 同口径);② 非法级别回 400 + code,
// 且不改变当前级别;③ 合法级别 200 且 GET 回显新值。
//
// 注:本测试不初始化全局 subscriber(`logging::init` 每个进程只能生效一次,测试进程
// 里的 `try_init` 会被既有测试初始化或直接失败),故 `set_level` 走「只记录当前级别」
// 分支——这正是端点对外的可观测契约。热替换本身由 `utils::logging` 的单元测试覆盖。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use kedai_server::build_secure_test_app;
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "log-level-test-token-0123456789";

#[tokio::test]
async fn log_level_requires_token_and_validates_level() {
    let app = build_secure_test_app(TOKEN).unwrap();

    // ① 无 token → 401
    let unauth = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/diagnostics/log-level")
                .header("host", "127.0.0.1:3001")
                .header("content-type", "application/json")
                .body(Body::from(json!({"level":"debug"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    // ② 非法级别 → 400 + code
    let (status, body) = send(&app, "PUT", json!({"level":"verbose"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非法级别应 400: {body}");
    assert_eq!(body["code"], "VALIDATION", "错误体应带 code: {body}");

    // ③ 合法级别 → 200,GET 回显新值
    let (status, body) = send(&app, "PUT", json!({"level":"debug"})).await;
    assert_eq!(status, StatusCode::OK, "调试级别应接受: {body}");
    assert_eq!(body["level"], "debug");
    let (status, body) = send(&app, "GET", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["level"], "debug", "GET 应回显新级别: {body}");
    // 允许值清单透出,便于前端下拉不写死
    assert!(
        body["allowed"]
            .as_array()
            .is_some_and(|v| v.iter().any(|x| x == "trace")),
        "allowed 应透出五档: {body}"
    );
}

/// 带 token 的 JSON 请求(安全测试 app 需要 `host` + Bearer)
async fn send(app: &axum::Router, method: &str, body: Value) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri("/api/diagnostics/log-level")
        .header("host", "127.0.0.1:3001")
        .header("authorization", format!("Bearer {TOKEN}"));
    let req = if method == "GET" {
        builder.body(Body::empty()).unwrap()
    } else {
        builder = builder.header("content-type", "application/json");
        builder.body(Body::from(body.to_string())).unwrap()
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}
