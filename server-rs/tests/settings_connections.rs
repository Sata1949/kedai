// 多套连接配置的 API 面(2026-09-22 批次 4)。
//
// 本文件独立进程、独立 app:`settings_connector.rs` 那个 app 是「进程级共享 settings + 互斥串行」
// (该共享态正是既有 TEST-ISO-1 flake 的土壤,见 docs/遗留.md),这里不往里添用例 ——
// 新文件天然与它互不干扰,可以按需 PUT 设置而不必担心踩踏到别的断言。
//
// 用例内每个都先 PUT 一份「基线连接」再断言:文件内共享同一个 app,
// 不依赖上一个用例的残留状态(测试执行顺序不保证)。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

/// 串行化:同一 app 的 settings 是进程级共享的,并行 PUT 会互相覆盖
async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

fn test_app() -> &'static axum::Router {
    static APP: OnceLock<axum::Router> = OnceLock::new();
    APP.get_or_init(|| build_test_app().expect("构建测试应用失败"))
}

/// 返回 (状态码, JSON, 原始响应文本):文本用于断言「响应体里没有明文密钥」
async fn send_json(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

/// PUT 的响应是 `{ok, settings}`,GET 直接返回设置体 → 统一取出设置体再断言
fn settings_body(body: &Value) -> &Value {
    body.get("settings").unwrap_or(body)
}

/// 把连接数组收敛到一个已知基线(mock 类型、无凭据),供各用例起步
async fn reset_to_baseline(app: &axum::Router) -> String {
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "name": "基线", "connector_type": "mock",
            "base_url": "", "model": "", "enabled": true
        }]}),
    )
    .await;
    let settings = settings_body(&body);
    assert_eq!(status, StatusCode::OK, "基线重置失败:{body}");
    let conns = settings["connections"]
        .as_array()
        .unwrap_or_else(|| panic!("响应缺 connections:{body}"));
    assert_eq!(conns.len(), 1);
    conns[0]["id"].as_str().unwrap().to_string()
}

/// GET 只回掩码与布尔:响应体里绝不出现明文密钥;顶层兼容字段投影自默认连接
#[tokio::test]
async fn get_settings_masks_connection_keys() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_to_baseline(app).await;

    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "name": "主连接", "connector_type": "openai-compatible",
            "base_url": "https://api.example/v1", "api_key": "sk-plain-zzzz",
            "model": "gpt-x", "enabled": true
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");

    let (status, body, text) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !text.contains("sk-plain-zzzz"),
        "响应不得回传明文 Key:{text}"
    );
    let conns = body["connections"].as_array().expect("应返回连接数组");
    assert_eq!(conns.len(), 1);
    assert_eq!(conns[0]["name"], "主连接");
    assert_eq!(conns[0]["base_url"], "https://api.example/v1");
    assert_eq!(conns[0]["model"], "gpt-x");
    assert_eq!(conns[0]["enabled"], true);
    assert_eq!(conns[0]["has_api_key"], true);
    assert_eq!(conns[0]["api_key_masked"], "****zzzz");
    assert!(
        conns[0]["id"].as_str().is_some_and(|s| !s.is_empty()),
        "每条连接都要有 id:{body}"
    );
    assert_eq!(
        body["active_connection_id"], conns[0]["id"],
        "唯一一条启用连接应成为默认连接"
    );
    // 顶层兼容字段(旧前端读的三个)= 默认连接的投影
    assert_eq!(body["openai_base_url"], "https://api.example/v1");
    assert_eq!(body["model"], "gpt-x");
    assert_eq!(body["has_api_key"], true);
}

/// 全量数组语义:改名/停用/切默认/删除;空 api_key 表示保持原密钥
#[tokio::test]
async fn put_connections_supports_rename_switch_and_delete() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_to_baseline(app).await;

    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [
            {"name": "甲", "connector_type": "openai-compatible",
             "base_url": "https://a.example/v1", "api_key": "sk-a-1111",
             "model": "ma", "enabled": true},
            {"name": "乙", "connector_type": "openai-compatible",
             "base_url": "https://b.example/v1", "api_key": "sk-b-2222",
             "model": "mb", "enabled": true}
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let settings = settings_body(&body);
    let conns = settings["connections"].as_array().unwrap();
    assert_eq!(conns.len(), 2, "应新建两条(基线那条不在数组里 → 删除)");
    let first = conns[0]["id"].as_str().unwrap().to_string();
    let second = conns[1]["id"].as_str().unwrap().to_string();
    assert_ne!(first, second, "两条连接的 id 必须不同");

    // 改名 + 停用甲 + 切默认到乙;两条的 api_key 都留空(= 保持原密钥)
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "connections": [
                {"id": first, "name": "甲改名", "connector_type": "openai-compatible",
                 "base_url": "https://a.example/v1", "api_key": "", "model": "ma",
                 "enabled": false},
                {"id": second, "name": "乙", "connector_type": "openai-compatible",
                 "base_url": "https://b.example/v1", "api_key": "", "model": "mb",
                 "enabled": true}
            ],
            "active_connection_id": second
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let settings = settings_body(&body);
    let conns = settings["connections"].as_array().unwrap();
    assert_eq!(conns[0]["name"], "甲改名");
    assert_eq!(conns[0]["enabled"], false);
    assert_eq!(conns[0]["has_api_key"], true, "空 api_key = 保持原密钥");
    assert_eq!(settings["active_connection_id"], second);
    assert_eq!(
        settings["openai_base_url"], "https://b.example/v1",
        "投影应跟随新的默认连接"
    );
    assert_eq!(settings["model"], "mb");

    // 删除甲(数组里不再出现)→ 只剩乙,且默认连接不变
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [
            {"id": second, "name": "乙", "connector_type": "openai-compatible",
             "base_url": "https://b.example/v1", "api_key": "", "model": "mb",
             "enabled": true}
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let settings = settings_body(&body);
    assert_eq!(settings["connections"].as_array().unwrap().len(), 1);
    assert_eq!(settings["active_connection_id"], second);

    // 默认连接指向不存在的 id → 回退第一个启用的连接(不报错)
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"active_connection_id": "ghost-id"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(
        settings_body(&body)["active_connection_id"],
        second,
        "失效 id 应回退到可用连接"
    );
}

/// 旧客户端路径:只发扁平字段 → 写进默认连接(行为与改造前一致),并沿用「填了就生效」的自动切换
#[tokio::test]
async fn flat_patch_writes_into_active_connection() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_to_baseline(app).await;

    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "openai_base_url": "https://flat.example/v1",
            "openai_api_key": "sk-flat-3333",
            "model": "flat-model"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let settings = settings_body(&body);
    let conns = settings["connections"].as_array().unwrap();
    assert_eq!(conns.len(), 1, "扁平 patch 落在默认连接上,不应新建连接");
    assert_eq!(conns[0]["base_url"], "https://flat.example/v1");
    assert_eq!(conns[0]["model"], "flat-model");
    assert_eq!(conns[0]["has_api_key"], true);
    assert_eq!(
        conns[0]["connector_type"], "openai-compatible",
        "给 mock 类型的连接填了凭据 → 自动改回真实连接器类型(既有口径)"
    );
    assert_eq!(settings["openai_base_url"], "https://flat.example/v1");
    assert_eq!(settings["model"], "flat-model");
}

/// 上限校验:超过 20 套 → 400 VALIDATION;校验失败不得改动已保存的连接
#[tokio::test]
async fn put_rejects_too_many_connections() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_to_baseline(app).await;

    let many: Vec<Value> = (0..21)
        .map(|i| json!({"name": format!("c{i}"), "connector_type": "mock", "enabled": true}))
        .collect();
    let (status, body, _) =
        send_json(app, "PUT", "/api/settings", json!({ "connections": many })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    assert_eq!(body["code"], "VALIDATION");
    assert!(
        body["error"].as_str().unwrap_or("").contains("20"),
        "错误文案应说明上限:{body}"
    );

    // 校验失败在落盘之前返回 → 已保存的连接保持原样(仍是基线那一条)
    let (status, body, _) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["connections"].as_array().unwrap().len(), 1);
}

/// 接口方言批次(2026-10-01):api_style 落盘/回读;粘贴完整端点 URL 时剥离后缀并推断方言;
/// 显式非默认档不被覆盖;缺省 = 沿用已有值(旧客户端写入不丢方言)
#[tokio::test]
async fn api_style_roundtrip_infers_from_url_suffix() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_to_baseline(app).await;

    // 1) 粘贴完整端点(百炼形态)不带 api_style → 剥离后缀并推断 responses
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "name": "百炼", "connector_type": "openai-compatible",
            "base_url": "https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/responses",
            "api_key": "sk-x-9999", "model": "m", "enabled": true
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let settings = settings_body(&body);
    let conns = settings["connections"].as_array().unwrap();
    assert_eq!(
        conns[0]["base_url"], "https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        "完整端点 URL 应剥离 /responses 后缀"
    );
    assert_eq!(conns[0]["api_style"], "responses", "应由后缀推断方言");

    // 2) GET 回读一致(落盘 → 读入 往返不漂移)
    let (status, body, _) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let conns = body["connections"].as_array().unwrap().clone();
    assert_eq!(conns[0]["api_style"], "responses");
    assert_eq!(
        conns[0]["base_url"],
        "https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    );

    // 3) 显式选择非默认档(anthropic)+ 普通 base → 保持;无路径 base 照常补 /v1
    let id = conns[0]["id"].as_str().unwrap().to_string();
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "id": id, "name": "官方", "connector_type": "openai-compatible",
            "base_url": "https://api.anthropic.com", "api_key": "",
            "model": "claude-x", "api_style": "anthropic", "enabled": true
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let conns = settings_body(&body)["connections"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(conns[0]["base_url"], "https://api.anthropic.com/v1");
    assert_eq!(conns[0]["api_style"], "anthropic");

    // 4) 显式 chat 档(默认档)+ 带后缀 URL → 后缀推断覆盖默认档(文档口径:只改写默认档)
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "id": id, "name": "官方", "connector_type": "openai-compatible",
            "base_url": "https://api.anthropic.com/v1/messages", "api_key": "",
            "model": "claude-x", "api_style": "chat-completions", "enabled": true
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let conns = settings_body(&body)["connections"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(conns[0]["base_url"], "https://api.anthropic.com/v1");
    assert_eq!(
        conns[0]["api_style"], "anthropic",
        "chat 属默认档,应被后缀推断改写"
    );

    // 5) 缺省 api_style = 沿用已有值(旧客户端不认识该键,写入不丢方言)
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "id": id, "name": "官方改名", "connector_type": "openai-compatible",
            "base_url": "https://api.anthropic.com", "model": "claude-x", "enabled": true
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let conns = settings_body(&body)["connections"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(
        conns[0]["api_style"], "anthropic",
        "缺省 api_style = 沿用已有值"
    );
}

/// 视觉能力包 D1:连接能力位落盘/回读;缺省 = 沿用(旧客户端写入不清空);未勾选项缺省 false
#[tokio::test]
async fn connection_capability_flags_roundtrip_and_survive_omitted_keys() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_to_baseline(app).await;

    // 1) 勾选 supports_vision / image_auto_split 保存 → 响应即回读 true;未传的三项 false
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "name": "能力位", "connector_type": "mock",
            "base_url": "", "model": "", "enabled": true,
            "supports_vision": true, "image_auto_split": true
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let conns = settings_body(&body)["connections"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(conns[0]["supports_vision"], true);
    assert_eq!(conns[0]["image_auto_split"], true);
    assert_eq!(
        conns[0]["supports_structured_output"], false,
        "未勾选的仅声明能力位缺省 false"
    );
    assert_eq!(conns[0]["supports_prefix_completion"], false);
    assert_eq!(conns[0]["supports_mid_conversation_system"], false);

    // 2) GET 回读一致(落盘 → 读入 往返不漂移)
    let (status, body, _) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let conns = body["connections"].as_array().unwrap().clone();
    assert_eq!(conns[0]["supports_vision"], true);
    assert_eq!(conns[0]["image_auto_split"], true);

    // 3) 该 id 再次 PUT 但不带能力位键(旧客户端全量数组)→ 沿用 true,不被清空
    let id = conns[0]["id"].as_str().unwrap().to_string();
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "id": id, "name": "能力位改名", "connector_type": "mock",
            "base_url": "", "model": "", "enabled": true
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let conns = settings_body(&body)["connections"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(conns[0]["supports_vision"], true, "缺省 = 沿用(不清空)");
    assert_eq!(conns[0]["image_auto_split"], true);

    // 4) 显式 false 才清空(前端取消勾选路径)
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "id": id, "name": "能力位改名", "connector_type": "mock",
            "base_url": "", "model": "", "enabled": true,
            "supports_vision": false, "image_auto_split": false
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let conns = settings_body(&body)["connections"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(conns[0]["supports_vision"], false, "显式 false 应生效");
    assert_eq!(conns[0]["image_auto_split"], false);
}
