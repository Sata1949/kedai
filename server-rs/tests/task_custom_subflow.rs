// 静态子图(二维批次 6b)集成测试:节点挂载子流程 → 子图产出正确注入下游、
// 嵌套记账(phase=subflow.<路径>)不重复计入、记账不变量不破。
//
// 独立成文件的原因同 task_custom_graph.rs:流程库是**全局单例**(data/agent_flows.json
// + current_flow_id),而同一测试文件内的用例共享一个 app 实例 → 单开文件即单开进程,
// 单开数据目录。文件内多条用例仍共享流程库,故统一用 test_lock() 串行(既有惯例,
// 见 tests/agent_flows.rs)。
//
// mock 钩子 [[reply_if:needle|内容]]:needle 命中任一 **system** 消息(钩子语法自身的
// 出现不算命中)时返回指定内容;故把钩子写进任务目标(它同时进 user 与 system 的
// 「任务目标」段),再让各步骤的 system 提示词含各自 needle,即可为每个节点指定确定性产出。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

fn test_app() -> &'static axum::Router {
    static APP: OnceLock<axum::Router> = OnceLock::new();
    APP.get_or_init(|| build_test_app().expect("构建测试应用失败"))
}

/// 串行化全局共享状态(流程库)的测试
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

/// 保存流程并返回分配到的 id(按名字在库里回查;PUT 响应即携带 library)
async fn save_flow(app: &axum::Router, name: &str, steps: Value) -> String {
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": { "id": "", "name": name, "enabled": true, "steps": steps } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "保存流程「{name}」应 200: {json}");
    json["library"]["flows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .unwrap_or_else(|| panic!("保存后库里应有流程「{name}」: {json}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn create_task(app: &axum::Router, title: &str) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": "custom" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

async fn run_task(app: &axum::Router, id: &str) {
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
}

async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    for _ in 0..50 {
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

/// 取某条调用行(phase + step_index)的 prompt_summary
fn call_prompt(calls: &Value, phase: &str, step_index: i64) -> String {
    calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["phase"] == phase && c["step_index"] == step_index)
        .unwrap_or_else(|| panic!("应有 phase={phase} step_index={step_index} 的调用行: {calls}"))
        ["prompt_summary"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// 二维批次 6b:节点挂载静态子流程——子图产出注入下游、记账按 subflow.<路径> 归属、
/// 记账不变量(各行求和 == usage_total)不破。
///
/// 主流程(线性三步):起草 → 扩写(挂子流程) → 收尾;子流程(线性两步):子一 → 子二。
/// 「扩写」节点自己不发起模型调用,只用子图成果顶替自己的产出。
#[tokio::test]
async fn custom_sub_flow_injects_draft_and_keeps_usage_invariant() {
    let _guard = test_lock().await;
    let app = test_app();

    // 先存子流程(引用必须已存在),再存引用它的主流程(主流程成为当前流程)
    let sub_id = save_flow(
        app,
        "摘要子流程",
        json!([
            {"id":"s-a","name":"子一","enabled":true,"goal":"子一目标","action":"direct","generates":true,
             "system_prompt":"【本步指令·子一本步】只输出子一内容"},
            {"id":"s-b","name":"子二","enabled":true,"goal":"子二目标","action":"direct","generates":true,
             "is_output":true,"system_prompt":"【本步指令·子二本步】只输出子二内容"}
        ]),
    )
    .await;

    save_flow(
        app,
        "主流程",
        json!([
            {"id":"m-draft","name":"起草","enabled":true,"goal":"起草目标","action":"direct","generates":true,
             "system_prompt":"【本步指令·起草本步】只输出起草内容"},
            // 挂子流程的节点:自身的提示词/工具都被旁路(与 6a「档位优先」同一纪律)
            {"id":"m-expand","name":"扩写","enabled":true,"goal":"扩写目标","action":"direct","generates":true,
             "sub_flow_id": sub_id,
             "system_prompt":"【本步指令·扩写本步】本段不应被执行"},
            {"id":"m-polish","name":"收尾","enabled":true,"goal":"收尾目标","action":"direct","generates":true,
             "is_output":true,"system_prompt":"【本步指令·收尾本步】只输出收尾内容"}
        ]),
    )
    .await;

    let title = "[[reply_if:起草本步|起草产出]][[reply_if:扩写本步|扩写产出]]\
                 [[reply_if:子一本步|子一产出]][[reply_if:子二本步|子二产出]]\
                 [[reply_if:收尾本步|收尾成果]] 子流程任务目标";
    let id = create_task(app, title).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "挂子流程的 custom 任务应完成: {detail}");
    assert_eq!(
        detail["task"]["result"].as_str().unwrap_or(""),
        "收尾成果",
        "成果应为显式标注的收尾节点产出: {detail}"
    );

    // plan 恒为**外层**流程的节点列表(子图内部节点不占行),三行全 done;
    // 挂子流程那一行的产出 = 子图成果(证明子图产出正确注入)
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 3, "plan 行数 = 外层流程节点数: {detail}");
    assert_eq!(plan[0]["name"], "起草");
    assert_eq!(plan[1]["name"], "扩写");
    assert_eq!(plan[2]["name"], "收尾");
    for step in plan {
        assert_eq!(step["status"], "done", "节点应 done: {detail}");
    }
    assert_eq!(
        plan[1]["result"].as_str().unwrap_or(""),
        "子二产出",
        "挂子流程节点的产出应为子图成果(末个生成节点): {detail}"
    );

    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;

    // 记账归属:外层节点走 phase=step,子图节点走 phase=subflow.<父节点下标>
    let rows: Vec<&Value> = calls["calls"].as_array().unwrap().iter().collect();
    let step_idx: Vec<i64> = rows
        .iter()
        .filter(|c| c["phase"] == "step")
        .map(|c| c["step_index"].as_i64().unwrap())
        .collect();
    assert_eq!(
        step_idx,
        vec![0, 2],
        "外层只应有起草(0)与收尾(2)两次调用——挂子流程的节点自己不发起调用: {calls}"
    );
    let sub_idx: Vec<i64> = rows
        .iter()
        .filter(|c| c["phase"] == "subflow.1")
        .map(|c| c["step_index"].as_i64().unwrap())
        .collect();
    assert_eq!(
        sub_idx,
        vec![0, 1],
        "子图两个节点各一行,归到 subflow.1: {calls}"
    );
    // phase 集合等值(替换原先的 `!any(phase == "subflow")`:child_path 恒非空,
    // 裸 "subflow" 不可能出现——那条断言永远成立,没有判别力)
    let phases: BTreeSet<&str> = rows
        .iter()
        .map(|c| c["phase"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        phases,
        BTreeSet::from(["step", "subflow.1"]),
        "调用行 phase 集合应恰为外层与子图两层: {calls}"
    );

    // 上游产出流进子图:子图源节点收到的正是「扩写」节点的输入消息
    let sub_first = call_prompt(&calls, "subflow.1", 0);
    assert!(
        sub_first.contains("起草产出"),
        "子图源节点应收到外层上游产出(起草的产出): {sub_first}"
    );
    // 子图成果流向下游:收尾节点的输入里应出现子图成果
    let polish = call_prompt(&calls, "step", 2);
    assert!(
        polish.contains("子二产出"),
        "下游节点应拿到子图成果作为上游产出: {polish}"
    );
    // 方括号旁路证据:整条链上不应出现被旁路提示词的产出
    assert!(
        !rows.iter().any(|c| c["response_summary"]
            .as_str()
            .unwrap_or("")
            .contains("扩写产出")),
        "挂子流程节点的自身提示词不应被执行: {calls}"
    );

    // 记账不变量:全部调用行 token 求和 == 详情 usage_total
    //(子图若漏记或父子两层各记一次,这里会直接失败)
    let call_prompt_sum: i64 = rows
        .iter()
        .map(|c| c["prompt_tokens"].as_i64().unwrap_or(0))
        .sum();
    let call_completion_sum: i64 = rows
        .iter()
        .map(|c| c["completion_tokens"].as_i64().unwrap_or(0))
        .sum();
    let usage = &detail["usage_total"];
    assert_eq!(
        call_prompt_sum,
        usage["prompt_tokens"].as_i64().unwrap_or(-1),
        "各行 prompt token 求和应等于 usage_total: {detail}"
    );
    assert_eq!(
        call_completion_sum,
        usage["completion_tokens"].as_i64().unwrap_or(-1),
        "各行 completion token 求和应等于 usage_total: {detail}"
    );
}

/// 二维批次 6b:悬空引用在**保存期**被拒(400),且拒绝不落盘——
/// 当前流程仍是上一个合法流程(校验先于状态变更)。
#[tokio::test]
async fn custom_sub_flow_dangling_ref_rejected_without_state_change() {
    let _guard = test_lock().await;
    let app = test_app();

    let ok_id = save_flow(
        app,
        "合法单步流程",
        json!([
            {"id":"n","name":"单步","enabled":true,"goal":"单步目标","action":"direct","generates":true}
        ]),
    )
    .await;

    let (status, json) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": {
            "id": "", "name": "悬空引用流程", "enabled": true,
            "steps": [
                {"id":"n","name":"挂空","enabled":true,"goal":"挂空目标","action":"direct","generates":true,
                 "sub_flow_id":"no-such-flow"}
            ]
        }}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "引用不存在的子流程应 400: {json}"
    );
    let msg = json["error"].as_str().unwrap_or("");
    assert!(msg.contains("不存在"), "错误应点名悬空引用: {json}");

    let (_, lib) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    assert_eq!(
        lib["library"]["current_flow_id"].as_str(),
        Some(ok_id.as_str()),
        "被拒的保存不应改变当前流程: {lib}"
    );
    assert!(
        !lib["library"]["flows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "悬空引用流程"),
        "被拒的流程不应进入库: {lib}"
    );
}

/// 二维批次 6b 修复(R-1):同一子流程被**两个节点**挂载。
///
/// 回归点:聊天侧展开若原样拼接步骤 id,扁平列表会出现重复 id,而展开产物随后还要过一遍
/// `make_custom_plan` → `resolve_graph`(`effective_inputs` 以「步骤 id 重复」拒绝),
/// 于是**同一份流程任务侧照常跑完、聊天侧 400**。本用例两侧都跑,断言行为一致。
#[tokio::test]
async fn custom_sub_flow_mounted_twice_succeeds_on_both_sides() {
    let _guard = test_lock().await;
    let app = test_app();

    let sub_id = save_flow(
        app,
        "复用子流程",
        json!([
            {"id":"s-a","name":"子一","enabled":true,"goal":"子一目标","action":"direct","generates":true,
             "system_prompt":"【本步指令·复用子一本步】只输出子一内容"},
            {"id":"s-b","name":"子二","enabled":true,"goal":"子二目标","action":"direct","generates":true,
             "is_output":true,"system_prompt":"【本步指令·复用子二本步】只输出子二内容"}
        ]),
    )
    .await;
    let sub_ref = sub_id.as_str();

    // 主流程:起草 → 扩写甲(挂子流程) → 中转 → 扩写乙(挂同一子流程) → 定稿
    save_flow(
        app,
        "两处挂载主流程",
        json!([
            {"id":"m-draft","name":"起草","enabled":true,"goal":"起草目标","action":"direct","generates":true,
             "system_prompt":"【本步指令·起草本步】只输出起草内容"},
            {"id":"m-a","name":"扩写甲","enabled":true,"goal":"扩写甲目标","action":"direct","generates":true,
             "inputs":["m-draft"], "sub_flow_id": sub_ref},
            {"id":"m-mid","name":"中转","enabled":true,"goal":"中转目标","action":"direct","generates":true,
             "inputs":["m-a"], "system_prompt":"【本步指令·中转本步】只输出中转内容"},
            {"id":"m-b","name":"扩写乙","enabled":true,"goal":"扩写乙目标","action":"direct","generates":true,
             "inputs":["m-mid"], "sub_flow_id": sub_ref},
            {"id":"m-final","name":"定稿","enabled":true,"goal":"定稿目标","action":"direct","generates":true,
             "is_output":true,"inputs":["m-b"], "system_prompt":"【本步指令·定稿本步】只输出定稿成果"}
        ]),
    )
    .await;

    // ---- 聊天侧:预览 plan(展开后 id 曾重复 → 此处 400) ----
    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent/plan",
        json!({ "message": "两处挂载任务目标", "agent_mode": "custom" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "两处挂载同一子流程的流程应能预览 plan: {resp}"
    );
    let names: Vec<&str> = resp["plan"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        names,
        vec![
            "起草",
            "【子流程「复用子流程」】子一",
            "【子流程「复用子流程」】子二",
            "中转",
            "【子流程「复用子流程」】子一",
            "【子流程「复用子流程」】子二",
            "定稿",
        ],
        "两处挂载各自就地展开: {resp}"
    );

    // ---- 任务侧:同一份流程真跑一遍(两侧行为必须一致) ----
    let title = "[[reply_if:起草本步|起草产出]][[reply_if:中转本步|中转产出]]\
                 [[reply_if:复用子一本步|子一产出]][[reply_if:复用子二本步|子二产出]]\
                 [[reply_if:定稿本步|定稿成果]] 两处挂载任务目标";
    let id = create_task(app, title).await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "两处挂载的子流程任务应完成: {detail}");
    assert_eq!(
        detail["task"]["result"].as_str().unwrap_or(""),
        "定稿成果",
        "成果仍取外层标注的成果节点: {detail}"
    );

    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 5, "plan 仍是外层节点列表: {detail}");
    for (i, expect) in ["起草", "扩写甲", "中转", "扩写乙", "定稿"]
        .iter()
        .enumerate()
    {
        assert_eq!(plan[i]["name"], *expect, "plan 第 {i} 行: {detail}");
        assert_eq!(plan[i]["status"], "done", "plan 第 {i} 行应 done: {detail}");
    }
    assert_eq!(
        plan[1]["result"], "子二产出",
        "第一处挂载取子图成果: {detail}"
    );
    assert_eq!(
        plan[3]["result"], "子二产出",
        "第二处挂载取子图成果: {detail}"
    );

    // 记账:两处挂载的路径不同(subflow.1 / subflow.3),id 重写不改变归属
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let rows: Vec<&Value> = calls["calls"].as_array().unwrap().iter().collect();
    let phases: BTreeSet<&str> = rows
        .iter()
        .map(|c| c["phase"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        phases,
        BTreeSet::from(["step", "subflow.1", "subflow.3"]),
        "两处挂载各按自己的路径记账: {calls}"
    );

    // 记账不变量:各行求和 == usage_total(两处子图若只在父子某一层记账,这里会失败)
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
