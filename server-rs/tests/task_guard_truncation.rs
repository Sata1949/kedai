// SKY-6 第 2 项回归:team 步骤被空转守卫截断时必须记 partial,不得记 done。
//
// 断言链(修复前全部红):
//   ① 步骤状态 `partial`,步骤 result 带 `[守卫截断]` 标记;
//   ② 任务终态 `partial`,`tasks.error` 点名被截断的子目标;
//   ③ 审计调用的 prompt 里能看到截断标记(半成品对审计可见,不再只靠终审偶然抓);
//   ④ 守卫事件照常透出(保持既有可解释性)。
//
// 独立成文件的原因同其它 task_*.rs:设置是进程级共享的,单文件即单进程/单数据目录。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
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

fn temp_workspace(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("kedai-sky6b-{tag}-{}-{ns}", std::process::id()));
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
async fn team_guard_truncation_marks_step_partial_and_audit_sees_it() {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let app = test_app();

    // 只测只读计数守卫:关语义熔断(否则第 8 次同输出读会先被 HB-2 收掉),预算拉高。
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

    let ws = temp_workspace("guard-partial");
    std::fs::write(ws.join("a.txt"), "hello\n").expect("写测试文件失败");

    // 单主单子目标:20 轮 fs_read 不写声明文件 out.txt → 第 8 次提醒、第 16 次熔断。
    // 审计/汇总 hooks 让后续阶段正常收尾;标题尽量短,确保截断标记落在
    // task_llm_calls.prompt_summary 的截断窗口内(每条消息 800 字符)。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"探索主","goals":[{"name":"读文件","goal":"\u005b\u005btool_loop_any_text:fs_read|20 {\"path\":\"a.txt\"}\u005d\u005d 读文件","files":["out.txt"]}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 守卫截断用例"
    );
    let id = create_team_task(app, title, &ws).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "partial",
        "守卫截断的半成品不得判 done(SKY-6 第 2 项): {detail}"
    );

    let plan = detail["task"]["plan"].as_array().expect("plan 应为数组");
    assert_eq!(plan.len(), 1, "单主单子目标: {detail}");
    assert_eq!(
        plan[0]["status"], "partial",
        "步骤应记 partial 而不是 done: {detail}"
    );
    let step_result = plan[0]["result"].as_str().unwrap_or("");
    assert!(
        step_result.contains("[守卫截断]"),
        "步骤结果应带截断标记,供前端/审计直接看到: {step_result:?}"
    );
    let task_error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        task_error.contains("守卫截断"),
        "任务 error 应点名被截断的子目标: {task_error:?}"
    );

    // 守卫事件仍照常透出(既有可解释性不回退)。
    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=2000"),
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
        joined.contains("连续只读空转熔断"),
        "应透出只读熔断事件: {joined}"
    );

    // 审计可见性:审计调用的 prompt_summary 里应出现截断标记。
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "拉取调用记录应 200: {calls}");
    let audit = calls["calls"]
        .as_array()
        .and_then(|rows| rows.iter().find(|c| c["phase"] == "audit"))
        .unwrap_or_else(|| panic!("应有 phase=audit 的调用行: {calls}"));
    let prompt = audit["prompt_summary"].as_str().unwrap_or("");
    assert!(
        prompt.contains("[守卫截断]"),
        "审计输入应包含截断标记,不能只看到一段正常正文: {prompt:?}"
    );

    // 复位设置(同进程其它用例共享)
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
