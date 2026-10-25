// SKY-6 回归:team 集成主(有上游依赖)必须豁免「连续只读」与「声明文件无进展」两个
// 计数闸门——总装要先读完全部分片再写大文件,16 轮内不写自己的交付文件是合法形态。
//
// 判别力(修复前必红):
//   * 修复前:集成主连续 20 轮 fs_read → 第 16 轮触发「无文件进展收尾」、只读守卫
//     也会先提醒再熔断,fs_read 结果事件停在 16 条以内;
//   * 修复后:20 轮读满并正常收尾,事件里不出现「只读探索提醒 / 连续只读空转熔断 /
//     无文件进展收尾」。
//
// 语义熔断在本用例里关闭(20 次读同一文件输出相同,否则第 4 轮就被 HB-2 抓走),
// 本文件只验证两个计数闸门;HB-2 对纯读工具的收敛是 SKY-6 的另一项。
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
use tokio::sync::Mutex;
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
    let dir = std::env::temp_dir().join(format!("kedai-sky6-{tag}-{}-{ns}", std::process::id()));
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
async fn team_integration_main_skips_readonly_and_no_progress_guards() {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let app = test_app();

    // 只测两个计数闸门:关语义熔断,预算拉高。
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

    let ws = temp_workspace("integration");
    std::fs::write(ws.join("shared.txt"), "上游分片内容\n").expect("写上游文件失败");

    // 主 2 有 after:[1](集成主),目标里连续 20 轮 fs_read(不写自己的 index.html)。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"分片主","after":[],"goals":[{"name":"写分片","goal":"\u005b\u005breply:分片产出ABC\u005d\u005d 写分片","files":["shared.txt"]}]},"#,
        r#"{"name":"总装主","after":[1],"goals":[{"name":"总装","goal":"\u005b\u005btool_loop_any_text:fs_read|20 {\"path\":\"shared.txt\"}\u005d\u005d 总装入口","files":["index.html"]}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 集成主守卫用例"
    );
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": title,
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
    assert_ne!(st, "error", "集成主用例不应 error: {detail}");

    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=2000"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = events["events"].as_array().cloned().unwrap_or_default();
    let details: Vec<String> = rows
        .iter()
        .map(|e| e["detail"].as_str().unwrap_or("").to_string())
        .collect();
    let joined = details.join("\n");
    for forbidden in ["只读探索提醒", "连续只读空转熔断", "无文件进展收尾"] {
        assert!(
            !joined.contains(forbidden),
            "集成主不得触发「{forbidden}」: {joined}"
        );
    }
    // SKY-8:集成主读满 8 次 fs_read 后注入一次「先写入口骨架」软提醒;
    // 只提醒不熔断,后续读轮仍然继续(下方 read_results 断言 ≥20)。
    let nudge_count = details
        .iter()
        .filter(|d| d.contains("分片读取提醒"))
        .count();
    assert_eq!(
        nudge_count, 1,
        "集成主第 8 次成功 fs_read 后应恰好提醒一次: {joined}"
    );
    let read_results = details
        .iter()
        .filter(|d| d.contains("工具 fs_read 已返回结果"))
        .count();
    assert!(
        read_results >= 20,
        "集成主应能读满 20 轮(修复前第 16 轮被守卫截断),实际 {read_results} 条"
    );

    // 复位设置
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "loop_guard_semantic_min_calls": 12 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = std::fs::remove_dir_all(&ws);
}
