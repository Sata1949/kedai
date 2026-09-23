// 对比模式(二维批次 7b)集成测试:根流程照常执行 + 名单内流程作为 `run_flow` 工具
// 释放给宽松节点,模型自主调用、取回成果;三道闸(名单/环/深度/预算)在**任何模型调用
// 之前**生效(被拒的调用不产生调用行,即「不烧钱」)。
//
// 独立成文件的原因同其它 task_custom_*.rs:流程库是全局单例(data/agent_flows.json +
// current_flow_id),而同一测试文件内的用例共享一个 app 实例 → 单开文件即单开进程、
// 单开数据目录;文件内多条用例仍共享流程库,故统一 test_lock() 串行。
//
// mock 钩子:`[[tool_echo:run_flow {args}]]` 首轮返回 2 个 ToolCall、有工具结果后回显
// 完整消息序列(于是**工具返回的正文**可断言——这是「成果真的回灌进后续轮次」的证据);
// `[[tool_loop:run_flow|N {args}]]` 前 N 轮各返回 1 个 ToolCall(参数逐轮变化,用于预算)。
// `[[reply_if:needle|内容]]` 由被调流程节点的 system 提示词携带 needle 时命中。
//
// **嵌套标记的转义纪律**:mock 的标记解析在**首个 `]]`** 处截断,而「调用 B 并把一段
// input 传给它」要求 input 里带内层标记(含 `]]`)。故所有标记参数一律经 `args_text`
// 序列化——把 `]` 写成 `\u005d`,serde 解析时解回原字符,被调流程节点看到的输入文本
// 与手写完全一致,而外层标记不会被内层的 `]]` 提前截断。
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

/// 保存流程并返回 id(**取自 PUT 响应的 `config.id`**,不按名字回查)。
///
/// 为什么必须这样取:流程库是**全局单例**,本文件多条用例各自建同名流程(「甲流程」…),
/// 按名字回查会命中**上次用例**建的那一份(第一条同名项),于是用例之间互相偷 id
/// ——冻结/改库类用例会改成别人的流程,表现成随机失败。响应里的 config 才是本次新建的那份。
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

/// 单节点**宽松**流程:节点带判别 needle 的 system 提示词 + 全部工具(可进工具循环)。
/// 宽松档(tools=[] ∪ 缺省 kind)= run_flow 的释放面;严格档看不到它(不在本文件范围)。
fn loose_node_flow(needle: &str) -> Value {
    json!([{
        "id": "n1",
        "name": format!("节点{needle}"),
        "enabled": true,
        "goal": "按本步指令产出",
        "action": "direct",
        "generates": true,
        "is_output": true,
        "system_prompt": format!("【本步指令·{needle}】只输出钩子指定的内容"),
        "tools": []
    }])
}

/// 纯生成节点流程(不挂工具):被调流程里那些「只负责产出内容」的节点用
fn plain_node_flow(needle: &str) -> Value {
    json!([{
        "id": "n1",
        "name": format!("节点{needle}"),
        "enabled": true,
        "goal": "按本步指令产出",
        "action": "direct",
        "generates": true,
        "is_output": true,
        "system_prompt": format!("【本步指令·{needle}】只输出钩子指定的内容")
    }])
}

/// mock 标记参数序列化:`]` 写成 `\u005d`(理由见文件头「嵌套标记的转义纪律」)
fn args_text(v: &Value) -> String {
    serde_json::to_string(v).unwrap().replace(']', "\\u005d")
}

/// `run_flow` 的参数 JSON(可带 input)
fn run_flow_args(flow: &str, input: Option<&str>) -> Value {
    match input {
        Some(i) => json!({ "flow": flow, "input": i }),
        None => json!({ "flow": flow }),
    }
}

/// 首轮 2 个调用、次轮回显消息序列的标记(工具返回正文因此可断言)
fn echo_marker(args: &Value) -> String {
    format!("[[tool_echo:run_flow {}]]", args_text(args))
}

/// 前 N 轮各 1 个调用的标记(参数逐轮变化 → 不与重复调用熔断混淆)
fn loop_marker(n: usize, args: &Value) -> String {
    format!("[[tool_loop:run_flow|{n} {}]]", args_text(args))
}

fn reply_if(needle: &str, content: &str) -> String {
    format!("[[reply_if:{needle}|{content}]]")
}

/// 创建 custom 任务(可绑根流程 + 带可调用名单)
async fn create_custom_task(
    app: &axum::Router,
    title: &str,
    flow_id: Option<&str>,
    flow_ids: Option<&[String]>,
) -> (StatusCode, Value) {
    let mut body = json!({ "title": title, "task_mode": "custom" });
    if let Some(id) = flow_id {
        body["flow_id"] = json!(id);
    }
    if let Some(ids) = flow_ids {
        body["flow_ids"] = json!(ids);
    }
    send_json(app, "POST", "/api/tasks", body).await
}

async fn create_ok(
    app: &axum::Router,
    title: &str,
    flow_id: Option<&str>,
    flow_ids: Option<&[String]>,
) -> String {
    let (status, json) = create_custom_task(app, title, flow_id, flow_ids).await;
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

async fn calls_of(app: &axum::Router, id: &str) -> Value {
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    calls
}

/// 动态调用层的调用行(phase 以 `call.` 起头)——「是否烧过 token」的判据
fn dynamic_rows(calls: &Value) -> Vec<String> {
    calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["phase"].as_str().unwrap_or("").to_string())
        .filter(|p| p.starts_with("call."))
        .collect()
}

/// 动态调用层的路径**层级数** = `call.` 之后的段数(`call.d1` = 1 层,`call.d1.d2` = 2 层)
fn dynamic_depth(phase: &str) -> usize {
    phase
        .strip_prefix("call.")
        .map(|rest| rest.matches('.').count() + 1)
        .unwrap_or(0)
}

/// 记账不变量:全部调用行 token 求和 == 详情 usage_total
/// (被调流程漏记、或父子两层各记一次都会在这里失败)
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

/// 对比模式:模型调用名单内流程 → 被调流程真的跑了一整套编排(有 `call.d<n>` 调用行)、
/// 其成果经工具返回**回灌进后续轮次**(根节点第二轮的回显里能看到它)、
/// 记账不变量不破(被调流程各节点各记一行,宿主节点不重复计入)。
#[tokio::test]
async fn compare_calls_listed_flow_and_feeds_result_back() {
    let _guard = test_lock().await;
    let app = test_app();

    let b = save_flow(app, "乙流程", plain_node_flow("乙本步")).await;
    let a = save_flow(app, "甲流程", loose_node_flow("甲本步")).await;

    let args = run_flow_args(&b, Some(&reply_if("乙本步", "乙成果")));
    let title = format!("{} 对比模式用例目标", echo_marker(&args));
    let id = create_ok(app, &title, Some(&a), Some(std::slice::from_ref(&b))).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "对比模式任务应完成: {detail}");

    let calls = calls_of(app, &id).await;
    let rows = dynamic_rows(&calls);
    assert_eq!(
        rows.len(),
        2,
        "首轮 2 个 ToolCall → 两次动态调用各一层(call.d1/call.d2): {calls}"
    );
    assert!(
        rows.contains(&"call.d1".to_string()) && rows.contains(&"call.d2".to_string()),
        "动态层 phase 应为 call.d<n>: {calls}"
    );
    assert!(
        result_of(&detail).contains(r#"[tool] "乙成果""#),
        "被调流程成果必须经工具结果回灌进后续轮次: {}",
        result_of(&detail)
    );
    assert_usage_invariant(&detail, &calls);
}

/// 名单外流程:拒绝且**不产生任何调用行**(不烧钱)。
///
/// 判别性设计:丙流程是**名单成员乙流程的子流程**——它因此**在冻结闭包内**(运行期取得到),
/// 唯一拦下它的是「名单」这道闸。若把名单判定放宽成「闭包内即可调用」,本用例立刻变红。
#[tokio::test]
async fn compare_rejects_flow_outside_list_without_burning() {
    let _guard = test_lock().await;
    let app = test_app();

    let c = save_flow(app, "丙流程", plain_node_flow("丙本步")).await;
    // 乙静态挂载丙 → 丙进冻结闭包(但**不在**名单里)
    let b = save_flow(
        app,
        "乙流程",
        json!([{
            "id": "n1", "name": "乙挂丙", "enabled": true, "goal": "跑丙",
            "action": "direct", "generates": true, "is_output": true, "sub_flow_id": c
        }]),
    )
    .await;
    let a = save_flow(app, "甲流程", loose_node_flow("甲本步")).await;

    let args = run_flow_args(&c, Some("丙不应被调用"));
    let title = format!("{} 名单外拒绝用例", echo_marker(&args));
    let id = create_ok(app, &title, Some(&a), Some(std::slice::from_ref(&b))).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "名单外被拒不应让任务失败: {detail}");

    let calls = calls_of(app, &id).await;
    assert!(
        dynamic_rows(&calls).is_empty(),
        "名单外流程被拒绝时不得产生任何动态调用行(不烧钱): {calls}"
    );
    assert!(
        calls["calls"].as_array().unwrap().len() == 1,
        "根节点自身仍有一条调用行(证明任务真的跑了): {calls}"
    );
    let res = result_of(&detail);
    assert!(
        res.contains("不在本任务的可调用名单内"),
        "工具返回应说明为何被拒: {res}"
    );
    assert_usage_invariant(&detail, &calls);
}

/// 跨流程环:被调流程的节点再调用自己 → 调用链环守卫拦下(图内环检测挡不住跨流程环),
/// 且不产生第二层的调用行。
#[tokio::test]
async fn compare_blocks_cycle_across_dynamic_layers() {
    let _guard = test_lock().await;
    let app = test_app();

    let b = save_flow(app, "乙流程", loose_node_flow("乙本步")).await;
    let a = save_flow(app, "甲流程", loose_node_flow("甲本步")).await;

    // 乙的节点再调乙自己(名单内,但已在调用链上)
    let inner = echo_marker(&run_flow_args(&b, None));
    let args = run_flow_args(&b, Some(&inner));
    let title = format!("{} 环守卫用例", echo_marker(&args));
    let id = create_ok(app, &title, Some(&a), Some(&[b])).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "环被拦后任务仍应完成(错误回灌给模型): {detail}");

    let calls = calls_of(app, &id).await;
    let rows = dynamic_rows(&calls);
    assert_eq!(
        rows.len(),
        2,
        "只有第一层被调用(call.d1/d2),第二层被环守卫拦下: {calls}"
    );
    assert!(
        rows.iter().all(|p| dynamic_depth(p) == 1),
        "不得出现第二层动态路径: {rows:?}"
    );
    let res = result_of(&detail);
    assert!(res.contains("调用链存在环"), "回灌文本应说明环被拦: {res}");
    assert_usage_invariant(&detail, &calls);
}

/// 动态嵌套深度:甲→乙→丙→丁 四层,第三层动态调用被 `MAX_FLOW_CALL_DEPTH` 拦下
/// (每层都会把 token 消耗成倍放大)。路径命名同时被锁定:`call.d1` / `call.d1.d2`。
#[tokio::test]
async fn compare_depth_guard_blocks_third_level() {
    let _guard = test_lock().await;
    let app = test_app();

    let d = save_flow(app, "丁流程", plain_node_flow("丁本步")).await;
    let c = save_flow(app, "丙流程", loose_node_flow("丙本步")).await;
    let b = save_flow(app, "乙流程", loose_node_flow("乙本步")).await;
    let a = save_flow(app, "甲流程", loose_node_flow("甲本步")).await;

    // 逐层拼装:丙的节点调丁;乙的节点调丙(把它作为 input 传给丙);甲的节点调乙
    let call_d = echo_marker(&run_flow_args(&d, None));
    let call_c = echo_marker(&run_flow_args(&c, Some(&call_d)));
    let call_b = echo_marker(&run_flow_args(&b, Some(&call_c)));
    let title = format!("{call_b} 深度守卫用例");
    let id = create_ok(app, &title, Some(&a), Some(&[b, c, d])).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "深度被拦后任务仍应完成: {detail}");

    let calls = calls_of(app, &id).await;
    let rows = dynamic_rows(&calls);
    assert!(!rows.is_empty(), "前两层应正常执行: {calls}");
    assert!(
        rows.iter().all(|p| dynamic_depth(p) <= 2),
        "最深只到两层动态路径(call.d1 / call.d1.d2),不得出现第三层: {rows:?}"
    );
    assert!(
        rows.iter().any(|p| p == "call.d1.d2"),
        "第二层路径应为 call.d1.d2(嵌套命名口径): {rows:?}"
    );
    let res = result_of(&detail);
    assert!(
        res.contains("嵌套超过 2 层"),
        "回灌文本应说明深度被拦: {res}"
    );
    assert_usage_invariant(&detail, &calls);
}

/// 每任务调用预算:模型串行地反复调用同一套流程时,第 9 次起拒绝(深度与环都拦不住
/// 这种形态)。断言动态调用行恰好 8 条。
#[tokio::test]
async fn compare_call_budget_halts_further_calls() {
    let _guard = test_lock().await;
    let app = test_app();

    let b = save_flow(app, "乙流程", plain_node_flow("乙本步")).await;
    let a = save_flow(app, "甲流程", loose_node_flow("甲本步")).await;

    // 参数里的 input 用 reply_if 钩子(被调流程节点因此产出确定内容),
    // 且每轮 `_mock_round` 注入使指纹互不相同 → 走的是预算闸而不是重复调用熔断
    let args = run_flow_args(&b, Some(&reply_if("乙本步", "乙成果")));
    let title = format!("{} 预算用例", loop_marker(10, &args));
    let id = create_ok(app, &title, Some(&a), Some(&[b])).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "done",
        "超预算的调用被拒后任务仍应完成(错误回灌给模型): {detail}"
    );

    let calls = calls_of(app, &id).await;
    let rows = dynamic_rows(&calls);
    assert_eq!(
        rows.len(),
        8,
        "每任务预算 8 次:第 9、10 次调用应在任何模型调用之前被拒: {rows:?}"
    );
    assert_usage_invariant(&detail, &calls);
}

/// 强制模式(未给名单 = 老客户端行为):`run_flow` 不在任何工具清单里,模型臆造调用
/// 被闸门拒且不产生动态调用行。「聊天路径看不到 run_flow」与这里同源:
/// 该工具只由 custom 执行器在对比模式下逐节点下发。
#[tokio::test]
async fn forced_mode_rejects_hallucinated_run_flow() {
    let _guard = test_lock().await;
    let app = test_app();

    let b = save_flow(app, "乙流程", plain_node_flow("乙本步")).await;
    let a = save_flow(app, "甲流程", loose_node_flow("甲本步")).await;

    let args = run_flow_args(&b, Some("强制模式不该能调用流程"));
    let title = format!("{} 强制模式用例", echo_marker(&args));
    // 不给 flow_ids = 强制模式
    let id = create_ok(app, &title, Some(&a), None).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "臆造调用被拒不应让任务失败: {detail}");

    let calls = calls_of(app, &id).await;
    assert!(
        dynamic_rows(&calls).is_empty(),
        "强制模式下不得产生任何动态调用行: {calls}"
    );
    let res = result_of(&detail);
    assert!(
        res.contains("不在当前任务策略允许的工具清单内"),
        "臆造调用应被工具闸门拒绝: {res}"
    );
}

/// 创建期校验(400 四例)+ 快照冻结:名单成员进冻结域,绑定后改库不影响这个任务。
#[tokio::test]
async fn compare_validates_list_at_creation_and_freezes_members() {
    let _guard = test_lock().await;
    let app = test_app();

    let b = save_flow(app, "乙流程", plain_node_flow("乙本步")).await;
    let a = save_flow(app, "甲流程", loose_node_flow("甲本步")).await;
    let mut off = plain_node_flow("停用本步");
    off[0]["enabled"] = json!(false);
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": { "id": "", "name": "停用流程", "enabled": false, "steps": off } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "保存停用流程应 200: {json}");
    let off_id = json["config"]["id"].as_str().unwrap().to_string();

    // ① 非 custom 模式给了 flow_ids → 400
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": "非 custom", "task_mode": "solo", "flow_ids": [b] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("自定义流程模式"),
        "{json}"
    );

    // ② 空名单 → 400(空名单 = 名存实亡)
    let (status, json) = create_custom_task(app, "空名单", None, Some(&[])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert!(
        json["error"].as_str().unwrap_or("").contains("名单为空"),
        "{json}"
    );

    // ③ 名单成员不存在 / ④ 未启用 → 400,且逐项点名
    let (status, json) =
        create_custom_task(app, "成员不存在", None, Some(&["flow-不存在".into()])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert!(
        json["error"].as_str().unwrap_or("").contains("流程不存在"),
        "{json}"
    );
    let (status, json) = create_custom_task(app, "成员未启用", None, Some(&[off_id])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert!(
        json["error"].as_str().unwrap_or("").contains("未启用"),
        "{json}"
    );

    // ⑤ 绑定的根流程出现在名单里 → 400(根流程正在执行,被调用必然撞环守卫)
    let (status, json) = create_custom_task(app, "名单含根", Some(&a), Some(std::slice::from_ref(&a))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("不能包含根流程"),
        "{json}"
    );

    // ⑥ 正常创建:名单成员进冻结闭包 → 快照里能查到;改库(把乙改坏)不影响本次执行
    let args = run_flow_args(&b, Some(&reply_if("乙本步", "乙成果")));
    let title = format!("{} 冻结用例", echo_marker(&args));
    let id = create_ok(app, &title, Some(&a), Some(std::slice::from_ref(&b))).await;
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    let frozen: Vec<String> = detail["flow_snapshot"]["flows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        frozen.contains(&b),
        "对比模式的名单成员必须在冻结快照里: {frozen:?}"
    );

    // 改库:把乙流程的节点 needle 换掉(库内那份已不是任务冻结的那份)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": { "id": b, "name": "乙流程", "enabled": true,
            "steps": plain_node_flow("乙改后本步") } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "覆盖更新乙流程应 200");

    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "{detail}");
    assert!(
        result_of(&detail).contains(r#"[tool] "乙成果""#),
        "任务跑的是**冻结**的那份乙流程(改库不该影响已建任务): {}",
        result_of(&detail)
    );
}
