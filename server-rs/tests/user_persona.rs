// 用户设定(USERPERSONA-1)端点集成测试:CRUD / 校验 / 预览层 / 任务隔离。
// 用户设定为全局共享(所有角色会话生效);同 binary 内用 test_lock 串行并在收尾复位。
mod common;
use axum::http::StatusCode;
use common::{send_json, test_app, test_lock};
use serde_json::{json, Value};

fn default_config() -> Value {
    json!({ "enabled": false, "name": "", "description": "" })
}

async fn put_config(app: &axum::Router, config: Value) -> (StatusCode, Value) {
    send_json(app, "PUT", "/api/user-persona", json!({ "config": config })).await
}

/// GET 即使 enabled=false 也返回已存文本;PUT 往返;超长字段 400 且文案含字段名。
#[tokio::test]
async fn user_persona_crud_and_validation() {
    let _guard = test_lock().await;
    let app = test_app();
    let _ = put_config(app, default_config()).await;

    // 默认
    let (status, body) = send_json(app, "GET", "/api/user-persona", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["config"], default_config());

    // 往返(enabled=false 也保留文本)
    let saved = json!({
        "enabled": false,
        "name": " 林 ",
        "description": "退役侦探,怕黑,随身带打火机"
    });
    let (status, body) = put_config(app, saved).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["config"]["name"], "林", "写入侧应 trim");
    let (status, body) = send_json(app, "GET", "/api/user-persona", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["config"]["name"], "林");
    assert_eq!(body["config"]["description"], "退役侦探,怕黑,随身带打火机");

    // 超长 name → 400 且点名 name
    let too_long_name = "名".repeat(65);
    let (status, body) = put_config(
        app,
        json!({ "enabled": true, "name": too_long_name, "description": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap_or("").contains("name"),
        "错误文案应含字段名: {body}"
    );

    // 超长 description → 400 且点名 description
    let (status, body) = put_config(
        app,
        json!({ "enabled": true, "name": "林", "description": "设".repeat(4001) }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap_or("").contains("description"),
        "错误文案应含字段名: {body}"
    );

    // 卫生复位
    let (status, _) = put_config(app, default_config()).await;
    assert_eq!(status, StatusCode::OK);
}

/// 预览:roleplay 含 user_persona 层(与真实下发同源),task 不含;未启用时不注入。
#[tokio::test]
async fn user_persona_preview_layer_roleplay_only() {
    let _guard = test_lock().await;
    let app = test_app();
    let _ = put_config(app, default_config()).await;

    // 未启用:不出现 user_persona 层
    let (status, preview) = send_json(app, "GET", "/api/settings/prompt-preview", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !preview["layers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["source"] == json!("user_persona")),
        "未启用不得注入用户设定层: {preview}"
    );

    // 启用:roleplay 预览含该层,内容与 block() 同源(untrusted 包裹)
    let (status, _) = put_config(
        app,
        json!({
            "enabled": true,
            "name": "林",
            "description": "退役侦探,怕黑"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, preview) = send_json(app, "GET", "/api/settings/prompt-preview", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let layers = preview["layers"].as_array().cloned().unwrap_or_default();
    let layer = layers
        .iter()
        .find(|l| l["source"] == json!("user_persona"))
        .unwrap_or_else(|| panic!("缺 user_persona 层: {preview}"));
    assert_eq!(layer["role"], "system");
    let content = layer["content"].as_str().unwrap_or("");
    assert!(content.contains("【用户角色】"), "{content}");
    assert!(content.contains("称呼: 林"), "{content}");
    assert!(
        content.contains(r#"<UNTRUSTED_PROMPT_SOURCE source="user_persona">"#),
        "设定应经 untrusted 包裹: {content}"
    );

    // task 预览不含该层(任务模式不注入)
    let (status, task_preview) = send_json(
        app,
        "GET",
        "/api/settings/prompt-preview?mode=task",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !task_preview["layers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["source"] == json!("user_persona")),
        "task 预览不得含用户设定层: {task_preview}"
    );

    // 卫生复位
    let (status, _) = put_config(app, default_config()).await;
    assert_eq!(status, StatusCode::OK);
}
