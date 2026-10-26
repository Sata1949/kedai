// TMR3-3 回归:team 主级依赖分波 + 上游产出注入。
//
// 背景:team 各主此前纯并行(JoinSet),规划器把 index.html 总装派给主 A、
// parts/*.js 派给主 B 时,主 A 可能在分片尚未写出时就开始总装。
//
// 本用例用规划器 JSON 的 `after:[1]` 声明「总装主依赖分片主」,并断言:
//   ① 事件里有「团队依赖调度:第1波…第2波…」;
//   ② 总装主的 prompt 里出现上游主产出摘要(证明它等到了分片主);
//   ③ 两个步骤都 done。
use axum::http::StatusCode;
use serde_json::{json, Value};
mod common;
use common::{send_json, test_app};

/// 任务终态轮询:沿用本文件历史的 150 次 × 100ms 超时,实现见 `common`.
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    common::wait_terminal_with_attempts(app, id, 150).await
}

#[tokio::test]
async fn team_runs_dependent_main_after_upstream_and_injects_output() {
    let app = test_app();
    // 规划器拓扑:主 2 用 after:[1] 依赖主 1(1-based);两个主目标各带固定回复。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"分片主","after":[],"goals":[{"name":"写分片","goal":"\u005b\u005breply:分片产出ABC\u005d\u005d 写分片","files":["parts/a.js"]}]},"#,
        r#"{"name":"总装主","after":[1],"goals":[{"name":"总装","goal":"\u005b\u005breply:总装完成\u005d\u005d 总装入口","files":["index.html"]}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 依赖分波目标"
    );
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": "team" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert!(
        st == "done" || st == "partial",
        "依赖分波应正常收尾,实际 {st}: {detail}"
    );
    let plan = detail["task"]["plan"].as_array().expect("plan 应为数组");
    assert_eq!(plan.len(), 2, "两主各一个子目标: {detail}");
    for step in plan {
        assert_eq!(step["status"], "done", "两主都应完成: {detail}");
    }

    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=1000"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = events["events"].as_array().cloned().unwrap_or_default();
    let schedule = rows
        .iter()
        .find(|e| e["detail"].as_str().unwrap_or("").contains("团队依赖调度"))
        .unwrap_or_else(|| panic!("应透出团队依赖调度事件: {events}"));
    let detail_text = schedule["detail"].as_str().unwrap_or("");
    assert!(
        detail_text.contains("第1波") && detail_text.contains("第2波"),
        "应有 2 波调度: {detail_text}"
    );

    // 总装主的调用行 prompt 应带上游主产出摘要(证明它排在分片主之后且拿到了产出)
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let _ = status;
    let call_rows = calls["calls"].as_array().expect("calls 应为数组");
    let assembly = call_rows
        .iter()
        .find(|c| c["phase"] == "agent" && c["step_index"] == 1)
        .unwrap_or_else(|| panic!("应有总装主调用行: {calls}"));
    let prompt = assembly["prompt_summary"].as_str().unwrap_or("");
    assert!(
        prompt.contains("依赖的上游主 agent 产出") && prompt.contains("分片产出ABC"),
        "总装主 prompt 应包含上游产出摘要: {prompt}"
    );
}
