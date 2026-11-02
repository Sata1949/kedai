//! 任务侧世界书段落前缀的按模式分叉（RTSPLIT-R4，2026-11-02）。
//!
//! 背景：`prompt_kit::constant_world_text` 的前缀「世界书设定」是**角色扮演侧**口径
//! （「设定」暗示创作背景）。任务链把同一批条目当**资料依据**输入，沿用「设定」会让
//! 模型把可核对的资料读成创作背景。
//!
//! 本条走真实引擎装配：上传常驻世界书 → 建任务 → 用 mock 的 `[[floors]]` 钩子让执行者
//! 回显它实际收到的完整消息，断言落在**真实下发的 system 文本**上。
//! 纯函数层（前缀分叉只在前缀、正文逐字节一致）的用例在 `services::prompt_kit::tests`。
//!
//! 工具描述的按模式分叉（R-3）由 `services::task_engine::tool_policy::tests` 的
//! `compile_returns_task_oriented_descriptions` 覆盖——那条路径上的 `compile` 与
//! `ToolDeps::dummy_for_test` 都是 `pub(crate)`，集成测试够不着。
mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{send_json, test_app_with_prompt_isolation, test_lock, wait_terminal};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

/// 上传一份只含一条常驻条目的世界书，返回其 id。
async fn upload_constant_world_book(app: &axum::Router, sentinel: &str) -> String {
    let book = json!({
        "name": "R4 世界书",
        "entries": [
            { "uid": 0, "comment": "资料项", "keys": [], "content": sentinel,
              "constant": true, "disable": false, "position": 0 }
        ]
    });
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"wb.json\"\r\n\
         Content-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        book
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/world-books/upload")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "世界书上传应成功");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["id"].as_str().unwrap().to_string()
}

/// 任务侧世界书段落必须用中性前缀「世界书资料」。
#[tokio::test]
async fn task_executor_world_book_uses_neutral_prefix() {
    let _guard = test_lock().await;
    let app = test_app_with_prompt_isolation();

    let sentinel = "R4-CONSTANT-ENTRY-CONTENT";
    let _book_id = upload_constant_world_book(app, sentinel).await;

    // 规划器只回 1 步；步骤 goal 带 [[floors]] 让执行器回显实际消息序列
    let title = r#"[[reply:[{"name":"回显步骤","goal":"\u005b\u005bfloors\u005d\u005d"}] ]]"#;
    let (status, created) = send_json(app, "POST", "/api/tasks", json!({ "title": title })).await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {created}");
    let id = created["task"]["id"].as_str().unwrap().to_string();

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成，详情: {detail}");

    let echo = detail["subtasks"][0]["result"].as_str().unwrap_or("");
    assert!(
        echo.contains(sentinel),
        "常驻世界书条目必须进入任务执行者 system: {echo}"
    );
    assert!(
        echo.contains("世界书资料:"),
        "任务侧世界书段落应使用中性前缀「世界书资料」: {echo}"
    );
    assert!(
        !echo.contains("世界书设定:"),
        "任务侧不得沿用角色扮演侧前缀「世界书设定」: {echo}"
    );
}
