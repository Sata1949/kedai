// 节点级上下文上限的回归(二维批次 8;`PlanStep.max_context`)。
//
// 口径(见 `services/prompt_kit.rs::select_segments_within_budget` 与
// `task_engine/custom.rs::apply_step_max_context`):
//   - 留空 = **不裁剪**(与批次 8 之前逐字节一致,存量流程零行为变化);
//   - 超预算时从**最旧**的上游产出段起省略(正文换成标记、标签行保留),任务目标恒保留;
//   - 任务目标单独超预算 → 该节点**明确报错**(不静默截断);
//   - 挂载子流程的节点**旁路**该字段(子图各节点各自裁剪),配置保留。
//
// 观测手段:mock 的 `[[tool_echo:…]]` 钩子在「已有工具结果」的后续轮**回显完整消息序列**
// (每行 `[角色] 内容`),而本文件把钩子写进**任务目标**(标题)——它出现在每个节点的 user
// 消息里(源节点即目标本身,下游节点在「任务目标:」段)。选中的成果节点即回显节点,于是
// **进 prompt 的节点输入**可直接在任务 result 上断言。
//
// 独立成文件的原因同其它 task_custom_*.rs:流程库是全局单例(data/agent_flows.json +
// current_flow_id),单文件即单进程/单数据目录;文件内多条用例共享流程库,故统一 test_lock()。
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

/// 省略标记(后端单一出处 `services/prompt_kit.rs::OMITTED_SEGMENT_MARKER`)
const OMIT_MARKER: &str = "(因本节点上下文上限省略)";

/// 上游节点 system prompt 里的**长探针**:出现与否即「上游产出是否原样进了下游 prompt」
const NEEDLE: &str = "裁剪探针甲乙丙";

/// 上游节点:宽松档 + 只读工具(不吃钩子,走普通工具循环),system prompt 带长探针——
/// 于是它的产出(回显/回复)足够长,足以撑爆下游 256 token 的预算。
fn upstream_step(system_prompt: String) -> Value {
    json!({
        "id": "n1",
        "name": "上游节点",
        "enabled": true,
        "goal": "产出上游内容",
        "action": "direct",
        "generates": true,
        "is_output": false,
        "kind": "loose",
        "tools": ["read"],
        "system_prompt": system_prompt,
    })
}

/// 下游节点:成果节点 + 回显钩子(钩子在任务目标里,故本节点 user 消息含它)。
/// `max_context` 由用例给定(None = 不带该键,即留空不裁剪)。
fn downstream_step(max_context: Option<u32>) -> Value {
    let mut step = json!({
        "id": "n2",
        "name": "下游节点",
        "enabled": true,
        "goal": "消费上游产出",
        "action": "direct",
        "generates": true,
        "is_output": true,
        "kind": "loose",
        "tools": ["read"],
    });
    if let Some(v) = max_context {
        step["max_context"] = json!(v);
    }
    step
}

/// 长探针正文:重复片段到足够长(约千 token 量级,远超 256 的下限预算)
fn long_probe() -> String {
    format!("【本步指令·上游】{NEEDLE}").to_string()
        + &"上下游内容占位甲乙丙丁戊己庚辛壬癸".repeat(120)
}

/// mock 标记参数序列化:`]` 写成 `\u005d`(理由见 `task_custom_compare.rs` 文件头)
fn args_text(v: &Value) -> String {
    serde_json::to_string(v).unwrap().replace(']', "\\u005d")
}

/// 回显钩子(写进任务目标,故每个节点的 user 消息都带它)
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

/// 取某个 plan 行的 `result`(节点级失败文案落在**行**上,不在 task.error——
/// 后者是终态汇总语,见 `task_engine/custom.rs::run_inner`)
fn plan_row_result(detail: &Value, step_name: &str) -> String {
    detail["task"]["plan"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["name"].as_str() == Some(step_name))
        .and_then(|row| row["result"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// 设了上限的下游节点:上游产出**被省略**、标记可见、任务目标保留。
///
/// 判别性:上游节点的 system prompt 带长探针,其产出因此很长(远超 256 token)。下游
/// `max_context = 256` 时,「任务目标(head) + 上游段」必然超预算,而 head 单独在预算内
/// ——按口径省略上游段。断言 result(下游节点的回显)里**没有**探针文本、**有**省略标记。
#[tokio::test]
async fn context_limit_omits_oldest_upstream_segment_and_marks_it() {
    let _guard = test_lock().await;
    let app = test_app();

    let flow = save_flow(
        app,
        "上下文上限流程",
        json!([upstream_step(long_probe()), downstream_step(Some(256))]),
    )
    .await;

    let probe = json!({ "path": "__kedai_ctx_probe__.txt" });
    let title = format!("{} 上下文裁剪用例", echo_marker("read", &probe));
    let id = create_ok(app, &title, &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "裁剪不该让任务失败: {detail}");

    let res = result_of(&detail);
    assert!(
        res.contains("任务目标:"),
        "任务目标恒保留(不得被省略): {res}"
    );
    assert!(
        res.contains(OMIT_MARKER),
        "被省略的上游段必须留下可见标记(不能静默少一段): {res}"
    );
    assert!(
        !res.contains(NEEDLE),
        "超出预算的上游产出不得进入下游 prompt(判别性:未裁剪时此处会命中探针): {res}"
    );
}

/// 回归(**不得误伤**):同一条流程、下游**不带** `max_context` → 上游产出原样进入下游 prompt。
///
/// 这条与上一条构成判别对:同一份长探针,唯一变量是下游有没有设上限。
#[tokio::test]
async fn node_without_limit_keeps_full_upstream_output() {
    let _guard = test_lock().await;
    let app = test_app();

    let flow = save_flow(
        app,
        "无上限流程",
        json!([upstream_step(long_probe()), downstream_step(None)]),
    )
    .await;

    let probe = json!({ "path": "__kedai_ctx_probe__.txt" });
    let title = format!("{} 无上限用例", echo_marker("read", &probe));
    let id = create_ok(app, &title, &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成: {detail}");

    let res = result_of(&detail);
    assert!(
        res.contains(NEEDLE),
        "留空 = 不裁剪:上游产出必须原样进入下游 prompt: {res}"
    );
    assert!(
        !res.contains(OMIT_MARKER),
        "留空 = 不裁剪:不得出现省略标记: {res}"
    );
}

/// 任务目标单独超预算 → 该节点**明确报错**(不静默截断):行状态 error + 点名原因与所设上限。
///
/// 任务整体是 `partial` 而非 `error`:成果节点(n2)失败后,成果选拔按既有口径回退到
/// 「末个非空生成产出」(n1),于是任务有成果可交——这正是「节点级失败不炸掉整条流程」的
/// 既有语义(见 `custom.rs::run_inner` 的 `select_draft`),本用例一并把它锁住。
#[tokio::test]
async fn goal_alone_over_limit_fails_node_with_named_error() {
    let _guard = test_lock().await;
    let app = test_app();

    let flow = save_flow(
        app,
        "目标超限流程",
        json!([upstream_step(long_probe()), downstream_step(Some(256))]),
    )
    .await;

    // 目标(标题)本身远超 256 token,而它是「恒保留」的 head → 无解,必须报错
    let probe = json!({ "path": "__kedai_ctx_probe__.txt" });
    let long_goal = "超长任务目标占位文字".repeat(120);
    let title = format!("{} {} 目标超限用例", echo_marker("read", &probe), long_goal);
    let id = create_ok(app, &title, &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(
        st, "partial",
        "成果节点失败但上游仍有成果 → 回退选中上游产出,任务记为 partial: {st}"
    );
    let row_status = detail["task"]["plan"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["name"].as_str() == Some("下游节点"))
        .and_then(|row| row["status"].as_str())
        .unwrap_or_default()
        .to_string();
    assert_eq!(row_status, "error", "超限节点必须标 error: {detail}");

    let row = plan_row_result(&detail, "下游节点");
    assert!(
        row.contains("上下文上限") && row.contains("任务目标"),
        "错误须点名原因(不静默截断任务目标): {row}"
    );
    assert!(
        row.contains("256"),
        "错误须带上所设的上限值,便于用户判断该调多大: {row}"
    );
}
