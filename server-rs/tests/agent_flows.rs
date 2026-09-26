// 自定义 Agent 执行流程(custom 模式)集成测试:
//  - PUT/GET /api/agent-flows 往返;非法流程 400
//  - custom 模式执行:2 步流程 + 步骤提示词(宏)→ mock [[floors]] 回显断言
//    system 含「[本步指令]」、步骤顺序与跨步骤 vars 持久化、Step 事件 index/total
//  - custom 未启用 → 400
// 流程配置为全局共享,测试间用 test_lock 串行化(仿 prompt_inject.rs)。
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
    // 测试进程级:运行时主提示词目录指向空目录,避免真实项目根 AGENTS_RUNTIME.md 混入断言。
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // uuid 唯一名(避免多测试二进制并发共用同名目录);守卫随本闭包析构即回收。
        // 目录是否存在不影响语义:with_dir 关闭内置默认回退,读不到文件即视为「无提示词」,
        // 与「指向一个空目录」等价(全仓无测试写该文件)。
        let dir = kedai_server::utils::test_support::TempDataDir::new("test-runtime-prompt-empty");
        std::env::set_var("KEDAI_RUNTIME_PROMPT_DIR", dir.path());
    });
    APP.get_or_init(|| build_test_app().expect("构建测试应用失败"))
}

/// 串行化全局共享状态(流程配置)的测试
async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
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

/// 上传角色 + 建会话,返回 (sid, cid)
async fn new_session(app: &axum::Router) -> (String, String) {
    let (_, char) = upload_character(app, "流程测试.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    (sid, cid)
}

/// 以指定 agent_mode 发送消息,解析 SSE 事件列表
async fn sse_custom_events(
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

/// 合法的两步流程:分析(生成,带步骤提示词 1)→ 成文(生成,带步骤提示词 2)
fn two_step_flow() -> Value {
    json!({
        "enabled": true,
        "steps": [
            {
                "id": "s1", "name": "分析", "goal": "分析用户意图",
                "action": "direct", "generates": true, "enabled": true,
                "system_prompt": "{{setvar::步骤计数::1}}先分析意图",
                "temperature": 0.7, "max_tokens": 1024, "tools": null,
                "tool_choice": "auto", "parallel_tool_calls": false
            },
            {
                "id": "s2", "name": "成文", "goal": "生成正文",
                "action": "direct", "generates": true, "enabled": true,
                "system_prompt": "基于分析成文,步骤数={{getvar::步骤计数}}",
                "temperature": 0.8, "max_tokens": 2048, "tools": null
            }
        ]
    })
}

/// 恢复默认(未启用):固定 id 覆盖同一流程,避免流程库膨胀
async fn reset_flow(app: &axum::Router) {
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": { "id": "test-reset-flow", "name": "重置测试流程", "enabled": false, "steps": [] } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "重置流程配置应 200");
}

#[tokio::test]
async fn agent_flows_crud_roundtrip() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;

    // 默认未启用
    let (status, def) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(def["config"]["enabled"], false);

    // PUT 两步流程 → GET 回读一致
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": two_step_flow() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, got) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    assert_eq!(got["config"]["enabled"], true);
    let steps = got["config"]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["id"], "s1");
    assert_eq!(
        steps[0]["system_prompt"],
        "{{setvar::步骤计数::1}}先分析意图"
    );
    assert_eq!(steps[1]["max_tokens"], 2048);
    assert_eq!(steps[0]["tool_choice"], "auto");
    assert_eq!(steps[0]["parallel_tool_calls"], false);
    assert_eq!(
        steps[0]["tools"],
        Value::Null,
        "tools=null 表示该步不使用工具"
    );

    reset_flow(app).await;
}

#[tokio::test]
async fn agent_flows_library_select_delete() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;

    // PUT 无 id → 新建并选中(library.flows 增加,config 指向新流程)
    let (status, resp) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": two_step_flow() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let flows = resp["library"]["flows"].as_array().unwrap();
    assert!(
        flows.len() >= 2,
        "库应含内置/重置流程 + 新建流程: {flows:?}"
    );
    let new_id = resp["config"]["id"].as_str().unwrap().to_string();
    assert!(!new_id.is_empty(), "新建流程应自动分配 id");
    assert_eq!(resp["library"]["current_flow_id"], new_id);

    // POST /select 切换回内置流程
    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent-flows/select",
        json!({ "id": "builtin-coordination" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resp: {resp}");
    assert_eq!(resp["config"]["name"], "文学创作协调流程");
    assert_eq!(resp["library"]["current_flow_id"], "builtin-coordination");

    // select 不存在的 id → 400
    let (status, _) = send_json(
        app,
        "POST",
        "/api/agent-flows/select",
        json!({ "id": "no-such-flow" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // DELETE 新建流程 → 库减少,当前仍指向内置流程
    let (status, resp) = send_json(
        app,
        "DELETE",
        &format!("/api/agent-flows/{new_id}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resp: {resp}");
    assert_eq!(resp["library"]["current_flow_id"], "builtin-coordination");
    let ids: Vec<&str> = resp["library"]["flows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["id"].as_str())
        .collect();
    assert!(
        !ids.contains(&new_id.as_str()),
        "删除后库中不应再有该流程: {ids:?}"
    );

    // DELETE 不存在的 id → 400
    let (status, _) = send_json(app, "DELETE", "/api/agent-flows/no-such-flow", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    reset_flow(app).await;
}

#[tokio::test]
async fn agent_flows_invalid_flow_400() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;

    // 空步骤 → 400
    let (status, resp) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": { "enabled": true, "steps": [] } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(
        resp["error"]
            .as_str()
            .unwrap_or("")
            .contains("自定义流程为空"),
        "resp: {resp}"
    );

    // 无生成步骤(仅理解 + 反思)→ 400
    let (status, resp) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({
            "config": {
                "enabled": true,
                "steps": [
                    { "id": "a", "name": "理解", "goal": "理解意图", "action": "direct", "generates": false, "enabled": true },
                    { "id": "b", "name": "反思", "goal": "检查质量", "action": "reflect", "enabled": true }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(
        resp["error"].as_str().unwrap_or("").contains("生成步骤"),
        "resp: {resp}"
    );

    // 未注册白名单工具 → 400,不能在执行阶段静默忽略
    let (status, resp) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({
            "config": {
                "enabled": true,
                "steps": [
                    { "id": "a", "name": "生成", "goal": "生成正文", "action": "direct", "generates": true, "enabled": true, "tools": ["missing_tool"] }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(resp["error"].as_str().unwrap_or("").contains("未注册工具"));

    // function 必须指向步骤有效工具 → 400
    let (status, resp) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({
            "config": {
                "enabled": true,
                "steps": [
                    { "id": "a", "name": "生成", "goal": "生成正文", "action": "direct", "generates": true, "enabled": true,
                      "tools": ["read"], "tool_choice": "function", "tool_choice_function": "calculator" }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(resp["error"]
        .as_str()
        .unwrap_or("")
        .contains("不在有效工具内"));

    // 反思步骤带系统提示词 → 400
    let (status, resp) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({
            "config": {
                "enabled": true,
                "steps": [
                    { "id": "a", "name": "生成", "goal": "生成正文", "action": "direct", "generates": true, "enabled": true },
                    { "id": "b", "name": "反思", "goal": "检查质量", "action": "reflect", "enabled": true, "system_prompt": "不应支持" }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(
        resp["error"].as_str().unwrap_or("").contains("反思步骤"),
        "resp: {resp}"
    );

    // 二维依赖边(二维批次 1):成环 → 400 并点名环上节点
    let (status, resp) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({
            "config": {
                "enabled": true,
                "steps": [
                    { "id": "a", "name": "甲", "goal": "甲目标", "action": "direct", "generates": true, "enabled": true, "inputs": ["b"] },
                    { "id": "b", "name": "乙", "goal": "乙目标", "action": "direct", "generates": true, "enabled": true, "inputs": ["a"] }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    let err = resp["error"].as_str().unwrap_or("");
    assert!(err.contains("环"), "resp: {resp}");
    assert!(
        err.contains("甲") && err.contains("乙"),
        "应点名环上节点: {resp}"
    );

    // 悬空上游 → 400(引用了不存在或未启用的步骤 id)
    let (status, resp) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({
            "config": {
                "enabled": true,
                "steps": [
                    { "id": "a", "name": "生成", "goal": "生成正文", "action": "direct", "generates": true, "enabled": true, "inputs": ["missing"] }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(
        resp["error"]
            .as_str()
            .unwrap_or("")
            .contains("不存在或未启用"),
        "resp: {resp}"
    );

    // 节点档位(二维批次 6a):loose / strict 都有已实现的语义 → 放行(不再 400);
    // 未知取值 → 400,不允许静默无效
    let (status, resp) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({
            "config": {
                "enabled": true,
                "steps": [
                    { "id": "a", "name": "生成", "goal": "生成正文", "action": "direct", "generates": true, "enabled": true, "kind": "strict" }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "strict 档位应可保存: {resp}");

    let (status, resp) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({
            "config": {
                "enabled": true,
                "steps": [
                    { "id": "a", "name": "生成", "goal": "生成正文", "action": "direct", "generates": true, "enabled": true, "kind": "medium" }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(
        resp["error"].as_str().unwrap_or("").contains("档位"),
        "resp: {resp}"
    );

    reset_flow(app).await;
}

#[tokio::test]
async fn custom_mode_runs_steps_with_prompts_and_progress() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;

    // 启用两步流程
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": two_step_flow() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (sid, cid) = new_session(app).await;
    let events = sse_custom_events(app, &sid, &cid, "查看注入 [[floors]]", "custom").await;

    // 1) Step 事件携带 index/total 进度(两步 → 1/2、2/2 各出现)
    let steps: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "step" && e["index"].is_u64())
        .collect();
    assert!(
        !steps.is_empty(),
        "custom 模式应发送带进度的事件: {events:?}"
    );
    let indices: Vec<(u64, u64)> = steps
        .iter()
        .map(|e| (e["index"].as_u64().unwrap(), e["total"].as_u64().unwrap()))
        .collect();
    assert!(indices.contains(&(1, 2)), "应出现 1/2 进度: {indices:?}");
    assert!(indices.contains(&(2, 2)), "应出现 2/2 进度: {indices:?}");

    // 2) finish 内容 = 最后一步生成结果(mock 回显全部 LLM 消息)
    let finish = events
        .iter()
        .find(|e| e["type"] == "finish")
        .expect("缺 finish 事件");
    let content = finish["content"].as_str().unwrap_or("");
    // 第二步步骤提示词追加到 system 末尾(mock 回显 [system] 行内)
    assert!(
        content.contains("[本步指令]"),
        "步骤提示词应带 [本步指令] 标记:\n{content}"
    );
    assert!(
        content.contains("基于分析成文,步骤数=1"),
        "第二步提示词应展开,且 getvar 读到第一步 setvar 的 1:\n{content}"
    );
    // 用户消息与角色开场白在生成视图中
    assert!(
        content.contains("[user] 查看注入 [[floors]]"),
        "生成视图应含用户消息:\n{content}"
    );
    assert!(
        content.contains("[assistant] 你好,我是测试角色"),
        "生成视图应含开场白:\n{content}"
    );

    // 3) 非 custom 模式事件不带进度字段(回归:fast 不携带 index/total)
    let events_fast = sse_custom_events(app, &sid, &cid, "查看注入 [[floors]]", "fast").await;
    let step_evts: Vec<&Value> = events_fast.iter().filter(|e| e["type"] == "step").collect();
    assert!(!step_evts.is_empty());
    assert!(
        step_evts.iter().all(|e| e.get("index").is_none()),
        "fast 模式不应携带 index: {step_evts:?}"
    );

    reset_flow(app).await;
}

#[tokio::test]
async fn custom_mode_disabled_returns_400() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;

    let (sid, cid) = new_session(app).await;
    // 未启用流程(默认)→ chat/send 400
    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "session_id": sid,
                "character_id": cid,
                "message": "你好",
                "agent_mode": "custom",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "未启用流程的 custom 请求应 400"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        body["error"].as_str().unwrap_or("").contains("自定义模式"),
        "resp: {body}"
    );

    // /api/agent/plan 预览同样 400
    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent/plan",
        json!({ "message": "你好", "agent_mode": "custom" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(
        resp["error"].as_str().unwrap_or("").contains("自定义模式"),
        "resp: {resp}"
    );
}

#[tokio::test]
async fn agent_plan_previews_custom_steps() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;

    let (status, _) = send_json(
        app,
        "PUT",
        "/api/agent-flows",
        json!({ "config": two_step_flow() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 预览:custom 计划按配置步骤生成(disabled 步骤被过滤)
    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent/plan",
        json!({ "message": "你好", "agent_mode": "custom" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resp: {resp}");
    assert_eq!(resp["summary"], "自定义流程:共 2 步");
    let steps = resp["plan"]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["name"], "分析");
    assert_eq!(steps[1]["name"], "成文");

    reset_flow(app).await;
}

// ==================== 流程搬运:导出/导入(二维批次 7a) ====================

/// 造一个带 id 的单步流程(搬运测试用;步骤 id 固定便于断言引用)
fn movable_flow(id: &str, name: &str, goal: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "enabled": true,
        "steps": [{
            "id": "n1", "name": "节点", "goal": goal,
            "action": "direct", "generates": true, "enabled": true,
            "tools": null, "tool_choice": "auto"
        }]
    })
}

/// 造一个挂载子流程的单步流程
fn mounting_flow(id: &str, name: &str, sub_id: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "enabled": true,
        "steps": [{
            "id": "n1", "name": "挂载", "goal": "跑子流程",
            "action": "direct", "generates": true, "enabled": true,
            "sub_flow_id": sub_id, "tools": null, "tool_choice": "auto"
        }]
    })
}

async fn put_flow(app: &axum::Router, config: Value) -> StatusCode {
    send_json(app, "PUT", "/api/agent-flows", json!({ "config": config }))
        .await
        .0
}

async fn delete_flow(app: &axum::Router, id: &str) -> StatusCode {
    send_json(
        app,
        "DELETE",
        &format!("/api/agent-flows/{}", id),
        json!({}),
    )
    .await
    .0
}

/// 导出「入口 + 可达子流程闭包」→ 空库导入:引用必须原样可达。
/// 旧的「只导单流程」路径会把这份文件变成悬空引用,导入必然 400。
#[tokio::test]
async fn flow_bundle_roundtrip_keeps_sub_flow_reference() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;
    let (sub, main) = ("int-bundle-sub", "int-bundle-main");
    assert_eq!(
        put_flow(app, movable_flow(sub, "搬运子", "子目标")).await,
        StatusCode::OK
    );
    assert_eq!(
        put_flow(app, mounting_flow(main, "搬运主", sub)).await,
        StatusCode::OK
    );

    let (status, resp) = send_json(
        app,
        "GET",
        &format!("/api/agent-flows/export?id={main}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resp: {resp}");
    let bundle = resp["bundle"].clone();
    assert_eq!(bundle["kedai_flow_bundle"], 1, "包版本键");
    assert_eq!(bundle["root_id"], main);
    assert_eq!(bundle["flows"].as_array().unwrap().len(), 2, "应含闭包");

    // 模拟「另一台机器」:把两份都删掉(先删引用方),再导入搬回来的包
    assert_eq!(delete_flow(app, main).await, StatusCode::OK);
    assert_eq!(delete_flow(app, sub).await, StatusCode::OK);
    let (status, after_delete) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !after_delete["library"]["flows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["id"] == main),
        "前置:两份已删除"
    );

    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent-flows/import",
        json!({ "flows": bundle["flows"], "root_id": main }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resp: {resp}");
    assert_eq!(resp["report"]["imported"], 2);
    assert_eq!(resp["report"]["skipped"], 0);
    assert_eq!(
        resp["library"]["current_flow_id"], main,
        "导入后选中入口流程"
    );
    let imported = resp["library"]["flows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == main)
        .expect("入口流程应搬回来");
    assert_eq!(
        imported["steps"][0]["sub_flow_id"], sub,
        "子流程引用必须原样可达"
    );

    // 收尾:先删引用方再删子流程(与导入顺序相反)
    assert_eq!(delete_flow(app, main).await, StatusCode::OK);
    assert_eq!(delete_flow(app, sub).await, StatusCode::OK);
    reset_flow(app).await;
}

/// 同 id 内容不同 → 分配新 id 新增,本机既有流程逐字节不变。
#[tokio::test]
async fn flow_bundle_import_remaps_conflicting_id() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;
    let id = "int-conflict";
    assert_eq!(
        put_flow(app, movable_flow(id, "冲突流程", "文件里的目标")).await,
        StatusCode::OK
    );
    let (_, resp) = send_json(
        app,
        "GET",
        &format!("/api/agent-flows/export?id={id}"),
        json!({}),
    )
    .await;
    let bundle = resp["bundle"].clone();

    // 本机把同 id 的流程改掉(内容与文件不同)
    assert_eq!(
        put_flow(app, movable_flow(id, "冲突流程", "本机改过的目标")).await,
        StatusCode::OK
    );

    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent-flows/import",
        json!({ "flows": bundle["flows"], "root_id": id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resp: {resp}");
    assert_eq!(resp["report"]["imported"], 1);
    assert_eq!(resp["report"]["renamed"].as_array().unwrap().len(), 1);
    let remap = &resp["report"]["renamed"][0];
    assert_eq!(remap["old_id"], id);
    let new_id = remap["new_id"].as_str().unwrap().to_string();
    assert_ne!(new_id, id, "必须分配新 id");

    let flows = resp["library"]["flows"].as_array().unwrap().clone();
    let local = flows.iter().find(|f| f["id"] == id).unwrap();
    assert_eq!(
        local["steps"][0]["goal"], "本机改过的目标",
        "本机既有流程绝不被覆盖"
    );
    let imported = flows.iter().find(|f| f["id"] == new_id.as_str()).unwrap();
    assert_eq!(imported["steps"][0]["goal"], "文件里的目标");
    assert_eq!(
        resp["library"]["current_flow_id"], new_id,
        "root_id 经重映射后仍选中导入的那一份"
    );

    // 同一份包再导一次:内容已一致 → 幂等跳过,不再新增
    let (status, repeat) = send_json(
        app,
        "POST",
        "/api/agent-flows/import",
        json!({ "flows": bundle["flows"], "root_id": id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resp: {repeat}");
    assert_eq!(repeat["report"]["imported"], 0, "重复导入应幂等");
    assert_eq!(repeat["report"]["skipped"], 1);
    assert_eq!(
        repeat["library"]["flows"].as_array().unwrap().len(),
        flows.len(),
        "库规模不变"
    );

    assert_eq!(delete_flow(app, &new_id).await, StatusCode::OK);
    assert_eq!(delete_flow(app, id).await, StatusCode::OK);
    reset_flow(app).await;
}

/// 三种非法输入都必须 400,且**库逐字节不变**(原子性)。
#[tokio::test]
async fn flow_bundle_import_rejects_bad_input_atomically() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;
    let (_, before) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    let before_json = serde_json::to_string(&before["library"]).unwrap();

    // 1) 悬空引用:文件里的流程引用了不存在的子流程
    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent-flows/import",
        json!({ "flows": [mounting_flow("int-dangling", "悬空", "int-missing-sub")] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    let msg = resp["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains("子流程") && msg.contains("不存在"),
        "错误应点名悬空引用:{msg}"
    );

    // 2) 空批次
    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent-flows/import",
        json!({ "flows": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(
        resp["error"]
            .as_str()
            .unwrap_or_default()
            .contains("没有流程"),
        "resp: {resp}"
    );

    // 3) 批内重复 id(文件自相矛盾)
    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent-flows/import",
        json!({ "flows": [
            movable_flow("int-dup", "重复", "A"),
            movable_flow("int-dup", "重复", "B"),
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(
        resp["error"].as_str().unwrap_or_default().contains("重复"),
        "resp: {resp}"
    );

    // 三次失败后库必须与开工前逐字节相同(旧逐个 PUT 路径会留下半份)
    let (_, after) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    assert_eq!(
        serde_json::to_string(&after["library"]).unwrap(),
        before_json,
        "校验失败不得改变库(原子)"
    );
    reset_flow(app).await;
}

/// 导出缺省 = 全库(root_id 取当前选择);未知 id 400。
#[tokio::test]
async fn flow_bundle_export_defaults_to_whole_library() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;
    let id = "int-export-all";
    assert_eq!(
        put_flow(app, movable_flow(id, "全库导出", "目标")).await,
        StatusCode::OK
    );
    let (_, lib) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    let size = lib["library"]["flows"].as_array().unwrap().len();

    let (status, resp) = send_json(app, "GET", "/api/agent-flows/export", json!({})).await;
    assert_eq!(status, StatusCode::OK, "resp: {resp}");
    assert_eq!(
        resp["bundle"]["flows"].as_array().unwrap().len(),
        size,
        "全库导出应带走每一份流程"
    );
    assert_eq!(
        resp["bundle"]["root_id"], id,
        "root_id = 当前选中流程(刚保存的那份)"
    );

    let (status, resp) = send_json(
        app,
        "GET",
        "/api/agent-flows/export?id=int-no-such-flow",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(
        resp["error"]
            .as_str()
            .unwrap_or_default()
            .contains("不存在"),
        "resp: {resp}"
    );
    assert_eq!(delete_flow(app, id).await, StatusCode::OK);
    reset_flow(app).await;
}

/// 导入覆盖模式(A 批 B4,端点级):`on_conflict=replace` 覆盖同 id 者、`replaced` 如实报告,
/// 未知取值 400(覆盖不可逆,静默回退到 rename 会让用户以为已覆盖)。
#[tokio::test]
async fn flow_bundle_import_replace_overwrites_via_api() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_flow(app).await;
    let id = "int-replace";
    assert_eq!(
        put_flow(app, movable_flow(id, "覆盖流程", "文件里的目标")).await,
        StatusCode::OK
    );
    let (_, resp) = send_json(
        app,
        "GET",
        &format!("/api/agent-flows/export?id={id}"),
        json!({}),
    )
    .await;
    let bundle = resp["bundle"].clone();

    // 本机把同 id 的流程改掉(内容与文件不同)
    assert_eq!(
        put_flow(app, movable_flow(id, "覆盖流程", "本机改过的目标")).await,
        StatusCode::OK
    );

    // 未知取值:严格拒绝(不静默按 rename 处理)
    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent-flows/import",
        json!({ "flows": bundle["flows"], "root_id": id, "on_conflict": "overwrite" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未知取值应 400: {resp}");
    assert!(
        resp["error"]
            .as_str()
            .unwrap_or("")
            .contains("rename/replace"),
        "文案应给出可选值: {resp}"
    );

    // replace:同 id 内容被**覆盖**(id 不变),报告里出现 replaced
    let (status, resp) = send_json(
        app,
        "POST",
        "/api/agent-flows/import",
        json!({ "flows": bundle["flows"], "root_id": id, "on_conflict": "replace" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resp: {resp}");
    assert_eq!(resp["report"]["imported"], 1, "resp: {resp}");
    assert!(
        resp["report"]["renamed"].as_array().unwrap().is_empty(),
        "覆盖模式不产生新 id: {resp}"
    );
    let replaced = resp["report"]["replaced"].as_array().unwrap();
    assert_eq!(replaced.len(), 1, "resp: {resp}");
    assert_eq!(replaced[0]["id"], id);
    assert_eq!(replaced[0]["name"], "覆盖流程");

    let flows = resp["library"]["flows"].as_array().unwrap().clone();
    let hit: Vec<&Value> = flows.iter().filter(|f| f["id"] == id).collect();
    assert_eq!(hit.len(), 1, "覆盖不是新增:同 id 只应有一份");
    assert_eq!(
        hit[0]["steps"][0]["goal"], "文件里的目标",
        "内容应被文件覆盖(本机那次改动被替换——这正是覆盖模式的语义)"
    );

    delete_flow(app, id).await;
    reset_flow(app).await;
}
