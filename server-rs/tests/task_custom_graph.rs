// 二维自定义流程(custom 图执行)集成测试:多上游扇入、成果节点选拔。
// 独立成文件的原因:流程库是**全局单例**(data/agent_flows.json + current_flow_id),
// 而同一测试文件内的用例共享一个 app 实例(tests/tasks.rs 的既有注释已记载该竞态),
// 单开文件即单开进程 → 单开数据目录,可与其它 custom 用例并行运行。
//
// mock 钩子 [[reply_if:needle|内容]]:needle 命中任一 **system** 消息(钩子语法自身的
// 出现不算命中)时返回指定内容;故把钩子写进任务目标,再让各步骤的 system 提示词
// 含各自 needle,即可为每个节点指定确定性产出。
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

/// 取某步骤(step_index)调用追踪行的 prompt_summary
async fn step_prompt(app: &axum::Router, id: &str, step_index: i64) -> String {
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["phase"] == "step" && c["step_index"] == step_index)
        .and_then(|c| c["prompt_summary"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// 二维批次 1:菱形图(左路 / 右路 汇入 合并),验证
///  - 多父输入按**数组下标升序**拼接(声明顺序逆序也不影响,顺序即结果可复现性);
///  - 显式 is_output 节点产出即最终成果;
///  - plan 按流程数组顺序展示、节点全部 done。
#[tokio::test]
async fn custom_graph_merges_multi_parent_inputs_in_index_order() {
    let app = test_app();

    let flow = json!({
        "config": {
            "id": "",
            "name": "测试菱形流程图",
            "enabled": true,
            "steps": [
                {"id":"n-left","name":"左路","enabled":true,"goal":"产左路要点","action":"direct","generates":true,
                 "system_prompt":"【本步指令·左路本步】只输出左路要点"},
                {"id":"n-right","name":"右路","enabled":true,"goal":"产右路要点","action":"direct","generates":true,
                 "system_prompt":"【本步指令·右路本步】只输出右路要点"},
                // inputs 故意写成逆序(先右后左):执行侧必须按下标升序拼接
                {"id":"n-merge","name":"合并","enabled":true,"goal":"合并两路","action":"direct","generates":true,
                 "inputs":["n-right","n-left"],"is_output":true,
                 "system_prompt":"【本步指令·合并本步】合并上游两路产出"}
            ]
        }
    });
    let (status, json) = send_json(app, "PUT", "/api/agent-flows", flow).await;
    assert_eq!(status, StatusCode::OK, "保存二维流程应 200: {json}");

    let title = "[[reply_if:左路本步|左路产出]][[reply_if:右路本步|右路产出]]\
                 [[reply_if:合并本步|合并成果]] 二维任务目标";
    let id = create_task_with_mode(app, title, "custom").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "二维 custom 任务应完成: {detail}");
    assert_eq!(
        detail["task"]["result"].as_str().unwrap_or(""),
        "合并成果",
        "成果应为显式标注的合并节点产出: {detail}"
    );

    // plan 恒按流程数组顺序展示(用户编排顺序);三节点全部 done
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 3, "plan 步骤数 = 流程启用步骤数: {detail}");
    assert_eq!(plan[0]["name"], "左路");
    assert_eq!(plan[1]["name"], "右路");
    assert_eq!(plan[2]["name"], "合并");
    for step in plan {
        assert_eq!(step["status"], "done", "节点应 done: {detail}");
    }

    // 合并节点的 user 消息:多父段存在、两路产出都在、**左路在前**(下标升序)
    let merge_prompt = step_prompt(app, &id, 2).await;
    assert!(
        merge_prompt.contains("上游节点产出:"),
        "多父节点应收到上游产出段: {merge_prompt}"
    );
    let left = merge_prompt.find("【左路】").unwrap_or(usize::MAX);
    let right = merge_prompt.find("【右路】").unwrap_or(usize::MAX);
    assert!(
        left < right,
        "多父拼接必须按数组下标升序(左路在前): {merge_prompt}"
    );
    assert!(
        merge_prompt.contains("左路产出") && merge_prompt.contains("右路产出"),
        "两路上游产出都应进入合并输入: {merge_prompt}"
    );
    // 源节点只有任务目标,不带上游段
    let left_prompt = step_prompt(app, &id, 0).await;
    assert!(
        !left_prompt.contains("上游节点产出:") && !left_prompt.contains("上一步「"),
        "源节点不应携带上游段: {left_prompt}"
    );
}

/// 并行流程:两个源节点 + 一个合并节点(与上面同构,供批次 2 的两条用例复用)。
/// 只给合并节点配回复钩子:两个源节点不命中 → 走 mock 默认逐字回复(每字 8ms,
/// 约 0.8s),制造足够宽的并发观测窗口。
async fn preset_parallel_flow(app: &axum::Router, max_parallel: u32) -> String {
    let flow = json!({
        "config": {
            "id": "",
            "name": "测试并行流程图",
            "enabled": true,
            "max_parallel_nodes": max_parallel,
            "steps": [
                {"id":"n-left","name":"左路","enabled":true,"goal":"产左路要点","action":"direct","generates":true,
                 "system_prompt":"【本步指令·左路本步】只输出左路要点"},
                {"id":"n-right","name":"右路","enabled":true,"goal":"产右路要点","action":"direct","generates":true,
                 "system_prompt":"【本步指令·右路本步】只输出右路要点"},
                {"id":"n-merge","name":"合并","enabled":true,"goal":"合并两路","action":"direct","generates":true,
                 "inputs":["n-left","n-right"],"is_output":true,
                 "system_prompt":"【本步指令·合并本步】合并上游两路产出"}
            ]
        }
    });
    let (status, json) = send_json(app, "PUT", "/api/agent-flows", flow).await;
    assert_eq!(status, StatusCode::OK, "保存并行流程应 200: {json}");
    let id =
        create_task_with_mode(app, "[[reply_if:合并本步|合并成果]] 并行任务目标", "custom").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    id
}

/// 二维批次 2:并行分支同时推进、完成后 plan 每行都落库(IFW-4 无丢更新)、
/// 逐节点记账之和与详情 usage_total 一致。
///
/// 并行证据:调度器在补满并发槽位时**不 await**,两个源节点必然先同时进入
/// running 再开始等待回复,故轮询一定能观测到 ≥2 行同时 running(窗口 ~0.8s)。
#[tokio::test]
async fn custom_graph_parallel_branches_keep_all_plan_rows() {
    let app = test_app();
    let id = preset_parallel_flow(app, 2).await;

    let mut max_running = 0usize;
    let mut terminal = String::new();
    let mut detail = Value::Null;
    for _ in 0..400 {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        if let Some(plan) = json["task"]["plan"].as_array() {
            let running = plan.iter().filter(|s| s["status"] == "running").count();
            max_running = max_running.max(running);
        }
        let st = json["task"]["status"].as_str().unwrap_or("").to_string();
        if matches!(st.as_str(), "done" | "partial" | "error" | "ended") {
            terminal = st;
            detail = json;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(terminal, "done", "并行任务应完成: {detail}");
    assert!(
        max_running >= 2,
        "两个源节点应同时在跑(max_parallel_nodes=2):观测到的最大同时运行数 {max_running}"
    );

    // 无丢更新(IFW-4):并发完成后每一行都必须是终态,不能残留 pending/running
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 3, "plan 步骤数 = 流程启用步骤数: {detail}");
    for step in plan {
        assert_eq!(
            step["status"], "done",
            "并发完成后每行都应落库为 done(丢更新会让某行停在 running): {detail}"
        );
    }
    assert_eq!(
        detail["task"]["result"].as_str().unwrap_or(""),
        "合并成果",
        "成果应为显式标注的合并节点产出: {detail}"
    );

    // 记账不变量:phase=step 逐节点一行,求和 == 详情 usage_total(并行下不漏记)
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let rows: Vec<&Value> = calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["phase"] == "step")
        .collect();
    assert_eq!(rows.len(), 3, "phase=step 行数应与节点数一致: {calls:?}");
    let call_prompt: i64 = rows
        .iter()
        .map(|c| c["prompt_tokens"].as_i64().unwrap_or(0))
        .sum();
    let call_completion: i64 = rows
        .iter()
        .map(|c| c["completion_tokens"].as_i64().unwrap_or(0))
        .sum();
    let usage = &detail["usage_total"];
    assert_eq!(
        call_prompt,
        usage["prompt_tokens"].as_i64().unwrap_or(-1),
        "各行 prompt token 求和应等于 usage_total: {detail}"
    );
    assert_eq!(
        call_completion,
        usage["completion_tokens"].as_i64().unwrap_or(-1),
        "各行 completion token 求和应等于 usage_total: {detail}"
    );
}

/// 二维批次 2:取消传播到并行分支——停止后任务收口到 ended,在飞分支被中止,
/// 不再有任何节点继续写 plan,也不再有新的调用行产生。
#[tokio::test]
async fn custom_graph_stop_aborts_parallel_branches() {
    let app = test_app();
    let id = preset_parallel_flow(app, 2).await;

    // 等两个源节点进入 running 再停止(stop 落在并行窗口内)
    let mut saw_parallel = false;
    for _ in 0..200 {
        let (_, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        if let Some(plan) = json["task"]["plan"].as_array() {
            if plan.iter().filter(|s| s["status"] == "running").count() >= 2 {
                saw_parallel = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(saw_parallel, "停止前应观测到两个分支同时在跑");

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "running 态 stop 应 200");

    let (terminal, detail) = wait_terminal(app, &id).await;
    assert_eq!(terminal, "ended", "stop 后任务应 ended: {detail}");

    // 收口:终态后再等一段时间,验证「不再启动新节点」——
    //  - plan 不再变化:取消路径只丢弃在飞 future,不写 plan;
    //  - 下游「合并」节点保持 pending(取消检查在启动之前,任何分支都不能再被拉起);
    //  - 调用行只允许在飞分支的中断留痕(「已中断」行是前端徽标的数据源,按设计保留),
    //    绝不允许出现从未启动节点的行。
    let plan_before = detail["task"]["plan"].clone();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (_, after) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(
        after["task"]["plan"], plan_before,
        "取消后不应再有分支继续写 plan: {after}"
    );
    let plan_after = after["task"]["plan"].as_array().unwrap();
    assert_eq!(
        plan_after[2]["status"], "pending",
        "取消后下游节点应保持 pending(不得被拉起): {after}"
    );
    let (_, calls_after) =
        send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let rows_after: Vec<&Value> = calls_after["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["phase"] == "step")
        .collect();
    assert!(
        rows_after.len() <= 2,
        "取消后不得再有新节点启动(至多两个源节点留有中断留痕): {calls_after:?}"
    );
    assert!(
        rows_after
            .iter()
            .all(|c| c["step_index"].as_i64() != Some(2)),
        "未启动的合并节点不应留下调用行: {calls_after:?}"
    );
}
