// 二维批次 5b(节点级连接)集成测试:同一任务内不同节点走不同连接。
//
// 为什么要独立文件:流程库(data/agent_flows.json + current_flow_id)与设置(data/settings.json)
// 都是**进程级全局单例**,同文件内多用例共享一个 app 会互相覆盖;单开文件即单开进程
// → 单开数据目录,可与其它 custom 用例并行运行(与 task_custom_kind/graph/subflow 同因)。
//
// 三个验收点(对应计划批次 5b 的验收断言):
//  ① 同一任务内两个节点走不同 provider,且调用追踪 `/calls` 的 model 列是**各自真实值**;
//  ② 引用的连接被删除 / 停用 → 该节点明确报错(文案点名),**不静默回退默认连接**;
//  ③ 未配连接的节点行为与 5b 之前一致(默认连接,model = mock-demo)。
// 另加:节点级 `max_tool_rounds` 生效(用 mock 的 `[[tool_loop_text:]]` 钩子读轮次)。
//
// provider 可区分性怎么造:mock 连接的模型名被连接器固定为 `mock-demo`,两条 mock 连接
// 区分不开。故用 **openai-compatible + 本机不可达端口** 造第二条连接——连接器构造不联网,
// 调用必然失败(连接拒绝,两次退避重试约 1.5-2s),但失败行**照样落 model 列**,
// 「哪个节点走哪条连接」因此完全可断言,且不依赖任何外部网络。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

/// 串行化:设置(data/settings.json)与流程库(data/agent_flows.json)都是**进程级共享**的,
/// 同文件内的用例并发做「读-改-写」式的 PUT 会互相覆盖(基线连接的 id 会在两次 PUT 之间
/// 被别的用例换掉)。故本文件的用例一律先取这把锁 —— 与 `settings_connections.rs` 同款纪律。
async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

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

/// PUT 设置的响应是 `{ok, settings}`,GET 直接返回设置体
fn settings_body(body: &Value) -> &Value {
    body.get("settings").unwrap_or(body)
}

/// 保存流程并设为当前(第二个参数是 steps 数组)
async fn put_flow(app: &axum::Router, name: &str, steps: Value) {
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({"config": {"id": "", "name": name, "enabled": true, "steps": steps}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "保存流程应 200: {json}");
}

/// 写一份「mock 默认连接 + N 条 openai-compatible 连接」的设置:
/// 默认连接**显式钉在 mock 上**(active_connection_id),否则 openai 连接会被选成默认,
/// 整进程的默认 provider 都会跟着变(那是另一个语义,不是本文件要测的)。
/// 返回 (mock 连接 id, [openai 连接 id...])。
async fn put_connections(app: &axum::Router, openai_models: &[&str]) -> (String, Vec<String>) {
    // 第一步:只放一条 mock 连接 → 它必然是默认连接,拿到它的 id
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "name": "默认", "connector_type": "mock",
            "base_url": "", "model": "", "enabled": true
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "基线连接应保存成功: {body}");
    let default_id = settings_body(&body)["connections"][0]["id"]
        .as_str()
        .expect("基线连接应有 id")
        .to_string();

    // 第二步:带上 openai 连接,并把默认连接钉回 mock
    let mut list = vec![json!({
        "id": default_id, "name": "默认", "connector_type": "mock",
        "base_url": "", "model": "", "enabled": true
    })];
    for (i, model) in openai_models.iter().enumerate() {
        list.push(json!({
            "name": format!("外部连接{i}"),
            // 本机保留端口 + 必然拒绝:不联网就能得到「连接失败」这一确定形态
            "connector_type": "openai-compatible",
            "base_url": "http://127.0.0.1:1/v1",
            "api_key": "sk-test-conn",
            "model": model,
            "enabled": true
        }));
    }
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": list, "active_connection_id": default_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "连接数组应保存成功: {body}");
    let conns = settings_body(&body)["connections"]
        .as_array()
        .expect("响应应有 connections")
        .clone();
    assert_eq!(conns.len(), openai_models.len() + 1);
    let openai_ids = conns
        .iter()
        .filter(|c| c["connector_type"] == "openai-compatible")
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        settings_body(&body)["active_connection_id"],
        json!(default_id),
        "默认连接应被钉在 mock 上"
    );
    (default_id, openai_ids)
}

async fn create_custom_task(app: &axum::Router, goal: &str) -> String {
    // 任务目标即流程源节点的输入(故 mock 钩子写在 title 里)
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": goal, "task_mode": "custom" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 轮询到终态(含失败节点的用例要等两次退避重试,故上限放到 20s;既有用例的 5s 不够宽)
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    for _ in 0..200 {
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

/// 取某步骤(step_index)的调用追踪行
async fn step_call(app: &axum::Router, id: &str, step_index: i64) -> Value {
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["phase"] == "step" && c["step_index"] == step_index)
        .cloned()
        .unwrap_or(Value::Null)
}

/// 用 title(任务目标)创建任务并跑到终态,返回 (任务 id, 状态, 详情)
async fn run_custom_task(app: &axum::Router, goal: &str) -> (String, String, Value) {
    let id = create_custom_task(app, goal).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    (id, st, detail)
}

/// 记账不变量:phase=step 各行 token 求和 == 详情 usage_total(5b 失败调用不计 token,
/// 若把「未发起的调用」也算进去,这条就会破)
async fn assert_usage_invariant(app: &axum::Router, id: &str, detail: &Value) {
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let rows: Vec<&Value> = calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["phase"] == "step")
        .collect();
    let prompt: i64 = rows
        .iter()
        .map(|c| c["prompt_tokens"].as_i64().unwrap_or(0))
        .sum();
    let completion: i64 = rows
        .iter()
        .map(|c| c["completion_tokens"].as_i64().unwrap_or(0))
        .sum();
    assert_eq!(
        prompt,
        detail["usage_total"]["prompt_tokens"]
            .as_i64()
            .unwrap_or(-1),
        "各行 prompt token 求和应等于 usage_total: {detail}"
    );
    assert_eq!(
        completion,
        detail["usage_total"]["completion_tokens"]
            .as_i64()
            .unwrap_or(-1),
        "各行 completion token 求和应等于 usage_total: {detail}"
    );
}

/// 二维批次 5b 的核心断言:两个节点分别绑两个不同的 provider,
/// 调用追踪里各自记**自己连接的真实模型**,而没配连接的节点仍走默认(mock)。
#[tokio::test]
async fn custom_node_connection_routes_each_node_and_traces_real_model() {
    let _guard = test_lock().await;
    let app = test_app();
    let (_default_id, conns) = put_connections(app, &["model-b", "model-c"]).await;
    assert_eq!(conns.len(), 2);

    put_flow(
        app,
        "连接路由流程图",
        json!([
            // 0:无连接 → 默认连接(mock),且标为成果节点(成果不依赖后面失败的节点)
            {"id":"n-default","name":"默认连接步","enabled":true,"goal":"产出默认成果",
             "action":"direct","generates":true,"is_output":true},
            // 1:绑第一条外部连接(不可达 → 该节点失败,但调用行照样携带真实模型)
            {"id":"n-b","name":"外部连接B","enabled":true,"goal":"走外部连接B",
             "action":"direct","generates":true,"inputs":["n-default"],
             "connection_id": conns[0]},
            // 2:绑第二条外部连接(与 B 同为「默认连接步」的下游 → 两路并行,
            //    恰好也覆盖「并行节点各自路由到自己的连接」)
            {"id":"n-c","name":"外部连接C","enabled":true,"goal":"走外部连接C",
             "action":"direct","generates":true,"inputs":["n-default"],
             "connection_id": conns[1]}
        ]),
    )
    .await;

    let (id, st, detail) = run_custom_task(app, r#"[[reply:默认连接产出]] 节点级连接用例"#).await;
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 3);

    // 默认连接的节点:mock 正常完成
    assert_eq!(plan[0]["status"], "done", "默认连接节点应完成: {detail}");
    assert_eq!(plan[0]["result"], "默认连接产出");

    // 两个外部连接节点:必然失败(端口 1 拒绝),但**失败形态可读**——文案来自连接器
    for (i, name) in [(1usize, "B"), (2usize, "C")] {
        assert_eq!(
            plan[i]["status"], "error",
            "外部连接{name}节点应失败(地址不可达): {detail}"
        );
    }
    // 成果取 is_output 节点 → 任务 partial(有 error 节点但成果已产出)
    assert_eq!(st, "partial", "有失败节点但成果已产出 → partial: {detail}");
    assert_eq!(detail["task"]["result"], "默认连接产出");

    // **关键断言:调用追踪的 model 列 = 各节点连接的真实模型**
    let row0 = step_call(app, &id, 0).await;
    assert_eq!(
        row0["model"], "mock-demo",
        "默认连接节点的 model 应是 mock 连接器的真实模型: {row0:?}"
    );
    assert_eq!(row0["status"], "ok");
    let row1 = step_call(app, &id, 1).await;
    let row2 = step_call(app, &id, 2).await;
    assert_eq!(
        row1["model"], "model-b",
        "节点 1 应记它自己连接的模型(而不是全局默认): {row1:?}"
    );
    assert_eq!(
        row2["model"], "model-c",
        "节点 2 应记它自己连接的模型: {row2:?}"
    );
    assert_eq!(row1["status"], "error");
    assert_eq!(row2["status"], "error");

    // 记账不变量:两个失败行 prompt_tokens = 0(连接都没建起来,token 无从产生),
    // 故 usage_total 恰等于默认连接节点那一行 —— 未发起的调用绝不计入成本。
    assert_usage_invariant(app, &id, &detail).await;
    let ok_prompt = row0["prompt_tokens"].as_i64().unwrap_or(0);
    assert!(ok_prompt > 0, "成功行应有 token: {row0:?}");
    assert_eq!(
        row1["prompt_tokens"].as_i64(),
        Some(0),
        "连接失败不该产生 prompt token: {row1:?}"
    );
    assert_eq!(
        detail["usage_total"]["prompt_tokens"].as_i64(),
        Some(ok_prompt),
        "合计应恰等于成功那一行(失败调用不计成本): {detail}"
    );
}

/// 二维批次 5b:引用的连接**不在设置里**(被删除 / 跨机导入的流程)→ 该节点明确报错,
/// 文案点名连接 id;**不静默回退默认连接**;其余节点照跑。
#[tokio::test]
async fn custom_node_connection_missing_is_rejected_not_fallen_back() {
    let _guard = test_lock().await;
    let app = test_app();
    put_connections(app, &[]).await;
    put_flow(
        app,
        "失效连接流程图",
        json!([
            {"id":"n-ok","name":"正常步","enabled":true,"goal":"产出成果",
             "action":"direct","generates":true,"is_output":true},
            {"id":"n-missing","name":"失效引用步","enabled":true,"goal":"引用不存在的连接",
             "action":"direct","generates":true,"connection_id":"conn-已删除"}
        ]),
    )
    .await;

    let (id, st, detail) = run_custom_task(app, r#"[[reply:正常产出]] 失效连接用例"#).await;
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan[0]["status"], "done", "无关节点不该受牵连: {detail}");
    assert_eq!(plan[0]["result"], "正常产出");
    assert_eq!(plan[1]["status"], "error");
    let msg = plan[1]["result"].as_str().unwrap_or("");
    assert!(
        msg.contains("conn-已删除") && msg.contains("不在设置里"),
        "错误文案应点名连接 id 并说明原因(不得静默回退默认连接): {msg}"
    );
    assert_eq!(st, "partial");

    // 解析失败发生在**发起调用之前**:该节点不留调用行(mock 也没被偷偷调用)
    let missing_row = step_call(app, &id, 1).await;
    assert!(
        missing_row.is_null(),
        "引用失效的连接不得回退默认连接发起调用: {missing_row:?}"
    );
}

/// 二维批次 5b:引用的连接**被停用** → 同样是明确报错(文案说「已停用」),
/// 与「已删除」区分开——两种成因给用户的下一步动作不同。
#[tokio::test]
async fn custom_node_connection_disabled_is_rejected_with_distinct_wording() {
    let _guard = test_lock().await;
    let app = test_app();
    // 默认连接 + 一条**停用**的 openai 连接
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [
            {"name": "默认", "connector_type": "mock", "base_url": "", "model": "", "enabled": true},
            {"name": "外部连接", "connector_type": "openai-compatible",
             "base_url": "http://127.0.0.1:1/v1", "api_key": "sk-x",
             "model": "model-d", "enabled": false}
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "保存应成功: {body}");
    let conns = settings_body(&body)["connections"]
        .as_array()
        .unwrap()
        .clone();
    let disabled_id = conns
        .iter()
        .find(|c| c["enabled"] == false)
        .expect("应有停用连接")["id"]
        .as_str()
        .unwrap()
        .to_string();

    put_flow(
        app,
        "停用连接流程图",
        json!([
            {"id":"n-ok","name":"正常步","enabled":true,"goal":"产出成果",
             "action":"direct","generates":true,"is_output":true},
            {"id":"n-off","name":"停用引用步","enabled":true,"goal":"引用停用的连接",
             "action":"direct","generates":true,"connection_id": disabled_id}
        ]),
    )
    .await;

    let (id, _st, detail) = run_custom_task(app, r#"[[reply:正常产出]] 停用连接用例"#).await;
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan[0]["status"], "done");
    assert_eq!(plan[1]["status"], "error");
    let msg = plan[1]["result"].as_str().unwrap_or("");
    assert!(
        msg.contains("已停用"),
        "错误文案应说明「已停用」而不是「不存在」: {msg}"
    );
    assert!(step_call(app, &id, 1).await.is_null());
}

/// 二维批次 5b 的回归面:不带 `connection_id` 的节点行为与 5b 之前一致
/// (默认连接、model 记 mock 连接器的真实值、任务照常完成)。
#[tokio::test]
async fn custom_nodes_without_connection_keep_default_behaviour() {
    let _guard = test_lock().await;
    let app = test_app();
    put_connections(app, &[]).await;
    put_flow(
        app,
        "无连接流程图",
        json!([
            {"id":"n1","name":"起草","enabled":true,"goal":"起草",
             "action":"direct","generates":true},
            {"id":"n2","name":"定稿","enabled":true,"goal":"定稿",
             "action":"direct","generates":true,"is_output":true}
        ]),
    )
    .await;

    let (id, st, detail) = run_custom_task(app, r#"[[reply:默认产出]] 无连接用例"#).await;
    assert_eq!(st, "done", "无连接节点应正常跑完: {detail}");
    assert_eq!(detail["task"]["result"], "默认产出");
    for i in [0i64, 1] {
        let row = step_call(app, &id, i).await;
        assert_eq!(row["status"], "ok");
        assert_eq!(
            row["model"], "mock-demo",
            "默认连接下 model 列仍是连接器真实值(mock-demo): {row:?}"
        );
    }
}

/// 二维批次 5b:节点级 `max_tool_rounds` 真在裁决「跑几轮」——
/// 同一条流程里,限额 1 的节点停在工具轮(第 1 轮),未限额的节点跑到完成轮。
/// 判别依据与 6a 档位用例同款:mock 每轮 prompt token 恒为 5,故 5 = 单轮、15 = 三轮。
#[tokio::test]
async fn custom_node_tool_rounds_limit_actually_caps_the_loop() {
    let _guard = test_lock().await;
    let app = test_app();
    put_connections(app, &[]).await;
    put_flow(
        app,
        "轮次上限流程图",
        json!([
            {"id":"n-capped","name":"限额步","enabled":true,"goal":"最多跑一轮",
             "action":"direct","generates":true,"tools":["calculator"],
             "max_tool_rounds": 1},
            {"id":"n-full","name":"不限额步","enabled":true,"goal":"跑满工具循环",
             "action":"direct","generates":true,"is_output":true,"tools":["calculator"]}
        ]),
    )
    .await;

    // 钩子语义:前 2 轮返回「短正文 + ToolCall」,第 3 轮返回完成正文
    let goal = r#"[[tool_loop_text:calculator|2 {"expression":"1+1"}]] 轮次上限用例"#;
    let (id, st, detail) = run_custom_task(app, goal).await;
    assert_eq!(st, "done", "两节点都该有产出: {detail}");
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(
        plan[0]["result"], "（第1轮说明）",
        "限额 1 的节点应停在第一轮: {detail}"
    );
    assert_eq!(
        plan[1]["result"], "（模拟回复）工具循环已完成,最终回复。",
        "未限额的节点应跑满三轮: {detail}"
    );

    let capped = step_call(app, &id, 0).await;
    let full = step_call(app, &id, 1).await;
    assert_eq!(
        capped["prompt_tokens"].as_i64(),
        Some(5),
        "限额 1 = 恰 1 次模型调用: {capped:?}"
    );
    assert_eq!(
        full["prompt_tokens"].as_i64(),
        Some(15),
        "未限额 = 3 次模型调用(两轮工具 + 完成轮): {full:?}"
    );
}

/// 二维批次 5b:节点级连接的**保存期不校验引用是否存在**(流程要能跨机导入),
/// 但流程 JSON 必须原样保留该字段(否则导入后连接选择静默丢失)。
#[tokio::test]
async fn connection_field_roundtrips_through_flow_library() {
    let _guard = test_lock().await;
    let app = test_app();
    put_connections(app, &[]).await;
    put_flow(
        app,
        "往返流程图",
        json!([
            {"id":"n1","name":"步骤","enabled":true,"goal":"产出",
             "action":"direct","generates":true,
             "connection_id":"本机不存在的外部连接","max_tool_rounds":7}
        ]),
    )
    .await;

    let (status, json) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let step = json["library"]["flows"]
        .as_array()
        .and_then(|fs| fs.iter().find(|f| f["name"] == "往返流程图"))
        .map(|f| f["steps"][0].clone())
        .expect("库内应有该流程");
    assert_eq!(step["connection_id"], "本机不存在的外部连接");
    assert_eq!(step["max_tool_rounds"], 7);
}
