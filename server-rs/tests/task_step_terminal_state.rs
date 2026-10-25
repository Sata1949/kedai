// SKY-8 回归:team 步骤终态以 `TaskStep.files` 的磁盘事实为准。
//
// 判别力(修复前必红):
//   * 守卫截断 + 声明文件已齐全 → 修复前记 partial,修复后记 done;
//   * 预算收尾 + 声明文件缺失 → 修复前记 done(交付门槛事后才拦),修复后步骤即 partial。
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

fn temp_workspace(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "kedai-step-state-{tag}-{}-{ns}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    for _ in 0..400 {
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

/// 单主、单子目标的 team 标题;goal 里的 `[[...]]` 必须写成 `\u005b\u005b...\u005d\u005d`,
/// 否则会被外层 `reply_if` 的首个 `]]` 截断。
fn team_title(goal: &str, files: &str) -> String {
    format!(
        concat!(
            r#"[[reply_if:团队规划器|{{"mains":[{{"name":"单主","after":[],"goals":[{{"name":"写交付物","goal":"{}","files":[{}]}}]}}]}}]]"#,
            r#"[[reply_if:团队审计员|{{"通过":true,"打回":[],"结论":"通过"}}]]"#,
            r#"[[reply_if:任务汇总者|团队最终成果]]"#,
            " 步骤终态用例"
        ),
        goal, files
    )
}

async fn set_settings(app: &axum::Router, body: Value) {
    let (status, _) = send_json(app, "PUT", "/api/settings", body).await;
    assert_eq!(status, StatusCode::OK, "设置应可写入");
}

async fn create_team_task(app: &axum::Router, ws: &Path, title: &str) -> String {
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

async fn run_task(app: &axum::Router, id: &str) {
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
}

fn plan_step(detail: &Value, index: usize) -> &Value {
    detail["task"]["plan"]
        .as_array()
        .and_then(|rows| rows.get(index))
        .unwrap_or_else(|| panic!("plan[{index}] 不存在: {detail}"))
}

fn delivery_list(detail: &Value, key: &str) -> Vec<String> {
    detail["delivery"][key]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

async fn event_details(app: &axum::Router, id: &str) -> Vec<String> {
    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=2000"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    events["events"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|e| e["detail"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn team_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

const BASE_SETTINGS: &str = r#"{
    "loop_guard_semantic_min_calls": 0,
    "task_step_budget_secs": 3600,
    "task_team_budget_secs": 3600,
    "task_total_budget_secs": 0
}"#;

/// 守卫截断 + 声明文件齐全 → 步骤 done;不因「循环被截断」把已完成的交付判 partial。
#[tokio::test]
async fn guard_truncated_step_with_complete_declared_file_is_done() {
    let _guard = team_lock().lock().await;
    let app = test_app();
    let mut base: Value = serde_json::from_str(BASE_SETTINGS).expect("合法设置 JSON");
    // 本用例只验证无进展守卫:关语义熔断,避免 16 轮同构写先被 HB-2 收走。
    base["loop_guard_semantic_min_calls"] = json!(0);
    set_settings(app, base).await;

    let ws = temp_workspace("guard-done");
    std::fs::write(ws.join("out.txt"), "healthy deliverable\n").expect("预置交付物失败");
    // 反复写未声明 scratch.txt:第 16 轮触发「声明文件无进展收尾」;
    // 声明文件 out.txt 已由工作区预置且内容健康 → SKY-8 应按磁盘事实记 done。
    let title = team_title(
        r#"\u005b\u005btool_loop_any_text:fs_write|20 {\"path\":\"scratch.txt\",\"content\":\"scratch\"}\u005d\u005d 写交付物"#,
        r#""out.txt""#,
    );
    let id = create_team_task(app, &ws, &title).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "done", "声明文件齐全时守卫截断应记 done: {detail}");
    let step = plan_step(&detail, 0);
    assert_eq!(step["status"], "done", "步骤应为 done: {step}");
    assert!(
        step["result"].as_str().unwrap_or("").contains("[收尾说明]"),
        "done 结果应保留守卫截断留痕: {step}"
    );
    assert!(
        delivery_list(&detail, "truncated").is_empty(),
        "已按完成计的步骤不得进入 truncated: {detail}"
    );
    assert!(
        delivery_list(&detail, "missing").is_empty(),
        "声明文件已存在,missing 应为空: {detail}"
    );
    assert!(
        ws.join("scratch.txt").is_file(),
        "模型确实写过未声明临时文件(证明守卫是真实触发而非没跑)"
    );
    let details = event_details(app, &id).await;
    assert!(
        details.iter().any(|d| d.contains("无文件进展收尾")),
        "应触发无进展守卫: {details:?}"
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

/// 预算收尾 + 声明文件缺失 → 步骤 partial(修复前按 TMR-3 记 done,只有任务门槛事后拦)。
#[tokio::test]
async fn budget_stopped_step_with_missing_declared_file_is_partial() {
    let _guard = team_lock().lock().await;
    let app = test_app();
    let mut base: Value = serde_json::from_str(BASE_SETTINGS).expect("合法设置 JSON");
    base["task_step_budget_secs"] = json!(1);
    set_settings(app, base).await;

    let ws = temp_workspace("budget-missing");
    // 每轮真睡 1.2s,第 1 轮末即超过 1s 步骤预算;out.txt 从未落盘。
    let title = team_title(
        r#"\u005b\u005btool_loop_text:sleep|6 {\"ms\":1200}\u005d\u005d 写交付物"#,
        r#""out.txt""#,
    );
    let id = create_team_task(app, &ws, &title).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "partial", "预算收尾且文件缺失应为 partial: {detail}");
    let step = plan_step(&detail, 0);
    assert_eq!(step["status"], "partial", "步骤应为 partial: {step}");
    let step_result = step["result"].as_str().unwrap_or("");
    assert!(
        step_result.contains("[预算收尾]"),
        "步骤结果应说明预算收尾: {step_result}"
    );
    assert!(
        step_result.contains("out.txt:文件缺失"),
        "步骤结果应点名缺失文件: {step_result}"
    );
    assert!(
        delivery_list(&detail, "missing")
            .iter()
            .any(|p| p == "out.txt"),
        "交付摘要应列出 out.txt: {detail}"
    );
    assert!(
        detail["task"]["error"]
            .as_str()
            .unwrap_or("")
            .contains("未完成"),
        "任务 error 应说明子目标未完成: {detail}"
    );

    let _ = std::fs::remove_dir_all(&ws);
    let _ = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "task_step_budget_secs": 1200 }),
    )
    .await;
}

/// 守卫截断 + 声明文件存在但含未替换槽位 → partial,内容问题点名文件。
#[tokio::test]
async fn guard_truncated_step_with_placeholder_file_is_partial() {
    let _guard = team_lock().lock().await;
    let app = test_app();
    let mut base: Value = serde_json::from_str(BASE_SETTINGS).expect("合法设置 JSON");
    base["loop_guard_semantic_min_calls"] = json!(0);
    set_settings(app, base).await;

    let ws = temp_workspace("guard-placeholder");
    std::fs::write(ws.join("out.txt"), "a\n/*@@MAIN@@*/\n").expect("预置槽位文件失败");
    let title = team_title(
        r#"\u005b\u005btool_loop_any_text:fs_write|20 {\"path\":\"scratch.txt\",\"content\":\"scratch\"}\u005d\u005d 写交付物"#,
        r#""out.txt""#,
    );
    let id = create_team_task(app, &ws, &title).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "partial", "声明文件内容体检不过应为 partial: {detail}");
    let step = plan_step(&detail, 0);
    assert_eq!(step["status"], "partial", "步骤应为 partial: {step}");
    assert!(
        step["result"]
            .as_str()
            .unwrap_or("")
            .contains("@@ 未替换槽位"),
        "步骤结果应点名槽位问题: {step}"
    );
    assert!(
        delivery_list(&detail, "content_issues")
            .iter()
            .any(|i| i.contains("out.txt") && i.contains("@@")),
        "delivery.content_issues 应点名 out.txt: {detail}"
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
