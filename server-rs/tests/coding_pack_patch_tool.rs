// 编码能力包 · fs_patch 包闸门端到端(Q6,2026-10-01)。
//
// 两问:**关包时** mock 的 `[[tool:fs_patch]]` 钩子不会命中(fs_patch 不在本轮下发工具面)
// → 不产生任何文件与台账;**开包时**同一题面真跑一次结构化补丁 → 文件落盘且台账记账
// (create/source=tool)。单元侧已锁策略层闸门,这里补的是「设置开关 → 工具面 → 真执行」
// 的整链证据。
//
// 为什么单独一个文件:本用例要翻转编码包开关,而设置与流程库是进程内共享全局
// (仓库既有裁定:共享全局状态的用例拆文件 + 独立 DATA_DIR,见 coding_pack_flows.rs 头注)。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use kedai_server::utils::test_support::TempDataDir;
use serde_json::{json, Value};
use std::sync::OnceLock;
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

/// 终态轮询(体例照 `coding_pack_flow_runs.rs`)
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    for _ in 0..50 {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let st = json["task"]["status"].as_str().unwrap_or("").to_string();
        if st == "done" || st == "partial" || st == "error" || st == "ended" {
            return (st, json);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("任务 {id} 未在超时内到达终态");
}

#[tokio::test]
async fn fs_patch_is_gated_by_coding_pack_and_runs_when_enabled() {
    let app = test_app();
    let ws_guard = TempDataDir::new("q6-ws");
    let ws = ws_guard.path().to_string_lossy().into_owned();

    // mock 钩子:首轮返回一次真实的 fs_patch 工具调用(Add File 无需先读,单轮可完成)
    let title = r#"[[tool:fs_patch {"patch":"*** Add File: made.txt\n+hi\n"}]] 创建 made.txt"#;

    let create = |title: &'static str, ws: String| json!({ "title": title, "task_mode": "solo", "workspace": ws });

    // ① 关包(缺省):fs_patch 不在工具面 → mock 钩子不命中 → 任务照常 done 但零文件零台账
    let (status, created) = send_json(app, "POST", "/api/tasks", create(title, ws.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "创建任务失败: {created}");
    let id = created["task"]["id"].as_str().unwrap().to_string();
    let (status, r) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {r}");
    let (st, _) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "关包题面应照常完成(mock 回默认文本)");
    assert!(
        !ws_guard.path().join("made.txt").exists(),
        "关包时 fs_patch 不得被执行"
    );
    let (_, changes) = send_json(app, "GET", &format!("/api/tasks/{id}/changes"), json!({})).await;
    assert!(
        changes["changes"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(false),
        "关包时不应有文件台账: {changes}"
    );

    // ② 开包:同一题面 → fs_patch 进工具面 → mock 钩子命中 → 文件落盘并记账
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_coding_bundle_enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "开包写设置失败: {r}");
    let (status, created) = send_json(app, "POST", "/api/tasks", create(title, ws.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "创建任务失败: {created}");
    let id = created["task"]["id"].as_str().unwrap().to_string();
    let (status, r) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {r}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "开包题面应完成: {detail}");
    assert_eq!(
        std::fs::read_to_string(ws_guard.path().join("made.txt")).expect("文件应落盘"),
        "hi\n"
    );
    let (_, changes) = send_json(app, "GET", &format!("/api/tasks/{id}/changes"), json!({})).await;
    let list = changes["changes"].as_array().cloned().unwrap_or_default();
    assert!(
        list.iter()
            .any(|c| c["path"] == "made.txt" && c["op"] == "create" && c["source"] == "tool"),
        "开包后应有一条 made.txt/create/tool 台账: {changes}"
    );
}
