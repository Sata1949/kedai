// 任务模式集成测试:任务 CRUD + 执行(规划→子智能体→汇总)+ 停止。
// 使用 build_test_app()(mock 连接器 + 临时数据目录 + 免鉴权)。
// mock 钩子 [[reply:内容]] 让规划器返回 JSON 计划、子智能体返回指定结果,实现确定性端到端。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
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

async fn send_empty(app: &axum::Router, method: &str, path: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    resp.status()
}

/// 创建任务并返回 id
async fn create_task(app: &axum::Router, title: &str) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 轮询任务详情直到终态(done/error/ended)或超时;返回最终 status 与详情
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    for _ in 0..50 {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let st = json["task"]["status"].as_str().unwrap_or("").to_string();
        if st == "done" || st == "error" || st == "ended" {
            return (st, json);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("任务 {id} 未在超时内到达终态");
}

#[tokio::test]
async fn task_crud_roundtrip() {
    let app = test_app();

    // 空标题被拒
    let (status, json) = send_json(app, "POST", "/api/tasks", json!({ "title": "   " })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空标题应 400: {json}");

    // 创建
    let id = create_task(app, "写一篇短文").await;

    // 列表包含
    let (status, json) = send_json(app, "GET", "/api/tasks", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = json["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert!(ids.contains(&id.as_str()), "列表应包含新任务");

    // 详情:status=pending,plan 空,subtasks 空
    let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["task"]["status"], "pending");
    assert_eq!(json["task"]["plan"].as_array().unwrap().len(), 0);
    assert_eq!(json["subtasks"].as_array().unwrap().len(), 0);

    // 删除
    let status = send_empty(app, "DELETE", &format!("/api/tasks/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 删除后 404
    let (status, _) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 端到端:mock 规划器返回 2 步,各子智能体返回结果,汇总后 status=done。
#[tokio::test]
async fn task_run_to_done() {
    let app = test_app();

    // title 内嵌 [[reply:...]] 让 mock 规划器直接返回 JSON 计划(2 步,goal 为普通文本);
    // 步骤 goal 无特殊标记,mock 子智能体返回默认非空回复。
    // 注意:mock reply 钩子在首个 "]]" 截断,故数组闭合 "]" 与标记闭合 "]]" 之间须留空格。
    let title = r#"[[reply:[{"name":"步骤一","goal":"写第一段"},{"name":"步骤二","goal":"写第二段"}] ]]"#;
    let id = create_task(app, title).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成,详情: {detail}");

    // 计划 2 步均 done
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "计划应拆为 2 步");
    for step in plan {
        assert_eq!(step["status"], "done");
    }

    // 子任务 2 条均 done 且结果非空
    let subtasks = detail["subtasks"].as_array().unwrap();
    assert_eq!(subtasks.len(), 2);
    for st in subtasks {
        assert_eq!(st["status"], "done");
        assert!(!st["result"].as_str().unwrap().is_empty());
    }

    // 最终结果非空
    assert!(!detail["task"]["result"].as_str().unwrap().is_empty());
}

/// 停止一个 pending 任务:status 转 ended。
#[tokio::test]
async fn task_stop_pending() {
    let app = test_app();
    let id = create_task(app, "待停止的任务").await;

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    assert_eq!(status, StatusCode::OK);

    let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["task"]["status"], "ended");
}

/// 不存在的任务:run/stop/get 均 4xx。
#[tokio::test]
async fn task_not_found() {
    let app = test_app();
    let (status, _) = send_json(app, "POST", "/api/tasks/不存在/run", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send_json(app, "POST", "/api/tasks/不存在/stop", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send_json(app, "GET", "/api/tasks/不存在", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
