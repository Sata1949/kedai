// 二维批次 5a:任务绑定流程 + 流程快照(计划.md §四 批次 5 改动点 5/6/7)。
//
// 独立成文件的原因与其它 custom 用例一致:流程库是**全局单例**
// (data/agent_flows.json + current_flow_id),而同一测试文件内的用例共享一个 app 实例,
// 故用文件级 test_lock 串行化(仿 agent_flows.rs),单开文件即单开数据目录。
//
// mock 钩子 [[reply_if:needle|内容]](语法见 connectors/mock.rs::extract_reply_if_marker):
// needle 命中任一 **system** 消息时返回指定内容。把钩子写进任务目标、让各流程节点的
// system 提示词含各自 needle,就能为「每个流程/每个节点」指定确定性产出——
// 本批正是靠它区分「到底跑了哪一份编排」。
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

/// 保存流程(PUT 同时把它设为当前选中),返回其 id。
/// 传入 `"id": ""` 为新建;传既有 id 为**覆盖更新**(本批用它验证「改了流程不影响已绑定任务」)。
async fn save_flow(app: &axum::Router, config: Value) -> String {
    let (status, json) =
        send_json(app, "PUT", "/api/agent-flows", json!({ "config": config })).await;
    assert_eq!(status, StatusCode::OK, "保存流程应 200: {json}");
    json["config"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("保存响应应带流程 id: {json}"))
        .to_string()
}

/// 单步流程:`step_id` 节点的 system 提示词带判别 needle(产出内容由任务目标里的钩子给出)。
fn single_step_flow(name: &str, step_id: &str, needle: &str) -> Value {
    json!({
        "id": "",
        "name": name,
        "enabled": true,
        "steps": [{
            "id": step_id, "name": format!("{name}节点"), "enabled": true,
            "goal": "按本步指令产出", "action": "direct", "generates": true,
            "is_output": true,
            "system_prompt": format!("【本步指令·{needle}】只输出钩子指定内容")
        }]
    })
}

fn hook(needle: &str, content: &str) -> String {
    format!("[[reply_if:{needle}|{content}]]")
}

async fn create_task(
    app: &axum::Router,
    title: &str,
    mode: &str,
    flow_id: Option<&str>,
) -> (StatusCode, Value) {
    let body = match flow_id {
        Some(id) => json!({ "title": title, "task_mode": mode, "flow_id": id }),
        None => json!({ "title": title, "task_mode": mode }),
    };
    send_json(app, "POST", "/api/tasks", body).await
}

async fn create_task_ok(app: &axum::Router, title: &str, flow_id: Option<&str>) -> String {
    let (status, json) = create_task(app, title, "custom", flow_id).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "创建 custom 任务应 201: {json}"
    );
    json["task"]["id"].as_str().unwrap().to_string()
}

async fn run_task(app: &axum::Router, id: &str) {
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
}

async fn get_detail(app: &axum::Router, id: &str) -> Value {
    let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "详情应 200: {json}");
    json
}

async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    for _ in 0..50 {
        let json = get_detail(app, id).await;
        let st = json["task"]["status"].as_str().unwrap_or("").to_string();
        if matches!(st.as_str(), "done" | "partial" | "error" | "ended") {
            return (st, json);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("任务 {id} 未在超时内到达终态");
}

/// 跑一次任务并断言终态与成果(返回详情)
async fn run_and_expect(
    app: &axum::Router,
    id: &str,
    want_status: &str,
    want_result: &str,
) -> Value {
    run_task(app, id).await;
    let (st, detail) = wait_terminal(app, id).await;
    assert_eq!(st, want_status, "任务终态应为 {want_status}: {detail}");
    assert_eq!(
        detail["task"]["result"].as_str().unwrap_or(""),
        want_result,
        "成果应为 {want_result}(判别「到底跑了哪一份编排」): {detail}"
    );
    detail
}

/// 快照里的流程 id 集合(排序后比较,顺序无关)
fn snapshot_flow_ids(detail: &Value) -> Vec<String> {
    let mut ids: Vec<String> = detail["flow_snapshot"]["flows"]
        .as_array()
        .unwrap_or_else(|| panic!("详情应带 flow_snapshot.flows: {detail}"))
        .iter()
        .map(|f| f["id"].as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// 二维批次 5a:任务**绑定**流程后,当前流程换成另一份也不影响它——跑的是绑定的那一份。
///
/// 判别性:甲流程(当前选中)与乙流程(任务绑定)各带自己的节点 needle/产出;
/// 若执行期误读「当前流程」,成果会是甲产出而不是乙产出。
#[tokio::test]
async fn custom_task_binding_wins_over_later_current_flow_change() {
    let _guard = test_lock().await;
    let app = test_app();

    let id_a = save_flow(app, single_step_flow("甲流程", "a-step", "甲流程本步")).await;
    let id_b = save_flow(app, single_step_flow("乙流程", "b-step", "乙流程本步")).await;
    // 保存乙流程后它是当前流程;此处显式切回甲流程,制造「当前流程 ≠ 任务绑定」
    let (status, _) = send_json(
        app,
        "POST",
        "/api/agent-flows/select",
        json!({ "id": id_a }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "切换当前流程应 200");

    let title = format!(
        "{} {} 绑定乙流程的任务",
        hook("甲流程本步", "甲流程产出"),
        hook("乙流程本步", "乙流程产出")
    );
    let (status, created) = create_task(app, &title, "custom", Some(&id_b)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "创建绑定流程的任务应 201: {created}"
    );
    let id = created["task"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        created["task"]["flow_id"],
        id_b.as_str(),
        "创建响应应回显绑定: {created}"
    );

    let detail = run_and_expect(app, &id, "done", "乙流程产出").await;
    assert_eq!(
        detail["task"]["flow_id"],
        id_b.as_str(),
        "详情应回显绑定: {detail}"
    );
    assert_eq!(
        detail["flow_snapshot"]["root_id"],
        id_b.as_str(),
        "快照根应为绑定的乙流程: {detail}"
    );
    assert_eq!(
        snapshot_flow_ids(&detail),
        {
            let mut v = vec![id_b.clone()];
            v.sort();
            v
        },
        "绑定流程的快照应只含乙流程: {detail}"
    );
}

/// 二维批次 5a:绑定任务在**创建时**冻结快照,之后编辑那份流程本身也不影响它;
/// 而编辑后**新建**的任务才会用到新版本(证明编辑真的生效,断言不是空跑)。
#[tokio::test]
async fn custom_task_binding_survives_editing_the_bound_flow() {
    let _guard = test_lock().await;
    let app = test_app();

    let fid = save_flow(app, single_step_flow("可变流程", "v-step", "流程V1本步")).await;
    let id = create_task_ok(
        app,
        &format!("{} 绑定后改流程", hook("流程V1本步", "V1产出")),
        Some(&fid),
    )
    .await;

    // 同 id 覆盖更新:节点 needle 换成 V2(库内该流程已变)
    let mut v2 = single_step_flow("可变流程", "v-step", "流程V2本步");
    v2["id"] = json!(fid);
    let saved = save_flow(app, v2).await;
    assert_eq!(saved, fid, "同 id 保存应是覆盖更新");

    run_and_expect(app, &id, "done", "V1产出").await;

    // 编辑后新建的任务(仍绑定同一流程 id)拿到的是 V2 —— 判别性对照
    let id2 = create_task_ok(
        app,
        &format!("{} 编辑后新建", hook("流程V2本步", "V2产出")),
        Some(&fid),
    )
    .await;
    run_and_expect(app, &id2, "done", "V2产出").await;
}

/// 二维批次 5a:子流程闭包也被冻结——父流程挂载的子流程后来被改动,
/// 已绑定任务仍按冻结时的子流程执行。
#[tokio::test]
async fn custom_task_sub_flow_closure_is_frozen() {
    let _guard = test_lock().await;
    let app = test_app();

    // 子流程:单步(子流程自身有生成步才合法)
    let sub_id = save_flow(
        app,
        json!({
            "id": "", "name": "子流程", "enabled": true,
            "steps": [{
                "id": "s-step", "name": "子步骤", "enabled": true,
                "goal": "按本步指令产出", "action": "direct", "generates": true,
                "is_output": true,
                "system_prompt": "【本步指令·子流程V1本步】只输出钩子指定内容"
            }]
        }),
    )
    .await;
    // 父流程:单节点挂载子流程(挂载节点自己不发起调用)
    let parent_id = save_flow(
        app,
        json!({
            "id": "", "name": "父流程", "enabled": true,
            "steps": [{
                "id": "p-mount", "name": "挂载", "enabled": true,
                "goal": "挂载子流程", "action": "direct", "generates": true,
                "is_output": true, "sub_flow_id": sub_id
            }]
        }),
    )
    .await;

    let id = create_task_ok(
        app,
        &format!("{} 子流程闭包任务", hook("子流程V1本步", "子流程V1产出")),
        Some(&parent_id),
    )
    .await;

    // 改写子流程(同 id 覆盖)
    let sub_v2 = json!({
        "id": sub_id,
        "name": "子流程", "enabled": true,
        "steps": [{
            "id": "s-step", "name": "子步骤", "enabled": true,
            "goal": "按本步指令产出", "action": "direct", "generates": true,
            "is_output": true,
            "system_prompt": "【本步指令·子流程V2本步】只输出钩子指定内容"
        }]
    });
    assert_eq!(save_flow(app, sub_v2).await, sub_id);

    let detail = run_and_expect(app, &id, "done", "子流程V1产出").await;
    let mut want = vec![parent_id.clone(), sub_id.clone()];
    want.sort();
    assert_eq!(
        snapshot_flow_ids(&detail),
        want,
        "快照应含「父流程 + 可达子流程」的闭包: {detail}"
    );

    // 编辑后新建的任务拿到 V2 子流程 —— 证明子流程改动确实生效
    let id2 = create_task_ok(
        app,
        &format!("{} 子流程改后新建", hook("子流程V2本步", "子流程V2产出")),
        Some(&parent_id),
    )
    .await;
    run_and_expect(app, &id2, "done", "子流程V2产出").await;
}

/// 二维批次 5a:未绑定流程的任务**每次运行**按当时的当前流程捕获快照
/// (创建时不落快照;重跑跟随新的当前流程——旧客户端语义不变)。
#[tokio::test]
async fn custom_task_without_binding_captures_snapshot_each_run() {
    let _guard = test_lock().await;
    let app = test_app();

    let id_a = save_flow(app, single_step_flow("跟随甲", "fa-step", "跟随甲本步")).await;
    let title = format!(
        "{} {} 未绑定的任务",
        hook("跟随甲本步", "甲产出"),
        hook("跟随乙本步", "乙产出")
    );
    let id = create_task_ok(app, &title, None).await;

    // 创建期只冻结**显式绑定**;未绑定任务此时没有快照
    let before = get_detail(app, &id).await;
    assert!(
        before["flow_snapshot"].is_null(),
        "未绑定任务创建时不应有快照: {before}"
    );
    assert!(
        before["task"]["flow_id"].is_null(),
        "未绑定任务不应带 flow_id: {before}"
    );

    let detail = run_and_expect(app, &id, "done", "甲产出").await;
    assert_eq!(
        detail["flow_snapshot"]["root_id"],
        id_a.as_str(),
        "运行开始应捕获「当时当前流程」的快照: {detail}"
    );

    // 换当前流程(保存乙即成为当前)后**重跑同一个任务** → 跟随新的当前流程,快照随之更新
    let id_b = save_flow(app, single_step_flow("跟随乙", "fb-step", "跟随乙本步")).await;
    let detail2 = run_and_expect(app, &id, "done", "乙产出").await;
    assert_eq!(
        detail2["flow_snapshot"]["root_id"],
        id_b.as_str(),
        "未绑定任务重跑应跟随新的当前流程并覆写快照: {detail2}"
    );
}

/// 二维批次 5a:绑定不存在的流程 → 400 且**不建行**(创建期校验,不留半成品任务)。
#[tokio::test]
async fn custom_task_unknown_flow_id_is_rejected_without_creating_row() {
    let _guard = test_lock().await;
    let app = test_app();

    let (_, before) = send_json(app, "GET", "/api/tasks", json!({})).await;
    let count_before = before["tasks"].as_array().map(|a| a.len()).unwrap_or(0);

    let (status, json) = create_task(app, "绑定不存在的流程", "custom", Some("no-such-flow")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未知流程应 400: {json}");
    let msg = json["error"].as_str().unwrap_or_default();
    assert!(msg.contains("流程"), "错误文案应点名流程: {json}");

    let (_, after) = send_json(app, "GET", "/api/tasks", json!({})).await;
    assert_eq!(
        after["tasks"].as_array().map(|a| a.len()).unwrap_or(0),
        count_before,
        "校验失败不应建行: {after}"
    );
}

/// 二维批次 5a:绑定**未启用**流程 → 400(与执行期 `current_flow` 的 enabled 口径一致,
/// 提前拒绝,避免任务建好后一跑就 error)。
#[tokio::test]
async fn custom_task_disabled_flow_id_is_rejected() {
    let _guard = test_lock().await;
    let app = test_app();

    let mut cfg = single_step_flow("停用流程", "d-step", "停用本步");
    cfg["enabled"] = json!(false);
    let fid = save_flow(app, cfg).await;

    let (status, json) = create_task(app, "绑定停用流程", "custom", Some(&fid)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未启用流程应 400: {json}");
    let msg = json["error"].as_str().unwrap_or_default();
    assert!(msg.contains("未启用"), "错误文案应说明未启用: {json}");
}

/// 二维批次 5a:详情只在**绑定或跑过的 custom 任务**上下发快照;
/// 其余模式与未跑过的未绑定任务一律 null(前端按 optional 容错,零噪音)。
#[tokio::test]
async fn task_detail_flow_snapshot_only_for_custom_tasks() {
    let _guard = test_lock().await;
    let app = test_app();

    // legacy 任务:无 flow_id、无快照
    let (status, legacy) = create_task(app, "legacy 任务的详情", "legacy", None).await;
    assert_eq!(status, StatusCode::CREATED, "legacy 任务应 201: {legacy}");
    let legacy_id = legacy["task"]["id"].as_str().unwrap().to_string();
    let legacy_detail = get_detail(app, &legacy_id).await;
    assert!(
        legacy_detail["task"]["flow_id"].is_null() && legacy_detail["flow_snapshot"].is_null(),
        "非 custom 任务不应带绑定与快照: {legacy_detail}"
    );

    // 绑定 custom 任务(尚未运行):创建即冻结 → 快照在场
    let fid = save_flow(app, single_step_flow("创建即冻结", "f-step", "冻结本步")).await;
    let bound = create_task_ok(app, "绑定任务的详情", Some(&fid)).await;
    let bound_detail = get_detail(app, &bound).await;
    assert_eq!(
        bound_detail["flow_snapshot"]["root_id"],
        fid.as_str(),
        "绑定任务应在创建时即冻结快照: {bound_detail}"
    );
    assert_eq!(
        bound_detail["task"]["status"], "pending",
        "此时任务还没跑: {bound_detail}"
    );

    // 未绑定 custom 任务(尚未运行):无快照
    let unbound = create_task_ok(app, "未绑定任务的详情", None).await;
    let unbound_detail = get_detail(app, &unbound).await;
    assert!(
        unbound_detail["flow_snapshot"].is_null(),
        "未跑过的未绑定任务不应有快照: {unbound_detail}"
    );
}
