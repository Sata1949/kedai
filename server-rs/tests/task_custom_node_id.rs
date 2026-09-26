// 自定义流程的 plan 行 ↔ 流程节点映射(遗留.md IFW-5)。
//
// 为什么单开文件:流程库是**全局单例**(data/agent_flows.json + current_flow_id),而同一
// 测试文件内的用例共享一个 app 实例(见 task_custom_graph.rs 头部注释记载的既知竞态)。
// 本用例要求「当前流程」恰是自己保存的那一份(停用步骤的图),与图执行用例并存会互相覆盖,
// 故按本目录既有体例单开文件 = 单进程 = 单数据目录。
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

async fn create_task_with_mode(app: &axum::Router, title: &str, mode: &str) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": mode }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

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

/// 遗留.md IFW-5:`plan` 行携带 `node_id`(= 流程节点 id),且**停用步骤不入 plan**。
///
/// 为什么这条要看停用:plan 行由过滤后的启用步骤构造,故 plan 下标 ≠ 流程数组下标。
/// 若界面/调用方按下标对齐节点,停用中间步骤后整条映射都会错位——`node_id` 是唯一
/// 可靠的对应关系。本用例把「中线」停掉,断言:
///  - plan 只有两行,node_id 恰是启用节点的 id(中线缺席);
///  - plan 行名与 node_id 指向的节点名一致(映射不串位);
///  - 落库 JSON 里 node_id 是字符串字段(线格式,前端按此对回节点)。
#[tokio::test]
async fn custom_plan_rows_carry_node_id_and_skip_disabled_steps() {
    let app = test_app();

    let flow = json!({
        "config": {
            "id": "",
            "name": "测试 node_id 流程图",
            "enabled": true,
            "steps": [
                {"id":"n-first","name":"起头","enabled":true,"goal":"开头","action":"direct","generates":true,
                 "system_prompt":"【本步指令·起头本步】只输出开头"},
                // 停用:既不执行,也不进 plan —— 下标映射从这一行开始就是错的
                {"id":"n-off","name":"中线(已停用)","enabled":false,"goal":"不该跑","action":"direct","generates":true,
                 "system_prompt":"【本步指令·中线本步】绝不该被调用"},
                {"id":"n-last","name":"收尾","enabled":true,"goal":"收尾","action":"direct","generates":true,
                 "inputs":["n-first"],"is_output":true,
                 "system_prompt":"【本步指令·收尾本步】只输出收尾成果"}
            ]
        }
    });
    let (status, json) = send_json(app, "PUT", "/api/agent-flows", flow).await;
    assert_eq!(status, StatusCode::OK, "保存流程应 200: {json}");

    let title = "[[reply_if:起头本步|开头产出]][[reply_if:收尾本步|收尾成果]] node_id 任务";
    let id = create_task_with_mode(app, title, "custom").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成: {detail}");

    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(
        plan.len(),
        2,
        "停用步骤不入 plan(故 plan 下标 ≠ 流程下标): {detail}"
    );
    // plan 行按流程数组顺序展示,跳过停用步骤
    assert_eq!(plan[0]["name"], "起头");
    assert_eq!(plan[1]["name"], "收尾");
    // node_id 精确指向启用节点 id:停用节点"中线"完全缺席
    assert_eq!(
        plan[0]["node_id"], "n-first",
        "第一行 node_id 应为 n-first: {detail}"
    );
    assert_eq!(
        plan[1]["node_id"], "n-last",
        "第二行 node_id 应为 n-last(不是被停用的 n-off): {detail}"
    );
    let ids: Vec<&str> = plan.iter().filter_map(|s| s["node_id"].as_str()).collect();
    assert_eq!(ids, vec!["n-first", "n-last"]);
    assert!(!ids.contains(&"n-off"), "停用节点不应进入 plan: {detail}");

    // 停用节点确实没跑:调用追踪里没有它的行(phase=step 的两次调用属于起头/收尾)
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let step_rows: Vec<&Value> = calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["phase"] == "step")
        .collect();
    assert_eq!(step_rows.len(), 2, "只有两个启用节点发起调用: {calls:?}");
    for row in step_rows {
        let idx = row["step_index"].as_i64().unwrap();
        assert!(
            (0..2).contains(&idx),
            "step_index 只能落在 plan 两行上(停用节点不占位): {calls:?}"
        );
    }
}
