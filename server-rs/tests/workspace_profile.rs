// 工作区画像集成测试(2026-09-30 编码能力包 CODE-4)。
//
// 覆盖两层出口:独立探测端点 `GET /api/workspace/profile`(创建表单在任务存在之前用)
// 与任务详情顶层 `workspace_profile`(同一把尺:同一个 probe 纯函数)。单元行为
// (截断留痕 / 忽略集 / 扩展名匹配 / 零命令执行)在 `services::workspace_profile` 的
// 单测里;这里只验「HTTP 面 + 契约字段」。
//
// 共用前提沿用仓库体例:本文件自带辅助函数(见 tasks_crud.rs 顶部说明),
// `build_test_app()` 的 DATA_DIR 是 `%TEMP%\kedai-test-<pid>`,与本文件的夹具目录平级。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use kedai_server::utils::test_support::TempDataDir;
use serde_json::{json, Value};
use std::sync::OnceLock;
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

async fn send_get(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    send_json(app, "GET", path, Value::Null).await
}

/// 百分号编码:query 里带 Windows 绝对路径(盘符 + 反斜杠 + 可能的空格)时必须编码,
/// 否则 URI 解析与后端 query 解码都会失真。
fn enc(raw: &str) -> String {
    let mut out = String::new();
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn touch(root: &std::path::Path, rel: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, "").unwrap();
}

/// 本仓形态的夹具:根 `package.json` + 子目录 `server-rs/Cargo.toml`。
fn monorepo_fixture(tag: &str) -> TempDataDir {
    let ws = TempDataDir::new(tag);
    touch(ws.path(), "package.json");
    touch(ws.path(), "server-rs/Cargo.toml");
    ws
}

#[tokio::test]
async fn probe_endpoint_reports_project_types() {
    let ws = monorepo_fixture("ws-probe-ok");
    let uri = format!(
        "/api/workspace/profile?path={}",
        enc(&ws.path().to_string_lossy())
    );
    let (status, json) = send_get(test_app(), &uri).await;
    assert_eq!(status, StatusCode::OK, "探测应 200: {json}");

    let detected = json["profile"]["detected"]
        .as_array()
        .expect("detected 应为数组");
    let pairs: Vec<(&str, &str, u64)> = detected
        .iter()
        .map(|h| {
            (
                h["kind"].as_str().unwrap(),
                h["marker"].as_str().unwrap(),
                h["depth"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("rust", "server-rs/Cargo.toml", 1),
            ("node", "package.json", 0)
        ],
        "表序 rust 先;子目录项 marker 为相对路径"
    );
    assert_eq!(
        detected[0]["suggested_command"].as_str().unwrap(),
        "cargo test"
    );
    assert_eq!(json["profile"]["scanned_dirs"].as_u64().unwrap(), 1);
    assert_eq!(json["profile"]["truncated"], json!(false));
}

#[tokio::test]
async fn probe_endpoint_requires_path() {
    let (status, json) = send_get(test_app(), "/api/workspace/profile").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "缺 path 应 400: {json}");
    let msg = json["error"].as_str().unwrap_or_default();
    assert!(msg.contains("path"), "文案应点名 path: {msg}");
}

#[tokio::test]
async fn probe_endpoint_rejects_missing_dir() {
    let missing = std::env::temp_dir().join("kedai-ws-probe-definitely-missing");
    let uri = format!(
        "/api/workspace/profile?path={}",
        enc(&missing.to_string_lossy())
    );
    let (status, json) = send_get(test_app(), &uri).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "不存在的目录应 400: {json}"
    );
    let msg = json["error"].as_str().unwrap_or_default();
    assert!(msg.contains("不存在"), "文案应指明不存在: {msg}");
}

/// 数据目录本身是工作区禁地(创建期与工具期同一条判据),探测端点同样按同一把尺拒绝。
#[tokio::test]
async fn probe_endpoint_rejects_data_dir() {
    let data_dir = std::env::temp_dir().join(format!("kedai-test-{}", std::process::id()));
    let uri = format!(
        "/api/workspace/profile?path={}",
        enc(&data_dir.to_string_lossy())
    );
    let (status, json) = send_get(test_app(), &uri).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "数据目录应 400: {json}");
    let msg = json["error"].as_str().unwrap_or_default();
    assert!(msg.contains("数据目录"), "文案应指明数据目录: {msg}");
}

#[tokio::test]
async fn task_detail_carries_workspace_profile() {
    let ws = monorepo_fixture("ws-detail-bound");
    let (status, json) = send_json(
        test_app(),
        "POST",
        "/api/tasks",
        json!({ "title": "画像详情", "workspace": ws.path().to_string_lossy() }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();

    let (status, detail) = send_get(test_app(), &format!("/api/tasks/{id}")).await;
    assert_eq!(status, StatusCode::OK, "详情应 200: {detail}");
    let profile = &detail["workspace_profile"];
    assert!(profile.is_object(), "绑定工作区的任务应带画像: {detail}");
    let kinds: Vec<&str> = profile["detected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["rust", "node"], "画像内容应与探测端点一致");
    // 画像的 root 是**冻结值原样回显**(canonical,可能含 Windows 的 \\?\ 前缀)
    assert_eq!(
        profile["root"].as_str().unwrap(),
        detail["task"]["workspace"].as_str().unwrap()
    );
}

#[tokio::test]
async fn task_detail_without_workspace_has_null_profile() {
    let (status, json) = send_json(
        test_app(),
        "POST",
        "/api/tasks",
        json!({ "title": "无工作区" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();

    let (status, detail) = send_get(test_app(), &format!("/api/tasks/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        detail["workspace_profile"].is_null(),
        "未绑定工作区 → 画像为 null(前端不渲染该行): {detail}"
    );
}
