// 内置画布示范流程的端到端(FLOW-DEMO-1,2026-10-01):绑定 `builtin-research-demo`
//(多路调研示范流程)的 custom 任务在 mock 下跑完拓扑——
//   拆解 → 两路并行(facts ‖ risks)→ 汇合(reflect verify)→ 严格终稿(report)
// 断言:5 节点全 done、plan 行 node_id 对齐、**汇合节点输入含两路分段且按 inputs
// 下标升序**、成果 = 严格终稿输出。
//
// 独立成文件同其它 task_custom_*.rs:流程库是全局单例(data/agent_flows.json +
// current_flow_id),单文件即单进程/单数据目录;辅助函数按「谁用谁带」复制。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
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

async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    for _ in 0..120 {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let st = json["task"]["status"].as_str().unwrap_or("").to_string();
        if matches!(st.as_str(), "done" | "partial" | "error" | "ended") {
            return (st, json);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("任务 {id} 未在超时内到达终态");
}

/// 内置示范流程可直接被 custom 任务绑定并以冻结快照跑完:
/// `[[reply:]]` 钩子让每个节点的产出确定(节点消息都携带任务目标,标记随之生效)。
#[tokio::test]
async fn builtin_demo_flow_runs_topology_end_to_end() {
    let app = test_app();

    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": "[[reply:各节点成果文本]] 调研示范目标",
            "task_mode": "custom",
            "flow_id": "builtin-research-demo",
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "内置示范流程应可直接绑定(创建期快照校验通过): {json}"
    );
    let id = json["task"]["id"].as_str().unwrap().to_string();

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "示范流程应跑完拓扑: {detail}");

    // plan 行 = 流程数组序,node_id 对齐;五节点全部完成
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 5, "示范流程共 5 节点: {detail}");
    let ids: Vec<&str> = plan
        .iter()
        .map(|r| r["node_id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        ids,
        vec!["decompose", "facts", "risks", "verify", "report"],
        "plan 行应按流程数组序并带 node_id: {detail}"
    );
    for row in plan {
        assert_eq!(row["status"], "done", "全部节点应完成: {detail}");
    }

    // 成果 = 严格终稿(report,显式 is_output 选中)
    assert_eq!(
        detail["task"]["result"].as_str().unwrap_or(""),
        "各节点成果文本",
        "成果应为终稿输出: {detail}"
    );

    // 汇合节点(verify,step_index=3)的输入含两路分段,且按 inputs 下标升序
    //(facts 先于 risks;多上游拼接格式见 custom.rs::node_input)
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let verify_call = calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["phase"] == "step" && c["step_index"] == 3)
        .unwrap_or_else(|| panic!("应有 verify 节点的调用行: {calls}"));
    let ps = verify_call["prompt_summary"].as_str().unwrap_or("");
    let facts_at = ps
        .find("【事实与现状】")
        .unwrap_or_else(|| panic!("汇合输入应含 facts 分段: {ps}"));
    let risks_at = ps
        .find("【问题与风险】")
        .unwrap_or_else(|| panic!("汇合输入应含 risks 分段: {ps}"));
    assert!(
        facts_at < risks_at,
        "多上游分段应按 inputs 下标升序(facts 在前): {ps}"
    );
}
