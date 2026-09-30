// 编码能力包流程预设集集成测试(2026-09-30 CODE-5):
//  - 关包 → 内置库与今天逐条一致(1 条);开包 → 立刻 1+2 条(运行期钩子,无需重启)
//  - 幂等 / 关包不回收 / 删过不复活 / 既有条目逐字不变
//
// **独立测试二进制**:本文件独占一个进程与一套 `%TEMP%\kedai-test-<pid>`(见 tasks_crud.rs
// 顶部的拆分说明),故不必与 `agent_flows.rs` 抢 `test_lock`,也不可能把「开过包」的状态
// 漏给别的用例——这正是拆分的用意。辅助函数按「谁用谁带」复制,不建共享层。

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
        let dir = kedai_server::utils::test_support::TempDataDir::new("test-runtime-prompt-empty");
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

/// 按 task 视角写入编码能力包开关(设置页同一条路径:`?mode=task` + 该字段)
async fn set_coding_bundle(app: &axum::Router, enabled: bool) -> Value {
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_coding_bundle_enabled": enabled }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写设置失败: {r}");
    r
}

/// 完整走一遍「关 → 开 → 幂等 → 不回收 → 不复活」:运行期钩子的五条语义。
/// 串成一条用例是因为它们共享同一份进程内状态,拆开反而要互相复原。
#[tokio::test]
async fn coding_pack_flows_follow_the_switch() {
    let app = test_app();

    // ① 关包(默认):只有内置协调流程
    assert_eq!(
        flow_ids(app).await,
        vec!["builtin-coordination"],
        "关包时内置集应与今天逐条一致"
    );

    // 记下既有条目与库文件形态(后面比对「逐字不变」)
    let (_, before) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    let builtin_before = before["library"]["flows"][0].clone();

    // ② 开包(PUT 设置):**立即**可见 1+2 条 —— 走的是运行期钩子,不需要重启
    set_coding_bundle(app, true).await;
    assert_eq!(
        flow_ids(app).await,
        vec![
            "builtin-coordination",
            "builtin-code-review",
            "builtin-code-impl"
        ],
        "开包后应立刻并入两条编码流程(运行期钩子生效)"
    );

    // ③ 既有条目逐字不变(并入只 push 新条目)
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
    set_coding_bundle(app, true).await;
    assert_eq!(flow_ids(app).await.len(), 3, "重复开包不得重复注入");

    // ⑤ 关包不回收已注入副本(既定边界,同 LIT-5 Q2)
    set_coding_bundle(app, false).await;
    assert_eq!(flow_ids(app).await.len(), 3, "关包不回收已注入的副本");

    // ⑥ 可导出:注入进库后就是普通流程,导出闭包应当带上它(LIT-5 同款断言)
    let (status, bundle) = send_json(app, "GET", "/api/agent-flows/export", json!({})).await;
    assert_eq!(status, StatusCode::OK, "导出失败: {bundle}");
    let exported: Vec<&str> = bundle["bundle"]["flows"]
        .as_array()
        .expect("导出应含 bundle.flows 数组")
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert!(
        exported.contains(&"builtin-code-review"),
        "导出应含审查流程: {exported:?}"
    );
    assert!(
        exported.contains(&"builtin-code-impl"),
        "导出应含实现流程: {exported:?}"
    );

    // ⑦ 用户删过的不复活:删掉审查流程 → 再开包不得把它加回来
    let (status, del) = send_json(
        app,
        "DELETE",
        "/api/agent-flows/builtin-code-review",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "删除包流程失败: {del}");
    set_coding_bundle(app, true).await;
    assert_eq!(
        flow_ids(app).await,
        vec!["builtin-coordination", "builtin-code-impl"],
        "用户删过的包流程不得复活"
    );
}
