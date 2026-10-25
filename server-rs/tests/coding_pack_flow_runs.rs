// 编码能力包流程「真跑通」集成测试(2026-09-30 CODE-5)。
//
// 为什么**单独一个文件**:本用例要把编码能力包**打开**并绑定包流程跑任务,而
// `coding_pack_flows.rs` 那条生命周期用例会删掉包流程——同一进程内两个用例共享全局
// 流程库与设置,并行跑必然互相踩(仓库既有裁定:共享全局状态的用例要么串行化、要么拆文件;
// 拆文件的额外收益是 DATA_DIR 独立)。本文件只做一件事:包流程能作为 custom 任务跑完。
//
// 这是「常量合法」(单元侧 `validate_flow` 断言)之外的**可执行**证据:结构合法但执行不了的
// 流程(成果节点选错、严格档末步卡住、reflect 步骤判定异常)只有真跑一遍才会暴露。

use axum::http::StatusCode;
use serde_json::json;
mod common;
use common::{send_json, test_app_with_prompt_isolation as test_app, wait_terminal};

/// 开包 → 绑定 `builtin-code-impl` 建 custom 任务 → 跑到底。
/// 断言:五步按线性拓扑序执行且全 done、结果取末个生成步骤的产出、
/// 调用追踪 phase=step 行数与步骤数对齐(mock 连接器不产生工具调用,故每步一次)。
#[tokio::test]
async fn pack_flow_runs_as_a_custom_task() {
    let app = test_app();

    // 开包(设置走 task 覆盖层,与设置页同一条路径)
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_coding_bundle_enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写设置失败: {r}");

    let title = "[[reply:实现流程统一产出]] 完成一个小改动";
    let (status, created) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": "custom", "flow_id": "builtin-code-impl" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "建任务应 201: {created}");
    let id = created["task"]["id"].as_str().unwrap().to_string();

    let (status, run) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {run}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "包流程任务应跑完: {detail}");

    let plan = detail["task"]["plan"].as_array().unwrap();
    let names: Vec<&str> = plan.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "阅读与计划",
            "按计划实现",
            "跑验证并修复",
            "复盘核验",
            "交付摘要"
        ],
        "plan 步骤名应与流程步骤一致(线性拓扑序 = 数组顺序)"
    );
    for step in plan {
        assert_eq!(step["status"], "done", "步骤应 done: {detail}");
    }
    assert!(
        detail["task"]["result"]
            .as_str()
            .unwrap_or("")
            .contains("实现流程统一产出"),
        "结果应取末个生成步骤的产出: {detail}"
    );

    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let step_calls = calls["calls"]
        .as_array()
        .map(|arr| arr.iter().filter(|c| c["phase"] == "step").count())
        .unwrap_or(0);
    assert_eq!(step_calls, 5, "每个流程步骤一次调用(含严格档末步): {calls}");
}
