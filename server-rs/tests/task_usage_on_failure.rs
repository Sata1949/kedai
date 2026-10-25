// P0 回归:失败/中止的工具循环也必须把已累计 token 落账。
//
// 背景(2026-10-21 真实模型复测):team 主 2 在 ui 子目标跑了 234 轮后因上游错误
// 收尾，但 `run_agent_loop` 的错误分支只落错误文案、不落 `total_usage`，导致
// `task_llm_calls` 记 `0 + 0`、`usage_total` 完全缺失这段消耗。
//
// mock 钩子 `[[tool_loop_fail:name|N args]]` 构造「先跑 N 轮工具、再上游失败」的形态；
// 既有 `[[fail:]]` 首轮即失败，构造不出「有已累计 usage」的失败循环。
use axum::http::StatusCode;
use serde_json::{json, Value};
mod common;
use common::{send_json, test_app};

/// 任务终态轮询:沿用本文件历史的 120 次 × 100ms 超时,实现见 `common`.
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    common::wait_terminal_with_attempts(app, id, 120).await
}

#[tokio::test]
async fn failed_tool_loop_records_accumulated_usage() {
    let app = test_app();
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": r#"[[tool_loop_fail:sleep|3 {"ms":1}]] 失败循环用量用例"#,
            "task_mode": "solo",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "error", "上游失败应落 error 终态: {detail}");

    let usage = &detail["usage_total"];
    let prompt = usage["prompt_tokens"].as_i64().unwrap_or(0);
    let completion = usage["completion_tokens"].as_i64().unwrap_or(0);
    assert!(
        prompt > 0 && completion > 0,
        "失败循环已执行 3 轮(每轮 5/3 token),usage_total 必须非零: {detail}"
    );

    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let agent_error = calls["calls"]
        .as_array()
        .expect("calls 应为数组")
        .iter()
        .find(|c| c["phase"] == "agent" && c["status"] == "error")
        .unwrap_or_else(|| panic!("应有 agent 错误调用行: {calls}"));
    assert!(
        agent_error["prompt_tokens"].as_i64().unwrap_or(0) > 0,
        "错误调用行也必须带已累计 token,不能是 0: {agent_error}"
    );
    assert!(
        agent_error["completion_tokens"].as_i64().unwrap_or(0) > 0,
        "错误调用行 completion 也不能是 0: {agent_error}"
    );
}
