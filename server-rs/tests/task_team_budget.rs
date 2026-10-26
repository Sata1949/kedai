// TMR3-4 回归:team 执行阶段的团队总预算。
//
// 背景:team 的 `task_step_budget_secs` 是**每个子目标**重新起算(4 主 × 4 子目标
// 可跑到小时级),全局 `task_total_budget_secs` 默认 0;2026-10-21 实测单主跑到
// 26 分钟、外部 driver 30 分钟主动 stop。
//
// 本用例把 `task_team_budget_secs` 设为 1 秒(低于设置页下限 600,经
// `test_app_with_settings` 在进程内注入),主 1 用 sleep 工具拖过截止期,
// 主 2 声明 after:[1] —— 应看到:主 1 带产出收尾、主 2 标「团队总预算用尽(未执行)」、
// 任务以 partial 收尾而不是 error。
use axum::http::StatusCode;
use serde_json::{json, Value};
mod common;
use common::send_json;

/// 任务终态轮询:沿用本文件历史的 60s 墙钟 超时,实现见 `common`.
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    common::wait_terminal_within_secs(app, id, 60).await
}

#[tokio::test]
async fn team_budget_stops_remaining_mains_with_partial_delivery() {
    let app = common::test_app_with_settings(|s| s.task_team_budget_secs = 1);
    // 关闭语义熔断(本用例要拖过 1s 截止期,不能被同工具输出守卫先收掉);
    // 团队预算已在上面的构建期注入,PUT 不再带该字段(端点会拒绝 <600 的有限值)。
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_min_calls": 0,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "设置应 200: {json}");

    // 主 1:单轮 sleep 1500ms,第一轮即越过 1s 团队预算;主 2 依赖主 1。
    // SKY-7 起主 1 只拿到池的一半(500ms),但 sleep 在轮末才判预算,
    // 所以 1500ms 的睡眠同时越过份额与全局截止期——保留本用例「全局用尽」语义。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"慢主","after":[],"goals":[{"name":"慢活","goal":"\u005b\u005btool_loop_text:sleep|31 {\"ms\":1500}\u005d\u005d 慢活","files":["slow.txt"]}]},"#,
        r#"{"name":"总装主","after":[1],"goals":[{"name":"总装","goal":"\u005b\u005breply:总装完成\u005d\u005d 总装","files":["index.html"]}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"预算用例"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 团队预算目标"
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
        st == "partial" || st == "done",
        "团队预算收尾应保留已有产出(不制造 error),实际 {st}: {detail}"
    );
    let plan = detail["task"]["plan"].as_array().expect("plan 应为数组");
    assert_eq!(plan.len(), 2, "两主各一个子目标: {detail}");
    // SKY-8:慢主预算收尾时声明文件 slow.txt 未落盘 → 按磁盘事实记 partial
    // (修复前按 TMR-3 记 done);本用例仍验证「主 2 在截止期后不启动」。
    assert_eq!(
        plan[0]["status"], "partial",
        "主 1 预算收尾且文件缺失应为 partial: {detail}"
    );
    assert!(
        plan[0]["result"]
            .as_str()
            .unwrap_or("")
            .contains("[预算收尾]"),
        "预算收尾应在步骤结果留痕: {detail}"
    );
    assert_eq!(
        plan[1]["status"], "error",
        "主 2 在截止期后不得启动: {detail}"
    );
    assert!(
        plan[1]["result"]
            .as_str()
            .unwrap_or("")
            .contains("团队总预算用尽(未执行)"),
        "主 2 应标明团队预算未执行: {detail}"
    );

    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=1000"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = events["events"].as_array().cloned().unwrap_or_default();
    assert!(
        rows.iter().any(|e| e["detail"]
            .as_str()
            .unwrap_or("")
            .contains("团队总预算用尽")),
        "应透出团队总预算用尽事件: {events}"
    );
    // TMR3-L4:预算事实必须在开工时可见(默认 1200s 是 TMR3 新增行为,不能等用尽才告知)
    assert!(
        rows.iter()
            .any(|e| e["detail"].as_str().unwrap_or("").contains("团队执行预算")),
        "团队任务启动时应透出预算事实: {events}"
    );
}
