// SKY-7 回归:集成主判定必须是依赖 DAG 的**汇点**,不能只看 `after` 非空。
//
// 判别力(修复前必红):
//   * 修复前:链式 1→2→3 里主 2 有 after,被当成集成主,20 轮只读无写不触发
//     「无文件进展收尾」,步骤记 done;
//   * 修复后:主 2(中段生产者,下游还有主 3)恢复两道计数守卫——第 8 轮先提醒、
//     第 16 轮收尾,步骤 partial;主 3(汇点总装)豁免,读满 20 轮正常收尾。
//
// 语义熔断关闭(20 次读同一文件输出相同,否则会被 HB-2 先抓走);本文件只验证
// 两个计数闸门的**作用范围**。
//
// 独立成文件的原因同其它 task_*.rs:设置是进程级共享的,单文件即单进程/单数据目录。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::path::PathBuf;
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
    let dir = std::env::temp_dir().join(format!("kedai-sky7-{tag}-{}-{ns}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    for _ in 0..300 {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let st = json["task"]["status"].as_str().unwrap_or("").to_string();
        if matches!(st.as_str(), "done" | "partial" | "error" | "ended") {
            return (st, json);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("任务 {id} 未在超时内到达终态");
}

#[tokio::test]
async fn non_sink_dependent_main_keeps_guards_while_sink_is_exempt() {
    let app = test_app();
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "exec_enabled": true,
            "loop_guard_semantic_min_calls": 0,
            "task_step_budget_secs": 3600,
            "task_team_budget_secs": 3600,
            "task_total_budget_secs": 0,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "设置应可写入: {json}");

    let ws = temp_workspace("guard-scope");
    std::fs::write(ws.join("shared.txt"), "上游分片内容\n").expect("写上游文件失败");

    // 链式:分片主 → 中段主 → 总装主。主 2 有 after 但下游还有主 3,
    // 不是汇点;主 3 是汇点。
    //
    // 中段主用 bash(写类工具)逐轮探索:只读守卫会被 bash 清零,因此本用例
    // 精确验证「声明文件无进展」守卫对非汇点中段主仍然生效;主 3 用 fs_read
    // 连续 20 轮,验证汇点主仍豁免只读守卫。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"分片主","after":[],"goals":[{"name":"写分片","goal":"\u005b\u005breply:分片产出ABC\u005d\u005d 写分片","files":["parts/a.js"]}]},"#,
        r#"{"name":"中段主","after":[1],"goals":[{"name":"中段探索","goal":"\u005b\u005btool_loop_any_text:bash|20 {\"command\":\"echo probe\"}\u005d\u005d 中段探索","files":["parts/b.js"]}]},"#,
        r#"{"name":"总装主","after":[2],"goals":[{"name":"总装","goal":"\u005b\u005btool_loop_any_text:fs_read|20 {\"path\":\"shared.txt\"}\u005d\u005d 总装入口","files":["index.html"]}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 守卫边界用例"
    );
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": "team", "workspace": ws.to_string_lossy() }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_ne!(st, "error", "守卫用例不应 error: {detail}");
    let plan = detail["task"]["plan"].as_array().expect("plan 应为数组");
    assert_eq!(plan.len(), 3, "三主各一个子目标: {detail}");

    // 中段主:非汇点 → 两道守卫生效,16 轮无声明文件写入即收尾并降 partial。
    assert_eq!(
        plan[1]["status"], "partial",
        "中段主应被无进展守卫截断: {detail}"
    );
    let mid_result = plan[1]["result"].as_str().unwrap_or("");
    assert!(
        mid_result.contains("声明文件无进展收尾"),
        "中段主结果应带守卫截断原因: {mid_result}"
    );

    // 汇点总装主:豁免,读满 20 轮正常收尾。
    assert_eq!(
        plan[2]["status"], "done",
        "汇点总装主应豁免两道计数守卫: {detail}"
    );

    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=3000"),
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

    let stops = details
        .iter()
        .filter(|d| d.contains("无文件进展收尾"))
        .count();
    assert_eq!(stops, 1, "只应中段主触发一次无进展收尾: {joined}");
    assert!(
        details.iter().any(|d| d.contains("无文件进展提醒")),
        "中段主应在第 8 轮先收到中途提醒: {joined}"
    );

    let mid_bash = details
        .iter()
        .filter(|d| d.contains("主 agent 2 子目标 1 调用工具 bash"))
        .count();
    assert!(
        mid_bash <= 16,
        "中段主应在第 16 轮被无进展守卫截断,实际 {mid_bash} 次: {joined}"
    );
    let sink_reads = details
        .iter()
        .filter(|d| d.contains("主 agent 3 子目标 1 调用工具 fs_read"))
        .count();
    assert!(
        sink_reads >= 20,
        "汇点总装主应读满 20 轮(修复前会被误截断),实际 {sink_reads} 次: {joined}"
    );

    let _ = std::fs::remove_dir_all(&ws);
}
