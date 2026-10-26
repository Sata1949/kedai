// SKY-9-L1 回归:运行期自检问题必须能从详情接口的交付摘要读到。
//
// 背景:自检本身要起无头浏览器,不能放进详情请求的读时合成;终态门槛把问题写进
// `tasks.runtime_issues` 后,详情接口只读这一列。本用例不依赖浏览器——直接写库
// 模拟「上一轮终态自检留下了问题」,验证 API 下发与重跑清空两条链路。
//
// 独立成文件的理由同其它 task_*.rs:测试 app 的 DATA_DIR 按进程隔离。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
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

fn temp_workspace(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("kedai-l1-{tag}-{}-{ns}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

/// 测试 app 的数据目录(由 `build_test_app` 写入环境变量;同进程内唯一)。
fn db_path() -> PathBuf {
    let data_dir = std::env::var("DATA_DIR").expect("测试 app 应设置 DATA_DIR");
    Path::new(&data_dir).join("kedai.db")
}

#[tokio::test]
async fn detail_delivery_reads_persisted_runtime_issues_and_reset_clears_them() {
    let app = test_app();
    let ws = temp_workspace("runtime-issues");
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": "运行期问题下发用例",
            "workspace": ws.to_string_lossy(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();

    // 模拟终态门槛留下的运行期问题(含中文与冒号,验证 JSON 往返)。
    let issue = "index.html:页面没有 canvas 画布(3D/可视化交付未渲染)";
    let conn = Connection::open(db_path()).expect("打开测试库失败");
    conn.busy_timeout(Duration::from_secs(5))
        .expect("设置 busy_timeout 失败");
    let changed = conn
        .execute(
            "UPDATE tasks SET runtime_issues = ?1 WHERE id = ?2",
            rusqlite::params![serde_json::to_string(&[issue]).unwrap(), &id],
        )
        .expect("写入运行期问题失败");
    assert_eq!(changed, 1, "应命中刚创建的任务行");
    drop(conn);

    let (status, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "详情应 200: {detail}");
    let runtime = detail["delivery"]["runtime_issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        runtime.iter().filter_map(Value::as_str).collect::<Vec<_>>(),
        vec![issue],
        "详情交付摘要应下发持久化的运行期问题: {detail}"
    );
    // 静态内容体检与运行期问题是两个维度,不得把运行期项混进 content_issues。
    let content = detail["delivery"]["content_issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !content.iter().any(|v| v.as_str() == Some(issue)),
        "运行期问题不应混入静态 content_issues: {detail}"
    );

    // 重跑起跑即清空上一轮结论(不等执行完成,reset_task 是同步写)。
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (status, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "详情应 200: {detail}");
    assert!(
        detail["delivery"]["runtime_issues"]
            .as_array()
            .is_none_or(|v| v.is_empty()),
        "重跑起跑后不得残留上一轮运行期问题: {detail}"
    );

    // 收尾:停止本轮执行并清理工作区,避免后台任务拖住测试进程。
    let _ = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    let _ = std::fs::remove_dir_all(&ws);
}
