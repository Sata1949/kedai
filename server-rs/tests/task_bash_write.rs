// TMR3-L6 回归:bash 写入的两种边界。
//
// ① bash 重定向写「本主声明文件」必须算进展——否则 16 轮无进展守卫会把
//    「每轮都用 bash 追加分片」的合法写法提前收尾(此前只认 fs_write/fs_edit);
// ② bash 写「别的主 agent 声明的文件」必须被所有权拒绝——否则多主并行时
//    仍可绕过 TMR-4 的 last-writer-wins 防护。
//
// 与 tasks_modes_team.rs 分开成独立进程:本用例要关闭语义熔断(重复写同一文件
// 会被输出指纹判空转),并打开命令执行总开关;设置是进程级共享的。
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
    for _ in 0..200 {
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
    let dir = std::env::temp_dir().join(format!("kedai-l6-{tag}-{}-{ns}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

async fn create_team_task(app: &axum::Router, title: &str, ws: &Path) -> String {
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
    json["task"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn bash_writes_count_as_progress_and_respect_ownership() {
    let app = test_app();
    // 打开命令执行;关闭语义熔断(同一命令重复追加会被指纹判空转,测不到进度判定)
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "exec_enabled": true, "loop_guard_semantic_min_calls": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "设置应 200: {json}");

    let ws = temp_workspace("bash-write");
    // 规划:主 1 用 bash 追加 a.txt(声明 a.txt);主 2 用 bash 覆盖 a.txt(未声明)。
    // 主 2 的目标文本提到 a.txt,依赖推断会排到主 1 之后,覆盖顺序确定。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"甲","goals":[{"name":"写分片","goal":"\u005b\u005btool_loop_text:bash|20 {\"command\":\"echo A >> a.txt\"}\u005d\u005d 用 bash 逐轮追加","files":["a.txt"]}]},"#,
        r#"{"name":"乙","goals":[{"name":"越权","goal":"\u005b\u005btool_loop_text:bash|2 {\"command\":\"echo B > a.txt\"}\u005d\u005d 尝试覆盖别的主的文件"}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " bash 写盘边界"
    );
    let id = create_team_task(app, title, &ws).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert!(
        st == "done" || st == "partial",
        "任务应带产出结束,实际 {st}: {detail}"
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

    let content = std::fs::read_to_string(ws.join("a.txt")).unwrap_or_default();
    assert!(
        content.contains('A'),
        "主 1 的 bash 写入应落盘: {content:?}"
    );
    assert!(
        !content.contains('B'),
        "主 2 的越权 bash 写入不应落盘: {content:?}"
    );

    let has_conflict = rows.iter().any(|e| {
        e["detail"]
            .as_str()
            .unwrap_or("")
            .contains("文件所有权冲突")
    });
    assert!(has_conflict, "越权 bash 应被所有权拒绝: {events}");
    let stopped = rows.iter().any(|e| {
        e["detail"]
            .as_str()
            .unwrap_or("")
            .contains("无文件进展收尾")
    });
    assert!(
        !stopped,
        "bash 写声明文件应计为进展,不应触发 16 轮无进展收尾: {events}"
    );
    let bash_calls = rows
        .iter()
        .filter(|e| e["detail"].as_str().unwrap_or("").contains("调用工具 bash"))
        .count();
    assert!(
        bash_calls >= 17,
        "主 1 应跑满 20 轮 bash(另含主 2 的 2 次),实际 {bash_calls} 次: {events}"
    );

    let _ = std::fs::remove_dir_all(&ws);
}
