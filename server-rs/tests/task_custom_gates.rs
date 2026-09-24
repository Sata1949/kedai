// 调用闸与默认节点上下文的**可配置性**回归(A 批 A3/A4)。
//
// 口径(见 `交接稿-自定义流程AB批.md` R4/R5):
//   - A3:`max_flow_call_depth`(默认 2,1..=5)与 `max_flow_calls_per_task`(默认 8,1..=64)
//     是**任务侧**设置项,`agent_flow_service` 的两个常量退居缺省值与测试基准;
//     消费点在 `task_engine/flow_call.rs`(取执行开始时的设置快照,执行期改设置不影响本轮);
//   - A4:`default_node_max_context`(默认 **0 = 不裁剪**,否则 256..=1048576)只在该节点
//     没写 `max_context` 时生效(节点值优先);0 时与批次 8 之前逐字节一致。
//
// 独立成文件的原因:本文件**会改进程级设置**(任务侧覆盖层),与其它文件共享进程会让
// 它们的缺省假设失效——这正是 `settings_connector` 的 TEST-ISO-1 踩过的坑,
// 故凡改设置的用例一律新开文件(单文件即单进程/单数据目录)。
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

/// 串行化全局共享状态(流程库 + 设置 + current_flow_id)的用例
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

/// 任务侧设置写入(路径带 mode=task,故落在 ModeSettings 覆盖层)
async fn put_task_settings(app: &axum::Router, patch: Value) -> (StatusCode, Value) {
    send_json(app, "PUT", "/api/settings?mode=task", patch).await
}

async fn task_settings(app: &axum::Router) -> Value {
    let (status, json) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    json
}

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

fn result_of(detail: &Value) -> String {
    detail["task"]["result"].as_str().unwrap_or("").to_string()
}

/// 省略标记(后端单一出处 `services/prompt_kit.rs::OMITTED_SEGMENT_MARKER`)
const OMIT_MARKER: &str = "(因本节点上下文上限省略)";
/// 上游节点 system prompt 里的长探针:出现与否即「上游产出是否原样进了下游 prompt」
const NEEDLE: &str = "默认预算探针甲乙丙";

/// `]` 写成 `\u005d`(理由见 `task_custom_compare.rs` 文件头)
fn args_text(v: &Value) -> String {
    serde_json::to_string(v).unwrap().replace(']', "\\u005d")
}

/// 回显钩子(写进任务目标,故每个节点的 user 消息都带它)
fn echo_marker(name: &str, args: &Value) -> String {
    format!("[[tool_echo:{name} {}]]", args_text(args))
}

/// 前 N 轮各返回 1 个 ToolCall(参数逐轮变化,用于预算闸用例)
fn loop_marker(n: usize, args: &Value) -> String {
    format!("[[tool_loop:run_flow|{n} {}]]", args_text(args))
}

fn run_flow_args(flow: &str) -> Value {
    json!({ "flow": flow })
}

async fn calls_of(app: &axum::Router, id: &str) -> Value {
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    calls
}

/// 动态调用层的调用行(phase 以 `call.` 起头):**闸门是否真的放行**的判据——
/// 被拒的调用在任何模型调用之前就返回,故不产生行。
fn dynamic_rows(calls: &Value) -> Vec<String> {
    calls["calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| c["phase"].as_str().unwrap_or("").to_string())
        .filter(|p| p.starts_with("call."))
        .collect()
}

/// 宽松档节点(带 read 工具)+ 可指定 system prompt(回显上游产出用)
fn node(id: &str, name: &str, is_output: bool, system_prompt: Option<String>) -> Value {
    let mut step = json!({
        "id": id,
        "name": name,
        "enabled": true,
        "goal": "推进任务",
        "action": "direct",
        "generates": true,
        "is_output": is_output,
        "kind": "loose",
        "tools": ["read"],
    });
    if let Some(sp) = system_prompt {
        step["system_prompt"] = json!(sp);
    }
    step
}

fn long_probe() -> String {
    format!("【本步指令·上游】{NEEDLE}") + &"上下游内容占位甲乙丙丁戊己庚辛壬癸".repeat(120)
}

/// A3:设置项真的驱动闸门——把「每任务流程调用上限」调到 1,只放行一次;
/// 同一份流程在缺省设置(8)下则放行三次。
///
/// 判别性:两轮只差**设置值**,调用行数 1 → 3;若闸仍读常量(8),两轮都会是 3 行。
#[tokio::test]
async fn flow_call_budget_setting_drives_the_gate() {
    let _guard = test_lock().await;
    let app = test_app();

    let callee = save_flow(
        app,
        "闸门设置被调流程",
        json!([node("c1", "被调节点", true, None)]),
    )
    .await;
    let root = save_flow(
        app,
        "闸门设置根流程",
        json!([node("r1", "根节点", true, None)]),
    )
    .await;

    // 每轮 `_mock_round` 注入使参数指纹互不相同 → 走预算闸而不是重复调用熔断
    let title = format!("{} 请连调三次", loop_marker(3, &run_flow_args(&callee)));

    let (status, body) = put_task_settings(app, json!({ "max_flow_calls_per_task": 1 })).await;
    assert_eq!(status, StatusCode::OK, "设置写入应 200: {body}");
    // 名单随任务创建一起下发(对比模式),故这里不能走 create_ok(那个不带名单)
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": title, "task_mode": "custom",
            "flow_id": root, "flow_ids": [callee]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();
    run_task(app, &id).await;
    let (_st, _detail) = wait_terminal(app, &id).await;
    let calls = calls_of(app, &id).await;
    assert_eq!(
        dynamic_rows(&calls).len(),
        1,
        "上限设为 1 时只应有 1 次动态调用落地: {calls}"
    );

    // 缺省设置(8)下同一份钩子应真的跑出 3 次
    let (status, _) = put_task_settings(app, json!({ "max_flow_calls_per_task": 8 })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": title, "task_mode": "custom",
            "flow_id": root, "flow_ids": [callee]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let id2 = json["task"]["id"].as_str().unwrap().to_string();
    run_task(app, &id2).await;
    let (_st, _detail) = wait_terminal(app, &id2).await;
    let calls2 = calls_of(app, &id2).await;
    assert_eq!(
        dynamic_rows(&calls2).len(),
        3,
        "缺省上限 8 下三次调用都应落地(证明差异来自设置而非流程本身): {calls2}"
    );
}

/// A3 设置项的读写与双模式隔离:任务侧写入不污染角色扮演侧。
#[tokio::test]
async fn gate_settings_roundtrip_and_stay_task_scoped() {
    let _guard = test_lock().await;
    let app = test_app();

    let (status, body) = put_task_settings(
        app,
        json!({ "max_flow_call_depth": 4, "max_flow_calls_per_task": 20 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "设置写入应 200: {body}");

    let task_view = task_settings(app).await;
    assert_eq!(task_view["max_flow_call_depth"], 4, "{task_view}");
    assert_eq!(task_view["max_flow_calls_per_task"], 20, "{task_view}");

    // 角色扮演侧(扁平值)不受影响:那里仍是缺省 2 / 8
    let (status, roleplay_view) =
        send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        roleplay_view["max_flow_call_depth"], 2,
        "任务侧改设置不得污染角色扮演侧: {roleplay_view}"
    );
    assert_eq!(
        roleplay_view["max_flow_calls_per_task"], 8,
        "{roleplay_view}"
    );

    // 越界一律 400(1..=5 / 1..=64)
    let (status, body) = put_task_settings(app, json!({ "max_flow_call_depth": 6 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "深度 6 应 400: {body}");
    let (status, body) = put_task_settings(app, json!({ "max_flow_calls_per_task": 0 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "次数 0 应 400: {body}");

    // 复位
    let (status, _) = put_task_settings(
        app,
        json!({ "max_flow_call_depth": 2, "max_flow_calls_per_task": 8 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// A4:缺省 0 = 不裁剪;设为 256 后**没有写 max_context 的节点**按它裁剪;
/// 节点级值仍优先(大值覆盖全局小值 → 不裁剪)。
#[tokio::test]
async fn default_node_max_context_applies_only_when_node_has_none() {
    let _guard = test_lock().await;
    let app = test_app();

    let downstream = |id: &str, max_context: Option<u32>| {
        let mut step = node(id, "下游节点", true, None);
        if let Some(v) = max_context {
            step["max_context"] = json!(v);
        }
        step
    };
    let upstream = node("n1", "上游节点", false, Some(long_probe()));
    // 回显钩子写进任务目标:每个节点都在「已有工具结果」的后续轮回显完整消息序列,
    // 于是**选中的成果节点**的回显文本里就能看到「上游产出是否原样进了它的 prompt」。
    // (上游节点自身也带 read 工具,故它的产出 = 自己那份回显,长到足以撑爆 256 预算。)
    let probe = json!({ "path": "__kedai_default_budget_probe__.txt" });
    let title = format!("{} 默认预算用例", echo_marker("read", &probe));

    // ① 设置 = 256:下游节点**未写** max_context → 应按设置裁剪(探针被省略、留标记)
    let flow = save_flow(
        app,
        "默认预算流程A",
        json!([upstream.clone(), downstream("n2", None)]),
    )
    .await;
    let (status, body) = put_task_settings(app, json!({ "default_node_max_context": 256 })).await;
    assert_eq!(status, StatusCode::OK, "设置写入应 200: {body}");
    let id = create_ok(app, &title, &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "裁剪不该让任务失败: {detail}");
    let echoed = result_of(&detail);
    assert!(
        !echoed.contains(NEEDLE),
        "设置 256 时下游节点的上游产出段应被省略: {echoed}"
    );
    assert!(
        echoed.contains(OMIT_MARKER),
        "省略应有标记(不静默): {echoed}"
    );

    // ② 节点级大值优先于全局小值:下游写 max_context=1048576 → 不裁剪
    let flow = save_flow(
        app,
        "默认预算流程B",
        json!([upstream.clone(), downstream("n2", Some(1_048_576))]),
    )
    .await;
    let id = create_ok(app, &format!("{title}(节点优先)"), &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "{detail}");
    let echoed = result_of(&detail);
    assert!(
        echoed.contains(NEEDLE),
        "节点级值应优先于全局默认(大值 = 不裁剪): {echoed}"
    );

    // ③ 设置复位 0 = 不裁剪:同一份流程(下游无 max_context)不再裁剪
    let (status, _) = put_task_settings(app, json!({ "default_node_max_context": 0 })).await;
    assert_eq!(status, StatusCode::OK);
    let flow = save_flow(
        app,
        "默认预算流程C",
        json!([upstream, downstream("n2", None)]),
    )
    .await;
    let id = create_ok(app, &format!("{title}(关闭)"), &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "{detail}");
    assert!(
        result_of(&detail).contains(NEEDLE),
        "设置 0 时必须与批次 8 之前一致(不裁剪): {detail}"
    );

    // ④ 越界:非 0 且低于下限 → 400
    let (status, body) = put_task_settings(app, json!({ "default_node_max_context": 100 })).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "100 既不是 0 也低于下限 256,应 400: {body}"
    );
}
