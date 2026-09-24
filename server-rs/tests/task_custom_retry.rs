// 节点级超时与空产出重试的回归(A 批 A1/A2;`PlanStep.call_timeout_secs` / `max_retries`)。
//
// 口径(见 `交接稿-自定义流程AB批.md` R2/R3 与 `docs/契约-协议与配置.md`):
//   - 超时:`None` = 宿主既有 300s 看门狗(行为不变);`Some(n)` = 覆盖本节点**每次**调用的
//     时间预算(可收紧可放宽),范围 30..=3600,超时 = 本节点失败且**不重试**;
//   - 重试:`max_retries` = **额外**尝试上限(总尝试 = 1 + n),**只重试空产出**,
//     每次尝试各记一行 `task_llm_calls`(phase 与首次相同),重试前输出预算按
//     `utils::retry::doubled_heal_budget` 翻倍(封顶 131072)。
//
// 观测手段:mock 的两个钩子——
//   * `[[empty]]`:无条件返回空产出(测「重试用尽」与「未配不重试」);
//   * `[[empty_below:N|内容]]`:预算 < N 时空、否则回复内容(测「预算翻倍后重试成功」——
//     这是 A2 的端到端判别手段,单靠 `[[empty]]` 无法区分「重试了」与「失败得快」)。
//
// 独立成文件的原因同其它 task_custom_*.rs:流程库是全局单例(data/agent_flows.json +
// current_flow_id),单文件即单进程/单数据目录;文件内多条用例共享流程库,故统一 test_lock()。
//
// 超时为什么不端到端测:字段下限是 30s(低于它会误伤正常调用),真等的用例跑不完;
// 「取哪个值」由 `agents/engine/executor.rs` 的 `resolve_call_watchdog` 单测锁定
// (`watchdog_tests::node_call_timeout_overrides_default`),本文件只锁**接线**
// (配了该字段的节点照常跑通)。
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

/// 保存流程并返回状态码(校验用例用:不入库,故不 assert 200)
async fn try_save_flow(app: &axum::Router, name: &str, steps: Value) -> (StatusCode, Value) {
    send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": { "id": "", "name": name, "enabled": true, "steps": steps } }),
    )
    .await
}

/// 单节点流程(成果节点)。`extra` 里的键直接并入该步骤(call_timeout_secs / max_retries /
/// max_tokens / tools …),便于各用例只声明自己关心的字段。
fn single_step(extra: Value) -> Value {
    let mut step = json!({
        "id": "n1",
        "name": "唯一节点",
        "enabled": true,
        "goal": "产出正文",
        "action": "direct",
        "generates": true,
        "is_output": true,
    });
    for (k, v) in extra.as_object().into_iter().flatten() {
        step[k] = v.clone();
    }
    json!([step])
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

async fn calls_of(app: &axum::Router, id: &str) -> Value {
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    calls
}

/// 本层节点的调用行(phase == "step"):重试的每次尝试各占一行,故行数 = 实际尝试次数。
/// 取行数而非「去重后的阶段数」正是本文件的判别点——去重会让「重试了 3 次」与「只跑了 1 次」
/// 长得一模一样。
fn node_call_rows(calls: &Value) -> Vec<&Value> {
    calls["calls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["phase"].as_str() == Some("step"))
        .collect()
}

/// 记账不变量:全部调用行 token 求和 == 详情 usage_total。
/// 重试口径的「每次尝试各记一行」在这里被锁死——若把多次尝试折成一行,
/// 求和与 `usage_total` 都会少算(而两者又**同时**变小,故必须与行数断言配合使用)。
fn assert_usage_invariant(detail: &Value, calls: &Value) {
    let rows = calls["calls"].as_array().unwrap();
    let usage = &detail["usage_total"];
    assert_eq!(
        rows.iter()
            .map(|c| c["prompt_tokens"].as_i64().unwrap_or(0))
            .sum::<i64>(),
        usage["prompt_tokens"].as_i64().unwrap_or(-1),
        "各行 prompt token 求和应等于 usage_total: {detail}"
    );
    assert_eq!(
        rows.iter()
            .map(|c| c["completion_tokens"].as_i64().unwrap_or(0))
            .sum::<i64>(),
        usage["completion_tokens"].as_i64().unwrap_or(-1),
        "各行 completion token 求和应等于 usage_total: {detail}"
    );
}

fn result_of(detail: &Value) -> String {
    detail["task"]["result"].as_str().unwrap_or("").to_string()
}

/// A2 成功路径:空产出 → 重试(预算翻倍)→ 第二次尝试拿到正文。
///
/// 判别性:`max_tokens = 500` 时 mock 返回空(低于 2000 阈值),翻倍到 1000 仍空,
/// 第三次 2000 才回复——三行调用行 + 结果含正文同时成立,才说明「重试真的发生了、
/// 且预算真的翻倍了」。只重试不翻倍会停在两行且结果为空的错误终态。
#[tokio::test]
async fn node_retry_recovers_after_budget_doubles() {
    let _guard = test_lock().await;
    let app = test_app();

    let flow = save_flow(
        app,
        "空产出重试流程",
        single_step(json!({ "max_tokens": 500, "max_retries": 2 })),
    )
    .await;

    let id = create_ok(app, "请产出 [[empty_below:2000|重试成功的正文]]", &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    let calls = calls_of(app, &id).await;

    assert_eq!(st, "done", "重试成功应让任务 done: {detail}");
    assert!(
        result_of(&detail).contains("重试成功的正文"),
        "成果应来自第二次尝试: {detail}"
    );
    assert_eq!(
        node_call_rows(&calls).len(),
        3,
        "总尝试 = 1 + max_retries(2) = 3,每次尝试各记一行: {calls}"
    );
    assert_usage_invariant(&detail, &calls);
}

/// A2 用尽路径:产出恒空 → 重试到上限后照常走既有「无成果」终态(error),
/// 且**每一次尝试都留痕**(不能因为重试把中间尝试吞掉)。
#[tokio::test]
async fn node_retry_exhausted_keeps_every_attempt_on_record() {
    let _guard = test_lock().await;
    let app = test_app();

    let flow = save_flow(
        app,
        "空产出重试用尽流程",
        single_step(json!({ "max_tokens": 500, "max_retries": 2 })),
    )
    .await;

    let id = create_ok(app, "恒空产出 [[empty]]", &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    let calls = calls_of(app, &id).await;

    assert_eq!(st, "error", "无任何成果时终态仍按既有口径 error: {detail}");
    assert!(
        detail["task"]["error"]
            .as_str()
            .unwrap_or("")
            .contains("未产出任何成果"),
        "错误文案应点名「未产出任何成果」: {detail}"
    );
    assert_eq!(
        node_call_rows(&calls).len(),
        3,
        "重试用尽也应留下 3 行(1 + 2): {calls}"
    );
    assert_usage_invariant(&detail, &calls);
}

/// 零行为变化护栏:未配 `max_retries` 的节点**不重试**——同一份恒空输入只产生一行。
/// 这条与上一条同输入、只差一个字段,故是 A2 的判别性对照(字段接线失效时两条会相等)。
#[tokio::test]
async fn node_without_retry_config_runs_once() {
    let _guard = test_lock().await;
    let app = test_app();

    let flow = save_flow(
        app,
        "未配重试流程",
        single_step(json!({ "max_tokens": 500 })),
    )
    .await;

    let id = create_ok(app, "恒空产出 [[empty]] 且未配重试", &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    let calls = calls_of(app, &id).await;

    assert_eq!(st, "error", "未配重试时与批之前逐字节一致: {detail}");
    assert_eq!(
        node_call_rows(&calls).len(),
        1,
        "未配 max_retries 不得重试(零行为变化): {calls}"
    );
    assert_usage_invariant(&detail, &calls);
}

/// A2 工具路径:宽松档 + 工具步骤走 `run_step_with_tools`(另一条消费点),
/// 预算覆盖必须同样生效——否则「重试只在纯生成路径有用」而用户看不出来。
#[tokio::test]
async fn tool_step_retry_recovers_with_budget_override() {
    let _guard = test_lock().await;
    let app = test_app();

    let flow = save_flow(
        app,
        "工具步骤重试流程",
        single_step(json!({
            "kind": "loose",
            "tools": ["read"],
            "max_tokens": 500,
            "max_retries": 1
        })),
    )
    .await;

    let id = create_ok(app, "请产出 [[empty_below:1000|工具步重试正文]]", &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    let calls = calls_of(app, &id).await;

    assert_eq!(st, "done", "工具步骤重试成功应 done: {detail}");
    assert!(
        result_of(&detail).contains("工具步重试正文"),
        "成果应来自第二次尝试: {detail}"
    );
    assert_eq!(
        node_call_rows(&calls).len(),
        2,
        "总尝试 = 1 + max_retries(1) = 2: {calls}"
    );
    assert_usage_invariant(&detail, &calls);
}

/// A1 接线:配了 `call_timeout_secs` 的节点照常跑通(取值语义由 `resolve_call_watchdog`
/// 单测锁定,此处只证「该字段被接受且不改变正常路径」)。
#[tokio::test]
async fn node_with_call_timeout_runs_normally() {
    let _guard = test_lock().await;
    let app = test_app();

    let flow = save_flow(
        app,
        "节点超时流程",
        single_step(json!({ "call_timeout_secs": 60 })),
    )
    .await;

    let id = create_ok(app, "请产出 [[reply:超时配置下的正文]]", &flow).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "done", "配了超时不该改变正常路径: {detail}");
    assert!(result_of(&detail).contains("超时配置下的正文"), "{detail}");
}

/// A1/A2 保存期校验:区间外的取值一律 400 且点名步骤与区间,不落库。
#[tokio::test]
async fn node_timeout_and_retry_ranges_are_validated() {
    let _guard = test_lock().await;
    let app = test_app();

    let (status, json) = try_save_flow(
        app,
        "超时越界流程",
        single_step(json!({ "call_timeout_secs": 5 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "5s 低于下限应 400: {json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("单次调用超时"),
        "错误文案应点名字段: {json}"
    );

    let (status, json) = try_save_flow(
        app,
        "超时越界流程2",
        single_step(json!({ "call_timeout_secs": 3601 })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "3601s 超上限应 400: {json}"
    );

    let (status, json) = try_save_flow(
        app,
        "重试越界流程",
        single_step(json!({ "max_retries": 0 })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "0 次重试应由「清空字段」表达,显式 0 应 400: {json}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("空产出重试次数"),
        "错误文案应点名字段: {json}"
    );

    let (status, json) = try_save_flow(
        app,
        "重试越界流程2",
        single_step(json!({ "max_retries": 6 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "6 次超上限应 400: {json}");
}
