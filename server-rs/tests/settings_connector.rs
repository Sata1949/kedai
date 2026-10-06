// 连接器切换测试(独立进程/独立 app 实例,不影响 api_integration.rs 的共享 mock app):
// mock 连接器下保存 API 设置(Base URL / Key)→ 自动切换到 openai-compatible,用户配置立即生效。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use kedai_server::services::settings_service::{
    default_literary_reflect_prompt, default_reflect_prompt,
};
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

/// 本文件所有用例共享同一个 app 实例(进程级 settings),且多数用例会 PUT
/// /api/settings(其中 Base URL/Key 会切换连接器)。此前无串行化,cargo test 默认
/// 并行下互相踩踏:实测 `mock_auto_switches_to_openai_on_save` 断言的「初始必须是
/// mock 连接器」会被并发用例保存 base_url 抢先切走而偶发失败(单跑恒过)。
async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

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

#[tokio::test]
async fn mock_auto_switches_to_openai_on_save() {
    let _guard = test_lock().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mock_provider = axum::Router::new().route(
        "/v1/models",
        axum::routing::get(|| async {
            axum::Json(json!({ "data": [{ "id": "local-test-model" }] }))
        }),
    );
    tokio::spawn(async move { axum::serve(listener, mock_provider).await.unwrap() });
    let app = test_app();

    // 初始:mock 连接器
    let (_, info0) = send_json(app, "GET", "/api/settings/info", json!({})).await;
    assert_eq!(info0["connector"], json!("mock"));

    // GET /api/settings:返回脱敏 Key,不含明文
    let (status, s) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(s["openai_base_url"].is_string());
    assert!(s["api_key_masked"].is_string());
    assert!(s["has_api_key"].is_boolean());
    assert!(s["model"].is_string());
    assert!(s["default_temperature"].is_number());
    assert!(s["default_top_p"].is_number());
    assert!(s["default_max_tokens"].is_number());
    assert!(s["max_context_tokens"].is_number());

    // 部分字段更新:默认参数(不触发连接器切换)
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "default_top_p": 0.7,
            "max_context_tokens": 65536,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "更新设置失败: {r}");
    assert_eq!(r["settings"]["default_top_p"], json!(0.7));
    assert_eq!(r["settings"]["max_context_tokens"], json!(65536));
    // 非法 top_p 被忽略
    let (_, r3) = send_json(app, "PUT", "/api/settings", json!({ "default_top_p": 5.0 })).await;
    assert_eq!(r3["settings"]["default_top_p"], json!(0.7));

    // 任一严格字段非法时整批拒绝,前面的合法字段也不得半提交。
    let (invalid_status, invalid) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "default_top_p": 0.4, "max_tool_rounds": 0 }),
    )
    .await;
    assert_eq!(invalid_status, StatusCode::BAD_REQUEST);
    assert!(invalid["error"]
        .as_str()
        .unwrap()
        .contains("max_tool_rounds"));
    let (_, after_invalid) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(after_invalid["default_top_p"], json!(0.7));

    // 保存 API 地址 + Key + 模型 → 自动切换到 openai-compatible,配置生效
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "openai_base_url": format!("http://{address}/v1"),
            "openai_api_key": "sk-test-123456",
            "model": "deepseek-chat",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "保存失败: {r}");
    assert_eq!(r["ok"], json!(true));
    assert_eq!(
        r["settings"]["openai_base_url"],
        json!(format!("http://{address}/v1"))
    );
    assert_eq!(r["settings"]["model"], json!("deepseek-chat"));
    assert_eq!(r["settings"]["has_api_key"], json!(true));
    // Key 已脱敏,不含明文
    assert_eq!(r["settings"]["api_key_masked"], json!("****3456"));
    assert!(!r["settings"]["api_key_masked"]
        .as_str()
        .unwrap()
        .contains("sk-test"));

    // 连接器已切换为 openai-compatible
    let (_, info) = send_json(app, "GET", "/api/settings/info", json!({})).await;
    assert_eq!(info["connector"], json!("openai-compatible"));

    // 从 API 加载模型列表接口(新连接器下调用可用;离线时回退当前模型)
    let (status, rm) = send_json(app, "POST", "/api/settings/refresh-models", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rm["ok"], json!(true));
    assert!(rm["models"].is_array());

    // 连接测试(无真实网络,Key 已配置即返回 ok)
    let (_, conn) = send_json(app, "POST", "/api/settings/connect", json!({})).await;
    assert_eq!(conn["ok"], json!(true));
    assert_eq!(conn["endpoint_reachable"], json!(true));
    assert_eq!(conn["authenticated"], json!(true));
    assert_eq!(conn["fallback_used"], json!(false));
    assert_eq!(conn["http_status"], json!(200));

    // 持久化文件包含保存的配置(build_test_app 的进程级共享数据目录;只读断言,不建目录)
    let settings_path = std::env::temp_dir()
        .join(format!("kedai-test-{}", std::process::id()))
        .join("settings.json");
    let text = std::fs::read_to_string(&settings_path).expect("settings.json 应已持久化");
    assert!(text.contains(&format!("http://{address}/v1")));
    assert!(text.contains("deepseek-chat"));
    assert!(text.contains("65536"));
}

/// 模式隔离 API 级回归(WP7 双向污染矩阵 D-2):写 task 覆盖层 agent_system_prompt
/// 不得改变 roleplay 扁平值;GET ?mode=task 返回覆盖值,?mode=roleplay 返回扁平权威值。
///
/// 卫生复位口径(2026-09-26 修顺序竞争):本 binary 共享进程级 app,复位必须写回**测试
/// 开始时的 task 视角原值**,不得写空串——`for_mode(Task)` 对 `Some("")` 是「显式清空」
/// 语义(不回退内置任务默认词,params.rs),写空串会让同 binary 的
/// `roleplay_default_prompt_visible_on_fresh_install` 第二条断言(「task 缺省应有任务向
/// 默认词」)按用例运行顺序偶发变红(实测文件级连跑 6 次红 3 次)。复位为原值后本用例
/// 对共享状态的净效果为零,顺序无关。
#[tokio::test]
async fn task_overlay_does_not_leak_into_roleplay_settings() {
    let _guard = test_lock().await;
    let app = test_app();

    // 记录 roleplay 扁平权威值(共享 app,可能已被同 binary 其他测试写动,取现场值)
    let (status, before_rp) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let flat_before = before_rp["agent_system_prompt"].clone();
    assert!(flat_before.is_string(), "扁平值应为裸字符串(线格式不变)");
    // 记录 task 覆盖层现值(与 roleplay 同理取现场值;这是复位目标)
    let (status, before_task) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let task_before = before_task["agent_system_prompt"].clone();
    assert!(task_before.is_string(), "task 视角应为裸字符串(线格式不变)");

    // 写 task 覆盖层
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "agent_system_prompt": "TASK-OVERLAY-MARKER 任务专用提示词" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写 task 覆盖层失败: {r}");

    // task 视角返回覆盖值;roleplay 视角扁平值不变(双向隔离)
    let (_, task_view) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(
        task_view["agent_system_prompt"],
        json!("TASK-OVERLAY-MARKER 任务专用提示词"),
        "task 应读到覆盖层值"
    );
    let (_, rp_view) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        rp_view["agent_system_prompt"], flat_before,
        "roleplay 扁平值不得被 task 覆盖层污染"
    );

    // 卫生复位:写回本用例开始时的 task 视角原值(不是空串——理由见用例文档注释)。
    // API 三态语义中无「复位 None」操作,故以「记录现场值 + 写回」实现净零影响。
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "agent_system_prompt": task_before }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, rp_after) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(rp_after["agent_system_prompt"], flat_before);
    let (_, task_after) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(
        task_after["agent_system_prompt"], task_before,
        "task 覆盖层应复位为原值(共享 app 下对本用例之外零影响)"
    );
}

/// 提示词预览按模式合并(批次 2,docs/契约-协议与配置.md 第五节):
/// task 模式追加规划器/执行者/汇总者三层固定提示词层 + 执行者工具纪律段
/// (文本与 task_core/prompt_consts.rs 单一来源逐字一致,预览即真实下发);
/// roleplay(缺省)不含。这些层都是内置指令,与 task 覆盖层状态无关
/// (共享 app 下不受同 binary 其他测试写动影响)。
/// 注意工具纪律段是**条件注入**:只有本轮真的下发了工具的执行者才拿到它
/// (legacy 的步骤没有工具,不发这一段);预览无「本轮有没有工具」的概念,
/// 故按「工具档」形态列出——口径写在契约文档第五节,避免读者以为它恒在。
#[tokio::test]
async fn prompt_preview_follows_mode() {
    let _guard = test_lock().await;
    let app = test_app();

    // 缺省 = roleplay:不得含任务固定提示词层
    let (status, rp) = send_json(app, "GET", "/api/settings/prompt-preview", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let rp_layers = rp["layers"].as_array().expect("layers 应为数组");
    assert!(
        !rp_layers
            .iter()
            .any(|l| l["source"].as_str() == Some("task_planner_prompt")),
        "roleplay 预览不得含任务固定提示词层"
    );

    // task:三层固定提示词 + 工具纪律段齐备,文本含内置指令原文
    let (status, t) = send_json(
        app,
        "GET",
        "/api/settings/prompt-preview?mode=task",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let layers = t["layers"].as_array().expect("layers 应为数组");
    for (src, needle) in [
        ("task_planner_prompt", "你是任务规划器"),
        ("task_executor_prompt", "你是任务执行者"),
        ("task_summarizer_prompt", "你是任务汇总者"),
        ("task_executor_tool_discipline", "自测通过即收尾"),
    ] {
        let hit = layers.iter().any(|l| {
            l["source"].as_str() == Some(src)
                && l["content"]
                    .as_str()
                    .map(|c| c.contains(needle))
                    .unwrap_or(false)
        });
        assert!(hit, "task 预览缺 {src} 层");
    }
}

/// MCP 设置(批次 6.2):GET 透出默认 关/空列表;PUT 全量替换并做卫生清理
/// (缺命令条目被丢弃);GET 往返还原;task 覆盖层与扁平层互不影响。
#[tokio::test]
async fn mcp_settings_put_get_roundtrip() {
    let _guard = test_lock().await;
    let app = test_app();

    // 默认:关 + 空列表
    let (status, s) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(s["mcp_enabled"], json!(false), "MCP 默认关闭: {s}");
    assert_eq!(s["mcp_servers"], json!([]), "MCP 服务器列表默认空: {s}");

    // PUT:开关 + 服务器列表(含一个缺命令的坏条目,应被清理)
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "mcp_enabled": true,
            "mcp_servers": [
                { "name": " fs ", "command": "npx", "args": ["-y", "@mcp/fs"] },
                { "name": "broken", "command": "" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(r["settings"]["mcp_enabled"], json!(true));
    let servers = r["settings"]["mcp_servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1, "缺命令条目应被清理: {servers:?}");
    assert_eq!(servers[0]["name"], json!("fs"), "名称应 trim");
    assert_eq!(servers[0]["enabled"], json!(true), "条目 enabled 缺省 true");

    // GET 往返还原
    let (_, after) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(after["mcp_enabled"], json!(true));
    assert_eq!(after["mcp_servers"].as_array().unwrap().len(), 1);

    // task 覆盖层独立:PUT ?mode=task 只写覆盖层,扁平层(roleplay 视图)不变
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "mcp_enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, task_view) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(task_view["mcp_enabled"], json!(false), "task 覆盖应生效");
    let (_, rp_view) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(
        rp_view["mcp_enabled"],
        json!(true),
        "扁平层不受 task 覆盖影响"
    );

    // 还原现场:共享 app,别给同 binary 其他测试留状态(扁平层回默认;
    // task 覆盖层无「清除」语义,留着 Some(false) 不影响其他测试——无断言依赖 task 的 mcp 字段)
    let _ = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "mcp_enabled": false, "mcp_servers": [] }),
    )
    .await;
}

/// TM-SET-2:两开关退役后,设置 API 不再认识 `task_persona_full` /
/// `task_prompt_inject_enabled`——PUT 带旧键**静默忽略**(serde 未声明字段不进
/// UpdateSettingsBody),200 且不落任何有效值;GET 响应不再含这两个键。
#[tokio::test]
async fn retired_task_switches_ignored_by_settings_api() {
    let _guard = test_lock().await;
    let app = test_app();

    // 旧客户端带旧键 PUT:不 400、不报错(向后兼容由 serde 缺省容忍提供)
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_persona_full": true, "task_prompt_inject_enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "旧键应被静默忽略: {r}");
    assert!(
        r["settings"].get("task_persona_full").is_none(),
        "响应不应再投影已退役字段: {r}"
    );

    // GET 两视图都不得再出现退役键
    for mode in ["task", "roleplay"] {
        let (_, s) = send_json(app, "GET", &format!("/api/settings?mode={mode}"), json!({})).await;
        assert!(
            s.get("task_persona_full").is_none() && s.get("task_prompt_inject_enabled").is_none(),
            "{mode} 视图不应含退役字段: {s}"
        );
    }
}

/// 新装/首装(含 Android)设置接口返回非空的角色扮演默认提示词:
/// build_test_app 用全新临时数据目录(无 settings.json)→ 走 from_config 路径,
/// 正是手机端首装场景。修复前该字段为空串,面板与执行时都拿不到默认人设词。
#[tokio::test]
async fn roleplay_default_prompt_visible_on_fresh_install() {
    let _guard = test_lock().await;
    let app = test_app();
    let (status, s) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let p = s["agent_system_prompt"].as_str().unwrap_or("");
    assert!(!p.trim().is_empty(), "首装角色扮演默认提示词不得为空: {s}");
    for ph in [
        "{{char}}",
        "{{personality}}",
        "{{scenario}}",
        "{{world_info}}",
    ] {
        assert!(p.contains(ph), "默认词应含占位符 {ph} 供宏展开");
    }
    assert!(p.contains("【创作总纲】"), "默认词应含创作总纲段: {p:.80}");

    // 模式隔离不因新默认退化:task 视角仍是任务向默认词,不含角色扮演宏
    let (_, t) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    let tp = t["agent_system_prompt"].as_str().unwrap_or("");
    assert!(!tp.trim().is_empty(), "task 缺省应有任务向默认词: {t}");
    assert!(
        !tp.contains("{{char}}"),
        "task 默认词不得继承角色扮演宏: {tp:.80}"
    );
}

/// TM-SET-1:任务向缺省生成参数的 API 可见性——task 视图返回任务缺省
/// (温度 0.3 / Top-P 1.0 / 输出上限「任务缺省与扁平值只抬不压的较大者」),
/// roleplay 视图仍读扁平权威值,互不影响。
/// 断言取现场值推导(本 binary 共享 app,其他用例可能已改扁平值),不做固定数假设。
#[tokio::test]
async fn task_generation_defaults_visible_in_task_view() {
    let _guard = test_lock().await;
    let app = test_app();

    let (_, rp) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    let flat_max = rp["default_max_tokens"]
        .as_u64()
        .expect("扁平 max_tokens 应为数字");
    let (status, t) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        t["default_temperature"],
        json!(0.3),
        "任务缺省温度 0.3(不沿用扁平值): {t}"
    );
    assert_eq!(
        t["default_top_p"],
        json!(1.0),
        "任务缺省 Top-P 1.0(不沿用扁平值): {t}"
    );
    assert_eq!(
        t["default_max_tokens"].as_u64(),
        Some(flat_max.max(8192)),
        "任务输出上限 = max(扁平值, 8192)(只抬不压)"
    );

    // 显式写任务覆盖值 → task 视图切换、扁平(roleplay 视图)不变
    let flat_temp = rp["default_temperature"].clone();
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "default_temperature": 0.55 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写任务覆盖应 200: {r}");
    let (_, t2) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(t2["default_temperature"], json!(0.55), "task 覆盖应生效");
    let (_, rp2) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        rp2["default_temperature"], flat_temp,
        "roleplay 扁平值不得被任务覆盖污染"
    );

    // 卫生复位:写回现场原值(覆盖层无「复位 None」操作,写回等值即净零)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "default_temperature": 0.3 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// TM-SET-1:任务模式默认连接的 API 往返与校验——
/// 非空必须是已存在且启用的连接(否则 400 点名连接);写入后 task 视图读到、
/// roleplay 视图(扁平层)不受影响;空串 = 清除(跟随默认连接)。
#[tokio::test]
async fn task_default_connection_roundtrip_and_validation() {
    let _guard = test_lock().await;
    let app = test_app();

    // 现场值(共享 app;本用例净零复位目标)
    let (_, before_task) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    let task_before = before_task["task_default_connection_id"].clone();
    let (_, before_rp) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    let flat_before = before_rp["task_default_connection_id"].clone();

    // 不存在的连接 → 400
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_default_connection_id": "no-such-connection" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "无效引用应 400: {r}");

    // 取一条真实的启用连接(默认连接由 from_config 播种,测试环境必然存在)
    let (_, s) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    let conn_id = s["connections"]
        .as_array()
        .expect("connections 应为数组")
        .iter()
        .find(|c| c["enabled"].as_bool() == Some(true))
        .and_then(|c| c["id"].as_str())
        .expect("应存在至少一条启用连接")
        .to_string();

    // 设置 → task 视图读到;roleplay 视图(扁平层)不变
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_default_connection_id": conn_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写任务默认连接应 200: {r}");
    assert_eq!(r["settings"]["task_default_connection_id"], json!(conn_id));
    let (_, task_view) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(
        task_view["task_default_connection_id"],
        json!(conn_id),
        "task 视图应读到覆盖值"
    );
    let (_, rp_view) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        rp_view["task_default_connection_id"], flat_before,
        "扁平层(roleplay 视图)不受 task 覆盖影响"
    );

    // 空串 = 清除(回到跟随默认连接)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_default_connection_id": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, cleared) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(cleared["task_default_connection_id"], json!(""));

    // 卫生复位:写回现场原值(Some("") 与缺省同效 = 跟随默认连接)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_default_connection_id": task_before }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// TM-SET-3:全局字段(task 模式编辑也生效)——搜索端点 / 授权三档 / 子代理三参数 /
/// 回退快照改为**直写扁平**(执行期读全局快照,模式覆盖永不生效)。本用例锁定:
/// PUT ?mode=task 写入后,task 视图与 roleplay 视图读到同一个值(两模式共用一份),
/// 不再是「写覆盖层但运行期不读」的静默死写。
#[tokio::test]
async fn global_fields_written_from_task_mode_are_effective() {
    let _guard = test_lock().await;
    let app = test_app();

    // 现场值(共享 app;净零复位目标)
    let (_, before) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    let before_depth = before["subagent_max_depth"].clone();
    let before_undo = before["undo_enabled"].clone();

    // 任务模式写入 → 两视图一致(直写扁平)
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "subagent_max_depth": 3, "undo_enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "任务模式写全局字段应 200: {r}");
    let (_, task_view) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    let (_, rp_view) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        task_view["subagent_max_depth"],
        json!(3),
        "task 视图应读到新值: {task_view}"
    );
    assert_eq!(
        rp_view["subagent_max_depth"],
        json!(3),
        "全局字段两模式共用(roleplay 视图同值): {rp_view}"
    );
    assert_eq!(task_view["undo_enabled"], json!(false));
    assert_eq!(rp_view["undo_enabled"], json!(false));

    // 卫生复位(写回现场值;同样是全局写入路径)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "subagent_max_depth": before_depth, "undo_enabled": before_undo }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 文学能力包双开关(LIT-1,2026-10-05):角色扮演侧开关是**纯扁平字段**——经任一模式写入
/// 都直写扁平、**不得产生** task 覆盖层(该侧没有覆盖层,走 apply! 会写出无意义的死写);
/// 任务侧开关对称编码包:mode=task 写覆盖层、roleplay 视图不受其影响。
/// 共享 app 下取现场值写入 + 卫生复位(净零影响,顺序无关)。
#[tokio::test]
async fn literary_bundle_switches_are_flat_and_overlay_respectively() {
    let _guard = test_lock().await;
    let app = test_app();

    // 现场值(共享 app;净零复位目标)
    let (_, before_rp) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    let flat_before = before_rp["literary_bundle_enabled"].clone();
    assert!(flat_before.is_boolean(), "扁平开关应为布尔:{before_rp}");
    let (_, before_task) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    let task_before = before_task["task_literary_bundle_enabled"].clone();

    // 角色扮演视图写角色扮演侧开关:直写扁平,两视图都读到(全局扁平字段)
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写角色扮演侧开关失败:{r}");
    let (_, rp_view) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(rp_view["literary_bundle_enabled"], json!(true));
    let (_, task_view) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(
        task_view["literary_bundle_enabled"],
        json!(true),
        "扁平字段两模式共用(且不得因 task 视图读而变)"
    );
    assert_eq!(
        task_view["task_literary_bundle_enabled"], task_before,
        "角色扮演侧开关写入不得带动任务侧开关(防 apply! 误用:纯粹扁平、不落覆盖层)"
    );

    // 任务视图写任务侧开关:进 task 覆盖层;roleplay 视图的扁平值不受影响
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_literary_bundle_enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写任务侧开关失败:{r}");
    let (_, task_view2) = send_json(app, "GET", "/api/settings?mode=task", json!({})).await;
    assert_eq!(task_view2["task_literary_bundle_enabled"], json!(true));
    let (_, rp_view2) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        rp_view2["task_literary_bundle_enabled"], task_before,
        "任务覆盖层不得泄漏进角色扮演视图(任务侧开关走覆盖层,角色扮演视图读扁平值)"
    );

    // 卫生复位(写回现场值;共享 app 对本用例之外零影响)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": flat_before }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({ "task_literary_bundle_enabled": task_before }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 文学能力包(LIT-3/LIT-6)预览同步:开关开时角色扮演预览应含「位置 4 增强段」「位置 0 文风段」
/// 与「位置 0 AN 段」三层(层号 4/0/0;文风段与 AN 段都经 untrusted 包裹,且**文风在 AN 之前**
/// ——与真实拼装同序);task 模式预览不含(角色扮演侧专属)。
/// 共享 app 下取现场值写入 + 卫生复位(净零影响,顺序无关)。
#[tokio::test]
async fn prompt_preview_shows_literary_layers_when_enabled() {
    let _guard = test_lock().await;
    let app = test_app();

    // 现场值(共享 app;净零复位目标)
    let (_, before_rp) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    let flat_before = before_rp["literary_bundle_enabled"].clone();
    let style_before = before_rp["literary_style_preset"].clone();

    // 先关(确保下游断言可判别),再开;同时选一档文风预设
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "关包失败:{r}");
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": true, "literary_style_preset": "plain" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "开包失败:{r}");

    // 角色扮演预览:三层都在,层号与 source 名正确;文风段与 AN 段都经 untrusted 包裹
    let (status, preview) = send_json(app, "GET", "/api/settings/prompt-preview", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let layers = preview["layers"].as_array().cloned().unwrap_or_default();
    let find = |src: &str| layers.iter().find(|l| l["source"] == json!(src));
    let tail = find("literary_enhancement").unwrap_or_else(|| panic!("缺位置 4 层:{preview}"));
    assert_eq!(tail["role"], "system");
    assert_eq!(tail["layer"], 4);
    assert!(tail["content"]
        .as_str()
        .unwrap_or("")
        .contains("【文学增强段】"));
    let style = find("literary_style").unwrap_or_else(|| panic!("缺位置 0 文风层:{preview}"));
    assert_eq!(style["role"], "user");
    assert_eq!(style["layer"], 0);
    assert!(
        style["content"]
            .as_str()
            .unwrap_or("")
            .contains(r#"<UNTRUSTED_PROMPT_SOURCE source="literary_style">"#),
        "文风层应经 untrusted 包裹(与真实下发一致):{style}"
    );
    let note = find("literary_note").unwrap_or_else(|| panic!("缺位置 0 层:{preview}"));
    assert_eq!(note["role"], "user");
    assert_eq!(note["layer"], 0);
    assert!(
        note["content"]
            .as_str()
            .unwrap_or("")
            .contains(r#"<UNTRUSTED_PROMPT_SOURCE source="literary_note">"#),
        "AN 层应经 untrusted 包裹(与真实下发一致):{note}"
    );
    // 顺序钉死:文风段排在 AN 段之前(与 build 层拼装同序)
    let style_idx = layers
        .iter()
        .position(|l| l["source"] == json!("literary_style"))
        .unwrap();
    let note_idx = layers
        .iter()
        .position(|l| l["source"] == json!("literary_note"))
        .unwrap();
    assert!(style_idx < note_idx, "预览层序应为「文风 → AN」:{preview}");

    // task 模式预览不含这三层(角色扮演侧专属)
    let (_, task_preview) = send_json(
        app,
        "GET",
        "/api/settings/prompt-preview?mode=task",
        json!({}),
    )
    .await;
    let task_layers = task_preview["layers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !task_layers
            .iter()
            .any(|l| l["source"] == json!("literary_enhancement")
                || l["source"] == json!("literary_note")
                || l["source"] == json!("literary_style")),
        "task 预览不得含文学包三层:{task_preview}"
    );

    // 卫生复位(写回现场值;共享 app 对本用例之外零影响)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": flat_before, "literary_style_preset": style_before }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 文学包选择型字段(LIT-6/LIT-7):未知取值 400(不静默回退、不落库);合法值往返;
/// 推荐档**显式选档才写入**既有三项数值、档间切换不重拍快照、选回空串**恢复写入前原值**
/// ——这一条与「恢复默认值」是两回事,故本用例特意把原值置成非默认值来判别。
/// 共享 app 下取现场值写入 + 卫生复位(净零影响,顺序无关)。
#[tokio::test]
async fn literary_presets_validate_and_recommend_round_trips() {
    let _guard = test_lock().await;
    let app = test_app();

    // 现场值(共享 app;净零复位目标)
    let (_, before) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    let style_before = before["literary_style_preset"].clone();
    let rec_before = before["literary_recommend_preset"].clone();
    let mode_before = before["compaction_mode"].clone();
    let thr_before = before["compaction_threshold"].clone();
    let keep_before = before["compaction_keep_recent"].clone();

    // ❶ 阈值是 f32(字段类型),JSON 往返后 0.7 会读成 0.699999988079071 —— 用容差比较
    let near = |v: &serde_json::Value, want: f32| {
        let got = v.as_f64().unwrap_or_default();
        assert!(
            (got - f64::from(want)).abs() < 1e-6,
            "阈值期望 {want},实际 {got}"
        );
    };

    // ① 未知取值 400(文风与推荐档各一),且不得落库
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_style_preset": "noir" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未知文风档应 400:{r}");
    assert!(
        r["error"]
            .as_str()
            .unwrap_or("")
            .contains("literary_style_preset"),
        "错误文案应指明字段:{r}"
    );
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_recommend_preset": "epic" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未知推荐档应 400:{r}");

    // ② 文风档往返(合法值;空 = 不注入也是合法值,见复位段)
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_style_preset": "hardboiled" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写文风档失败:{r}");
    let (_, after) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        after["literary_style_preset"],
        json!("hardboiled"),
        "文风档应往返一致"
    );

    // ⑦ 400 的未知取值不得落库(上面两次 400 之后读回现场值应原样)
    let (_, after_400) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        after_400["literary_recommend_preset"], rec_before,
        "400 的未知推荐档不得落库"
    );
    assert_eq!(
        after_400["compaction_threshold"].as_f64(),
        thr_before.as_f64(),
        "400 不得顺手改动既有数值"
    );

    // ③ 预置**非默认**原值:证明「恢复原值」不是「恢复默认值」
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({
            "compaction_mode": "manual",
            "compaction_threshold": 0.62,
            "compaction_keep_recent": 13,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "预置非默认原值失败:{r}");

    // ④ 首次选档 → 写入档值(长篇:auto / 0.7 / 8)
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_recommend_preset": "long" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "选档失败:{r}");
    let (_, v) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(v["literary_recommend_preset"], json!("long"));
    assert_eq!(v["compaction_mode"], json!("auto"));
    near(&v["compaction_threshold"], 0.7);
    assert_eq!(v["compaction_keep_recent"], json!(8));

    // ⑤ 档间切换 → 换档值,快照仍是「采纳前」(中篇:auto / 0.75 / 6)
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_recommend_preset": "medium" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "换档失败:{r}");
    let (_, v) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    near(&v["compaction_threshold"], 0.75);
    assert_eq!(v["compaction_keep_recent"], json!(6));

    // ⑥ 选回「不改变」→ 恢复**写入前原值**(0.62 / 13 / manual),不是默认值(0.8 / 4 / off)
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_recommend_preset": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "回退失败:{r}");
    let (_, v) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(v["literary_recommend_preset"], json!(""));
    assert_eq!(
        v["compaction_mode"],
        json!("manual"),
        "应恢复写入前原值(非默认 off)"
    );
    near(&v["compaction_threshold"], 0.62); // 应恢复写入前原值(非默认 0.8)
    assert_eq!(
        v["compaction_keep_recent"],
        json!(13),
        "应恢复写入前原值(非默认 4)"
    );

    // 卫生复位(写回现场值;共享 app 对本用例之外零影响)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({
            "literary_style_preset": style_before,
            "literary_recommend_preset": rec_before,
            "compaction_mode": mode_before,
            "compaction_threshold": thr_before,
            "compaction_keep_recent": keep_before,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// LIT-4:开关翻转**运行期即时生效**(不等重启)——设置写入期按同一判据重物化反思提示词:
/// 逐字等于内置默认才动;自定义文本与空串(显式关闭反思)都不动。
/// 2026-10-06 真实模型实测抓到「只在 load 期重物化 → 开关翻转要等重启」,本用例是回归钉子。
#[tokio::test]
async fn flipping_literary_bundle_rehydrates_reflect_prompt_without_restart() {
    let _guard = test_lock().await;
    let app = test_app();

    // 现场值(共享 app;净零复位目标)
    let (_, before) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    let prompt_before = before["reflect_prompt"].clone();
    let bundle_before = before["literary_bundle_enabled"].clone();
    let lit_default = default_literary_reflect_prompt();
    let plain_default = default_reflect_prompt();

    // ① 置成「未自定义」(= 内置现行默认)、开关关
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": false, "reflect_prompt": plain_default }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "预置失败:{r}");
    let (_, v) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        v["reflect_prompt"],
        json!(plain_default),
        "开关关 → 逐字等于现行版"
    );

    // ② 开包(同一实例、不重启)→ 立即重物化为文学版
    let (status, r) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "开包失败:{r}");
    let (_, v) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        v["reflect_prompt"],
        json!(lit_default),
        "开关开 → 运行期即时重物化为文学版"
    );

    // ③ 关包 → 回落现行版(逐字)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, v) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        v["reflect_prompt"],
        json!(plain_default),
        "关包 → 回落现行版"
    );

    // ④ 自定义文本不被覆盖(开关开也不动)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": true, "reflect_prompt": "CUSTOM-REFLECT-MARK" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, v) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(
        v["reflect_prompt"],
        json!("CUSTOM-REFLECT-MARK"),
        "自定义文本必须逐字优先"
    );

    // ⑤ 空串是「显式关闭反思」的有效值,不得被回填
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "reflect_prompt": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, v) = send_json(app, "GET", "/api/settings?mode=roleplay", json!({})).await;
    assert_eq!(v["reflect_prompt"], json!(""), "空串不得回填");

    // 卫生复位(写回现场值;共享 app 对本用例之外零影响)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings?mode=roleplay",
        json!({ "literary_bundle_enabled": bundle_before, "reflect_prompt": prompt_before }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
