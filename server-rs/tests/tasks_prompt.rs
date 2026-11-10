// 任务模式集成测试 · 提示词与人格隔离（人格污染 / 注入隔离 / 工具历史不裁剪）。
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

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
mod common;
use common::{send_json, test_app, wait_terminal};

/// 创建任务并返回 id
async fn create_task(app: &axum::Router, title: &str) -> String {
    let (status, json) = send_json(app, "POST", "/api/tasks", json!({ "title": title })).await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 创建 solo 任务(兼容字段 character_id 绑定旧角色卡)并跑到终态,返回任务详情。
/// character_id 仍被接受并原样落库(旧任务语义:世界书按角色过滤 / 占位符渲染 /
/// character_prompt 工具读取),故本夹具供依赖角色关联的用例如实取用。
async fn run_solo_task_with_character(
    app: &axum::Router,
    title: &str,
    character_id: &str,
) -> Value {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": "solo", "character_id": character_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "solo 任务应完成,详情: {detail}");
    detail
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

/// 上传「工具输出标记」角色卡(R3b 测试夹具),返回角色 id。
/// 标记放在 first_mes:read(type=character_prompt) 的工具输出含 first_mes,
/// 而任务执行者 system 的人设段已随 TM-SET-2 退役(人设不再注入)——
/// 故 prompt_summary 中 R3B-TOOLOUT-MARK 只可能来自 tool 消息,
/// 可按出现次数精确断言「完整保留了几轮工具结果」。
async fn upload_tool_marked_character(app: &axum::Router) -> String {
    let card = json!({
        "name": "工具循环标记角色",
        "description": "工具循环测试角色(此段进人设,不带标记)",
        "personality": "稳重",
        "first_mes": "R3B-TOOLOUT-MARK 工具输出原文"
    });
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"tool-mark.json\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        card
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/characters/upload")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "上传角色卡应 2xx,实际 {status}: {json}"
    );
    json["character"]["id"]
        .as_str()
        .or_else(|| json["id"].as_str())
        .expect("上传响应应含角色 id")
        .to_string()
}

/// 回归:roleplay 人设词不得污染任务模式执行器/汇总器提示词({{char}} 宏原文不得泄漏)。
/// 防退化背景:旧实现 task 模式无覆盖层时整体继承 roleplay 的 agent_system_prompt,
/// 任务目标「写一首关于秋天的短诗」被执行为言情小说(2026-08-25 实测)。
#[tokio::test]
async fn task_executor_prompt_not_contaminated_by_roleplay_persona() {
    let app = test_app();

    // 写入含宏与扮演标记的 roleplay 人设词(结束后还原)
    let (status, before) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let marker = "ROLEPLAY-PERSONA-MARKER";
    let persona = format!("你是 {{{{char}}}} 的扮演者,{marker},与用户进行沉浸式角色扮演");
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "agent_system_prompt": persona }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写入设置应 200: {json}");

    // 规划器返回 1 步;步骤 goal 带 [[floors]] 钩子,执行器回显其实际收到的完整消息序列。
    // 注意:[[floors]] 含 "]]" 会触发 mock reply 钩子提前截断,故 JSON 中用 unicode 转义。
    let title = r#"[[reply:[{"name":"回显步骤","goal":"\u005b\u005bfloors\u005d\u005d"}] ]]"#;
    let id = create_task(app, title).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成,详情: {detail}");

    // 执行器回显:不含 roleplay 人设词与宏原文,保留内置执行者指令与任务向默认提示词
    let echo = detail["subtasks"][0]["result"].as_str().unwrap_or("");
    assert!(!echo.is_empty(), "回显步骤应有结果: {detail}");
    assert!(
        !echo.contains(marker),
        "执行器提示词不得含 roleplay 人设词,实际: {echo}"
    );
    assert!(
        !echo.contains("{{char}}"),
        "宏原文不得泄漏进 LLM 上下文,实际: {echo}"
    );
    assert!(
        echo.contains("任务执行者"),
        "应保留内置执行者指令,实际: {echo}"
    );
    assert!(
        echo.contains("任务执行智能体"),
        "缺省应注入内置任务向默认提示词,实际: {echo}"
    );
    // WP7:用户可编辑的 Agent 提示词段应有 untrusted 边界包裹(内置执行者指令不包裹)
    assert!(
        echo.contains(r#"<UNTRUSTED_PROMPT_SOURCE source="agent_prompt">"#),
        "Agent 提示词段应有 untrusted 边界包裹,实际: {echo}"
    );

    // 汇总器同样不得被污染(其 user 消息含回显文本会再次触发 [[floors]] 回显自身消息序列)
    let summary = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        !summary.contains(marker) && !summary.contains("{{char}}"),
        "汇总器提示词不得含 roleplay 人设词,实际: {summary}"
    );

    // 还原 roleplay 人设词
    let prev = before["agent_system_prompt"].as_str().unwrap_or("");
    let _ = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "agent_system_prompt": prev }),
    )
    .await;
}

/// TM-SET-2:角色卡人设段**退役**——绑角色卡的 solo 任务(兼容字段 character_id,
/// 仅旧客户端可达)执行者 system 不再注入「写作风格参考」,执行者身份段只认执行者指令。
/// 断言人设段标题与正文全部缺席(世界书按角色过滤与占位符渲染不受影响)。
/// [[floors]] 钩子让 mock 执行者回显其实际收到的完整消息序列。
#[tokio::test]
async fn task_character_persona_never_injected_after_retirement() {
    let app = test_app();
    let cid = upload_tool_marked_character(app).await;
    let detail = run_solo_task_with_character(app, "[[floors]]", &cid).await;
    let echo = detail["task"]["result"].as_str().unwrap_or("");
    assert!(!echo.is_empty(), "回显应有结果: {detail}");
    // 绑定角色卡不再向 system 注入任何人设文本(世界书/占位符路径不受本断言影响)
    assert!(
        !echo.contains("写作风格参考"),
        "人设段应随 TM-SET-2 退役,不得再出现: {echo}"
    );
    assert!(!echo.contains("人设:"), "人设正文段标题不得出现: {echo}");
}

/// **工具历史不裁剪(2026-11-09 起)**:工具循环的 assistant/tool 历史原样留在
/// 消息数组里,不再被「最老轮摘要化」改写。
///
/// 为什么不裁:摘要化把替换点放在历史**中段**,而 prompt cache 靠「共同前缀」命中
/// ——中段一改,该点之后全部失效。实测(2026-11-09,deepseek-v4.1-flash)带裁剪的
/// 请求命中率 33.7%、不带的 85.9%;单会话 79 次请求触发 62 次裁剪后命中率从 97%
/// 掉到 25%。上下文压缩改由 `trim_to_context` 按窗口硬上限兜底(它只从最旧消息
/// 起整条丢弃,不改写中段),专项压缩机制另案设计。
///
/// 断言点 = 调用追踪(task_llm_calls)phase=agent 行的 prompt_summary(solo 整轮
/// 落一行,messages 即最终下发的真实数组):
/// - **零处**省略标记;
/// - 6 轮工具输出全部完整保留(R3B-TOOLOUT-MARK 出现 6 次)。
/// 同时验证向后兼容:旧客户端仍发 `tool_history_keep_rounds` 时不报错(字段已退役,
/// 设置结构不启用 `deny_unknown_fields`),且该值不再产生任何作用。
#[tokio::test]
async fn task_solo_tool_history_never_trimmed() {
    let app = test_app();
    let cid = upload_tool_marked_character(app).await;

    // 语义熔断会先把「同一工具、逐字相同输出」的循环收掉(第 4 轮),测不到全部轮次;
    // 本用例只关心历史是否被改写,故关掉无关的那个(同既有用例纪律)。
    // 同时发一个已退役的 keep_rounds=1:旧客户端形态,必须被静默忽略而非 400。
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "tool_history_keep_rounds": 1, "loop_guard_semantic_min_calls": 0 }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "退役字段应被静默忽略(不 400),实际 {json}"
    );

    let args = r#"{"queries":[{"type":"character_prompt"}]}"#;
    let detail =
        run_solo_task_with_character(app, &format!("[[tool_loop:read|6 {args}]]"), &cid).await;
    assert_eq!(
        detail["task"]["status"].as_str(),
        Some("done"),
        "6 轮工具循环后 solo 任务应完成: {detail}"
    );

    let (status, calls) = send_json(
        app,
        "GET",
        &format!(
            "/api/tasks/{}/calls",
            detail["task"]["id"].as_str().unwrap()
        ),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let agent_rows: Vec<&Value> = calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["phase"] == "agent")
        .collect();
    assert_eq!(
        agent_rows.len(),
        1,
        "solo 整轮应落一行 agent 追踪: {calls:?}"
    );
    let prompt = agent_rows[0]["prompt_summary"].as_str().unwrap_or("");
    assert!(!prompt.is_empty(), "prompt_summary 应非空: {calls:?}");

    const MARK: &str = "R3B-TOOLOUT-MARK";
    const SUMMARY_MARK: &str = "(较早工具结果已省略";
    assert_eq!(
        prompt.matches(SUMMARY_MARK).count(),
        0,
        "工具历史不得再被摘要化(缓存前缀必须稳定): {prompt}"
    );
    assert_eq!(
        prompt.matches(MARK).count(),
        6,
        "6 轮工具输出应全部完整保留: {prompt}"
    );

    // 还原现场(共享 app;语义熔断下限 12)
    let (status, json) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "loop_guard_semantic_min_calls": 12 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "还原语义熔断应 200: {json}");
}

// ==================== 批次 R2a:终态追加输入(followup) ====================

/// TM-SET-2:任务模式提示词注入**恒隔离**(原 2026-09-10 F2 实跑修复的默认侧固化)。
/// 角色扮演的 prompt_floors.json 通常承载「1200 字/第三人称/禁词表」等文章要求,
/// 注入任务 system 会与任务目标冲突(实测 legacy 追加「压缩到 200 字」后结果反而变长)。
/// 本用例:注入配置含唯一哨兵文本 → 任务 system **恒不含**哨兵(开关已退役,无恢复通道);
/// 用例末恢复注入配置,避免影响同 binary 其他用例。
#[tokio::test]
async fn task_prompt_inject_never_injected() {
    let app = test_app();
    const NEEDLE: &str = "注入哨兵ABCXYZ";

    // 注入配置设唯一哨兵(simple 模式;perspective 会进 system_inject_text;
    // prompt_inject 为全局共享,用例末恢复默认)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/prompt-inject",
        json!({
            "config": {
                "mode": "simple",
                "simple": {
                    "word_count_enabled": false,
                    "word_count": 1200,
                    "paraphrase_enabled": false,
                    "dialogue_enabled": false,
                    "perspective_enabled": true,
                    "perspective": NEEDLE,
                    "banned_words_enabled": false,
                    "banned_prompt": "",
                    "banned_words": []
                },
                "floors": []
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "设置注入配置应 200");

    // 注入配置在场的前提下,任务 system **恒不含**哨兵(开关退役,无恢复通道)
    let id = create_task_with_mode(app, "写一句关于秋天的话", "solo").await;
    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (_st, _) = wait_terminal(app, &id).await;
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let off_text = calls.to_string();
    assert!(
        !off_text.contains(NEEDLE),
        "任务模式注入恒隔离,任务调用提示词不得含注入哨兵: {off_text}"
    );

    // 还原注入配置(避免影响后续并行用例)
    let _ = send_json(
        app,
        "PUT",
        "/api/prompt-inject",
        json!({
            "config": {
                "mode": "simple",
                "simple": {
                    "word_count_enabled": false,
                    "word_count": 1200,
                    "paraphrase_enabled": false,
                    "dialogue_enabled": false,
                    "perspective_enabled": false,
                    "perspective": "第三人称",
                    "banned_words_enabled": false,
                    "banned_prompt": "",
                    "banned_words": []
                },
                "floors": []
            }
        }),
    )
    .await;
}
