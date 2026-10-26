// SKY-8 回归:确定性单文件装配工具 `fs_inline_html` 的端到端路径。
//
// 两主链:分片主写两个经典脚本,总装主用一次工具调用完成「读分片 + 内联 + 写入口」。
// 判别力:修复前工具未注册,`[[tool:fs_inline_html]]` 被任务策略拒绝,index.html 缺失。
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::Mutex;
mod common;
use common::{send_json, test_app};

fn temp_workspace(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "kedai-inline-tool-{tag}-{}-{ns}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

/// 任务终态轮询:沿用本文件历史的 400 次 × 100ms 超时,实现见 `common`.
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    common::wait_terminal_with_attempts(app, id, 400).await
}

/// 两主链标题。goal 内的 `[[...]]` 写成 `\u005b...\u005d`,避免被外层 reply_if 截断;
/// 工具参数里的 JSON 引号按 planner JSON 字符串转义。
const TITLE: &str = concat!(
    r#"[[reply_if:团队规划器|{"mains":["#,
    r#"{"name":"分片主","after":[],"goals":["#,
    r#"{"name":"写 a","goal":"\u005b\u005btool:fs_write {\"path\":\"parts/a.js\",\"content\":\"window.A=1;\"}\u005d\u005d 写分片 A","files":["parts/a.js"]},"#,
    r#"{"name":"写 b","goal":"\u005b\u005btool:fs_write {\"path\":\"parts/b.js\",\"content\":\"window.B=2;\"}\u005d\u005d 写分片 B","files":["parts/b.js"]}"#,
    r#"]},"#,
    r#"{"name":"总装主","after":[1],"goals":["#,
    r#"{"name":"总装","goal":"\u005b\u005btool:fs_inline_html {\"path\":\"index.html\",\"sources\":[\"parts/a.js\",\"parts/b.js\"],\"title\":\"装配\"}\u005d\u005d 总装入口","files":["index.html"]}"#,
    r#"]}"#,
    r#"]}]]"#,
    r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
    r#"[[reply_if:任务汇总者|团队最终成果]]"#,
    " 单文件装配用例"
);

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[tokio::test]
async fn team_assembly_main_uses_inline_tool_to_write_self_contained_entry() {
    let _guard = lock().lock().await;
    let app = test_app();
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_min_calls": 0,
            "task_step_budget_secs": 3600,
            "task_team_budget_secs": 3600,
            "task_total_budget_secs": 0,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "设置应可写入");

    let ws = temp_workspace("entry");
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": TITLE,
            "task_mode": "team",
            "workspace": ws.to_string_lossy(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "工具装配应正常完成: {detail}");
    let plan = detail["task"]["plan"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(plan.len(), 3, "两主三子目标: {detail}");
    for step in &plan {
        assert_eq!(step["status"], "done", "所有步骤应为 done: {step}");
    }

    let html = std::fs::read_to_string(ws.join("index.html")).expect("index.html 应落盘");
    assert!(html.contains("window.A=1;"), "应内联 a.js: {html}");
    assert!(html.contains("window.B=2;"), "应内联 b.js: {html}");
    assert_eq!(
        html.matches("<script>").count(),
        2,
        "两个分片各一段脚本: {html}"
    );
    assert!(!html.contains("src=\"parts/"), "不得留下本地外链: {html}");
    let delivery = &detail["delivery"];
    assert_eq!(
        delivery["missing"].as_array().map(|a| a.len()).unwrap_or(0),
        0,
        "不应缺失声明文件: {delivery}"
    );
    assert_eq!(
        delivery["content_issues"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0),
        0,
        "内容体检不应有问题: {delivery}"
    );

    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=2000"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let details: Vec<String> = events["events"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|e| e["detail"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let joined = details.join("\n");
    assert!(
        joined.contains("调用工具 fs_inline_html"),
        "应看到确定性装配工具调用: {joined}"
    );
    assert!(
        joined.contains("已返回结果") && joined.contains("fs_inline_html"),
        "工具应正常返回结果: {joined}"
    );

    let _ = std::fs::remove_dir_all(&ws);
    let _ = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "loop_guard_semantic_min_calls": 12 }),
    )
    .await;
}
