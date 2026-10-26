// TMR3-1 回归:任务模式单次调用遇到可重试上游失败时,原地重试一次。
//
// 背景(2026-10-21/22 实测):team 主 agent 的 9 个步骤里 8 个都因「模型调用超过
// 300s 未完成」直接失败;连接器只在首字节前重试,任务层不消费 retryable(),
// 一次上游停滞就等于一个子目标报废。
//
// mock 钩子 `[[fail_times:N 消息@分类]]`(TMR3-1 新增,进程内按 marker 计数):
// 前 N 次请求失败,之后落回 `[[reply:...]]`。既有 `[[fail:]]` 恒失败,构造不出
// 「先失败、重试后成功」的形态。
use axum::http::StatusCode;
use serde_json::{json, Value};
mod common;
use common::{send_json, test_app};

/// 任务终态轮询:沿用本文件历史的 150 次 × 100ms 超时,实现见 `common`.
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    common::wait_terminal_with_attempts(app, id, 150).await
}

async fn create_and_run(app: &axum::Router, title: &str) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": "solo" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    id
}

async fn events(app: &axum::Router, id: &str) -> Vec<Value> {
    let (status, json) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=1000"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "拉取任务事件应 200: {json}");
    json["events"].as_array().cloned().unwrap_or_default()
}

/// 超时一次 → 退避后重试成功:任务必须 done,并透出「上游重试」事件。
#[tokio::test]
async fn retryable_timeout_is_retried_once_and_task_completes() {
    let app = test_app();
    let title = "[[fail_times:1 模拟上游中断@timeout]][[reply:重试后完成]] 上游重试用例A";
    let id = create_and_run(app, title).await;

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "可重试失败经一次重试后应完成: {detail}");
    assert!(
        detail["task"]["result"]
            .as_str()
            .unwrap_or("")
            .contains("重试后完成"),
        "结果应来自重试成功的调用: {detail}"
    );

    let rows = events(app, &id).await;
    assert!(
        rows.iter()
            .any(|e| e["detail"].as_str().unwrap_or("").contains("上游重试")),
        "必须透出「上游重试」事件,否则用户看不到这次失败: {rows:?}"
    );

    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let agent = calls["calls"]
        .as_array()
        .expect("calls 应为数组")
        .iter()
        .find(|c| c["phase"] == "agent")
        .unwrap_or_else(|| panic!("应有 agent 调用行: {calls}"));
    assert_eq!(agent["status"], "ok", "最终调用行应为成功: {agent}");
}

/// 鉴权类错误不可重试:只调用一次即失败,不得出现「上游重试」事件。
#[tokio::test]
async fn auth_failure_is_not_retried() {
    let app = test_app();
    let title = "[[fail:鉴权失败@auth_failed]] 不可重试用例A";
    let id = create_and_run(app, title).await;

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "error", "鉴权失败应直接失败: {detail}");
    let rows = events(app, &id).await;
    assert!(
        !rows
            .iter()
            .any(|e| e["detail"].as_str().unwrap_or("").contains("上游重试")),
        "不可重试错误不得发起重试: {rows:?}"
    );
}
