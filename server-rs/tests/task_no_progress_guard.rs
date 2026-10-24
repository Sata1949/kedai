// P0-2 回归:team 主 agent 写出声明文件后,用临时文件/bash 自检不能无限跑。
//
// 与 `tasks_modes_team.rs` 分开成独立进程的原因:本用例要显式关闭语义熔断
// (`loop_guard_semantic_min_calls=0`),否则 mock 重复写同一临时文件会先被
// 「输出实质无变化」熔断在第 4 轮,测不到「无进展守卫」的 16 轮阈值。
// 设置是进程级共享的,独立文件避免污染其它 team 用例。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
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
    for _ in 0..120 {
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

fn temp_workspace(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("kedai-p0-{tag}-{}-{ns}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

async fn create_task_with_mode_workspace(
    app: &axum::Router,
    title: &str,
    mode: &str,
    ws: &Path,
) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": title,
            "task_mode": mode,
            "workspace": ws.to_string_lossy(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn no_progress_guard_stops_temp_file_loop_at_16_rounds() {
    let app = test_app();
    // 关闭语义熔断,专门测无进展守卫;只读守卫对 fs_write 本来就会清零。
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "loop_guard_semantic_min_calls": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "关闭语义熔断应 200: {json}");

    let ws = temp_workspace("no-progress");
    // SKY-7:中途提醒是 user 消息;普通 [[tool_loop_text:]] 只认末条 user,
    // 提醒后会失活。这里用跨提醒轮持久的 any 变体,验证仍跑到第 16 轮硬收尾。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"主","goals":[{"name":"写临时文件","goal":"\u005b\u005btool_loop_any_text:fs_write|30 {\"path\":\"temp.js\",\"content\":\"x\"}\u005d\u005d 只写临时文件","files":["out.txt"]}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 无进展守卫目标"
    );
    let id = create_task_with_mode_workspace(app, title, "team", &ws).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert!(
        st == "partial" || st == "done",
        "守卫收尾应带产出结束(不应 error),实际 {st}: {detail}"
    );
    let plan = detail["task"]["plan"].as_array().expect("plan 应为数组");
    assert_eq!(plan.len(), 1, "单主单子目标: {detail}");
    // SKY-6 第 2 项:守卫截断是「没跑完」,步骤不得记 done(修复前恰为 done)。
    assert_eq!(
        plan[0]["status"], "partial",
        "守卫截断应把步骤降为 partial: {detail}"
    );
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("守卫截断"),
        "任务 error 应说明被守卫截断: {error:?}"
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
    let stopped = rows.iter().any(|e| {
        e["detail"]
            .as_str()
            .unwrap_or("")
            .contains("无文件进展收尾")
    });
    assert!(stopped, "应透出无文件进展收尾事件: {events}");
    // SKY-7:硬收尾前必须在第 8 轮透出一次中途提醒(覆盖 bash 探索空转)。
    let nudged = rows.iter().any(|e| {
        e["detail"]
            .as_str()
            .unwrap_or("")
            .contains("无文件进展提醒")
    });
    assert!(nudged, "应在第 8 轮透出无文件进展提醒事件: {events}");
    let fs_write_calls = rows
        .iter()
        .filter(|e| {
            e["detail"]
                .as_str()
                .unwrap_or("")
                .contains("调用工具 fs_write")
        })
        .count();
    assert!(
        fs_write_calls <= 16,
        "守卫应在第 16 轮止损,实际 fs_write 事件 {fs_write_calls} 次: {events}"
    );
    // 语义熔断已关闭,不应由它先收尾
    let semantic = rows
        .iter()
        .any(|e| e["detail"].as_str().unwrap_or("").contains("重复空转熔断"));
    assert!(!semantic, "语义熔断已关闭,不应由它收尾: {events}");

    let _ = std::fs::remove_dir_all(&ws);
}
