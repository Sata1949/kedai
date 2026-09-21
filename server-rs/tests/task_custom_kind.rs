// 二维批次 6a(严格/宽松节点档位)集成测试:档位**优先于** tools 配置。
//
// 严格节点即使声明了工具也单次调用、不下发工具 —— 模型即便索要工具也无人执行(没有第二轮);
// 宽松节点在**同一份工具配置**下进入工具自循环。两条路径的差异必须可观测,故本用例用
// mock 钩子 [[tool_loop_text:name|N args]]:前 N 轮同时产出「短正文 + ToolCall」,
// 第 N+1 轮(已回填 N 条 tool 结果)产出完成正文 —— 于是「跑到第几轮」直接可读:
//   - 严格节点停在「（第1轮说明）」(单次调用,工具请求被忽略,没有 tool 结果可推进);
//   - 宽松节点推进到「（模拟回复）工具循环已完成,最终回复。」(三轮:两轮工具 + 完成轮)。
//
// 独立成文件的原因与 task_custom_graph.rs 相同:流程库是**全局单例**
// (data/agent_flows.json + current_flow_id),同文件内多用例共享一个 app 实例会互相覆盖;
// 单开文件即单开进程 → 单开数据目录,可与其它 custom 用例并行运行。
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

/// 二维批次 6a:严格档 = 单次模型调用、不下发工具;宽松档 = 工具自循环。
///
/// 验收断言来自计划批次 6(「严格节点不进入工具循环(断言只 1 次模型调用)」),
/// 落点为:严格节点产出停在 mock 的**第 1 轮**、调用行 prompt token = 单轮值;
/// 宽松节点在同配置下推进到完成轮(第 3 轮)且 token = 三轮之和。
#[tokio::test]
async fn strict_step_stays_single_call_while_loose_loops() {
    let app = test_app();
    // 两个节点**工具配置完全相同**,唯一差异是 kind 档位 —— 便于断言「档位优先于 tools」
    let flow = json!({
        "config": {
            "id": "",
            "name": "测试档位流程图",
            "enabled": true,
            "steps": [
                {"id":"n-strict","name":"原子步","enabled":true,"goal":"单次调用完成",
                 "action":"direct","generates":true,"kind":"strict","tools":["calculator"],
                 "system_prompt":"【本步指令·原子步】只输出一句话"},
                {"id":"n-loose","name":"循环步","enabled":true,"goal":"工具循环完成",
                 "action":"direct","generates":true,"is_output":true,"tools":["calculator"],
                 "system_prompt":"【本步指令·循环步】用工具算完再总结"}
            ]
        }
    });
    let (status, json) = send_json(app, "PUT", "/api/agent-flows", flow).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "严格档流程应可保存(二维批次 6a 放宽校验): {json}"
    );

    // mock 钩子:两轮工具调用(calculator 是安全工具,任务默认策略放行)
    let goal = r#"[[tool_loop_text:calculator|2 {"expression":"1+1"}]] 档位用例目标"#;
    let id = create_task_with_mode(app, goal, "custom").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "两节点都应正常完成: {detail}");

    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0]["status"], "done", "严格节点应完成: {detail}");
    assert_eq!(plan[1]["status"], "done", "宽松节点应完成: {detail}");
    assert_eq!(
        plan[0]["result"].as_str().unwrap_or(""),
        "（第1轮说明）",
        "严格节点必须停在单次调用(工具未下发 → 模型索要的工具无人执行 → 无第二轮): {detail}"
    );
    assert_eq!(
        plan[1]["result"].as_str().unwrap_or(""),
        "（模拟回复）工具循环已完成,最终回复。",
        "宽松节点应跑满工具循环(两轮工具 + 完成轮): {detail}"
    );
    assert_eq!(
        detail["task"]["result"].as_str().unwrap_or(""),
        "（模拟回复）工具循环已完成,最终回复。",
        "成果应取 is_output 节点产出: {detail}"
    );

    // 记账佐证「严格 = 1 次模型调用 / 宽松 = 3 轮」:mock 每轮 prompt token 恒为 5,
    // 任务侧逐节点聚合为 1 行(phase=step),故行数与真调用次数无关、token 之和才是证据。
    let strict_row = step_call(app, &id, 0).await;
    let loose_row = step_call(app, &id, 1).await;
    assert_eq!(
        strict_row["prompt_tokens"].as_i64(),
        Some(5),
        "严格节点应恰发生 1 次模型调用: {strict_row:?}"
    );
    assert_eq!(
        loose_row["prompt_tokens"].as_i64(),
        Some(15),
        "宽松节点应发生 3 次模型调用(两轮工具 + 完成轮): {loose_row:?}"
    );
}

/// 二维批次 6a:未知档位在保存期被 400 拒绝(不允许静默无效),且错误文案点名步骤与取值。
#[tokio::test]
async fn unknown_kind_is_rejected_on_save() {
    let app = test_app();
    let flow = json!({
        "config": {
            "id": "",
            "name": "非法档位流程图",
            "enabled": true,
            "steps": [
                {"id":"n1","name":"原子步","enabled":true,"goal":"产出",
                 "action":"direct","generates":true,"kind":"medium"}
            ]
        }
    });
    let (status, json) = send_json(app, "PUT", "/api/agent-flows", flow).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未知档位应 400: {json}");
    let msg = json["error"].as_str().unwrap_or("");
    assert!(
        msg.contains("档位") && msg.contains("原子步"),
        "错误文案应点名步骤与档位: {json}"
    );
}
