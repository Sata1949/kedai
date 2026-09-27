// 任务模式集成测试 · team 模式（审计 / 回退 / 重做）+ custom 模式。
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

/// custom 模式:预置两步启用流程(起草→润色,均 direct+generates 无工具),
/// 逐步顺序执行,终态 done;结果 = 末个生成步骤产出;plan 步骤名 = flow 步骤名、
/// 状态全 done;调用追踪 phase=step 行数与步骤数对齐。
/// 同一测试内串行覆盖「未启用流程 → error 终态」路径:流程库是共享全局状态,
/// 拆成两个测试会因并行执行互相覆盖当前流程(实测竞态),故合并。
#[tokio::test]
async fn task_custom_mode_flow_steps_and_disabled_flow() {
    let app = test_app();

    // 预置启用流程(保存即设为当前选中;validate_flow 要求至少一个生成步骤)。
    // 首步「理解」为 generates=false 的内部规划步:2026-09-10 实测修复后,
    // 它用内部规划指令产出要点(不吐正文)、不计入最终成果,但作为下一步上下文。
    let flow = json!({
        "config": {
            "id": "",
            "name": "测试三步流程",
            "enabled": true,
            "steps": [
                {"id":"s0","name":"理解","enabled":true,"goal":"分析目标","action":"direct","generates":false,"system_prompt":"【本步指令·理解本步】只做内部规划"},
                {"id":"s1","name":"起草","enabled":true,"goal":"撰写草稿","action":"direct","generates":true,"system_prompt":"【本步指令·起草本步】直接输出草稿"},
                {"id":"s2","name":"润色","enabled":true,"goal":"润色上一版","action":"direct","generates":true,"system_prompt":"【本步指令·润色本步】输出润色后的最终版"}
            ]
        }
    });
    let (status, json) = send_json(app, "PUT", "/api/agent-flows", flow).await;
    assert_eq!(status, StatusCode::OK, "保存流程应 200: {json}");

    // reply_if 钩子按 system 命中步骤提示词(钩子语法内的 needle 出现不算命中,
    // 故任务目标上下文(untrusted 包裹进 system)里的钩子文本不会自命中)
    let title = "[[reply_if:理解本步|内部规划要点]][[reply_if:起草本步|草稿正文]][[reply_if:润色本步|润色成果]] 自定义任务目标";
    let id = create_task_with_mode(app, title, "custom").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "custom 任务应完成,详情: {detail}");
    assert_eq!(
        detail["task"]["result"].as_str().unwrap_or(""),
        "润色成果",
        "结果应为末个生成步骤产出: {detail}"
    );

    // plan 步骤 = flow 启用步骤,状态全 done(前端 custom 渲染契约)
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 3, "plan 步骤数 = 流程启用步骤数: {detail}");
    assert_eq!(plan[0]["name"], "理解");
    assert_eq!(plan[1]["name"], "起草");
    assert_eq!(plan[2]["name"], "润色");
    for step in plan {
        assert_eq!(step["status"], "done", "步骤应 done: {detail}");
    }
    // 非生成步骤:产出带「(内部规划)」标注,且不进入最终成果
    let inner = plan[0]["result"].as_str().unwrap_or("");
    assert!(
        inner.contains("(内部规划)") && inner.contains("内部规划要点"),
        "generates=false 步骤应产内部规划要点: {detail}"
    );
    assert!(
        !detail["task"]["result"]
            .as_str()
            .unwrap_or("")
            .contains("内部规划要点"),
        "内部规划产出不得进入最终成果: {detail}"
    );

    // 调用追踪:phase=step 三行,step_index 0/1/2 对齐步骤顺序
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let step_calls: Vec<&Value> = calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["phase"] == "step")
        .collect();
    assert_eq!(
        step_calls.len(),
        3,
        "phase=step 行数应与步骤数对齐: {calls:?}"
    );
    assert_eq!(step_calls[0]["step_index"], 0);
    assert_eq!(step_calls[1]["step_index"], 1);
    assert_eq!(step_calls[2]["step_index"], 2);

    // 线性兼容流程(二维批次 1 的回归护栏):单上游必须仍是旧版逐字节格式,
    // 且不得误用多父段格式——存量一维流程的行为靠这两条锁住
    let polish_prompt = step_calls[2]["prompt_summary"].as_str().unwrap_or("");
    assert!(
        polish_prompt.contains("上一步「起草」产出:\n草稿正文"),
        "线性流程单上游格式应逐字节不变: {polish_prompt}"
    );
    assert!(
        !polish_prompt.contains("上游节点产出:"),
        "线性流程不得出现多父段格式: {polish_prompt}"
    );

    // usage 落库:逐步骤一行,聚合非零
    let usage = &detail["usage_total"];
    assert!(
        usage["prompt_tokens"].as_i64().unwrap_or(0) > 0,
        "usage_total 应非零: {detail}"
    );

    // ===== 未启用流程 → error 终态(enabled=false 时 validate 放行但执行拒绝) =====
    let flow = json!({
        "config": {
            "id": "",
            "name": "未启用流程",
            "enabled": false,
            "steps": []
        }
    });
    let (status, json) = send_json(app, "PUT", "/api/agent-flows", flow).await;
    assert_eq!(status, StatusCode::OK, "保存未启用流程应 200: {json}");

    let id = create_task_with_mode(app, "某目标", "custom").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200(后台执行报错): {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "error", "未启用流程应 error 终态: {detail}");
    assert!(
        detail["task"]["error"]
            .as_str()
            .unwrap_or("")
            .contains("启用"),
        "错误文案应提示启用流程: {detail}"
    );

    // 还原内置流程
    let _ = send_json(
        app,
        "POST",
        "/api/agent-flows/select",
        json!({ "id": "builtin-coordination" }),
    )
    .await;
}

/// team 模式:自动拓扑(规划器拆 2 主 agent,步骤名带【主Agent-N】前缀)→ 两主并行
///(goal 内嵌 \[ \] 转义的 reply 钩子确定性产出)→ 审计通过 → 升华整合;
/// result 含「## 审计结论」段(前端拆卡契约);调用追踪覆盖 planner/agent/audit/summary。
#[tokio::test]
async fn task_team_mode_auto_topology_audit_summary() {
    let app = test_app();

    // 规划器拓扑 JSON 内的 [[reply:...]] 用 \[ \] 转义(mock reply_if 内容在首个
    // "]]" 截断;规划输出经 serde 解析时还原为真实钩子,主 agent user 消息命中)。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"调研","goals":[{"name":"子目标一","goal":"\u005b\u005breply:主一产出\u005d\u005d 做调研"}]},"#,
        r#"{"name":"写作","goals":[{"name":"子目标二","goal":"\u005b\u005breply:主二产出\u005d\u005d 写正文"}]}]"#,
        r#" }]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"覆盖完整"}]]"#,
        "[[reply_if:任务汇总者|团队最终成果]]",
        " 团队总目标"
    );
    let id = create_task_with_mode(app, title, "team").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "team 任务应完成,详情: {detail}");

    // plan 步骤名带【主Agent-N】前缀(前端分工卡分组契约),状态全 done
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "两个主 agent 各 1 子目标: {detail}");
    assert_eq!(plan[0]["name"], "【主Agent-1】子目标一");
    assert_eq!(plan[1]["name"], "【主Agent-2】子目标二");
    for step in plan {
        assert_eq!(step["status"], "done", "步骤应 done: {detail}");
    }

    // result = 整合文本 + ## 审计结论 段(前端按此拆卡)
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("团队最终成果"),
        "应含升华整合文本: {result}"
    );
    assert!(result.contains("## 审计结论"), "应含审计结论段: {result}");
    assert!(result.contains("覆盖完整"), "应含审计文本: {result}");

    // 调用追踪:planner/agent(2 行,step_index 0/1)/audit/summary 全覆盖
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let calls = calls["calls"].as_array().unwrap();
    for phase in ["planner", "audit", "summary"] {
        assert!(
            calls
                .iter()
                .any(|c| c["phase"] == phase && c["status"] == "ok"),
            "应存在 phase={phase} 行: {calls:?}"
        );
    }
    let agent_calls: Vec<&Value> = calls.iter().filter(|c| c["phase"] == "agent").collect();
    assert_eq!(agent_calls.len(), 2, "两个主 agent 各一行: {calls:?}");
    assert!(
        agent_calls.iter().any(|c| c["step_index"] == 0)
            && agent_calls.iter().any(|c| c["step_index"] == 1),
        "主 agent step_index 应为主序号: {calls:?}"
    );

    // usage 落库:planner/agent/audit/summary 各阶段聚合非零
    let usage = &detail["usage_total"];
    assert!(
        usage["prompt_tokens"].as_i64().unwrap_or(0) > 0,
        "usage_total 应非零: {detail}"
    );
}

/// team 汇总失败 + 各主已产出(提交 2 部分成果兜底):升华整合失败不再整体丢成果 →
/// partial + result = 各主产出的确定性拼装,error 保留团队汇总失败原因。
/// (审计通过但汇总失败:审计结论段由汇总文本承载,失败时不存在该段,属预期。)
#[tokio::test]
async fn task_team_summary_failure_falls_back_to_main_outputs() {
    let app = test_app();

    // 拓扑经 [[reply_if:团队规划器]] 确定性给出;审计通过;汇总 system 含「任务汇总者」
    // → 空输出 → 同参数重试一次仍空 → 走降级拼装(其余阶段 system 不含该子串,不受影响)。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"调研","goals":[{"name":"子目标一","goal":"\u005b\u005breply:主一产出\u005d\u005d 做调研"}]},"#,
        r#"{"name":"写作","goals":[{"name":"子目标二","goal":"\u005b\u005breply:主二产出\u005d\u005d 写正文"}]}]"#,
        r#" }]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"覆盖完整"}]]"#,
        "[[empty_if:任务汇总者]]",
        " 团队汇总失败目标"
    );
    let id = create_task_with_mode(app, title, "team").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "partial",
        "汇总失败但各主已产出应部分完成(成果不得丢),详情: {detail}"
    );

    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "两个主 agent 各 1 子目标: {detail}");
    for step in plan {
        assert_eq!(step["status"], "done", "各主应执行成功: {detail}");
    }
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("主一产出") && result.contains("主二产出"),
        "result 应含各主产出: {result}"
    );
    assert!(
        result.contains("【主Agent-1】子目标一"),
        "result 应含步骤标题: {result}"
    );
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("汇总"),
        "error 应保留团队汇总失败原因,实际: {error}"
    );
}

/// team 审计打回(2026-08 真实模型实测修复):首轮审计输出打回主 1 → 主 1 带补做
/// 指令重跑(仅 1 轮)→ 补做完成后追加一次终审(TEAM_FINAL_AUDIT_PROMPT,只产出
/// 结论文本、不再打回),最终 result 的「## 审计结论」段用终审结论。旧实现拼进的
/// 永远是首次审计的打回原文——实测任务 done 而 result 结尾仍挂「需要补全」。
/// 断言:补做恰好 1 轮(phase=agent step_index=0 两行)、首次审计 1 行、终审 1 行。
#[tokio::test]
async fn task_team_mode_audit_kickback_redoes_once() {
    let app = test_app();

    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"调研","goals":[{"name":"子目标一","goal":"\u005b\u005breply:主一产出\u005d\u005d 做调研"}]},"#,
        r#"{"name":"写作","goals":[{"name":"子目标二","goal":"\u005b\u005breply:主二产出\u005d\u005d 写正文"}]}]"#,
        r#" }]]"#,
        r#"[[reply_if:团队审计员|{"通过":false,"打回":[{"main":1,"instruction":"请补充数据"}],"结论":"主一缺数据"}]]"#,
        r#"[[reply_if:团队终审员|{"通过":true,"结论":"终审结论:补做后已覆盖"}]]"#,
        "[[reply_if:任务汇总者|补做后最终成果]]",
        " 团队总目标"
    );
    let id = create_task_with_mode(app, title, "team").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "team 打回补做后应完成,详情: {detail}");

    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(result.contains("补做后最终成果"), "应含整合文本: {result}");
    // 「## 审计结论」段 = 终审口径,不得残留首次打回原文(前端按此拆卡)
    let audit_section = result.rsplit("## 审计结论").next().unwrap_or("");
    assert!(
        audit_section.contains("终审结论:补做后已覆盖"),
        "审计结论段应为终审结论: {result}"
    );
    assert!(
        !audit_section.contains("主一缺数据"),
        "审计结论段不得残留首次打回原文: {result}"
    );

    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let calls = calls["calls"].as_array().unwrap();
    // 补做发生且仅 1 轮:主 1(step_index=0)的 phase=agent 行 = 首轮 + 补做轮 = 2
    let main1_calls: Vec<&Value> = calls
        .iter()
        .filter(|c| c["phase"] == "agent" && c["step_index"] == 0)
        .collect();
    assert_eq!(
        main1_calls.len(),
        2,
        "主 1 应恰好跑 2 次(首轮+补做): {calls:?}"
    );
    // 主 2 不受打回影响,仅 1 次
    let main2_calls: Vec<&Value> = calls
        .iter()
        .filter(|c| c["phase"] == "agent" && c["step_index"] == 1)
        .collect();
    assert_eq!(main2_calls.len(), 1, "主 2 不应补做: {calls:?}");
    // 打回仅 1 轮:首次审计 1 行;终审不再打回,单独 1 行(phase=final_audit)
    let audit_calls: Vec<&Value> = calls.iter().filter(|c| c["phase"] == "audit").collect();
    assert_eq!(audit_calls.len(), 1, "首次审计应仅 1 轮: {calls:?}");
    let final_calls: Vec<&Value> = calls
        .iter()
        .filter(|c| c["phase"] == "final_audit")
        .collect();
    assert_eq!(final_calls.len(), 1, "补做后应恰好追加 1 次终审: {calls:?}");
}

/// team 子目标粒度补做(实跑问题 2):审计点名「主 1 第 2 个子目标」→ 仅重跑该
/// 子目标(step_index=1),同主第 1 个子目标不重跑(step_index=0 仅 1 次)且不留
/// running;终审通过后任务 done。
#[tokio::test]
async fn task_team_mode_kickback_scoped_to_subgoal() {
    let app = test_app();

    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"调研","goals":[{"name":"子一","goal":"\u005b\u005breply:子一成果\u005d\u005d 做调研一"},{"name":"子二","goal":"\u005b\u005breply:子二旧版\u005d\u005d 做调研二"}]},"#,
        r#"{"name":"写作","goals":[{"name":"子三","goal":"\u005b\u005breply:子三成果\u005d\u005d 写正文"}]}]"#,
        r#" }]]"#,
        // 审计只打回主 1 的第 2 个子目标
        r#"[[reply_if:团队审计员|{"通过":false,"打回":[{"main":1,"step":2,"instruction":"子二需补数据"}],"结论":"主一子二缺数据"}]]"#,
        r#"[[reply_if:团队终审员|{"通过":true,"结论":"补做后已覆盖"}]]"#,
        "[[reply_if:任务汇总者|子目标粒度补做后成果]]",
        " 团队子目标粒度目标"
    );
    let id = create_task_with_mode(app, title, "team").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "终审通过后应 done: {detail}");

    // 未被打回的步骤保持 done(不得因补做轮被置 running 后遗留)
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan[0]["status"], "done", "子一应保持 done: {detail}");
    assert_eq!(plan[1]["status"], "done", "子二补做后应 done: {detail}");

    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let calls = calls["calls"].as_array().unwrap();
    let count = |si: i64| {
        calls
            .iter()
            .filter(|c| c["phase"] == "agent" && c["step_index"] == si)
            .count()
    };
    assert_eq!(count(0), 1, "主 1 子一未被点名,不应重跑: {calls:?}");
    assert_eq!(
        count(1),
        2,
        "主 1 子二被点名,应首轮 + 补做共 2 次: {calls:?}"
    );
    assert_eq!(count(2), 1, "主 2 未被点名,不应重跑: {calls:?}");
}

/// team 补做轮 prior 种子(实跑问题 2):补做子目标必须看得到同主其他子目标成果
/// 与「自己被替换的旧版本」标注,否则重写时无参考、版本冲突原样复现。
/// 标题刻意保持精简:prompt_summary 对每条消息截 800 字符,标题越长 prior 段越容易被
/// 截掉;本测试用最短标题确保 prior 段完整落在摘要内(断言只针对 prior 内容)。
#[tokio::test]
async fn task_team_mode_kickback_prior_seed() {
    let app = test_app();

    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"甲","goals":[{"name":"一","goal":"\u005b\u005breply:一果\u005d\u005d"},{"name":"二","goal":"\u005b\u005breply:二果\u005d\u005d"}]},"#,
        r#"{"name":"乙","goals":[{"name":"三","goal":"\u005b\u005breply:三果\u005d\u005d"}]}]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":false,"打回":[{"main":1,"step":2,"instruction":"补"}],"结论":"缺"}]]"#,
        r#"[[reply_if:团队终审员|{"通过":true,"结论":"过"}]]"#,
        r#"[[reply_if:任务汇总者|成]]"#,
        " T"
    );
    let id = create_task_with_mode(app, title, "team").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "终审通过后应 done: {detail}");

    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let calls = calls["calls"].as_array().unwrap();
    let redo: Vec<&str> = calls
        .iter()
        .filter(|c| c["phase"] == "agent" && c["step_index"] == 1)
        .filter_map(|c| c["prompt_summary"].as_str())
        .collect();
    let redo = *redo.get(1).expect("应有补做轮的第 2 次 agent 调用");
    // 同主未打回子目标的产出被种入(版本衔接;mock 首轮产出为「一」子目标的结果文本)
    assert!(
        redo.contains("「一」"),
        "补做轮应种入同主未打回子目标的产出: {redo}"
    );
    // 被打回子目标标注旧版本(明确本次是替换而非新增)
    assert!(
        redo.contains("旧版本,本次产出将替换它"),
        "补做轮应标注被打回子目标的旧版本: {redo}"
    );
    // 审计补做指令仍随行
    assert!(
        redo.contains("审计补做指令"),
        "补做轮应携带补做指令: {redo}"
    );
}

/// team 审计未通过且无可补做项 → partial(实跑问题 2):审计是质量闸门,
/// 「通过=false 但打回为空」不得静默判 done;审计结论段保留未通过说明。
#[tokio::test]
async fn task_team_mode_audit_fail_without_kickback_is_partial() {
    let app = test_app();

    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"调研","goals":[{"name":"子一","goal":"\u005b\u005breply:主一产出\u005d\u005d 做调研"}]},"#,
        r#"{"name":"写作","goals":[{"name":"子二","goal":"\u005b\u005breply:主二产出\u005d\u005d 写正文"}]}]"#,
        r#" }]]"#,
        // 审计判定未通过,但打回为空(不可经补做修复)
        r#"[[reply_if:团队审计员|{"通过":false,"打回":[],"结论":"产出之间仍存在矛盾,无法通过补做修复"}]]"#,
        "[[reply_if:任务汇总者|未通过审计的成果]]",
        " 团队审计闸门目标"
    );
    let id = create_task_with_mode(app, title, "team").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "partial",
        "审计未通过且无可补做项应为 partial(不得静默 done): {detail}"
    );
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(result.contains("## 审计结论"), "应保留审计结论段: {result}");
    assert!(
        result.contains("仍存在矛盾"),
        "审计未通过说明应进入结论段: {result}"
    );
    // partial 须可解释(实跑问题 2 观感修复):error 字段写明未达标原因,前端状态行展示,
    // 不再只有与「执行中」同色的黄标而用户无从得知为何不是完成
    let err = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        err.contains("审计/终审未通过"),
        "partial 应写入可解释的 error 原因: {err}"
    );
}

/// team 同主多子目标(2026-08 真实模型实测修复):同一主 agent 的多个子目标逐个
/// 独立执行(每子目标一次 agent 调用),各写各自步骤的 result;同一主的多个子目标
/// 复用同一虚拟 session(task:{id}:main:{n})保持人设一致。旧实现对一个主只跑 1 次
/// run_agent_loop、同份产出写进它名下所有步骤——实测步骤 1/2 result 逐字节相同。
#[tokio::test]
async fn task_team_mode_same_main_goals_run_independently() {
    let app = test_app();

    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"调研","goals":[{"name":"子一","goal":"\u005b\u005breply:子一成果\u005d\u005d 做调研一"},{"name":"子二","goal":"\u005b\u005breply:子二成果\u005d\u005d 做调研二"}]},"#,
        r#"{"name":"写作","goals":[{"name":"子三","goal":"\u005b\u005breply:子三成果\u005d\u005d 写正文"}]}]"#,
        r#" }]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"覆盖完整"}]]"#,
        "[[reply_if:任务汇总者|多子目标最终成果]]",
        " 团队多子目标总目标"
    );
    let id = create_task_with_mode(app, title, "team").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "team 任务应完成,详情: {detail}");

    // 3 步(主 1 领 2 子目标 + 主 2 领 1 子目标),各自独立产出且互不重复
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 3, "主 1 两子目标 + 主 2 一子目标: {detail}");
    assert_eq!(plan[0]["name"], "【主Agent-1】子一");
    assert_eq!(plan[1]["name"], "【主Agent-1】子二");
    assert_eq!(plan[2]["name"], "【主Agent-2】子三");
    for step in plan {
        assert_eq!(step["status"], "done", "步骤应 done: {detail}");
    }
    let r0 = plan[0]["result"].as_str().unwrap_or("");
    let r1 = plan[1]["result"].as_str().unwrap_or("");
    let r2 = plan[2]["result"].as_str().unwrap_or("");
    assert!(r0.contains("子一成果"), "子目标一应得自身产出: {r0}");
    assert!(r1.contains("子二成果"), "子目标二应得自身产出: {r1}");
    assert!(r2.contains("子三成果"), "子目标三应得自身产出: {r2}");
    assert_ne!(
        r0, r1,
        "同主多子目标结果不得重复(实测逐字节相同回归): {detail}"
    );

    // 调用追踪:每个子目标一次 agent 调用(phase=agent,step_index=全局步骤下标)
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let calls = calls["calls"].as_array().unwrap();
    let agent_calls: Vec<&Value> = calls.iter().filter(|c| c["phase"] == "agent").collect();
    assert_eq!(agent_calls.len(), 3, "每个子目标一次 agent 调用: {calls:?}");
    for si in [0, 1, 2] {
        assert!(
            agent_calls.iter().any(|c| c["step_index"] == si),
            "应有 step_index={si} 的 agent 调用行: {calls:?}"
        );
    }
}

/// team 取消:规划器即时产出拓扑后主 agent 走 mock 默认逐字流式回复(约 1s 窗口),
/// running 中 stop → ended;终态后主 agent 步骤不残留 running/pending(无孤儿后台任务)。
#[tokio::test]
async fn task_team_mode_stop_running() {
    let app = test_app();

    // 主 agent 子目标不带 reply 钩子 → 主循环走默认慢速流式回复,留出 stop 窗口
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"甲","goals":[{"name":"子一","goal":"慢慢写第一段"}]},"#,
        r#"{"name":"乙","goals":[{"name":"子二","goal":"慢慢写第二段"}]}]"#,
        r#" }]]"#,
        " 团队取消目标"
    );
    let id = create_task_with_mode(app, title, "team").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    // 轮询到 running 且 plan 已产出再 stop
    let mut saw_running = false;
    for _ in 0..100 {
        let (_, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        let running = json["task"]["status"].as_str() == Some("running");
        let planned = !json["task"]["plan"].as_array().unwrap().is_empty();
        if running && planned {
            saw_running = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(saw_running, "应观测到 running 态(含 plan)再 stop");

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "running 态 stop 应 200");
    let (st, _) = wait_terminal(app, &id).await;
    assert_eq!(st, "ended", "stop 后任务应 ended");

    // 留一段宽限让后台主 agent 退出收尾,随后步骤不得残留 running/pending
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    for step in detail["task"]["plan"].as_array().unwrap() {
        let s = step["status"].as_str().unwrap_or("");
        assert!(
            s == "done" || s == "error",
            "终态后步骤不得残留 running/pending(孤儿后台任务): {detail}"
        );
    }
}

// ==================== 可观测性修复:finish_reason 透出 / multi 子 agent subtasks 可见 ====================

/// 问题③(team 主循环):某主 agent 失败(mock [[fail:]] 钩子)→ 其步骤 status=error
/// 且 result 携带失败原因;其余主成功 → 任务终态 partial(审计/汇总照常)。
#[tokio::test]
async fn task_team_mode_failed_main_step_carries_reason_text() {
    let app = test_app();

    // 主 1 子目标含转义 [[fail:...]](首轮即失败),主 2 正常产出
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"调研","goals":[{"name":"失败子目标","goal":"\u005b\u005bfail:模拟主一故障\u005d\u005d 做调研"}]},"#,
        r#"{"name":"写作","goals":[{"name":"正常子目标","goal":"\u005b\u005breply:主二产出\u005d\u005d 写正文"}]}]"#,
        r#" }]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"主一缺失已知"}]]"#,
        "[[reply_if:任务汇总者|部分团队成果]]",
        " 团队步骤错误文本目标"
    );
    let id = create_task_with_mode(app, title, "team").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "partial", "主一失败主二成功应为 partial,详情: {detail}");
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "两个主 agent 各 1 子目标: {detail}");
    assert_eq!(plan[0]["status"], "error", "主一步骤应 error: {detail}");
    let err_text = plan[0]["result"].as_str().unwrap_or("");
    assert!(
        err_text.contains("模拟主一故障"),
        "team 步骤 error 文本应携带失败原因(问题③),实际: {err_text}"
    );
    assert_eq!(plan[1]["status"], "done", "主二步骤应 done: {detail}");
    // partial 原因写入 error 字段(实跑问题 2 观感修复):前端状态行据此说明为何不是完成
    let task_err = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        task_err.contains("以下子目标执行失败") && task_err.contains("失败子目标"),
        "partial 应写入可解释的失败子目标清单: {task_err}"
    );
}

// ==================== R3b:工具循环历史回灌上限(集成) ====================
