// TMR3-2 回归:失败循环里的截断自愈也必须落调用行与 usage。
//
// 背景:`run_tool_loop` 的 self_heals 曾是局部 Vec,Err 传播时随错误丢弃——
// 首轮被截断、翻倍重发又失败时,两次调用的 token 既不在 task_llm_calls 的自愈行,
// 也不在 usage_total。P0-1 只修了「整轮累计 usage」,这条支路仍漏。
//
// mock 钩子 `[[tool_raw_fail:name {...}]]`:首轮返回半截 tool_call arguments +
// Finish(length)(触发截断自愈),翻倍重发轮直接返回上游错误 → 构造「自愈后仍失败」。
use axum::http::StatusCode;
use serde_json::{json, Value};
mod common;
use common::{send_json, test_app};

/// 任务终态轮询:沿用本文件历史的 150 次 × 100ms 超时,实现见 `common`.
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    common::wait_terminal_with_attempts(app, id, 150).await
}

#[tokio::test]
async fn failed_loop_records_self_heal_call_and_usage() {
    let app = test_app();
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": r#"[[tool_raw_fail:fs_write {"path":"a.txt","content":"x"}]] 自愈失败记账用例"#,
            "task_mode": "solo",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "error", "自愈重发失败应落 error 终态: {detail}");

    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let rows = calls["calls"].as_array().expect("calls 应为数组");
    let heal = rows
        .iter()
        .find(|c| {
            c["response_summary"]
                .as_str()
                .unwrap_or("")
                .contains("截断自愈")
        })
        .unwrap_or_else(|| panic!("必须有截断自愈调用行: {calls}"));
    let heal_prompt = heal["prompt_tokens"].as_i64().unwrap_or(0);
    assert!(
        heal_prompt > 0,
        "自愈行必须带被截断那次调用的 token,不能是 0: {heal}"
    );

    let usage = &detail["usage_total"];
    let prompt = usage["prompt_tokens"].as_i64().unwrap_or(0);
    assert!(
        prompt >= heal_prompt,
        "usage_total 必须包含自愈那次调用的消耗: usage={usage} heal={heal}"
    );

    let agent_error = rows
        .iter()
        .find(|c| c["phase"] == "agent" && c["status"] == "error")
        .unwrap_or_else(|| panic!("应有 agent 错误调用行: {calls}"));
    assert!(
        agent_error["status"] == "error",
        "失败行状态应为 error: {agent_error}"
    );
}
