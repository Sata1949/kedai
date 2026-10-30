// 任务模式集成测试 · team 模式（审计 / 回退 / 重做）+ custom 模式。
//
// 本文件由原 `tests/tasks.rs`（176KB / 65 用例）按 API 域拆分而来（QUALITY-FIX Q1-1,2026-09-27）：
// 纯移动、不改行为。**拆分的实质收益是测试隔离**——`tests/` 下每个 `.rs` 是一个独立测试进程，
// `build_test_app()` 的 `DATA_DIR`（`%TEMP%\kedai-test-<pid>`）与 `OnceLock` 单例 app 随
// 文件独立，原先「同一进程内共享 settings.json / 流程库」的顺序耦合由此消失。**别再把它们合回一个文件。**
//
// 测试辅助函数已收敛到 `tests/common`(2026-10-25 C-2E):`test_app` / `send_json` /
// `test_lock` / `wait_terminal` 只保留一份实现;每个测试文件 `mod common;` 仍编译为
// 独立测试二进制,进程级 app 与串行锁语义不变。`test_lock()` 仍只在本二进制内串行。
//
// 共用前提：`build_test_app()`（mock 连接器 + 临时数据目录 + 免鉴权）；mock 钩子 `[[reply:内容]]`
// 让规划器返回 JSON 计划、子智能体返回指定结果，实现确定性端到端。

use axum::http::StatusCode;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
mod common;
use common::{send_json, test_app, wait_terminal};

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

/// 创建指定模式 + 绑定工作区的任务(TMR-2 审计工具需要真实可读文件)
async fn create_task_with_mode_workspace(
    app: &axum::Router,
    title: &str,
    mode: &str,
    ws: &Path,
) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": title,
            "task_mode": mode,
            "workspace": ws.to_string_lossy(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 临时工作区(进程 id + 纳秒唯一后缀;用例结束显式清理)
fn temp_workspace(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("kedai-tmr-{tag}-{}-{ns}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
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

/// TMR-2:team 审计/终审挂只读工作区工具。审计首轮经 `[[tool_if:团队审计员|fs_read …]]`
/// 真实读取工作区文件,拿到 tool 结果后才产出 JSON;用持久化事件断言审计阶段确实调用了
/// fs_read,防止「审计没挂到工具、但纯文本 reply_if 仍然通过」的假绿。
#[tokio::test]
async fn task_team_audit_uses_readonly_workspace_tools() {
    let app = test_app();
    let ws = temp_workspace("team-audit");
    std::fs::write(ws.join("audit.txt"), "验收证据:6 个 PART 全部存在\n")
        .expect("写审计样本文件失败");

    // 审计 system 含「团队审计员」→ 只对该角色门控:
    //   [[tool_if:团队审计员|fs_read ...]] 首轮触发一次真实文件读取;
    //   拿到 tool 结果后由 [[reply_if:团队审计员|...]] 产出 JSON。
    // 若审计没有挂到 fs_read(工具面为空),tool_if 会被 tool_offered 过滤掉,
    // 后续工具调用事件断言即失败——本用例因此能钉住「审计真的看了文件」。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"核对","goals":[{"name":"产出","goal":"\u005b\u005btool:fs_write {\"path\":\"out.txt\",\"content\":\"audit\"}\u005d\u005d \u005b\u005breply:主产出\u005d\u005d 核对文件"}]}]"#,
        r#" }]]"#,
        r#"[[tool_if:团队审计员|fs_read {"path":"audit.txt"}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"已核对 audit.txt"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 审计工具目标"
    );
    let id = create_task_with_mode_workspace(app, title, "team", &ws).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "team 任务应完成,详情: {detail}");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("已核对 audit.txt"),
        "审计结论应来自拿到文件结果后的 JSON: {result}"
    );

    // 调用追踪:audit 行存在,且响应摘要就是核对后的结论
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let calls = calls["calls"].as_array().unwrap();
    let audit = calls
        .iter()
        .find(|c| c["phase"] == "audit" && c["status"] == "ok")
        .unwrap_or_else(|| panic!("应有 phase=audit 的成功调用行: {calls:?}"));
    assert!(
        audit["response_summary"]
            .as_str()
            .unwrap_or("")
            .contains("已核对"),
        "审计响应摘要应含文件核对结论: {audit}"
    );

    // 关键证据:审计阶段确实发生了 fs_read 工具调用(事件已持久化,可断点补拉)
    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=500"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "拉取任务事件应 200: {events}");
    let audit_tool = events["events"]
        .as_array()
        .map(|rows| {
            rows.iter().any(|e| {
                e["detail"]
                    .as_str()
                    .unwrap_or("")
                    .contains("审计 agent 调用工具 fs_read")
            })
        })
        .unwrap_or(false);
    assert!(
        audit_tool,
        "审计阶段应透出 fs_read 工具调用事件(证明不是纯文本自述审查): {events}"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

/// TMR-4:规划器声明的 `files` 必须落进 plan;跨主写同一声明文件时,声明归属者的
/// 内容胜出(另一主被所有权拦截,不落盘)。用两个主写同一 `a.txt`、内容不同来区分:
/// 若无所有权,最终内容取决于并行顺序;有所有权则恒为主 Agent-1 的版本。
#[tokio::test]
async fn task_team_plan_carries_files_and_declared_owner_wins() {
    let app = test_app();
    let ws = temp_workspace("team-files");

    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"甲","goals":[{"name":"写A","goal":"\u005b\u005btool:fs_write {\"path\":\"a.txt\",\"content\":\"from-main-1\"}\u005d\u005d \u005b\u005breply:主一产出\u005d\u005d","files":["a.txt"]}]},"#,
        r#"{"name":"乙","goals":[{"name":"写A2","goal":"\u005b\u005btool:fs_write {\"path\":\"a.txt\",\"content\":\"from-main-2\"}\u005d\u005d \u005b\u005breply:主二产出\u005d\u005d","files":[]}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 文件所有权目标"
    );
    let id = create_task_with_mode_workspace(app, title, "team", &ws).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "team 任务应完成,详情: {detail}");

    // 声明随 plan 落库并下发(空数组整键省略)
    let plan = detail["task"]["plan"].as_array().expect("plan 应为数组");
    assert_eq!(plan.len(), 2, "应有两步: {plan:?}");
    assert_eq!(plan[0]["files"], json!(["a.txt"]), "声明应随 plan 下发");
    assert!(
        plan[1].get("files").is_none() || plan[1]["files"] == json!([]),
        "空声明应按省略/空数组处理: {:?}",
        plan[1]
    );

    // 声明归属者胜出:主 2 的越权写必须被拒,文件内容保持主 1 版本
    assert_eq!(
        std::fs::read_to_string(ws.join("a.txt")).expect("a.txt 应存在"),
        "from-main-1",
        "跨主越权写不得覆盖声明归属者的内容"
    );

    // TMR-6 交付摘要:必需文件齐全、有变更、无缺失
    let delivery = &detail["delivery"];
    assert_eq!(
        delivery["checked"],
        json!(true),
        "有声明时 checked=true: {delivery}"
    );
    let required = delivery["required"].as_array().expect("required 应为数组");
    assert_eq!(required.len(), 1, "必需文件应去重: {required:?}");
    assert_eq!(required[0]["path"], "a.txt");
    assert_eq!(required[0]["exists"], json!(true));
    assert_eq!(delivery["missing"], json!([]), "不应有缺失: {delivery}");
    assert!(
        delivery["changed"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|p| p == "a.txt")),
        "变更清单应含 a.txt: {delivery}"
    );
    assert!(
        delivery["changed_count"].as_u64().unwrap_or(0) >= 1,
        "变更总数应 ≥1: {delivery}"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

/// TMR-6:规划器声明了两个必需文件,主 agent 只写了一个 → 终态 partial,
/// `tasks.error` 点名缺失文件,交付摘要的 missing 与磁盘一致。
#[tokio::test]
async fn task_team_missing_required_file_becomes_partial() {
    let app = test_app();
    let ws = temp_workspace("team-missing-file");

    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"甲","goals":[{"name":"写A","goal":"\u005b\u005btool:fs_write {\"path\":\"a.txt\",\"content\":\"only-a\"}\u005d\u005d \u005b\u005breply:主一产出\u005d\u005d","files":["a.txt","b.txt"]}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 必需文件缺失目标"
    );
    let id = create_task_with_mode_workspace(app, title, "team", &ws).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "partial", "缺少必需文件必须降 partial: {detail}");
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(error.contains("缺少必需文件"), "error 应点名缺失: {error}");
    assert!(error.contains("b.txt"), "error 应点名 b.txt: {error}");

    let delivery = &detail["delivery"];
    assert_eq!(delivery["checked"], json!(true));
    assert_eq!(
        delivery["missing"],
        json!(["b.txt"]),
        "缺失清单: {delivery}"
    );
    let required = delivery["required"].as_array().expect("required 应为数组");
    assert_eq!(required.len(), 2, "两个声明都应下发: {required:?}");
    assert_eq!(required[1]["exists"], json!(false));

    let _ = std::fs::remove_dir_all(&ws);
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

    // 主 agent 子目标不带 reply 钩子 → 主循环走默认回复;stop 窗口由 goal 里的慢流
    // 哨兵显式申请(测试构建默认零延时;哨兵无括号,不会截断 reply_if 的 `]]`)。
    let title = concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"甲","goals":[{"name":"子一","goal":"慢慢写第一段__KEDAI_SLOW_STREAM__"}]},"#,
        r#"{"name":"乙","goals":[{"name":"子二","goal":"慢慢写第二段__KEDAI_SLOW_STREAM__"}]}]"#,
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
