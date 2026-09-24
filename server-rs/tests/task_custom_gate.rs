// 任务侧工具闸门的**空集语义**回归(2026-09-24;承 `遗留.md` IFW-12「附带发现」)。
//
// 缺陷形态:`custom` 节点「任务策略 ∩ 步骤白名单 = 空集」时下发工具清单为空,而
// `ToolGate` 的旧语义「空名单 = 全量放行」让模型**臆造**的工具调用被 `custom_authorized=true`
// 放行执行——下发清单为空,实际却什么都放行(fail-open)。修复后空名单 = 拒绝一切。
//
// 独立成文件的原因同其它 task_custom_*.rs:流程库是全局单例(data/agent_flows.json +
// current_flow_id),单文件即单进程/单数据目录;文件内多条用例共享流程库,故统一 test_lock()。
//
// mock 钩子:`[[tool_echo:<name> {args}]]` 首轮返回 2 个 ToolCall、有工具结果后回显完整
// 消息序列——于是**工具结果文本**(闸门拒绝文案 / 工具真执行的结果)可直接在任务 result 上断言。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

fn test_app() -> &'static axum::Router {
    static APP: OnceLock<axum::Router> = OnceLock::new();
    APP.get_or_init(|| build_test_app().expect("构建测试应用失败"))
}

/// 串行化全局共享状态(流程库 + current_flow_id)的用例
async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
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

/// 保存流程并返回 id(取自 PUT 响应的 `config.id`,不按名字回查——理由见 `task_custom_compare.rs`)
async fn save_flow(app: &axum::Router, name: &str, steps: Value) -> String {
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": { "id": "", "name": name, "enabled": true, "steps": steps } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "保存流程「{name}」应 200: {json}");
    json["config"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("保存响应应带流程 id: {json}"))
        .to_string()
}

/// 单节点宽松流程:步骤工具白名单由用例给定(空数组 = 策略全量集)
fn whitelist_node_flow(needle: &str, tools: &[&str]) -> Value {
    json!([{
        "id": "n1",
        "name": format!("节点{needle}"),
        "enabled": true,
        "goal": "按本步指令产出",
        "action": "direct",
        "generates": true,
        "is_output": true,
        "system_prompt": format!("【本步指令·{needle}】只输出钩子指定的内容"),
        "tools": tools
    }])
}

/// mock 标记参数序列化:`]` 写成 `\u005d`(理由见 `task_custom_compare.rs` 文件头)
fn args_text(v: &Value) -> String {
    serde_json::to_string(v).unwrap().replace(']', "\\u005d")
}

/// 首轮 2 个调用、次轮回显消息序列的标记(name = 本文件的「臆造」目标工具)
fn echo_marker(name: &str, args: &Value) -> String {
    format!("[[tool_echo:{name} {}]]", args_text(args))
}

async fn create_ok(app: &axum::Router, title: &str, flow_id: &str) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": "custom", "flow_id": flow_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

async fn run_task(app: &axum::Router, id: &str) {
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
}

async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    for _ in 0..80 {
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

fn result_of(detail: &Value) -> String {
    detail["task"]["result"].as_str().unwrap_or("").to_string()
}

/// 闸门拒绝的固定文案(`agents/engine/executor.rs` 的 `gate.excludes` 分支)
const GATE_DENY: &str = "不在当前任务策略允许的工具清单内";

/// 空集节点必须**拒绝一切**臆造调用。
///
/// 判别性设计:步骤白名单只写 `write`(默认 `deny_dangerous` 策略剔除的危险工具),
/// 于是「策略 ∩ 白名单」为空、下发清单为空;模型臆造 `read`(策略内、但**未下发**)。
/// 修复前 gate 名单为空 = 全量放行 → read 真的执行,result 里不会出现闸门拒绝文案;
/// 修复后空集 = 拒绝一切,result 里应出现该文案。拒绝发生在工具执行前,不产生副作用。
#[tokio::test]
async fn empty_whitelist_node_denies_hallucinated_tool_call() {
    let _guard = test_lock().await;
    let app = test_app();

    let a = save_flow(app, "空集闸门流程", whitelist_node_flow("空集", &["write"])).await;

    let probe = json!({ "path": "__kedai_gate_probe_missing__.txt" });
    let title = format!("{} 空集闸门用例", echo_marker("read", &probe));
    let id = create_ok(app, &title, &a).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "done",
        "臆造调用被拒后任务仍应完成(错误回灌给模型): {detail}"
    );

    let res = result_of(&detail);
    assert!(
        res.contains(GATE_DENY),
        "空下发集的节点必须拒绝一切工具调用(旧语义「空名单 = 全量放行」会在这里放行 read 并真执行): {res}"
    );
}

/// 回归(**不得误伤**):白名单非空时,名单内工具照常放行——本次修复只改「空集」语义。
///
/// 白名单 `["read"]`(策略内)→ 下发集非空 → read 被闸门放行,执行结果(此处是「文件不存在」
/// 这类工具自身失败)照常回灌;断言里必须**没有**闸门拒绝文案。
#[tokio::test]
async fn non_empty_whitelist_still_authorizes_listed_tool() {
    let _guard = test_lock().await;
    let app = test_app();

    let a = save_flow(app, "非空闸门流程", whitelist_node_flow("非空", &["read"])).await;

    let probe = json!({ "path": "__kedai_gate_probe_missing__.txt" });
    let title = format!("{} 非空闸门用例", echo_marker("read", &probe));
    let id = create_ok(app, &title, &a).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "名单内工具失败不该让任务失败: {detail}");

    let res = result_of(&detail);
    assert!(
        !res.contains(GATE_DENY),
        "名单内工具不该被闸门拒绝(修复不得把非空名单一并收紧): {res}"
    );
}
