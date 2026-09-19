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
