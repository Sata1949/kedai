// SKY-7 回归:团队总预算按「主/子目标步数」公平分配,前波吃不完后续波的份额。
//
// 判别力(修复前必红):
//   * 修复前:主 1 先用 sleep 拖满 12s 团队预算,主 2/主 3 在波间检查时已到点,
//     步骤记「团队总预算用尽(未执行)」;主 3 永远没有启动窗口;
//   * 修复后:主 1 两子目标只拿到 6s 份额(2/4 步),第一子目标预算收尾、
//     第二子目标标「团队预算份额用尽」;主 2/主 3 各自拿到剩余份额并完成。
//
// 本用例故意让主 1 超时,验证「前波超时不会饿死后续波」;审计/汇总的预留
// (<120s 不生效)不在本用例范围,由 `post_exec_reserve` 单测覆盖。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
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

async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let st = json["task"]["status"].as_str().unwrap_or("").to_string();
        if matches!(st.as_str(), "done" | "partial" | "error" | "ended") {
            return (st, json);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "任务未在 45s 内收敛: {json}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn team_budget_share_keeps_later_waves_alive() {
    let app = test_app();
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_min_calls": 0,
            "task_step_budget_secs": 3600,
            "task_team_budget_secs": 12,
            "task_total_budget_secs": 0,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "设置应可写入: {json}");

    // 链式 1→2→3:主 1 两子目标(占 2/4 份额),主 2/主 3 各一个子目标。
    // 主 1 第一子目标每轮 sleep 900ms;6s 份额约 7 轮即预算收尾,第二子目标
    // 应因份额耗尽而不再启动。主 2/主 3 的固定回复瞬时完成。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"慢主","after":[],"goals":["#,
        r#"{"name":"慢活一","goal":"\u005b\u005btool_loop_any_text:sleep|31 {\"ms\":900}\u005d\u005d 慢活一","files":["parts/a.js"]},"#,
        r#"{"name":"慢活二","goal":"\u005b\u005breply:第二段\u005d\u005d 慢活二","files":["parts/b.js"]}"#,
        r#"]},"#,
        r#"{"name":"中段主","after":[1],"goals":[{"name":"快活","goal":"\u005b\u005breply:主2完成\u005d\u005d 快活","files":["parts/c.js"]}]},"#,
        r#"{"name":"总装主","after":[2],"goals":[{"name":"总装","goal":"\u005b\u005breply:总装完成\u005d\u005d 总装","files":["index.html"]}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"预算份额用例"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 预算份额目标"
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
        "份额收尾应保留已有产出(不制造 error),实际 {st}: {detail}"
    );
    let plan = detail["task"]["plan"].as_array().expect("plan 应为数组");
    assert_eq!(plan.len(), 4, "两主一子 + 两主各一子: {detail}");

    // 主 1 第一子目标:6s 份额内带产出收尾;声明文件 parts/a.js 未落盘,
    // SKY-8 起按磁盘事实记 partial(修复前按 TMR-3 记 done)。第二子目标:份额已耗尽。
    assert_eq!(
        plan[0]["status"], "partial",
        "主 1 第一子目标预算收尾且文件缺失应为 partial: {detail}"
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
        "主 1 第二子目标不得在份额耗尽后启动: {detail}"
    );
    assert!(
        plan[1]["result"]
            .as_str()
            .unwrap_or("")
            .contains("团队预算份额用尽(未执行)"),
        "第二子目标应标明份额用尽: {detail}"
    );

    // 最关键的修复面:后续波仍启动并完成。
    assert_eq!(
        plan[2]["status"], "done",
        "主 2 应拿到剩余份额并完成: {detail}"
    );
    assert_eq!(
        plan[3]["status"], "done",
        "主 3(总装)应拿到启动窗口并完成: {detail}"
    );

    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=1000"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "拉取任务事件应 200: {events}");
    let rows = events["events"].as_array().cloned().unwrap_or_default();
    let details: Vec<String> = rows
        .iter()
        .map(|e| e["detail"].as_str().unwrap_or("").to_string())
        .collect();
    let joined = details.join("\n");
    assert!(
        joined.contains("团队预算分配(第1波)") && joined.contains("主Agent-1"),
        "应透出第一波预算分配事实: {joined}"
    );
    assert!(
        joined.contains("团队预算分配(第2波)"),
        "存在后续波时应透出第二波分配事实: {joined}"
    );
}
