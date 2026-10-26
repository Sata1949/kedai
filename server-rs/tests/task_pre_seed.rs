// SKY-8 兜底回归:集成主启动前由引擎预置单文件最终入口,不依赖模型调用工具。
//
// 判别力(修复前必红):总装主两个子目标都只返回文本(不写任何文件),修复前
// index.html 缺失 → 任务 partial;修复后引擎用上游两个经典脚本预置 index.html,
// 中间模板步骤以最终入口为准记 done,交付必需项只剩 index.html。
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
    let dir =
        std::env::temp_dir().join(format!("kedai-pre-seed-{tag}-{}-{ns}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

/// 任务终态轮询:沿用本文件历史的 400 次 × 100ms 超时,实现见 `common`.
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    common::wait_terminal_with_attempts(app, id, 400).await
}

/// 两主链 + 单文件目标的 mcraft 形态:总装主两步(外壳模板 + 最终入口),
/// 两步都只回文本,不写文件;引擎应在主启动前预置 index.html。
const TITLE: &str = concat!(
    r#"[[reply_if:团队规划器|{"mains":["#,
    r#"{"name":"分片主","after":[],"goals":["#,
    r#"{"name":"写 a","goal":"\u005b\u005btool:fs_write {\"path\":\"parts/a.js\",\"content\":\"window.A=1;\"}\u005d\u005d 写分片 A","files":["parts/a.js"]},"#,
    r#"{"name":"写 b","goal":"\u005b\u005btool:fs_write {\"path\":\"parts/b.js\",\"content\":\"window.B=2;\"}\u005d\u005d 写分片 B","files":["parts/b.js"]}"#,
    r#"]},"#,
    r#"{"name":"总装主","after":[1],"goals":["#,
    r#"{"name":"外壳","goal":"\u005b\u005breply:外壳占位\u005d\u005d 写外壳","files":["src/shell.html"]},"#,
    r#"{"name":"入口","goal":"\u005b\u005breply:入口完成\u005d\u005d 写入口","files":["index.html"]}"#,
    r#"]}"#,
    r#"]}]]"#,
    r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
    r#"[[reply_if:任务汇总者|团队最终成果]]"#,
    " 单文件预置骨架用例"
);

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[tokio::test]
async fn engine_pre_seeds_final_entry_when_assembly_main_only_reads() {
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
    assert_eq!(st, "done", "引擎预置入口后应完成: {detail}");
    let plan = detail["task"]["plan"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(plan.len(), 4, "两主四子目标: {detail}");
    for step in &plan {
        assert_eq!(step["status"], "done", "全部步骤应为 done: {step}");
    }

    let html = std::fs::read_to_string(ws.join("index.html")).expect("index.html 应由引擎预置");
    assert!(html.contains("window.A=1;"), "应内联 a.js: {html}");
    assert!(html.contains("window.B=2;"), "应内联 b.js: {html}");
    assert!(!html.contains("@@"), "预置入口不得残留槽位: {html}");
    assert!(
        !ws.join("src/shell.html").exists(),
        "模型没有写中间模板;中间步骤仍应按最终入口记 done"
    );
    assert!(
        !html.contains("src=\"parts/"),
        "预置入口不得留下本地外链: {html}"
    );

    let delivery = &detail["delivery"];
    let required = delivery["required"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        required.len(),
        1,
        "单文件目标只把最终入口列为必需: {delivery}"
    );
    assert_eq!(required[0]["path"], "index.html", "{delivery}");
    assert_eq!(
        delivery["missing"].as_array().map(|a| a.len()).unwrap_or(0),
        0,
        "最终入口已预置,不应缺失: {delivery}"
    );
    assert_eq!(
        delivery["content_issues"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0),
        0,
        "预置入口内容体检应通过: {delivery}"
    );
    assert!(
        delivery["changed"]
            .as_array()
            .map(|a| a.iter().any(|v| v == "parts/a.js"))
            .unwrap_or(false),
        "分片变更应照常入账: {delivery}"
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
        joined.contains("引擎已在工作区预置最终入口 index.html"),
        "应透出引擎预置事件: {joined}"
    );
    assert!(
        !joined.contains("调用工具 fs_inline_html"),
        "本用例模型不调用工具;预置必须由引擎完成: {joined}"
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
