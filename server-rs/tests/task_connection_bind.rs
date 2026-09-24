// 任务级连接(A 批 B1)与改绑编排(A 批 B3)的集成回归。
//
// 口径(见 `交接稿-自定义流程AB批.md` R6/R7):
//   - B1:创建任务可带 `connection_id`(六模式通用,绑的是 provider 而非编排);
//     创建期**校验引用存在且启用**(400 点名连接);运行期引用失效即**明确报错**,
//     不静默回退默认连接;缺省 = 跟随设置的默认连接(零行为变化)。
//   - B3:`POST /api/tasks/{id}/bind` 全量替换 `flow_id` / `flow_ids` 并**重新冻结**快照;
//     仅 custom 模式、且 planning/running/planned 态拒绝;不存在 → 404。
//
// 独立成文件的原因:本文件会改**进程级设置**(连接列表),共享进程会让其它文件的
// 缺省假设失效(同 settings_connector 的 TEST-ISO-1 教训)。
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

/// 写入连接列表并返回第一个连接的 id(路径不带 mode:connections 是扁平真源)
async fn put_connection(app: &axum::Router, name: &str, enabled: bool) -> String {
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "name": name,
            "connector_type": "mock",
            "base_url": "",
            "model": "",
            "enabled": enabled
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写入连接应 200: {body}");
    body["settings"]["connections"][0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("连接应有 id: {body}"))
        .to_string()
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
    json["config"]["id"].as_str().unwrap().to_string()
}

fn plain_step(id: &str, name: &str) -> Value {
    json!({
        "id": id, "name": name, "enabled": true,
        "goal": "产出正文", "action": "direct", "generates": true, "is_output": true
    })
}

async fn create_task(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    send_json(app, "POST", "/api/tasks", body).await
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

/// B1:创建期校验——未知连接与停用连接都 400,且**点名**原因;缺省不带键照常创建。
#[tokio::test]
async fn create_validates_task_connection_reference() {
    let _guard = test_lock().await;
    let app = test_app();

    let (status, body) = create_task(
        app,
        json!({ "title": "未知连接", "task_mode": "legacy", "connection_id": "no-such-id" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未知连接应 400: {body}");
    assert!(
        body["error"].as_str().unwrap_or("").contains("不存在"),
        "文案应点名「不存在」: {body}"
    );

    let disabled = put_connection(app, "停用连接", false).await;
    let (status, body) = create_task(
        app,
        json!({ "title": "停用连接", "task_mode": "legacy", "connection_id": disabled }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "停用连接应 400: {body}");
    assert!(
        body["error"].as_str().unwrap_or("").contains("已停用"),
        "文案应点名「已停用」: {body}"
    );

    // 不带 connection_id:照常创建,且响应**不含**该键(存量线格式逐字节不变)
    let (status, body) =
        create_task(app, json!({ "title": "默认连接", "task_mode": "legacy" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(
        body["task"].get("connection_id").is_none(),
        "缺省时不应落该键(加性字段口径): {body}"
    );
}

/// B1:带连接创建 → 响应与详情都带该 id;库中的行也保留(重启/列表一致)。
#[tokio::test]
async fn create_persists_task_connection() {
    let _guard = test_lock().await;
    let app = test_app();
    let conn = put_connection(app, "任务连接", true).await;

    let (status, body) = create_task(
        app,
        json!({ "title": "带连接的任务", "task_mode": "legacy", "connection_id": conn }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["task"]["connection_id"], json!(conn), "{body}");
    let id = body["task"]["id"].as_str().unwrap().to_string();

    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(detail["task"]["connection_id"], json!(conn), "{detail}");

    let (_, list) = send_json(app, "GET", "/api/tasks", json!({})).await;
    let row = list["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["id"] == json!(id))
        .unwrap_or_else(|| panic!("列表应含该任务: {list}"));
    assert_eq!(row["connection_id"], json!(conn), "{row}");
}

/// B1:任务级连接**真的被运行期采用**,且引用失效时**明确报错**而不静默回退。
///
/// 判别性:先证明「带任务级连接的任务跑得通」(连接可用时的正常路径),再**删掉**该连接
/// 后重跑 —— 必须失败并点名该连接 id。若运行期忽略任务级连接(回退默认 mock),
/// 第二次照样 done,测试即失败。
#[tokio::test]
async fn task_connection_is_consulted_and_fails_loudly_when_stale() {
    let _guard = test_lock().await;
    let app = test_app();
    let conn = put_connection(app, "任务级连接", true).await;
    // 用 custom 模式(不跑规划器,mock 即可跑通),节点**不写** connection_id ——
    // 于是它唯一能拿到连接的来源就是任务级那一份
    let flow = save_flow(app, "任务级连接流程", json!([plain_step("n1", "唯一节点")])).await;

    let (status, body) = create_task(
        app,
        json!({
            "title": "连接失效用例", "task_mode": "custom",
            "flow_id": flow, "connection_id": conn
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["task"]["id"].as_str().unwrap().to_string();

    // ① 连接可用:该任务照常跑通
    let (status, body) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应受理: {body}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "连接可用时任务应跑通: {detail}");

    // ② 删掉该连接(连接可整体清空,见 IFW-9 的后果性事实)后重跑 → 明确报错
    let (status, body) = send_json(app, "PUT", "/api/settings", json!({ "connections": [] })).await;
    assert_eq!(status, StatusCode::OK, "清空连接应 200: {body}");
    let (status, body) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 请求本身应受理: {body}");
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(
        st, "error",
        "引用失效应让任务失败(而不是静默回退默认连接): {detail}"
    );
    let blob = detail.to_string();
    assert!(
        blob.contains(&conn) && blob.contains("连接"),
        "错误文案应点名失效的连接: {detail}"
    );
}

/// B3:改绑 = 全量替换 + 重新冻结快照;`flow_id: null` 回落到「跟随当前流程」并清空快照。
#[tokio::test]
async fn bind_replaces_binding_and_refreezes_snapshot() {
    let _guard = test_lock().await;
    let app = test_app();

    let flow_a = save_flow(app, "改绑流程A", json!([plain_step("a1", "A 节点")])).await;
    let flow_b = save_flow(app, "改绑流程B", json!([plain_step("b1", "B 节点")])).await;

    let (status, body) = create_task(
        app,
        json!({
            "title": "改绑用例", "task_mode": "custom",
            "flow_id": flow_a, "flow_ids": [flow_b]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["task"]["id"].as_str().unwrap().to_string();

    // 初次绑定:快照根 = A,名单含 B
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(
        detail["flow_snapshot"]["root_id"],
        json!(flow_a),
        "{detail}"
    );

    // 改绑到 B 且清空名单(全量替换语义:flow_ids: [] = 强制模式)
    let (status, body) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/bind"),
        json!({ "flow_id": flow_b, "flow_ids": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "改绑应 200: {body}");
    assert_eq!(body["task"]["flow_id"], json!(flow_b), "{body}");
    assert!(
        body["task"].get("flow_ids").is_none(),
        "清空名单后不应落 flow_ids 键: {body}"
    );

    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(
        detail["flow_snapshot"]["root_id"],
        json!(flow_b),
        "改绑必须重新冻结快照(旧快照是 A): {detail}"
    );

    // 解绑:flow_id 为 null = 跟随当前流程 → 快照清空(下次执行重新捕获)
    let (status, body) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/bind"),
        json!({ "flow_id": null, "flow_ids": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["task"].get("flow_id").is_none(),
        "解绑后不应落 flow_id 键: {body}"
    );
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert!(
        detail["flow_snapshot"].is_null(),
        "解绑即清空冻结快照(改为执行时捕获): {detail}"
    );

    // 改绑后照常可执行(快照解析路径未被破坏)
    let (status, body) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "解绑后按当前流程执行应成功: {detail}");
}

/// B3:非 custom 模式拒绝改绑;不存在的任务 → 404;越权的名单成员 → 400 且库不变。
#[tokio::test]
async fn bind_rejects_wrong_mode_missing_task_and_bad_members() {
    let _guard = test_lock().await;
    let app = test_app();

    let flow = save_flow(app, "改绑校验流程", json!([plain_step("n1", "节点")])).await;

    // 非 custom 模式:拒绝
    let (status, body) = create_task(
        app,
        json!({ "title": "legacy 任务", "task_mode": "legacy" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let legacy_id = body["task"]["id"].as_str().unwrap().to_string();
    let (status, body) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{legacy_id}/bind"),
        json!({ "flow_id": flow, "flow_ids": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非 custom 应 400: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("自定义流程模式"),
        "{body}"
    );

    // 不存在的任务:404
    let (status, _) = send_json(
        app,
        "POST",
        "/api/tasks/no-such-task/bind",
        json!({ "flow_id": flow, "flow_ids": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "不存在的任务应 404");

    // 名单成员不存在:400 且不写库
    let (status, body) = create_task(
        app,
        json!({ "title": "名单校验", "task_mode": "custom", "flow_id": flow }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["task"]["id"].as_str().unwrap().to_string();
    let (status, body) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/bind"),
        json!({ "flow_id": flow, "flow_ids": ["ghost-flow"] }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "幽灵名单成员应 400: {body}"
    );
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(
        detail["task"]["flow_id"],
        json!(flow),
        "校验失败不得改动绑定: {detail}"
    );
    assert!(
        detail["task"].get("flow_ids").is_none(),
        "校验失败不得落名单: {detail}"
    );
}
