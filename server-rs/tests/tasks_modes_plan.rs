// 任务模式集成测试 · plan 模式（审批 / 续跑 / 计划会话 / approve_* 全族）。
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
use kedai_server::utils::test_support::TempDataDir;
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

/// 轮询任务详情直到出现目标状态或超时
async fn wait_status(app: &axum::Router, id: &str, target: &str) -> Value {
    for _ in 0..50 {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        if json["task"]["status"].as_str() == Some(target) {
            return json;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("任务 {id} 未在超时内到达状态 {target}");
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

/// 造一个 planned 态任务(2 步计划),返回任务 id
async fn planned_task(app: &axum::Router, title: &str) -> String {
    let id = create_task_with_mode(app, title, "plan").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    wait_status(app, &id, "planned").await;
    id
}

/// 标准 plan 标题:规划返回两步计划(各步 goal 内嵌快速钩子)+ 汇总钩子
fn plan_title_two_steps() -> &'static str {
    concat!(
        r#"[[reply:[{"name":"甲步","goal":"\u005b\u005breply:甲步成果\u005d\u005d 产出甲"},"#,
        r#"{"name":"乙步","goal":"\u005b\u005breply:乙步成果\u005d\u005d 产出乙"}] ]]"#,
        "[[reply_if:任务汇总者|计划续跑最终成果]]",
        " 计划总目标"
    )
}

/// 规划输出截断打捞:mock 规划器返回 2 个完整步骤对象 + 半截字符串
/// (2026-08-27 exe 实测「解析计划失败: EOF while parsing a string」的形态,
/// 推理模型 max_tokens 被 reasoning 耗尽导致 JSON 尾部缺失),
/// parse_plan 应打捞恢复完整步骤并跑到 done,而不是整个任务报错。
#[tokio::test]
async fn task_plan_truncated_json_salvaged() {
    let app = test_app();

    // 截断形态:第三个对象只有半个字符串;内容不含 "]]",不会触发 mock reply 钩子提前截断。
    let title = r#"[[reply:[{"name":"收集意象","goal":"收集秋天意象"}, {"name":"写初稿","goal":"写出初稿"}, {"name":"润色","goal":"润 ]]"#;
    let id = create_task(app, title).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "截断计划打捞后任务应完成,详情: {detail}");

    // 打捞恢复 2 步(半截的第三步丢弃),均执行 done
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "应打捞 2 个完整步骤: {plan:?}");
    for step in plan {
        assert_eq!(step["status"], "done");
    }
    assert!(!detail["task"]["result"].as_str().unwrap().is_empty());
}

// ==================== 批次 4.2/4.3a:六模式(solo / plan / approve) ====================

/// plan 模式:run 只规划不执行(零副作用)→ planned 待批准;
/// approve(不带 plan)后按已批准计划逐步骤续跑 → done(批次 4.3 回归:
/// approve 不得按 task_mode 重派回规划器,否则再规划一遍回到 planned 死循环)。
/// planned 态 run 重入与非 planned 态 approve 均 400。
#[tokio::test]
async fn task_plan_mode_waits_for_approval_then_executes_plan() {
    let app = test_app();

    let title =
        r#"[[reply:[{"name":"步骤一","goal":"写第一段"},{"name":"步骤二","goal":"写第二段"}] ]]"#;
    let id = create_task_with_mode(app, title, "plan").await;

    // 非 planned 态 approve 应 400(任务尚未规划)
    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "pending 态 approve 应 400: {json}"
    );

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    // 只规划:到达 planned 待批准态,计划 2 步,零副作用(无子任务)
    let mut detail = wait_status(app, &id, "planned").await;
    assert_eq!(
        detail["task"]["plan"].as_array().unwrap().len(),
        2,
        "计划应拆为 2 步: {detail}"
    );
    assert!(
        detail["subtasks"].as_array().unwrap().is_empty(),
        "plan 模式不得执行任何步骤(零副作用): {detail}"
    );
    // 批次 R1:planned 态 result = 待批准的计划清单(批准前预览,含各步骤名)。
    // 由 run_inner 的 AwaitApproval 分支在状态置 planned 之后落库,须轮询等待写入
    for _ in 0..50 {
        if !detail["task"]["result"].as_str().unwrap_or("").is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (_, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        detail = json;
    }
    let planned_result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        planned_result.contains("计划已产出,共 2 步"),
        "planned 态 result 应为待批准的计划清单: {planned_result}"
    );
    assert!(
        planned_result.contains("步骤一") && planned_result.contains("步骤二"),
        "planned 态 result 应含各计划步骤名: {planned_result}"
    );

    // planned 态拒绝 run 重入(提示走 approve)
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "planned 态 run 应 400: {json}"
    );
    assert!(
        json["error"].as_str().unwrap_or("").contains("approve"),
        "错误文案应提示走 approve: {json}"
    );

    // 批准(不带 plan,按已产出计划原样批准)→ 按计划逐步骤续跑 → done
    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "approve 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "批准后按计划续跑应完成,详情: {detail}");
    assert!(!detail["task"]["result"].as_str().unwrap_or("").is_empty());
    // 续跑消费已批准计划:每步推进到 done 且各有 result(不得残留 pending)
    for step in detail["task"]["plan"].as_array().unwrap() {
        assert_eq!(step["status"], "done", "续跑后步骤应 done: {detail}");
        assert!(
            !step["result"].as_str().unwrap_or("").is_empty(),
            "续跑后步骤应有 result: {detail}"
        );
    }
}

/// plan 模式:approve 携修改后计划 → 计划被整体替换,续跑执行的是修改版计划。
#[tokio::test]
async fn task_plan_approve_with_modified_plan_replaces() {
    let app = test_app();

    let title =
        r#"[[reply:[{"name":"步骤一","goal":"写第一段"},{"name":"步骤二","goal":"写第二段"}] ]]"#;
    let id = create_task_with_mode(app, title, "plan").await;

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let detail = wait_status(app, &id, "planned").await;
    assert_eq!(detail["task"]["plan"].as_array().unwrap().len(), 2);

    // 携修改后计划批准:2 步 → 1 步(goal 内嵌 [[reply:]] 钩子让该步产出确定文本;
    // 标题内首个 [[reply:]] 钩子会被 mock 汇总轮吃到并回显原始规划 JSON,故「修改版生效」
    // 一律按 plan 数组断言,不按 result 文本断言)
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/approve"),
        json!({ "plan": [{ "name": "改写步", "goal": "[[reply:修改版产出]] 产出修改版" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "approve 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "批准后按计划续跑应完成,详情: {detail}");
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 1, "计划应被替换为修改后的 1 步: {detail}");
    assert_eq!(
        plan[0]["name"], "改写步",
        "计划内容应为批准时携带的版本: {detail}"
    );
    assert_eq!(
        plan[0]["status"], "done",
        "修改版步骤应被执行到 done: {detail}"
    );

    // 提交 2:续跑 result 只放成果(汇总文本),步骤信息不再重复拼进 result——
    // 前端「计划步骤」列表已按 task.plan 渲染同一份结构化数据(含每步 result)。
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        !result.contains("## 最终计划"),
        "result 不得再拼「## 最终计划」段(与计划步骤区重复): {result}"
    );
}

/// plan 批准续跑消费已批准计划(2026-08 真实模型实测修复):approve 后逐步执行——
/// 每步独立 run_agent_loop(步骤 name+goal 为该步指令,整体目标作上下文),完成即
/// 回写该步 status=done/result;全部步骤完成后 SUMMARIZER_PROMPT 汇总产出最终
/// result(对齐 legacy 执行段语义)。旧实现强制 solo 只吃 goal,已批准计划仅落库
/// 存档:实测续跑后 4 个步骤永远 pending、step result 全空,任务却 done。
#[tokio::test]
async fn task_plan_approve_resume_executes_plan_steps() {
    let app = test_app();

    // 步骤 goal 内嵌 \[ \] 转义的 reply 钩子:规划 JSON 经 serde 解析还原为真实钩子,
    // 续跑时各步骤 user 消息(当前步骤在前)首个 [[reply: 即本步钩子 → 步骤专属产出。
    let title = concat!(
        r#"[[reply:[{"name":"步骤一","goal":"\u005b\u005breply:步骤一成果\u005d\u005d 写第一段"},"#,
        r#"{"name":"步骤二","goal":"\u005b\u005breply:步骤二成果\u005d\u005d 写第二段"}] ]]"#,
        "[[reply_if:任务汇总者|计划续跑最终成果]]",
        " 计划总目标"
    );
    let id = create_task_with_mode(app, title, "plan").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let detail = wait_status(app, &id, "planned").await;
    assert_eq!(
        detail["task"]["plan"].as_array().unwrap().len(),
        2,
        "计划应拆为 2 步: {detail}"
    );

    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "approve 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "批准续跑应完成,详情: {detail}");

    // 逐步执行:每步 done 且各有专属 result(旧实现步骤永远 pending、result 全空)
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "计划仍为 2 步: {detail}");
    assert_eq!(plan[0]["status"], "done", "步骤一应 done: {detail}");
    assert_eq!(plan[1]["status"], "done", "步骤二应 done: {detail}");
    let r0 = plan[0]["result"].as_str().unwrap_or("");
    let r1 = plan[1]["result"].as_str().unwrap_or("");
    assert!(r0.contains("步骤一成果"), "步骤一应有自身产出: {r0}");
    assert!(r1.contains("步骤二成果"), "步骤二应有自身产出: {r1}");

    // 最终 result = SUMMARIZER_PROMPT 汇总产出
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("计划续跑最终成果"),
        "最终结果应为汇总产出: {result}"
    );

    // 调用追踪:每步一行 phase=agent(step_index=步骤下标)+ 汇总行
    //(phase=summarize,复用 legacy summarize_task_retry 的既有口径)
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let calls = calls["calls"].as_array().unwrap();
    let agent_calls: Vec<&Value> = calls.iter().filter(|c| c["phase"] == "agent").collect();
    assert_eq!(agent_calls.len(), 2, "每步一次 agent 调用: {calls:?}");
    assert!(
        agent_calls.iter().any(|c| c["step_index"] == 0)
            && agent_calls.iter().any(|c| c["step_index"] == 1),
        "agent 调用 step_index 应为步骤下标: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c["phase"] == "summarize" && c["status"] == "ok"),
        "应有汇总调用行: {calls:?}"
    );

    // F4(2026-09-10 实测修复):续跑 goal 显式要求「以已批准计划为准」,
    // 防止用户经 plan-chat 改过步数后,汇总仍沿用原始目标里的旧步数描述。
    assert!(
        agent_calls.iter().any(|c| c["prompt_summary"]
            .as_str()
            .unwrap_or("")
            .contains("步骤数量与内容一律以本清单为准")),
        "续跑 goal 应含「以已批准计划为准」约束: {calls:?}"
    );
}

/// 提交 2:plan 批准续跑完成的 result **只含汇总文本**,不再拼「## 最终计划」段
/// (批次 R1 的旧契约)。理由:那一段是「各步名称/状态/result 概要」的文本副本,
/// 而前端已按 task.plan 渲染同一份数据(计划步骤区,含每步 result)——同一份信息
/// 不该在文本与结构里各存一份(写小说场景下用户拿到的是「正文 + 模式记账」)。
/// 本用例锁定「段已移除」+「信息仍在 plan 结构里」。
#[tokio::test]
async fn task_plan_resume_result_is_summary_only() {
    let app = test_app();

    // 钩子布局同 task_plan_approve_resume_executes_plan_steps:规划器吃首个 [[reply:]]
    // 出计划;各步骤 goal 内嵌转义 [[reply:]] 出步骤专属产出;汇总轮吃 reply_if(任务汇总者)。
    let title = concat!(
        r#"[[reply:[{"name":"步骤一","goal":"\u005b\u005breply:步骤一成果\u005d\u005d 写第一段"},"#,
        r#"{"name":"步骤二","goal":"\u005b\u005breply:步骤二成果\u005d\u005d 写第二段"}] ]]"#,
        "[[reply_if:任务汇总者|批次R1汇总成果]]",
        " 最终计划段总目标"
    );
    let id = create_task_with_mode(app, title, "plan").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    wait_status(app, &id, "planned").await;

    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "approve 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "批准续跑应完成,详情: {detail}");

    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("批次R1汇总成果"),
        "result 应含汇总产出: {result}"
    );
    assert!(
        !result.contains("## 最终计划"),
        "result 不得再拼「## 最终计划」段(前端已有步骤列表): {result}"
    );
    // 信息不丢:各步名称/状态/产出一律由 plan 结构承载(前端计划步骤区渲染)
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2);
    assert!(plan.iter().all(|s| s["status"] == "done"), "{detail}");
    assert_eq!(plan[0]["name"], "步骤一");
    assert_eq!(
        plan[0]["result"].as_str().unwrap_or(""),
        "步骤一成果",
        "步骤产出应落在 plan 结构里: {detail}"
    );
    assert_eq!(plan[1]["result"].as_str().unwrap_or(""), "步骤二成果");
}

/// plan 批准续跑汇总失败 + 各步已产出(提交 2 部分成果兜底):汇总失败不再整体丢成果 →
/// partial + result = 各步产出的确定性拼装(「## 步骤名」+ 正文),error 保留失败原因;
/// 且拼装文本同样不带「## 最终计划」段。
#[tokio::test]
async fn task_plan_resume_summary_failure_falls_back_to_step_outputs() {
    let app = test_app();

    // 步骤 goal 内嵌转义 [[reply:]] 出确定产出;汇总 system 含「任务汇总者」→ 空输出 →
    // 分级重试后仍空 → 走降级拼装(规划器/步骤 system 不含该子串,不受影响)。
    let title = concat!(
        r#"[[reply:[{"name":"步骤一","goal":"\u005b\u005breply:步骤一成果\u005d\u005d 写第一段"},"#,
        r#"{"name":"步骤二","goal":"\u005b\u005breply:步骤二成果\u005d\u005d 写第二段"}] ]]"#,
        "[[empty_if:任务汇总者]]",
        " 汇总失败计划目标"
    );
    let id = create_task_with_mode(app, title, "plan").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    wait_status(app, &id, "planned").await;

    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "approve 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "partial",
        "汇总失败但各步已产出应部分完成(成果不得丢),详情: {detail}"
    );

    let plan = detail["task"]["plan"].as_array().unwrap();
    for step in plan {
        assert_eq!(step["status"], "done", "各步应执行成功: {detail}");
    }
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("## 步骤一") && result.contains("步骤一成果"),
        "result 应含第一步产出: {result}"
    );
    assert!(
        result.contains("## 步骤二") && result.contains("步骤二成果"),
        "result 应含第二步产出: {result}"
    );
    assert!(
        !result.contains("## 最终计划"),
        "降级拼装同样不得带最终计划段: {result}"
    );
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("空内容") && error.contains("finish_reason"),
        "error 应保留汇总失败原因,实际: {error}"
    );
}

/// 含失败步骤时终态 partial(对齐 legacy「有产出则 partial」语义),失败信息落在
/// **plan 结构**里(每步 status=error + result=失败原因),result 只放汇总文本。
/// 注:ApprovedPlanExecutor 每步 user 消息都携带完整计划上下文,[[fail:]]/[[empty]]
/// 等 user 侧无条件钩子会毒化全部步骤;[[empty_if:任务执行者]] 只匹配步骤执行的
/// system 提示词(EXECUTOR_PROMPT),规划器(任务规划器)/汇总器(任务汇总者)不命中,
/// 是 mock 下可精确只让步骤失败的钩子(mock 条件钩子仅匹配 system 消息)。
#[tokio::test]
async fn task_plan_resume_error_step_marks_partial_in_plan() {
    let app = test_app();

    // 钩子布局:规划器吃首个 [[reply:]] 出计划;步骤轮 system 含「任务执行者」
    // → empty_if 命中返回空内容,run_agent_loop 报错「步骤 N返回空内容」;
    // 汇总轮吃 reply_if(任务汇总者)出确定性汇总文本。
    let title = concat!(
        r#"[[reply:[{"name":"步骤一","goal":"写第一段"},"#,
        r#"{"name":"步骤二","goal":"写第二段"}] ]]"#,
        "[[reply_if:任务汇总者|有失败步的汇总成果]]",
        "[[empty_if:任务执行者]]",
        " 失败步计划目标"
    );
    let id = create_task_with_mode(app, title, "plan").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    wait_status(app, &id, "planned").await;

    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "approve 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "partial", "含失败步骤应部分完成,详情: {detail}");

    // 失败信息在 plan 结构里:两步 error、原因带「返回空内容」
    let plan = detail["task"]["plan"].as_array().unwrap();
    for step in plan {
        assert_eq!(step["status"], "error", "步骤应失败: {detail}");
        assert!(
            step["result"].as_str().unwrap_or("").contains("返回空内容"),
            "步骤 result 应带失败原因: {detail}"
        );
    }
    // result 只放汇总文本(汇总成功即产出成果):不再拼步骤概要段
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(result.contains("有失败步的汇总成果"), "{result}");
    assert!(
        !result.contains("## 最终计划"),
        "result 不得再拼步骤概要段: {result}"
    );
}

/// plan 批准续跑取消:规划器即时产出计划后,步骤执行走 mock 默认逐字流式回复
///(约 1s/步窗口),running 中 stop → ended;终态后剩余 pending 步骤统一置 error
///「任务已停止」(与 team 口径对齐,不留永远 pending/running 的步骤)。
#[tokio::test]
async fn task_plan_approve_resume_stop_marks_remaining_steps_error() {
    let app = test_app();

    // 规划钩子用 reply_if(仅匹配规划器 system 提示词「任务规划器」):步骤执行的
    // system 提示词不含该子串,故步骤调用落空到默认慢速流式回复,留出 stop 窗口
    let title = concat!(
        r#"[[reply_if:任务规划器|[{"name":"步骤一","goal":"慢慢写第一段"},{"name":"步骤二","goal":"慢慢写第二段"}] ]]"#,
        " 计划取消目标"
    );
    let id = create_task_with_mode(app, title, "plan").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let detail = wait_status(app, &id, "planned").await;
    assert_eq!(
        detail["task"]["plan"].as_array().unwrap().len(),
        2,
        "计划应拆为 2 步: {detail}"
    );

    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "approve 应 200: {json}");

    // 轮询到续跑 running(步骤一执行中)再 stop
    let mut saw_running = false;
    for _ in 0..100 {
        let (_, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        if json["task"]["status"].as_str() == Some("running") {
            saw_running = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(saw_running, "应观测到续跑 running 态再 stop");

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "running 态 stop 应 200");
    let (st, _) = wait_terminal(app, &id).await;
    assert_eq!(st, "ended", "stop 后任务应 ended");

    // 宽限收尾后:步骤不得残留 pending/running(剩余 pending 应被置 error「任务已停止」)
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    for step in detail["task"]["plan"].as_array().unwrap() {
        let s = step["status"].as_str().unwrap_or("");
        assert!(
            s == "done" || s == "error",
            "终态后步骤不得残留 pending/running(取消兜底须置 error): {detail}"
        );
    }
}

/// 提交 2:plan 续跑**取消**时,已完成步骤的产出落进 result(而非留着 planned 态的
/// 计划清单预览)——批准即让预览失效(`executor.rs::approve` 的 clear_result),
/// 否则成果卡会把「计划」当成「已完成部分的成果」展示(2026-09-26 真模型 plan/写作
/// 实测命中:某步已完成 560 字,result 里仍是计划清单)。
#[tokio::test]
async fn task_plan_resume_stop_keeps_done_step_output() {
    let app = test_app();

    // 规划器用 [[reply_if:任务规划器|…]] (只命中规划器 system):步骤的 user 消息里
    // 携带整份已批准计划(含各步 goal),任何 user 侧无条件钩子都会毒化全部步骤——
    // 步骤统一落回 mock 默认逐字流式回复(约 8ms/字符 ≈ 1s/步),留出 stop 窗口。
    let title = concat!(
        r#"[[reply_if:任务规划器|[{"name":"甲步","goal":"写第一段"},"#,
        r#"{"name":"乙步","goal":"写第二段"},{"name":"丙步","goal":"写第三段"}] ]]"#,
        " 计划取消目标"
    );
    let id = create_task_with_mode(app, title, "plan").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let planned = wait_status(app, &id, "planned").await;
    // planned 态 result = 待批准的计划清单(预览语义)
    assert!(
        planned["task"]["result"]
            .as_str()
            .unwrap_or("")
            .contains("计划已产出"),
        "planned 态 result 应是计划清单预览: {planned}"
    );

    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "approve 应 200: {json}");
    // 批准即清空预览(结构性判据:预览不再可能被当成成果)
    let (_, after_approve) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert!(
        after_approve["task"]["result"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
        "批准后 planned 预览应清空: {after_approve}"
    );

    // 轮询到甲步 done 且任务仍在跑 → stop
    let mut stop_ready = false;
    let mut last = Value::Null;
    for _ in 0..200 {
        let (_, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        let st = json["task"]["status"].as_str().unwrap_or("");
        last = json.clone();
        if json["task"]["plan"][0]["status"].as_str() == Some("done")
            && matches!(st, "running" | "planning")
        {
            stop_ready = true;
            break;
        }
        if matches!(st, "done" | "partial" | "error" | "ended") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        stop_ready,
        "应观测到甲步 done 且任务仍在跑再 stop,最后观测: {last}"
    );

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "running 态 stop 应 200");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "ended", "stop 后任务应 ended,详情: {detail}");

    let result = detail["task"]["result"].as_str().unwrap_or("");
    let step0 = detail["task"]["plan"][0]["result"].as_str().unwrap_or("");
    assert!(!step0.is_empty(), "甲步应有产出: {detail}");
    assert!(
        result.contains("## 甲步") && result.contains(step0),
        "取消后 result 应是已完成步骤的拼装成果: {result}"
    );
    assert!(
        !result.contains("计划已产出"),
        "result 不得残留 planned 态的计划清单预览: {result}"
    );
}

/// 问题①截断自愈(plan 批准续跑路径,实测原发形态):计划步骤 goal 内嵌转义的
/// [[tool_raw:]] 钩子 → approve 续跑步骤 agent 调用首轮半截 tool_call + finish=length
/// → 单轮自愈翻倍重发 → 步骤成功 done,任务终态 done。
#[tokio::test]
async fn task_plan_resume_step_truncation_self_heals() {
    let app = test_app();

    // 规划钩子用 reply_if(仅匹配规划器 system「任务规划器」):步骤执行 system 不含该子串;
    // 步骤 goal 内 \[ \] 转义的 [[tool_raw:...]] 经计划 JSON 解析还原为真实钩子
    // (内层 JSON 引号须 \" 转义;内容不得含 "]]")。
    let title = concat!(
        r#"[[reply_if:任务规划器|[{"name":"计算步","goal":"\u005b\u005btool_raw:calculator {\"expression\":\"12*34\"}\u005d\u005d 算一下乘法"}] ]]"#,
        "[[reply_if:任务汇总者|续跑截断自愈成果]]",
        " 续跑截断自愈目标"
    );
    let id = create_task_with_mode(app, title, "plan").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let detail = wait_status(app, &id, "planned").await;
    assert_eq!(
        detail["task"]["plan"].as_array().unwrap().len(),
        1,
        "计划应为 1 步: {detail}"
    );

    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "approve 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "截断自愈后续跑应完成,详情: {detail}");

    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan[0]["status"], "done", "截断步骤应自愈成功: {detail}");
    // 调用追踪:phase=agent 两行(截断 error 标注行 + 成功行)
    let calls = wait_calls(app, &id, |cs| {
        cs.iter().filter(|c| c["phase"] == "agent").count() >= 2
    })
    .await;
    let agent_calls: Vec<&Value> = calls.iter().filter(|c| c["phase"] == "agent").collect();
    assert_eq!(
        agent_calls.len(),
        2,
        "续跑步骤应产生两次调用记录: {calls:?}"
    );
    assert!(
        agent_calls.iter().any(|c| c["status"] == "error"
            && c["response_summary"]
                .as_str()
                .unwrap_or("")
                .contains("截断自愈")),
        "应有截断自愈标注行: {calls:?}"
    );
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("续跑截断自愈成果"),
        "最终结果应为汇总产出: {result}"
    );
}

/// 问题②规划器只读侦察:plan 模式 run 的规划阶段,模型可先调只读工具收集信息
/// (mock 首轮 [[tool:read]] 命中 → 侦察轮,工具结果回填),次轮产出计划 JSON
/// (has_tool_result 分支的回复钩子守卫,[[reply:计划]] 生效);
/// 侦察轮与计划轮均落 task_llm_calls(phase=planner),计划解析契约不变
///(进 planned 待批准,计划内容 = mock 返回的 JSON)。
#[tokio::test]
async fn task_plan_mode_planner_scout_collects_then_plans() {
    let app = test_app();

    // 首轮:[[tool:read]] 命中(read 在规划器只读白名单内)→ 侦察轮;
    // 次轮:工具结果回填后 has_tool_result 分支回复守卫命中 [[reply:计划]]。
    // read 的文件不存在 → 工具返回错误文本回填(侦察失败不致命,计划照常产出)。
    let title = r#"[[tool:read {"type":"file","name":"notes.md"}]] [[reply:[{"name":"侦察步","goal":"基于收集的信息写作"}] ]] 侦察规划目标"#;
    let id = create_task_with_mode(app, title, "plan").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let detail = wait_status(app, &id, "planned").await;
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 1, "计划解析契约不变(1 步): {detail}");
    assert_eq!(
        plan[0]["name"], "侦察步",
        "计划内容应为 mock 回复的 JSON: {detail}"
    );

    // 侦察轮 + 计划轮均落调用追踪(phase=planner 恰好 2 行,均 status=ok)
    let calls = wait_calls(app, &id, |cs| {
        cs.iter().filter(|c| c["phase"] == "planner").count() >= 2
    })
    .await;
    let planner_calls: Vec<&Value> = calls.iter().filter(|c| c["phase"] == "planner").collect();
    assert_eq!(
        planner_calls.len(),
        2,
        "侦察轮与计划轮应各落一行 planner 调用: {calls:?}"
    );
    for c in &planner_calls {
        assert_eq!(c["status"], "ok", "侦察/计划轮均应成功: {c}");
    }
    // 计划轮(末行)的响应应为计划 JSON
    let last = planner_calls.last().unwrap();
    assert!(
        last["response_summary"]
            .as_str()
            .unwrap_or("")
            .contains("侦察步"),
        "计划轮应产出计划 JSON: {last}"
    );
    // F6(2026-09-10 实测修复):侦察轮(带工具调用)的 finish_reason 应为 tool_calls,
    // 此前因 sse_parser 在 tool_calls 完成时不发 Finish 块而落空串。
    let scout = planner_calls.first().unwrap();
    assert_eq!(
        scout["finish_reason"].as_str(),
        Some("tool_calls"),
        "带工具调用的侦察轮 finish_reason 应为 tool_calls: {scout}"
    );
}

/// TM-SCOUT-1 D1(b):绑定工作区的 plan 任务,规划器**输入**含系统侧预取的
/// 「工作区侦察快照」段(user 消息 · 目标段之后,prompt_summary 可观测):
/// 真实目录清单 + 入口文档摘要 + 项目类型,机器说明句在 untrusted 包裹外、
/// 采集正文经包裹(source=workspace_snapshot);未绑定任务的 scratch 空目录
/// 注入「当前无可见文件」形态(不假装有内容,计划照常产出)。
#[tokio::test]
async fn task_plan_mode_planner_gets_workspace_scout_snapshot() {
    let app = test_app();
    let ws_guard = TempDataDir::new("tm-scout1-snap");
    std::fs::write(ws_guard.path().join("README.md"), "快照取证入口文档").unwrap();
    std::fs::write(ws_guard.path().join("package.json"), "{}").unwrap();
    std::fs::create_dir_all(ws_guard.path().join("src")).unwrap();
    std::fs::write(
        ws_guard.path().join("src/main.mjs"),
        "export const x = 1;\n",
    )
    .unwrap();

    let title =
        r#"[[reply_if:任务规划器|[{"name":"快照步","goal":"基于快照规划"}] ]] 快照规划目标"#;
    let (status, created) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": title,
            "task_mode": "plan",
            "workspace": ws_guard.path().to_string_lossy(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务失败: {created}");
    let id = created["task"]["id"].as_str().unwrap().to_string();
    let (status, r) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {r}");
    wait_status(app, &id, "planned").await;

    let calls = wait_calls(app, &id, |cs| cs.iter().any(|c| c["phase"] == "planner")).await;
    let first = calls
        .iter()
        .find(|c| c["phase"] == "planner")
        .expect("应有 planner 调用行");
    let ps = first["prompt_summary"].as_str().unwrap_or("");
    // 五要素:段标头、untrusted 包裹与来源名、可见清单、入口文档摘要、项目类型行
    for needle in [
        "【工作区侦察快照】",
        r#"source="workspace_snapshot""#,
        "- README.md",
        "- src/",
        "- main.mjs",
        "入口文档摘要:",
        "- README.md: 快照取证入口文档",
        "项目类型: node(package.json → npm test)",
    ] {
        assert!(ps.contains(needle), "规划器输入应含「{needle}」: {ps}");
    }

    // 未绑定任务 = scratch 空目录:注入「当前无可见文件」形态,计划照常产出
    let title2 =
        r#"[[reply_if:任务规划器|[{"name":"空目录步","goal":"照常出计划"}] ]] 空目录规划目标"#;
    let id2 = create_task_with_mode(app, title2, "plan").await;
    let (status, r) = send_json(app, "POST", &format!("/api/tasks/{id2}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {r}");
    wait_status(app, &id2, "planned").await;
    let calls2 = wait_calls(app, &id2, |cs| cs.iter().any(|c| c["phase"] == "planner")).await;
    let ps2 = calls2
        .iter()
        .find(|c| c["phase"] == "planner")
        .and_then(|c| c["prompt_summary"].as_str())
        .unwrap_or("");
    assert!(ps2.contains("【工作区侦察快照】"), "{ps2}");
    assert!(
        ps2.contains("当前无可见文件"),
        "空 scratch 应有显式留痕: {ps2}"
    );
}

/// 问题③步骤 error 文本落库(plan 批准续跑):步骤 agent 调用失败(mock [[fail:]]
/// 钩子模拟上游故障)→ 该步 status=error 且 result 携带失败原因(与 status 同一次
/// set_plan 落库),不得只置状态不留文本;步骤全败但汇总仍产出 → 任务终态 partial。
///
/// 剧本说明(2026-09-02 修正):approve 续跑的每步 user 消息 = 当前步骤段 + 「整体目标
/// 与已批准计划」上下文段,计划段携带全部步骤 goal(含他步还原后的钩子文本),而 mock
/// [[fail:]] 按 last_user 全文匹配且优先级最高——「一成一败」剧本不可构造(成功步必然
/// 被计划段的他步 fail 钩子命中)。故本用例设计为两步皆败、各携不同错误文案:
/// extract_fail_marker 取首个出现,而当前步骤段排在计划段之前,每步命中的恰是本步
/// 自己的钩子——可分别断言各步 result 携带本步失败原因(而非串味成他步的)。
///「一成一败 → partial」语义由 team 用例(task_team_mode_failed_main_step_carries_reason_text)
/// 覆盖:team 各主 user 不含他主子目标原文,天然免疫该污染。
#[tokio::test]
async fn task_plan_resume_step_error_carries_reason_text() {
    let app = test_app();

    // 两步 goal 内 \[ \] 转义的 [[fail:...]]:经计划 JSON 还原后在各自步骤 user 消息
    // 的当前步骤段命中(不经截断自愈:「模拟上游故障」非截断形态,直接走原错误路径)
    let title = concat!(
        r#"[[reply_if:任务规划器|[{"name":"步骤甲","goal":"\u005b\u005bfail:模拟上游故障甲\u005d\u005d 写一段"},"#,
        r#"{"name":"步骤乙","goal":"\u005b\u005bfail:模拟上游故障乙\u005d\u005d 写另一段"}] ]]"#,
        "[[reply_if:任务汇总者|续跑部分成果]]",
        " 步骤错误文本目标"
    );
    let id = create_task_with_mode(app, title, "plan").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    wait_status(app, &id, "planned").await;
    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "approve 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "partial",
        "步骤失败但汇总产出应为 partial,详情: {detail}"
    );
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "计划应为 2 步: {detail}");
    for (i, marker) in ["模拟上游故障甲", "模拟上游故障乙"].iter().enumerate() {
        assert_eq!(
            plan[i]["status"],
            "error",
            "步骤 {} 应 error: {detail}",
            i + 1
        );
        let err_text = plan[i]["result"].as_str().unwrap_or("");
        assert!(
            err_text.contains(marker),
            "步骤 {} error 文本应携带本步失败原因(问题③),实际: {err_text}",
            i + 1
        );
    }
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("续跑部分成果"),
        "最终结果应为汇总产出: {result}"
    );
}

/// plan-chat 修订全链路:planned 态用户反馈 → 规划器携带(原始目标 + 当前计划
/// JSON + plan_chat 对话历史)重新产出修订计划 → set_plan 替换 + planned 态
/// result 计划清单文本同步刷新 + assistant 修订说明落 task_messages →
/// 任务保持 planned;第二轮反馈时历史随提示词携带(经 GET calls 的
/// prompt_summary 断言历史出现在规划器入参)。
/// mock 剧本:title 双 reply_if 钩子按序区分轮次(reply_if 只匹配 system,
/// title 在 user 消息,钩子自身出现不算命中):
/// 修订轮 system = PLANNER_PROMPT + 修订指引段(含独特词「计划修订指引」)
///   → 钩子1 命中 → 计划B;
/// 首轮规划 system 仅 PLANNER_PROMPT(含「任务规划器」,不含修订指引)
///   → 钩子1 跳过、钩子2 命中 → 计划A。
#[tokio::test]
async fn task_plan_chat_revises_plan_and_keeps_planned() {
    let app = test_app();

    let title = concat!(
        r#"[[reply_if:计划修订指引|[{"name":"修订步骤甲","goal":"修订目标甲"}] ]]"#,
        r#"[[reply_if:任务规划器|[{"name":"原步骤一","goal":"原目标一"}] ]]"#,
        " 请帮我写一份调研报告"
    );
    let id = create_task_with_mode(app, title, "plan").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    wait_status(app, &id, "planned").await;

    // 首轮:计划A 落库,planned 态 result = 计划清单文本,messages 为空
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(detail["task"]["status"], "planned");
    assert_eq!(
        detail["task"]["plan"][0]["name"], "原步骤一",
        "首轮应为计划A: {detail}"
    );
    assert!(
        detail["task"]["result"]
            .as_str()
            .unwrap_or("")
            .contains("计划已产出"),
        "planned 态 result 应为计划清单: {detail}"
    );
    assert_eq!(
        detail["messages"].as_array().map(|m| m.len()),
        Some(1),
        "对话前应只有创建时落的 1 条目标消息: {detail}"
    );

    // ===== 第一轮反馈:修订为计划B,任务保持 planned =====
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/plan-chat"),
        json!({ "message": "把步骤换成先做竞品调研" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "planned 态 plan-chat 应 200: {json}"
    );
    assert_eq!(
        json["plan"][0]["name"], "修订步骤甲",
        "响应应携修订后计划: {json}"
    );

    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(
        detail["task"]["status"], "planned",
        "修订后任务应保持 planned: {detail}"
    );
    assert_eq!(
        detail["task"]["plan"][0]["name"], "修订步骤甲",
        "计划应被替换: {detail}"
    );
    assert_eq!(detail["task"]["plan"][0]["goal"], "修订目标甲");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("计划已修订"),
        "planned 态清单文本应随修订刷新: {result}"
    );
    assert!(
        result.contains("修订目标甲"),
        "清单应含新步骤目标: {result}"
    );

    // messages 三行:目标 + user 反馈 + assistant 修订说明(后两行 kind=plan_chat)
    let messages = detail["messages"].as_array().expect("详情应含 messages");
    assert_eq!(
        messages.len(),
        3,
        "目标 + 一轮对话两行 = 3 行: {messages:?}"
    );
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["kind"], "goal");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["kind"], "plan_chat");
    assert_eq!(messages[1]["content"], "把步骤换成先做竞品调研");
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[2]["kind"], "plan_chat");
    let note = messages[2]["content"].as_str().unwrap_or("");
    assert!(
        note.contains("计划已修订") && note.contains("修订步骤甲"),
        "assistant 说明应含步数与步骤名: {note}"
    );

    // ===== 第二轮反馈:历史随提示词携带(prompt_summary 可见首轮反馈与本轮反馈) =====
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/plan-chat"),
        json!({ "message": "粒度再细一点" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "第二轮 plan-chat 应 200: {json}");
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(
        detail["task"]["status"], "planned",
        "多轮修订后仍应 planned: {detail}"
    );
    let messages = detail["messages"].as_array().unwrap();
    // 目标(goal)+ 两轮 plan_chat 各两行 = 5 行
    assert_eq!(
        messages.len(),
        5,
        "目标 + 两轮对话应累计 5 行: {messages:?}"
    );
    assert_eq!(messages[0]["kind"], "goal");
    assert_eq!(messages[3]["role"], "user");
    assert_eq!(messages[3]["content"], "粒度再细一点");

    // 历史携带:第二轮规划器调用的 prompt_summary 应含首轮反馈与本轮反馈
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let rows = calls["calls"].as_array().expect("calls 应为数组");
    let planner_rows: Vec<&Value> = rows
        .iter()
        .filter(|r| r["phase"].as_str() == Some("planner"))
        .collect();
    let last = planner_rows.last().expect("应有 planner 调用行");
    let prompt = last["prompt_summary"].as_str().unwrap_or("");
    assert!(
        prompt.contains("把步骤换成先做竞品调研"),
        "第二轮规划入参应携带首轮反馈(历史): {prompt}"
    );
    assert!(
        prompt.contains("粒度再细一点"),
        "第二轮规划入参应含本轮反馈: {prompt}"
    );
}

/// plan-chat 状态门禁:仅 planned 态可用;不存在 404(NOT_FOUND)、
/// 空反馈 400(VALIDATION,先于门禁)、pending/done 均 409(CONFLICT)。
#[tokio::test]
async fn task_plan_chat_state_gate() {
    let app = test_app();

    // 404:任务不存在
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks/不存在/plan-chat",
        json!({ "message": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "不存在应 404: {json}");
    assert_eq!(json["code"], "NOT_FOUND");

    // 400:空反馈(校验先于状态门禁;用 pending 任务验证校验顺序)
    let id = create_task_with_mode(app, "门禁测试目标", "solo").await;
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/plan-chat"),
        json!({ "message": "  " }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空反馈应 400: {json}");
    assert_eq!(json["code"], "VALIDATION");

    // 409:pending(未启动)
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/plan-chat"),
        json!({ "message": "改下计划" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "pending 应 409: {json}");
    assert_eq!(json["code"], "CONFLICT");

    // 409:done(终态非 planned;终态追加请走 followup)
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, _) = wait_terminal(app, &id).await;
    assert_eq!(st, "done");
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/plan-chat"),
        json!({ "message": "改下计划" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "done 应 409(规划对话仅 planned 态): {json}"
    );
    assert_eq!(json["code"], "CONFLICT");
}

/// 缺省 exec_mode:仍按已批准计划逐步执行(改造前行为,回归锁)
#[tokio::test]
async fn approve_without_exec_mode_keeps_approved_plan_executor() {
    let app = test_app();
    let id = planned_task(app, plan_title_two_steps()).await;

    let (status, json) =
        send_json(app, "POST", &format!("/api/tasks/{id}/approve"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "缺省批准应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "缺省批准后应完成: {detail}");
    // 逐步执行的证据:result 是汇总文本(提交 2 起不再拼「## 最终计划」段——该段是
    // task.plan 的文本副本,前端「计划步骤」区已按结构渲染),各步产出落在 plan 里
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("计划续跑最终成果"),
        "缺省批准应走逐步执行(汇总轮命中 reply_if): {result}"
    );
    assert!(
        !result.contains("## 最终计划"),
        "提交 2 起 result 只放汇总文本: {result}"
    );
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "逐步执行按已批准计划原样保留两步: {detail}");
    assert_eq!(
        plan[0]["result"].as_str().unwrap_or(""),
        "甲步成果",
        "逐步执行应回写每步产出: {detail}"
    );
    assert_eq!(plan[1]["result"].as_str().unwrap_or(""), "乙步成果");
}

/// 显式 exec_mode=approved_plan 与缺省等价
#[tokio::test]
async fn approve_with_approved_plan_mode_matches_default() {
    let app = test_app();
    let id = planned_task(app, plan_title_two_steps()).await;

    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/approve"),
        json!({ "exec_mode": "approved_plan" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "approved_plan 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("计划续跑最终成果") && !result.contains("## 最终计划"),
        "approved_plan 应走逐步执行(汇总文本、无最终计划段): {result}"
    );
}

/// exec_mode=solo:以单 Agent 整体执行(不再是逐步执行)——调用追踪只一行 agent
#[tokio::test]
async fn approve_with_solo_mode_runs_single_agent_loop() {
    let app = test_app();
    let id = planned_task(app, plan_title_two_steps()).await;

    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/approve"),
        json!({ "exec_mode": "solo" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "solo 批准应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "solo 执行应完成: {detail}");
    // 提交 2 起任何模式都不产「## 最终计划」段(那是批次 R1 的旧契约,已移除);
    // 保留反向断言防回退
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        !result.contains("## 最终计划"),
        "solo 模式不应产「最终计划」段: {result}"
    );
    // 关键区分:逐步执行会逐步落 agent 行,而 solo 只有一行
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let agent_rows = calls["calls"]
        .as_array()
        .map(|rows| rows.iter().filter(|c| c["phase"] == "agent").count())
        .unwrap_or(0);
    assert_eq!(
        agent_rows, 1,
        "solo 应只有一行 agent 调用(逐步执行会每步一行): {calls}"
    );
}

/// exec_mode=multi:复用 multi 执行器(与 solo 同骨架 + 子 agent 工具化)
#[tokio::test]
async fn approve_with_multi_mode_runs() {
    let app = test_app();
    let id = planned_task(app, plan_title_two_steps()).await;

    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/approve"),
        json!({ "exec_mode": "multi" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "multi 批准应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "multi 执行应完成: {detail}");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        !result.contains("## 最终计划"),
        "multi 不应产「最终计划」段(提交 2 起全模式已移除该段): {result}"
    );
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let agent_rows = calls["calls"]
        .as_array()
        .map(|rows| rows.iter().filter(|c| c["phase"] == "agent").count())
        .unwrap_or(0);
    assert_eq!(agent_rows, 1, "multi 亦为单行 agent 调用: {calls}");
}

/// 未知/不支持的执行方式 → 400,错误文案列出可选值,且不改任务状态。
/// 同时锁死两个**刻意排除**的值:legacy(自带规划会与已批准计划重复劳动)、
/// plan(会再规划回到 planned 死循环,批次 4.3 回归的根因)。
#[tokio::test]
async fn approve_with_unknown_exec_mode_is_rejected() {
    let app = test_app();
    let id = planned_task(app, plan_title_two_steps()).await;

    for bad in ["nope", "legacy", "plan"] {
        let (status, json) = send_json(
            app,
            "POST",
            &format!("/api/tasks/{id}/approve"),
            json!({ "exec_mode": bad }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "exec_mode={bad} 应 400: {json}"
        );
        assert!(
            json["error"]
                .as_str()
                .unwrap_or("")
                .contains("approved_plan"),
            "错误文案应列出可选值(exec_mode={bad}): {json}"
        );
    }
    // 拒绝不得改变任务状态(仍待批准)
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(
        detail["task"]["status"], "planned",
        "拒绝非法执行方式后任务应仍是 planned: {detail}"
    );
}
