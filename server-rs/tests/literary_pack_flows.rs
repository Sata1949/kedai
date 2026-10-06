// 文学能力包流程预设集集成测试(LIT-5,2026-10-06):
//  - 关包 → 内置库 = 协调 + 示范两条(FLOW-DEMO-1 起示范随构造期并入,与包无关);
//    开包 → 立刻 2+4 条(运行期钩子,无需重启)
//  - **开关取 OR**:角色扮演侧或任务侧任一开启即并入(流程库是双模式共用设施,
//    四条流程本身是创作场景;只按任务侧会漏掉「只想在聊天里用预设流程」的用户)
//  - 幂等 / 关包不回收 / 删过不复活 / 既有条目逐字不变 / 导出闭包含包流程
//
// **独立测试二进制**:同 `coding_pack_flows.rs` 的理由——本文件独占一个进程与一套
// `%TEMP%\kedai-test-<pid>`,故不必与 `agent_flows.rs` 抢 `test_lock`,也不会把
// 「开过包」的状态漏给别的用例。辅助函数按「谁用谁带」复制,不建共享层。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tower::ServiceExt;

fn test_app() -> &'static axum::Router {
    static APP: OnceLock<axum::Router> = OnceLock::new();
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let dir = kedai_server::utils::test_support::TempDataDir::new("test-lit-pack-flows");
        std::env::set_var("KEDAI_RUNTIME_PROMPT_DIR", dir.path());
    });
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

/// 流程库当前的全部流程 id(按库内顺序)
async fn flow_ids(app: &axum::Router) -> Vec<String> {
    let (status, r) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    assert_eq!(status, StatusCode::OK, "读取流程库失败: {r}");
    r["library"]["flows"]
        .as_array()
        .expect("library.flows 应为数组")
        .iter()
        .map(|f| f["id"].as_str().unwrap().to_string())
        .collect()
}

/// 写角色扮演侧文学包开关(扁平字段;即使请求带 mode=task 也直写扁平)
async fn set_roleplay_bundle(app: &axum::Router, enabled: bool) {
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": enabled }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写角色扮演侧开关失败: {r}");
}

/// 写任务侧文学包开关(task 视角,与设置页同一条路径)
async fn set_task_bundle(app: &axum::Router, enabled: bool) {
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_literary_bundle_enabled": enabled }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写任务侧开关失败: {r}");
}

const LIT_IDS: [&str; 4] = [
    "builtin-lit-chapter",
    "builtin-lit-polish",
    "builtin-lit-consistency",
    "builtin-lit-voice",
];

/// 完整走一遍「关 → 单侧开(OR 语义)→ 幂等 → 不回收 → 不复活 → 导出」。
/// 串成一条用例是因为它们共享同一份进程内状态,拆开反而要互相复原。
#[tokio::test]
async fn literary_pack_flows_follow_either_switch() {
    let app = test_app();

    // ① 两侧都关(默认):内置协调 + 内置示范,文学流程一条都不在
    assert_eq!(
        flow_ids(app).await,
        vec!["builtin-coordination", "builtin-research-demo"],
        "关包时内置集 = 协调 + 示范两条"
    );

    // 记下既有条目与库文件形态(后面比对「逐字不变」)
    let (_, before) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    let builtin_before = before["library"]["flows"][0].clone();

    // ② **只开角色扮演侧** → 立刻 2+4 条:OR 语义(任务侧仍关)
    set_roleplay_bundle(app, true).await;
    let mut expect = vec!["builtin-coordination", "builtin-research-demo"];
    expect.extend(LIT_IDS);
    assert_eq!(
        flow_ids(app).await,
        expect,
        "任一侧开包即并入(OR 语义);运行期钩子无需重启"
    );

    // ③ 既有条目逐字不变 + 当前选中不变
    let (_, after) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    assert_eq!(
        after["library"]["flows"][0], builtin_before,
        "既有内置流程必须逐字不变"
    );
    assert_eq!(
        after["library"]["current_flow_id"], before["library"]["current_flow_id"],
        "并入不得改当前选中流程"
    );

    // ④ 幂等:重复开包不重复注入
    set_roleplay_bundle(app, true).await;
    assert_eq!(flow_ids(app).await.len(), 6, "重复开包不得重复注入");

    // ⑤ 关掉角色扮演侧 → 不回收已注入副本(既定边界,同 Q2=(a) 裁定)
    set_roleplay_bundle(app, false).await;
    assert_eq!(flow_ids(app).await.len(), 6, "关包不回收已注入的副本");

    // ⑥ 用户删过的不复活:删掉一条 → 只开**任务侧**也不得把它加回来
    let (status, del) = send_json(
        app,
        "DELETE",
        "/api/agent-flows/builtin-lit-voice",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "删除包流程失败: {del}");
    set_task_bundle(app, true).await;
    let ids = flow_ids(app).await;
    assert_eq!(ids.len(), 5, "用户删过的包流程不得复活: {ids:?}");
    assert!(!ids.iter().any(|i| i == "builtin-lit-voice"));

    // ⑦ 可导出:注入进库后就是普通流程,导出闭包应带上其余三条
    let (status, bundle) = send_json(app, "GET", "/api/agent-flows/export", json!({})).await;
    assert_eq!(status, StatusCode::OK, "导出失败: {bundle}");
    let exported: Vec<&str> = bundle["bundle"]["flows"]
        .as_array()
        .expect("导出应含 bundle.flows 数组")
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    for id in ["builtin-lit-chapter", "builtin-lit-polish", "builtin-lit-consistency"] {
        assert!(exported.contains(&id), "导出应含 {id}: {exported:?}");
    }
}
