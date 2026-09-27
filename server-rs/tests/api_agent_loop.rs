// API 集成测试 · Agent 工具循环 / 预算熔断 / 语义熔断 / MVU 增量 / 反思。
//
// 本文件由原 `tests/api_integration.rs`（137KB / 62 用例）按 `/api/*` 路由域拆分而来
// （QUALITY-FIX Q1-2,2026-09-27）：纯移动、不改行为。**拆分的实质收益是测试隔离**——
// `tests/` 下每个 `.rs` 是独立测试进程，`build_test_app()` 的 `DATA_DIR`
// （`%TEMP%\kedai-test-<pid>`）与 `OnceLock` 单例 app 随文件独立，原先「同进程共享
// settings.json / 设置覆盖层 / 流程库」的顺序耦合由此消失（既有 flake `TEST-ISO-1` 正是这一土壤）。
// **别再把它们合回一个文件。**
//
// 测试辅助函数按「谁用谁带」复制（仓库既有体例：40 个测试文件里 36 个各自持有 `test_app`），
// 刻意不建 `tests/common` 共享层：那会让 count-tests 的集成文件数口径虚高，也会给多会话并行
// 改测试制造新的争用点。`test_lock()` 是**本二进制内**的串行锁，随文件带走即可。
//
// 共用前提：与 Node 版 `server/tests/api.test.ts` 对齐；mock 连接器 + 临时数据目录 + 免鉴权。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
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

async fn upload_character(app: &axum::Router, name: &str) -> (StatusCode, Value) {
    // 构造 multipart 表单:字段 file
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        json!({
            "spec": "chara_card_v2",
            "spec_version": "1.0",
            "name": "测试角色",
            "description": "测试描述",
            "first_mes": "你好,我是测试角色",
            "custom_field": { "unknown": true }
        })
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
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// 串行化修改全局配置(settings / agent-flows)的测试,避免同进程内相互污染
async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // 测试进程级:把运行时主提示词目录指向空目录,避免真实项目根的 AGENTS_RUNTIME.md
    // 被注入 mock 断言(与 build_test_app 临时数据目录的隔离语义一致)。Once 保证只设一次。
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // uuid 唯一名(避免多测试二进制并发共用同名目录);守卫随本闭包析构即回收。
        // 目录是否存在不影响语义:with_dir 关闭内置默认回退,读不到文件即视为「无提示词」,
        // 与「指向一个空目录」等价(全仓无测试写该文件)。
        let dir = kedai_server::utils::test_support::TempDataDir::new("test-runtime-prompt-empty");
        std::env::set_var("KEDAI_RUNTIME_PROMPT_DIR", dir.path());
    });
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

/// 以指定 agent_mode 发送消息,解析 SSE 事件列表
async fn sse_events(
    app: &axum::Router,
    sid: &str,
    cid: &str,
    message: &str,
    agent_mode: &str,
) -> Vec<Value> {
    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "session_id": sid,
                "character_id": cid,
                "message": message,
                "agent_mode": agent_mode,
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "chat/send 应 200");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    text.split("\n\n")
        .filter_map(|block| {
            let block = block.trim();
            if block.is_empty() {
                return None;
            }
            let data_line = block.lines().find(|l| l.starts_with("data: "))?;
            serde_json::from_str(&data_line[6..]).ok()
        })
        .collect()
}

/// 恢复流程配置为未启用
async fn reset_flow(app: &axum::Router) {
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": { "enabled": false, "steps": [] } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "重置流程配置应 200");
}

/// 回归:反思持续失败必须有界结束,绝不无限循环。
/// 复现 2026-08-06 日志事故:同一会话每毫秒一条「输出为空或过短」reflect 洪流
/// (根因:反思失败回退逻辑在 plan 的 direct 步骤前原地打转,attempt 永不递增)。
/// mock [[empty]] 钩子让每轮生成都输出空内容 → 反思必失败 → 引擎应重试有限次后正常 Finish。
#[tokio::test]
async fn agent_reflect_failure_is_bounded() {
    let app = test_app();
    let (_, char) = upload_character(app, "空输出测试.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "session_id": sid, "character_id": cid,
                "message": "你好 [[empty]]", "agent_mode": "deep"
            })
            .to_string(),
        ))
        .unwrap();
    // 有界保护:旧代码在此会无限紧密循环,10 秒超时直接判失败
    let resp = tokio::time::timeout(std::time::Duration::from_secs(10), app.clone().oneshot(req))
        .await
        .expect("反思失败场景未在 10 秒内有界结束(回归:无限循环)")
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let events: Vec<Value> = text
        .split("\n\n")
        .filter_map(|block| {
            let block = block.trim();
            if block.is_empty() {
                return None;
            }
            let data_line = block.lines().find(|l| l.starts_with("data: "))?;
            serde_json::from_str(&data_line[6..]).ok()
        })
        .collect();
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(types.contains(&"finish"), "缺 finish: {types:?}");
    // 反思重试次数有界:max_attempts=3,前 2 次失败重试,第 3 次失败放弃
    let retries = events
        .iter()
        .filter(|e| e["type"] == "step" && e["step"] == "重新生成")
        .count();
    assert_eq!(
        retries, 2,
        "空输出应恰好重试 2 次后放弃,实际 {retries} 次: {text}"
    );
    let failed_reflects = events
        .iter()
        .filter(|e| e["type"] == "step" && e["step"] == "反思未通过")
        .count();
    assert_eq!(
        failed_reflects, 3,
        "应恰好反思失败 3 次,实际 {failed_reflects} 次"
    );
}

/// 回归:纯变量更新消息(正文为空)必须落库并携带 extra.mvu 快照。
/// 否则前端 loadHistory 回放会把变量树回滚到更新前(刷新丢变量/状态栏回退)。
#[tokio::test]
async fn pure_mvu_update_message_persists_with_snapshot() {
    let app = test_app();
    let (_, char) = upload_character(app, "纯变量更新.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    // 模型仅输出 <UpdateVariable> 块(无正文)
    let patch = r#"<UpdateVariable><JSONPatch>[{"op":"replace","path":"/hp","value":88}]</JSONPatch></UpdateVariable>"#;
    let events = sse_events(
        app,
        &sid,
        &cid,
        &format!("更新变量 [[reply:{patch}]]"),
        "deep",
    )
    .await;
    // 补丁已应用并推送 vars 事件
    let vars_evt = events
        .iter()
        .find(|e| e["type"] == "vars")
        .expect("应有 vars 事件");
    assert_eq!(vars_evt["stat_data"]["hp"], json!(88));
    assert!(
        events.iter().any(|e| e["type"] == "finish"),
        "应有 finish 事件"
    );
    // 核心:消息落库且 extra.mvu 快照正确(正文为空也落库)
    let (status, hist) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let msgs = hist["messages"].as_array().unwrap();
    let last = msgs.last().expect("应有 assistant 消息");
    assert_eq!(last["role"], "assistant");
    assert_eq!(
        last["extra"]["mvu"]["stat_data"]["hp"],
        json!(88),
        "纯变量更新消息必须落库快照(否则刷新回放会回滚变量树): {hist}"
    );
}

/// 配置反思提示词后,deep 模式反思步骤调用 LLM 判定:
/// mock [[reply:FAIL ...]] 让草稿与反思判定都输出 FAIL → 反思失败重试 3 次(共 4 次判定)后有界结束。
/// 未配置提示词时,机械规则对该草稿(非空、非截断、无提问)会直接通过——
/// 因此「反思未通过」恰好 4 次即证明 LLM 判定生效。
#[tokio::test]
async fn reflect_prompt_triggers_llm_reflection() {
    let _guard = test_lock().await;
    let app = test_app();
    // 配置反思提示词(结束前恢复,避免污染同进程其他测试)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "reflect_prompt": "你是质检员:检查草稿是否回应了用户,输出 PASS 或 FAIL。" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "设置 reflect_prompt 应 200");
    let (_, char) = upload_character(app, "反思提示词.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    let events = sse_events(
        app,
        &sid,
        &cid,
        "这段回复如何 [[reply:FAIL 回复未回应提问]]",
        "deep",
    )
    .await;
    let failed_reflects = events
        .iter()
        .filter(|e| e["type"] == "step" && e["step"] == "反思未通过")
        .count();
    assert_eq!(
        failed_reflects, 4,
        "LLM 反思应判定失败 4 次(重试 3 次)后有界结束(机械规则不会判该草稿失败): {events:?}"
    );
    assert!(events.iter().any(|e| e["type"] == "finish"), "应有 finish");
    // 恢复默认(机械规则)
    let (status, _) = send_json(app, "PUT", "/api/settings", json!({ "reflect_prompt": "" })).await;
    assert_eq!(status, StatusCode::OK, "恢复 reflect_prompt 应 200");
}

/// 状态自动注入:角色卡仅有 [InitVar](无 {{format_message_variable}} 常驻条目)时,
/// 引擎仍会把「当前状态」+ 更新协议注入 system —— 模型看得到状态,才会输出 <UpdateVariable>。
#[tokio::test]
async fn state_block_auto_injected_into_system() {
    let _guard = test_lock().await;
    let app = test_app();
    let card = json!({
        "spec": "chara_card_v2",
        "spec_version": "2.0",
        "name": "状态角色",
        "description": "测试状态自动注入",
        "first_mes": "你好,我是状态角色",
        "data": {
            "name": "状态角色",
            "description": "测试状态自动注入",
            "character_book": {
                "entries": [
                    {
                        "id": 0,
                        "comment": "[InitVar]",
                        "content": "hp: 100\n好感度: 50",
                        "constant": false,
                        "enabled": false,
                        "position": 0,
                        "order": 100
                    }
                ]
            }
        }
    });
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"状态角色.json\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        card
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/characters/upload")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let char: Value = serde_json::from_slice(&bytes).unwrap();
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    // mock [[floors]] 回显全部 LLM 消息 → 断言 system 含自动注入的状态块。
    // 注意:注入文本中的 <UpdateVariable> 示例会被引擎按「模型输出的补丁」正常剥离
    // (parse_update_variable),故只断言状态与协议标题。
    let events = sse_events(app, &sid, &cid, "你好 [[floors]]", "fast").await;
    let finish = events
        .iter()
        .find(|e| e["type"] == "finish")
        .expect("缺 finish 事件");
    let content = finish["content"].as_str().unwrap_or("");
    assert!(
        content.contains("[当前状态]"),
        "system 应含自动注入的状态块:\n{content}"
    );
    assert!(
        content.contains("好感度"),
        "状态 JSON 应含 InitVar 初始化的值:\n{content}"
    );
    assert!(
        content.contains("[状态更新协议]"),
        "应含更新协议说明:\n{content}"
    );
}

/// custom 多步流程:中间步骤的 <UpdateVariable> 补丁在步骤完成时立即应用。
/// 断言:第一个 vars 事件出现在步骤 2 执行之前(旧实现只在收尾解析,中间步骤变量会丢),
/// 且最终消息 extra.mvu 快照落库正确。
#[tokio::test]
async fn custom_flow_applies_mvu_updates_per_step() {
    let _guard = test_lock().await;
    let app = test_app();
    // 两步 direct 生成流程
    let flow = json!({
        "enabled": true,
        "steps": [
            {
                "id": "s1", "name": "第一步", "goal": "生成变量",
                "action": "direct", "generates": true, "enabled": true,
                "system_prompt": "", "temperature": 0.7, "max_tokens": 1024, "tools": null
            },
            {
                "id": "s2", "name": "第二步", "goal": "生成正文",
                "action": "direct", "generates": true, "enabled": true,
                "system_prompt": "", "temperature": 0.7, "max_tokens": 1024, "tools": null
            }
        ]
    });
    let (status, _) = send_json(app, "PUT", "/api/agent-flows", json!({ "config": flow })).await;
    assert_eq!(status, StatusCode::OK, "配置流程应 200");
    let (_, char) = upload_character(app, "自定义变量.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    let patch = r#"<UpdateVariable><JSONPatch>[{"op":"replace","path":"/hp","value":77}]</JSONPatch></UpdateVariable>"#;
    let events = sse_events(
        app,
        &sid,
        &cid,
        &format!("第一步 [[reply:{patch}]]"),
        "custom",
    )
    .await;
    // 最终消息落库快照正确
    let (_, hist) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let msgs = hist["messages"].as_array().unwrap();
    let last = msgs.last().expect("应有 assistant 消息");
    assert_eq!(
        last["extra"]["mvu"]["stat_data"]["hp"],
        json!(77),
        "快照应落库: {hist}"
    );
    // 时序:第一个 vars 事件在步骤 2「执行中…」(index=2)之前推送
    let first_vars = events
        .iter()
        .position(|e| e["type"] == "vars")
        .expect("应有 vars 事件");
    let step2_exec = events
        .iter()
        .position(|e| e["type"] == "step" && e["step"] == "执行中…" && e["index"] == json!(2))
        .expect("应有步骤 2 执行中事件");
    assert!(
        first_vars < step2_exec,
        "中间步骤补丁应在步骤 2 执行前应用(旧实现只在收尾解析,中间步骤变量会丢): {events:?}"
    );
    // 恢复流程配置
    reset_flow(app).await;
}

/// custom 流程反思回退时,即时应用的变量补丁必须回滚到步骤基线:
/// 否则 delta 等非幂等补丁被重生成重复应用,变量值翻倍(10 → 30 而非 10)。
#[tokio::test]
async fn custom_flow_reflect_retry_rolls_back_mvu_delta() {
    let _guard = test_lock().await;
    let app = test_app();
    // 两步流程:生成(delta 补丁 + 逗号结尾文本 → 截断规则必失败)→ 反思
    let flow = json!({
        "enabled": true,
        "steps": [
            {
                "id": "s1", "name": "生成", "goal": "生成变量",
                "action": "direct", "generates": true, "enabled": true,
                "system_prompt": "", "temperature": 0.7, "max_tokens": 1024, "tools": null
            },
            {
                "id": "s2", "name": "反思", "goal": "检查质量",
                "action": "reflect", "enabled": true
            }
        ]
    });
    let (status, _) = send_json(app, "PUT", "/api/agent-flows", json!({ "config": flow })).await;
    assert_eq!(status, StatusCode::OK, "配置流程应 200");
    let (_, char) = upload_character(app, "回滚变量.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    // 草稿以逗号结尾 → 反思截断规则失败 → 回退重试;delta 补丁随每次生成即时应用
    let patch = r#"文本,<UpdateVariable><JSONPatch>[{"op":"delta","path":"/hp","value":10}]</JSONPatch></UpdateVariable>"#;
    let events = sse_events(
        app,
        &sid,
        &cid,
        &format!("开始 [[reply:{patch}]]"),
        "custom",
    )
    .await;
    // 确实发生了反思回退(截断规则失败)
    let retries = events
        .iter()
        .filter(|e| e["type"] == "step" && e["step"] == "重新生成")
        .count();
    assert!(
        retries >= 2,
        "截断草稿应触发反思重试,实际 {retries} 次: {events:?}"
    );
    // 核心:变量值只加一次(delta 不因回退重生成而翻倍;delta 补丁值为 f64)
    let (_, hist) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let msgs = hist["messages"].as_array().unwrap();
    let last = msgs.last().expect("应有 assistant 消息");
    assert_eq!(
        last["extra"]["mvu"]["stat_data"]["hp"].as_f64(),
        Some(10.0),
        "delta 补丁经反思回退后应只应用一次(基线回滚),实际: {hist}"
    );
    // 恢复流程配置
    reset_flow(app).await;
}

/// M1 回归:AGENT 模式工具循环轮次上限由设置驱动(默认 32),不再硬编码 8 轮。
/// 9 轮工具调用 + 设置上限 10 → 全部执行,卡片均有 tool_result 终态(无 running 悬挂),
/// 且模型请求轮数与工具执行轮数一致(无 off-by-one 丢弃)。
#[tokio::test]
async fn agent_tool_loop_exceeds_eight_rounds() {
    let _guard = test_lock().await;
    let app = test_app();
    // 上限提到 10(结束时恢复默认,避免污染同进程其他测试)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "max_tool_rounds": 10 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "设置 max_tool_rounds 应 200");
    let (_, char) = upload_character(app, "多轮工具.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    // mock 钩子:连续 9 轮返回 read 工具调用,第 10 轮返回正文
    let args = r#"{"queries":[{"type":"character_prompt"}]}"#;
    let events = sse_events(
        app,
        &sid,
        &cid,
        &format!("[[tool_loop:read|9 {args}]]"),
        "agent",
    )
    .await;
    let tool_calls = events.iter().filter(|e| e["type"] == "tool_call").count();
    let tool_results = events.iter().filter(|e| e["type"] == "tool_result").count();
    assert_eq!(
        tool_calls, 9,
        "9 轮工具调用应全部执行(旧实现最多 8 轮): {events:?}"
    );
    assert_eq!(
        tool_results, 9,
        "每轮工具调用都应有 tool_result 终态(无 running 悬挂): {events:?}"
    );
    assert!(
        events.iter().any(|e| e["type"] == "finish"),
        "应有 finish 事件"
    );
    // 无工具循环上限提示(9 < 10)
    assert!(
        !events
            .iter()
            .any(|e| e["type"] == "step" && e["step"] == "达到工具调用轮次上限"),
        "未达上限不应提示: {events:?}"
    );
    // 恢复默认
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "max_tool_rounds": 32 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "恢复 max_tool_rounds 应 200");
}

/// P0-2(2026-09-14):工具循环「重复调用熔断」端到端回归。
///
/// 背景:此前普通 agent 工具循环**没有任何重复调用检测**,唯一终止条件是
/// max_tool_rounds。实测 plan 模式任务在单步反复调用同一工具时,7 分钟烧掉
/// 150 万 prompt token 仍未收敛,必须人工 stop。
///
/// 本用例用 [[tool_loop_repeat:...]] 钩子模拟「模型以完全相同参数反复调用同一工具」
/// (真实死循环形态),断言:
///   1. 引擎在远小于 max_tool_rounds 的轮次内自行中止;
///   2. 中止理由以 step 事件显式透出(不静默截断);
///   3. 已执行的每个工具调用都有 tool_result 终态(无 running 悬挂)。
#[tokio::test]
async fn agent_tool_loop_duplicate_call_breaks_early() {
    let _guard = test_lock().await;
    let app = test_app();
    // 上限放大到 32:确保中止来自熔断而非轮次上限
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "max_tool_rounds": 32 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, char) = upload_character(app, "重复调用熔断.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    // 参数逐字相同重复 20 轮(远小于上限 32)
    let args = r#"{"queries":[{"type":"character_prompt"}]}"#;
    let events = sse_events(
        app,
        &sid,
        &cid,
        &format!("[[tool_loop_repeat:read|20 {args}]]"),
        "agent",
    )
    .await;

    let tool_calls = events.iter().filter(|e| e["type"] == "tool_call").count();
    let tool_results = events.iter().filter(|e| e["type"] == "tool_result").count();
    let broke = events
        .iter()
        .any(|e| e["type"] == "step" && e["step"] == "重复调用熔断");

    assert!(broke, "同参数重复调用应触发熔断并透出理由: {events:?}");
    assert!(
        tool_calls < 20,
        "应在 mock 请求的 20 轮之前中止(实际 {tool_calls} 轮): {events:?}"
    );
    assert_eq!(
        tool_results, tool_calls,
        "已执行的工具调用必须都有 tool_result 终态(无 running 悬挂): {events:?}"
    );
    // 熔断文案应指明工具名,便于用户诊断
    let detail = events
        .iter()
        .find(|e| e["type"] == "step" && e["step"] == "重复调用熔断")
        .and_then(|e| e["detail"].as_str())
        .unwrap_or("");
    assert!(
        detail.contains("read") && detail.contains("重复调用"),
        "熔断理由应含工具名与说明,实际: {detail}"
    );
}

/// M1 回归:轮次上限配置生效——设置上限 3,模型持续请求工具时,每次生成尝试内恰好
/// 执行 3 轮后停止(不再发起新的模型请求)。agent 模式对空结果会反思重试(既有语义),
/// 故断言每次尝试内工具调用数 = min(mock 请求轮数 5, 上限 3) = 3,共 3 次尝试 = 9。
/// 若无上限,每次尝试将执行满 5 轮;若 off-by-one,上限轮会被丢弃(无 tool_result)。
#[tokio::test]
async fn agent_tool_loop_respects_configured_limit() {
    let _guard = test_lock().await;
    let app = test_app();
    let (status, _) = send_json(app, "PUT", "/api/settings", json!({ "max_tool_rounds": 3 })).await;
    assert_eq!(status, StatusCode::OK, "设置 max_tool_rounds 应 200");
    let (_, char) = upload_character(app, "轮次上限.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    // mock 钩子:模型会持续请求 5 轮,但引擎应在上限 3 轮后停止
    let args = r#"{"queries":[{"type":"character_prompt"}]}"#;
    let events = sse_events(
        app,
        &sid,
        &cid,
        &format!("[[tool_loop:read|5 {args}]]"),
        "agent",
    )
    .await;
    let tool_calls = events.iter().filter(|e| e["type"] == "tool_call").count();
    let tool_results = events.iter().filter(|e| e["type"] == "tool_result").count();
    // 每次尝试内恰 3 轮:上限提示出现 3 次(每次尝试一次),即每次尝试被限制在 3 轮
    let limit_hits = events
        .iter()
        .filter(|e| e["type"] == "step" && e["step"] == "达到工具调用轮次上限")
        .count();
    assert_eq!(limit_hits, 3, "3 次尝试各应触发一次上限提示: {events:?}");
    // 每次尝试 3 轮 × 3 次尝试 = 9;无上限时每次尝试会执行 5 轮
    assert_eq!(
        tool_calls, 9,
        "每次尝试工具轮数应受配置上限 3 限制(否则为 5 轮/次): {events:?}"
    );
    assert_eq!(
        tool_results, 9,
        "已执行轮次应全部有 tool_result 终态(无 running 悬挂): {events:?}"
    );
    assert!(
        events.iter().any(|e| e["type"] == "finish"),
        "应有 finish 事件"
    );
    // 恢复默认
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "max_tool_rounds": 32 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "恢复 max_tool_rounds 应 200");
}

/// token 预算 stop 档(HB-1,2026-09-18):达到预算即停工具循环,但仍按正常终态收尾——
/// 正文落库、用量记账、extra.budget_exceeded 留痕(刷新后仍看得出停止原因)。
///
/// 触发确定性:预算下限 1024,mock 工具轮每轮 8 token(5+3)→ 第 128 轮越线。
/// 用 [[tool_loop_text:...]] 让工具轮带正文:否则工具轮正文恒空,on-stop 无内容可落库,
/// 「停止时保留产出」这条断言就无从校验。
#[tokio::test]
async fn token_budget_stop_halts_tool_loop_and_keeps_output() {
    // 全局设置是进程级共享:预算/轮次上限类用例必须串行(同文件并发会互相踩踏)
    let _guard = test_lock().await;
    let app = test_app();
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "max_tool_rounds": 200,
            "session_token_budget": 1024,
            "session_budget_action": "stop",
            // 这些用例要跑满 128 轮同工具循环:语义熔断(HB-2)会在第 12 轮先熔断,故关闭
            "loop_guard_semantic_min_calls": 0,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "预算设置应保存成功");
    let (_, char) = upload_character(app, "预算停止.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    let args = r#"{"queries":[{"type":"character_prompt"}]}"#;
    let events = sse_events(
        app,
        &sid,
        &cid,
        &format!("[[tool_loop_text:read|200 {args}]]"),
        "agent",
    )
    .await;

    let budget_step = events
        .iter()
        .find(|e| {
            e["type"] == "step" && e["step"].as_str().unwrap_or("").contains("Token 预算超限")
        })
        .unwrap_or_else(|| panic!("应透出预算停止理由: {events:?}"));
    assert!(
        !events
            .iter()
            .any(|e| e["type"] == "step" && e["step"] == "达到工具调用轮次上限"),
        "预算(1024)必须先于 200 轮上限触发: {events:?}"
    );
    let tool_calls = events.iter().filter(|e| e["type"] == "tool_call").count();
    let tool_results = events.iter().filter(|e| e["type"] == "tool_result").count();
    assert_eq!(
        tool_calls, 128,
        "每轮 8 token、预算 1024 → 第 128 轮越线: {tool_calls}"
    );
    assert_eq!(
        tool_results, tool_calls,
        "已执行的工具调用都必须有 tool_result 终态(预算停止不得留悬挂): {events:?}"
    );
    assert!(
        events.iter().any(|e| e["type"] == "finish"),
        "预算停止是正常收尾(保留产出),不是错误终态: {events:?}"
    );
    assert!(
        !events.iter().any(|e| e["type"] == "error"),
        "不得产生 error 终态: {events:?}"
    );
    let detail = budget_step["detail"].as_str().unwrap_or("");
    assert!(
        detail.contains("1024") && detail.contains("预算"),
        "停止理由应含已用 token 与预算: {budget_step}"
    );

    // 停止时保留产出:正文落库(第 128 轮的短正文)+ extra.budget_exceeded 留痕
    let (status, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let msgs = history["messages"].as_array().expect("messages 数组");
    let last = msgs
        .iter()
        .rev()
        .find(|m| m["role"] == "assistant")
        .unwrap_or_else(|| panic!("停止时应落库一条 assistant 消息: {history}"));
    assert!(
        last["content"]
            .as_str()
            .unwrap_or("")
            .contains("第128轮说明"),
        "落库正文应为停止轮的正文: {last}"
    );
    assert_eq!(
        last["extra"]["budget_exceeded"],
        json!(true),
        "extra 应留预算停止痕迹(刷新后仍可见): {last}"
    );

    // 恢复默认(同进程内其他用例共享设置)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "max_tool_rounds": 32,
            "session_token_budget": 0,
            "session_budget_action": "warn",
            "loop_guard_semantic_min_calls": 12,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// token 预算 warn 档(HB-1):只提示一次且不中断循环——默认档必须零行为变更。
#[tokio::test]
async fn token_budget_warn_notices_once_and_continues() {
    let _guard = test_lock().await;
    let app = test_app();
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "max_tool_rounds": 200,
            "session_token_budget": 1024,
            "session_budget_action": "warn",
            "loop_guard_semantic_min_calls": 0,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, char) = upload_character(app, "预算提示.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    let args = r#"{"queries":[{"type":"character_prompt"}]}"#;
    // N=140:预算在第 128 轮越线,循环继续到模型自然收尾(第 140 轮后给完成回复)
    let events = sse_events(
        app,
        &sid,
        &cid,
        &format!("[[tool_loop:read|140 {args}]]"),
        "agent",
    )
    .await;

    let notices = events
        .iter()
        .filter(|e| {
            e["type"] == "step" && e["step"].as_str().unwrap_or("").contains("Token 预算超限")
        })
        .count();
    assert_eq!(notices, 1, "warn 档只提示一次: {events:?}");
    let tool_calls = events.iter().filter(|e| e["type"] == "tool_call").count();
    assert_eq!(
        tool_calls, 140,
        "warn 不中断循环,应跑满模型请求的轮数: {tool_calls}"
    );
    let text: String = events
        .iter()
        .filter(|e| e["type"] == "token")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert!(
        text.contains("工具循环已完成"),
        "warn 档循环应正常收尾并输出正文: {text:?}"
    );
    let step = events
        .iter()
        .find(|e| {
            e["type"] == "step" && e["step"].as_str().unwrap_or("").contains("Token 预算超限")
        })
        .expect("应有预算提示");
    assert!(
        step["step"].as_str().unwrap_or("").contains("仅提示")
            || step["detail"].as_str().unwrap_or("").contains("warn 档"),
        "提示文案应说明是 warn 档: {step}"
    );

    // 恢复默认
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "max_tool_rounds": 32,
            "session_token_budget": 0,
            "session_budget_action": "warn",
            "loop_guard_semantic_min_calls": 12,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 语义熔断(HB-2,2026-09-18):同一工具反复调用、**参数每轮都变而输出实质无变化**
/// → 熔断。既有「重复调用熔断」按「工具名+参数」指纹,对参数略变的空转无效
/// (实测 54 轮各不相同的命令烧 137 万 prompt token,遗留 L22)。
///
/// 形态:mock 的 [[tool_loop:...]] 每轮经 vary_tool_args 注入新 _mock_round(指纹互不相同),
/// 而 read 返回同一内容 → 只可能被输出侧判定抓住。
#[tokio::test]
async fn semantic_loop_guard_breaks_on_unchanged_output() {
    let _guard = test_lock().await;
    let app = test_app();
    let (_, char) = upload_character(app, "空转熔断.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    let args = r#"{"queries":[{"type":"character_prompt"}]}"#;
    let events = sse_events(
        app,
        &sid,
        &cid,
        &format!("[[tool_loop:read|20 {args}]]"),
        "agent",
    )
    .await;

    let broke = events
        .iter()
        .any(|e| e["type"] == "step" && e["step"] == "重复空转熔断");
    assert!(broke, "输出无变化的同工具空转应熔断并透出理由: {events:?}");
    // 对照:参数每轮都变,指纹熔断不该触发(说明抓住它的确实是输出侧判定)
    assert!(
        !events
            .iter()
            .any(|e| e["type"] == "step" && e["step"] == "重复调用熔断"),
        "本形态参数各不相同,不该走指纹熔断: {events:?}"
    );
    // 熔断发生在收到第 12 个同工具结果之后(每步各算各的窗口:agent 模式后续步骤
    // 会再起一轮工具循环,故这里只看**首次**熔断之前的调用数)
    let trip_at = events
        .iter()
        .position(|e| e["type"] == "step" && e["step"] == "重复空转熔断")
        .expect("熔断事件位置");
    let calls_before = events[..trip_at]
        .iter()
        .filter(|e| e["type"] == "tool_call")
        .count();
    assert_eq!(
        calls_before, 12,
        "默认下限 12:首次熔断前应恰有 12 次同工具调用(而非跑满 20 轮): {calls_before}"
    );
    let tool_calls = events.iter().filter(|e| e["type"] == "tool_call").count();
    let tool_results = events.iter().filter(|e| e["type"] == "tool_result").count();
    assert_eq!(
        tool_results, tool_calls,
        "已执行的工具调用必须都有 tool_result 终态(无悬挂): {events:?}"
    );
    assert!(
        events.iter().any(|e| e["type"] == "finish"),
        "熔断按正常终态收尾(不是 error): {events:?}"
    );
}

/// HB-7 接线:mvu_model / mvu_temperature 的 API 往返与清除语义。
/// 此前两者「可落盘、无 API 通路」(mvu_model 连消费点都没有),属死配置——
/// 本用例锁住接线的对外契约:GET 透出、PATCH 设置(trim)、越界拒绝、清除哨兵。
#[tokio::test]
async fn mvu_model_temperature_roundtrip_and_clear() {
    let _guard = test_lock().await;
    let app = test_app();

    // 默认:未配置(None → JSON null)
    let (_, s0) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(s0["mvu_model"], Value::Null, "默认应为未配置: {s0}");
    assert_eq!(s0["mvu_temperature"], Value::Null, "默认应为未配置: {s0}");

    // 设置:模型两侧空白应被 trim
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "mvu_model": "  aux-model  ", "mvu_temperature": 0.2 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, s1) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(s1["mvu_model"], json!("aux-model"), "应 trim 后落库: {s1}");
    assert_eq!(s1["mvu_temperature"], json!(0.2), "温度应落库: {s1}");

    // 越界拒绝(温度上限 2.0)
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "mvu_temperature": 3.0 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "越界应拒绝: {body}");

    // 清除:空串 = 回到与正文共用;负值 = 回到内置 0.3
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "mvu_model": "", "mvu_temperature": -1.0 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, s2) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(s2["mvu_model"], Value::Null, "空串应清除: {s2}");
    assert_eq!(s2["mvu_temperature"], Value::Null, "负值应清除: {s2}");
}
