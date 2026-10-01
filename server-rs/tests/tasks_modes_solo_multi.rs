// 任务模式集成测试 · solo / multi 两模式（含 legacy 子任务、截断自愈、多线程运行时）。
//
// 本文件由原 `tests/tasks.rs`（176KB / 65 用例）按 API 域拆分而来（QUALITY-FIX Q1-1,2026-09-27）：
// 纯移动、不改行为。**拆分的实质收益是测试隔离**——`tests/` 下每个 `.rs` 是一个独立测试进程，
// `build_test_app()` 的 `DATA_DIR`（`%TEMP%\kedai-test-<pid>`）与 `OnceLock` 单例 app 随
// 文件独立，原先「同一进程内共享 settings.json / 流程库」的顺序耦合由此消失。**别再把它们合回一个文件。**
//
// 测试辅助函数按「谁用谁带」复制（仓库既有体例：40 个测试文件里 36 个各自持有 `test_app`），
// 刻意不建 `tests/common` 共享层：那会让 count-tests 的集成文件数口径虚高，也会给多会话并行
// 改测试制造新的争用点，与本次拆分的用意相悖。
//
// 共用前提：`build_test_app()`（mock 连接器 + 临时数据目录 + 免鉴权）；mock 钩子 `[[reply:内容]]`
// 让规划器返回 JSON 计划、子智能体返回指定结果，实现确定性端到端。

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

/// 创建任务并返回 id
async fn create_task(app: &axum::Router, title: &str) -> String {
    let (status, json) = send_json(app, "POST", "/api/tasks", json!({ "title": title })).await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 轮询任务详情直到终态(done/partial/error/ended)或超时;返回最终 status 与详情
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

/// 创建指定模式的任务并返回 id
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

/// 轮询调用追踪直到满足条件或超时;返回最终 calls 数组
async fn wait_calls(app: &axum::Router, id: &str, pred: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    for _ in 0..50 {
        let (status, json) =
            send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let calls = json["calls"].as_array().cloned().unwrap_or_default();
        if pred(&calls) {
            return calls;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (_, json) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    panic!("任务 {id} 调用追踪未在超时内满足条件: {json:?}");
}

/// solo 模式:目标直接进 run_tool_loop(mock 不感知 tools,一轮即出),
/// 终态 done,结果 = mock reply;调用追踪含 phase="agent" 行。
#[tokio::test]
async fn task_solo_mode_runs_to_done() {
    let app = test_app();

    let id = create_task_with_mode(app, "[[reply:主agent最终成果]]", "solo").await;

    // create 透传模式:详情应为 solo
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(
        detail["task"]["task_mode"], "solo",
        "创建后模式应为 solo: {detail}"
    );

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "solo 任务应完成,详情: {detail}");
    assert_eq!(
        detail["task"]["result"].as_str().unwrap_or(""),
        "主agent最终成果",
        "结果应为 mock reply 文本: {detail}"
    );

    // 调用追踪:solo 经统一出口落 phase="agent" 行(token 为整轮累计)
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let calls = calls["calls"].as_array().unwrap();
    assert!(
        calls
            .iter()
            .any(|c| c["phase"] == "agent" && c["status"] == "ok"),
        "应存在 phase=agent 且 status=ok 的调用追踪行: {calls:?}"
    );
}

/// multi 模式:主 agent 经 [[tool:agentgo]] 钩子派子 agent——任务模式下子 agent 链路
/// 真实可用(task: 前缀虚拟 session 走内存覆盖层,子 agent 跑 run_tool_loop 白名单),
/// 终态 done;调用追踪含 phase=agent(主)与 phase=subagent(子)行,usage 聚合非零。
#[tokio::test]
async fn task_multi_mode_subagent_runs_to_done() {
    let app = test_app();

    // agentgo 参数内的 [[reply:...]] 用 \[ \] 转义:一是避免 mock tool 钩子在首个
    // "]]" 截断 JSON 参数;二是 agentgo 解析参数 JSON 时还原为真实钩子文本,
    // 使子 agent 的 user 消息(instruction)命中 mock reply 钩子。
    let title = r#"[[tool:agentgo {"tasks":[{"name":"子一","instruction":"\u005b\u005breply:子agent成果\u005d\u005d"}]}]] 主目标:调研并总结"#;
    let id = create_task_with_mode(app, title, "multi").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "multi 任务应完成,详情: {detail}");
    assert!(
        !detail["task"]["result"].as_str().unwrap_or("").is_empty(),
        "主 agent 应有最终产出: {detail}"
    );

    // 主 agent 调用追踪(phase=agent,工具循环一轮游)
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        calls["calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["phase"] == "agent" && c["status"] == "ok"),
        "应存在 phase=agent 且 status=ok 行: {calls:?}"
    );

    // 子 agent 后台执行,可能晚于主循环完成:轮询直至 phase=subagent 行出现
    let calls = wait_calls(app, &id, |cs| {
        cs.iter()
            .any(|c| c["phase"] == "subagent" && c["status"] == "ok")
    })
    .await;
    assert!(
        calls.iter().any(|c| c["phase"] == "subagent"),
        "子 agent 调用追踪应落库: {calls:?}"
    );

    // usage 落库(批次 4.3b 口径):主循环 phase=agent + 子 agent phase=subagent 均有 token
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    let usage = &detail["usage_total"];
    assert!(
        usage["prompt_tokens"].as_i64().unwrap_or(0) > 0
            && usage["completion_tokens"].as_i64().unwrap_or(0) > 0,
        "usage_total 应非零(主+子 agent 均已落库): {detail}"
    );
}

/// multi 取消:运行中 stop → ended(主循环走 mock 默认逐字流式回复,约 1s 窗口)。
#[tokio::test]
async fn task_multi_mode_stop_running() {
    let app = test_app();
    let id = create_task_with_mode(app, "慢慢写一篇长文,不要任何钩子", "multi").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    // 轮询到 running 再 stop
    let mut saw_running = false;
    for _ in 0..100 {
        let (_, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        if json["task"]["status"].as_str() == Some("running") {
            saw_running = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(saw_running, "应观测到 running 态再 stop");

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "running 态 stop 应 200");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "ended", "stop 后任务应 ended,详情: {detail}");
}

/// 问题⑤:multi 模式子 agent 记录走 task: 前缀内存覆盖层(不落 agent_subtasks 表),
/// 修复前 GET /api/tasks/{id} 的 subtasks 只查 task_subtasks 表 → 恒 [];
/// 修复后读路径合并覆盖层记录,子 agent 对 API 可见且字段映射正确。
/// 注意:覆盖层随进程存活,重启后 subtasks 仅 DB 行可见(既定取舍)。
#[tokio::test]
async fn task_multi_mode_subtasks_visible_from_memory_overlay() {
    let app = test_app();

    // 与 task_multi_mode_subagent_runs_to_done 同款钩子:主 agent 派一个子 agent,
    // 子 agent instruction 内嵌 \[ \] 转义的 reply 钩子,产出「子agent成果」。
    let title = r#"[[tool:agentgo {"tasks":[{"name":"子一","instruction":"\u005b\u005breply:子agent成果\u005d\u005d"}]}]] 主目标:调研并总结"#;
    let id = create_task_with_mode(app, title, "multi").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, _) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "multi 任务应完成");

    // 子 agent 后台执行,可能晚于主循环完成:轮询详情直至覆盖层子任务出现且 done
    let mut subtasks: Vec<Value> = Vec::new();
    for _ in 0..50 {
        let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        subtasks = detail["subtasks"].as_array().cloned().unwrap_or_default();
        if subtasks.iter().any(|s| s["status"] == "done") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !subtasks.is_empty(),
        "multi 子 agent 记录应对 API 可见(修复前恒 [])"
    );
    let sub = subtasks
        .iter()
        .find(|s| s["name"] == "子一")
        .expect("应存在名为「子一」的子任务");
    assert_eq!(
        sub["task_id"].as_str(),
        Some(id.as_str()),
        "task_id 应回填所属任务: {sub}"
    );
    assert!(
        sub["instruction"]
            .as_str()
            .unwrap_or("")
            .contains("reply:子agent成果"),
        "instruction 应映射覆盖层记录原文: {sub}"
    );
    assert_eq!(sub["status"].as_str(), Some("done"), "子任务应 done: {sub}");
    assert!(
        sub["result"].as_str().unwrap_or("").contains("子agent成果"),
        "result 应携带子 agent 产出: {sub}"
    );
    assert!(sub["created_at"].as_str().is_some(), "缺 created_at: {sub}");
    assert!(sub["updated_at"].as_str().is_some(), "缺 updated_at: {sub}");
    // 批次 4:终态时刻一并透出,调用方据 finished_at 判「何时完成」,
    // 不必再用 ended 兼表「完成后召回」与「中途中断」
    assert!(
        sub["finished_at"].as_str().is_some_and(|s| !s.is_empty()),
        "done 子任务应带非空 finished_at: {sub}"
    );
}

/// 问题⑤(回归):legacy 模式 subtasks 仍来自 task_subtasks 表(DB 路径),行为不变。
#[tokio::test]
async fn task_legacy_subtasks_still_from_db() {
    let app = test_app();

    let title = r#"[[reply:[{"name":"步骤一","goal":"写第一段"}] ]]"#;
    let id = create_task_with_mode(app, title, "legacy").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done");

    let subtasks = detail["subtasks"].as_array().unwrap();
    assert_eq!(subtasks.len(), 1, "legacy 每步一个 DB 子任务: {detail}");
    assert_eq!(subtasks[0]["name"].as_str(), Some("步骤一"));
    assert_eq!(subtasks[0]["status"].as_str(), Some("done"));
}

// ==================== 2026-08-31 实测修复:截断自愈 / 规划器侦察 / 步骤 error 文本 ====================

/// 问题①截断自愈:solo 任务经 mock [[tool_raw:]] 钩子模拟「max_tokens 把 tool_call
/// 参数 JSON 切成半截 + finish=length」(2026-08-31 deepseek 实测形态:calculator/
/// agentgo 参数截断直接判步骤 error)——run_tool_loop 单轮自愈:max_tokens 翻倍
/// (8192→16384,任务侧首轮预算 = TM-SET-1 起的任务缺省 8192)原样重发,第二轮预算充足
/// 返回完整 tool_call,工具正常执行,任务 done;
/// 调用追踪产生两次记录(被截断的 error 标注行 + 最终成功行)。
#[tokio::test]
async fn task_solo_truncated_tool_call_self_heals() {
    let app = test_app();

    // [[tool_raw:]] 钩子:max_tokens < 16384 返回左半 arguments(非法 JSON)+ finish=length;
    // ≥16384(自愈翻倍后)返回完整 tool_call。内容不得含 "]]"(首个 "]]" 截断语义)。
    let title = r#"[[tool_raw:calculator {"expression":"12*34"}]] 截断自愈目标"#;
    let id = create_task_with_mode(app, title, "solo").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "截断自愈后任务应完成,详情: {detail}");
    assert!(
        !detail["task"]["result"].as_str().unwrap_or("").is_empty(),
        "自愈后应有最终产出: {detail}"
    );

    // 调用追踪:phase=agent 恰好两行——被截断调用的 error 标注行 + 重发后的成功行
    let calls = wait_calls(app, &id, |cs| {
        cs.iter().filter(|c| c["phase"] == "agent").count() >= 2
    })
    .await;
    let agent_calls: Vec<&Value> = calls.iter().filter(|c| c["phase"] == "agent").collect();
    assert_eq!(
        agent_calls.len(),
        2,
        "应恰好两次调用记录(截断+重发): {calls:?}"
    );
    let heal_row = agent_calls
        .iter()
        .find(|c| c["status"] == "error")
        .unwrap_or_else(|| panic!("应有截断自愈标注行: {calls:?}"));
    let summary = heal_row["response_summary"].as_str().unwrap_or("");
    assert!(
        summary.contains("截断自愈") && summary.contains("工具参数 JSON 截断"),
        "截断行应标注自愈与成因: {summary}"
    );
    assert!(
        agent_calls.iter().any(|c| c["status"] == "ok"),
        "重发后应有成功行: {calls:?}"
    );
}

/// TM-EMPTY-1:截断自愈 + 收尾提醒两级都在场——[[finish:length|]] 钩子模拟「空正文 +
/// finish=length」:循环内自愈重发一次(预算翻倍)仍空 → 循环结束正文为空 → 收尾重试
/// 追加一次无工具提醒轮(预算按 length 分级抬升、温度保持)→ 提醒轮末条 user 已换、
/// 钩子不再命中,落到 mock 默认回复 → 任务 done。
/// 调用追踪三行(截断自愈标注行 + 空内容行 + 提醒轮成功行);对照修复前:
/// 同场景直接 error 终态(收尾重试救回,本用例即其回归锁)。
#[tokio::test]
async fn task_solo_truncation_heals_then_final_nudge_rescues() {
    let app = test_app();

    // [[finish:length|]]:内容为空串(标记 '|' 后即 "]]")→ 空正文 + finish=length
    let title = "[[finish:length|]] 空截断目标";
    let id = create_task_with_mode(app, title, "solo").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "收尾提醒轮应救回任务(done),详情: {detail}");
    assert!(
        !detail["task"]["result"].as_str().unwrap_or("").is_empty(),
        "结果应为提醒轮产出: {detail}"
    );

    // 调用追踪:截断自愈标注行(error)+ 空内容行(empty)+ 提醒轮成功行(ok)
    let calls = wait_calls(app, &id, |cs| {
        cs.iter().filter(|c| c["phase"] == "agent").count() >= 3
    })
    .await;
    let agent_calls: Vec<&Value> = calls.iter().filter(|c| c["phase"] == "agent").collect();
    assert_eq!(
        agent_calls.len(),
        3,
        "应恰好三条调用记录(自愈 + 首轮空 + 提醒轮): {calls:?}"
    );
    assert!(
        agent_calls.iter().any(|c| c["status"] == "error"
            && c["response_summary"]
                .as_str()
                .unwrap_or("")
                .contains("截断自愈")),
        "应有截断自愈标注行: {calls:?}"
    );
    assert!(
        agent_calls.iter().any(|c| c["status"] == "empty"),
        "应有空内容行: {calls:?}"
    );
    assert!(
        agent_calls.iter().any(|c| c["status"] == "ok"),
        "应有提醒轮成功行: {calls:?}"
    );
}

/// TM-EMPTY-1(无 length 形态):[[empty]] 钩子让首轮返回空文本(无 finish、无工具)——solo
/// 修复前直接判 Failed;修复后追加一次无工具提醒轮,提醒轮钩子不再命中(末条 user 换成
/// 提醒语)→ mock 默认回复救回任务。两行调用记录(空 + 成功),即「空回不再等于失败」。
#[tokio::test]
async fn task_solo_empty_final_answer_gets_nudge_once() {
    let app = test_app();

    let title = "[[empty]] 恒空首轮目标";
    let id = create_task_with_mode(app, title, "solo").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "提醒轮应救回任务: {detail}");
    assert!(
        !detail["task"]["result"].as_str().unwrap_or("").is_empty(),
        "结果应为提醒轮产出: {detail}"
    );

    let calls = wait_calls(app, &id, |cs| {
        cs.iter().filter(|c| c["phase"] == "agent").count() >= 2
    })
    .await;
    let agent_calls: Vec<&Value> = calls.iter().filter(|c| c["phase"] == "agent").collect();
    assert_eq!(
        agent_calls.len(),
        2,
        "首轮空 + 提醒轮成功 = 两行(有界 +1): {calls:?}"
    );
    assert!(
        agent_calls.iter().any(|c| c["status"] == "empty"),
        "应有空内容行: {calls:?}"
    );
    assert!(
        agent_calls.iter().any(|c| c["status"] == "ok"),
        "应有提醒轮成功行: {calls:?}"
    );
}

/// TM-EMPTY-1(提醒后仍空):[[empty_any]] 从**全量消息**命中——提醒语追加后原目标消息
/// 仍在数组内,故首轮与提醒轮都空。收尾重试**有界**(恰 +1 次),仍空走原错误路径:
/// error 终态、文案保留 finish_reason/输出上限/建议句(单一出处 empty_output_error);
/// 两行调用记录(空 + 空),不因修复放大重试。
#[tokio::test]
async fn task_solo_final_nudge_persistent_empty_stays_error() {
    let app = test_app();

    let title = "[[empty_any]] 恒空目标";
    let id = create_task_with_mode(app, title, "solo").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "error", "提醒后仍空应走原错误路径: {detail}");
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("返回空内容"),
        "错误文案应含「返回空内容」: {error}"
    );
    // 提交 3 · D6 的口径在收尾重试后不丢:输出上限与建议句仍在(且上限=重试所用值)
    assert!(
        error.contains("提高单次生成上限") && error.contains("当前输出上限"),
        "错误文案应含当前上限与建议动作: {error}"
    );

    let calls = wait_calls(app, &id, |cs| {
        cs.iter().filter(|c| c["phase"] == "agent").count() >= 2
    })
    .await;
    let agent_calls: Vec<&Value> = calls.iter().filter(|c| c["phase"] == "agent").collect();
    assert_eq!(
        agent_calls.len(),
        2,
        "恰 +1 次收尾重试(首轮空 + 仍空): {calls:?}"
    );
    assert!(
        agent_calls.iter().all(|c| c["status"] == "empty"),
        "两行都应为空内容: {calls:?}"
    );
}

/// 审计 B(端到端):multi 子 agent 被 max_tokens 截断 → 子任务 status=error、
/// error 含「截断」、result 保留半截正文。修复前只要正文非空就静默 done,是假成功主通道。
/// 钩子内容取到首个 "]]"(与 [[reply:]] 同截断语义),故内容内不得含 "]]"。
#[tokio::test]
async fn task_multi_mode_subagent_truncation_marks_error() {
    let app = test_app();

    // 子 agent instruction 内嵌 \[ \] 转义的 finish 钩子:一是避免主 agent 的 tool 钩子
    // 在首个 "]]" 截断参数 JSON;二是 agentgo 解析参数时还原为真实钩子文本,使子 agent
    // 的 user 消息(instruction)命中 mock finish 钩子 → finish_reason=length + 半截正文。
    let title = r#"[[tool:agentgo {"tasks":[{"name":"截断子","instruction":"\u005b\u005bfinish:length|半截成果在前\u005d\u005d"}]}]] 主目标:调研"#;
    let id = create_task_with_mode(app, title, "multi").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, _) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "主任务应完成(子任务失败不阻塞主循环)");

    // 子 agent 后台执行,可能晚于主循环:轮询详情直至子任务落到终态
    let mut sub: Option<Value> = None;
    for _ in 0..50 {
        let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        let subs = detail["subtasks"].as_array().cloned().unwrap_or_default();
        if let Some(s) = subs
            .iter()
            .find(|s| s["name"] == "截断子")
            .filter(|s| s["status"] != "pending" && s["status"] != "running")
        {
            sub = Some(s.clone());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let sub = sub.expect("截断子任务应出现在任务详情");
    assert_eq!(
        sub["status"].as_str(),
        Some("error"),
        "截断应记 error 而非 done: {sub}"
    );
    assert!(
        sub["error"].as_str().unwrap_or("").contains("截断"),
        "error 应含截断定性: {sub}"
    );
    assert!(
        sub["result"]
            .as_str()
            .unwrap_or("")
            .contains("半截成果在前"),
        "被截断的正文必须保留在 result: {sub}"
    );
}

/// 多线程 runtime 上的任务端到端 + 并发读不被落库阻塞(2026-09-16 性能批次 P-6)。
///
/// 覆盖缺口:本文件其余用例都是 `#[tokio::test]`(current_thread),而**生产是
/// multi-thread**——两者在 `utils::blocking::park_worker` 里走**不同分支**
/// (current_thread 直接执行,multi-thread 才 `block_in_place` 让出调度核心)。
/// 只在 current_thread 下测,等于没测到生产的落库路径,故这里显式用 multi_thread。
///
/// 断言两件事:
///   ① 任务仍能正常跑到 done(证明让出/归还调度核心没有破坏落库与事件链路);
///   ② 任务执行期间并发读持续成功(证明落库不再把 worker 卡死——这是 P-6 的目的)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_run_on_multi_thread_runtime_keeps_reads_alive() {
    let app = test_app();

    let title =
        r#"[[reply:[{"name":"步骤一","goal":"写第一段"},{"name":"步骤二","goal":"写第二段"}] ]]"#;
    let id = create_task(app, title).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    // ② 执行期间持续并发读:任何一次失败都说明 worker 被落库阻塞/饿死
    let mut reads = 0usize;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        // 并发发起 4 个读请求,验证读不被落库阻塞
        let mut handles = Vec::new();
        for _ in 0..4 {
            let app = app.clone();
            let id = id.clone();
            handles.push(tokio::spawn(async move {
                let (st, _) = send_json(&app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
                st
            }));
        }
        for h in handles {
            assert_eq!(
                h.await.unwrap(),
                StatusCode::OK,
                "任务执行期间并发读详情必须成功(worker 不应被落库阻塞)"
            );
            reads += 1;
        }

        let (st, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(st, StatusCode::OK);
        let ts = detail["task"]["status"].as_str().unwrap_or("");
        if ts == "done" || ts == "partial" || ts == "error" || ts == "ended" {
            // ① 终态正确
            assert_eq!(ts, "done", "multi-thread 下任务应正常完成: {detail}");
            assert!(
                !detail["task"]["result"].as_str().unwrap_or("").is_empty(),
                "结果非空: {detail}"
            );
            let plan = detail["task"]["plan"].as_array().unwrap();
            assert_eq!(plan.len(), 2);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "任务未在 20s 内到终态(疑似落库阻塞或死锁)"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(reads > 4, "应观察到多轮并发读,实际 {reads} 次");
}

// ===== 执行者库(2026-09-17:任务执行者与角色扮演角色卡解耦) =====
//
// 需求背景:任务创建下拉此前直接列角色扮演角色卡,两个模式的角色资产互相污染。
// 本组用例锁定三条契约:
//   ① 执行者库 CRUD 与落盘(独立于角色卡);
//   ② 任务按 executor_id 绑定执行者,执行时注入该执行者的指令段;
//   ③ 执行者与角色卡互斥:绑定执行者时不注入任何角色卡人设(解耦的核心断言)。
