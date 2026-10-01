// 视觉能力包 D2 · 聊天图像附件端到端(独立进程 / 独立 app;与 api_chat_sse.rs 同前提:
// mock 连接器 + 临时数据目录 + 免鉴权;不往 settings_connector.rs 的进程级共享 app 里添用例)。
//
// 覆盖:能力闸门(未勾「视觉输入」→ 400 可操作文案)、附件落盘与 extra.image_refs、
// GET /api/images 路由(魔数内容类型 / 不存在 404)、非法载荷拒绝(类型白名单 / 魔数 /
// 张数上限)、重发路径拒绝附件。请求体三方言映射与引擎历史携带分别由连接器单测
// (openai_compatible/tests_dialects)与引擎单测(compaction::project_history_carries_aligned_image_refs、
// build_tests::history_images_attach_by_index)锁定;本文件锁「从 HTTP 入口到磁盘与历史」的线。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
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

/// 串行化:同一 app 的 settings 是进程级共享的,并行 PUT 会互相覆盖
async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

/// 1x1 PNG(真实魔数;与 services/image_service 单测同一份字节)
const PNG_1PX: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

fn png_data_url() -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(PNG_1PX)
    )
}

/// 返回 (状态码, JSON, 原始响应文本):文本用于断言错误文案与「不含 base64」类判别
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

/// 上传最小角色卡,返回 character_id
async fn upload_character(app: &axum::Router, name: &str) -> String {
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        json!({
            "spec": "chara_card_v2",
            "spec_version": "1.0",
            "name": "视觉测试角色",
            "description": "测试描述",
            "first_mes": "你好"
        })
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/characters/upload")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    json["id"].as_str().unwrap().to_string()
}

async fn create_session(app: &axum::Router, cid: &str) -> String {
    let (status, session, _) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "建会话失败:{session}");
    session["id"].as_str().unwrap().to_string()
}

/// 把默认连接换为 mock(无视觉)或 mock+视觉——基线重置(全量数组语义)
async fn reset_connection(app: &axum::Router, supports_vision: bool) {
    let (status, body, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "name": "视觉基线", "connector_type": "mock",
            "base_url": "", "model": "", "enabled": true,
            "supports_vision": supports_vision
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "连接基线重置失败:{body}");
}

/// 发送一条带附件的聊天消息,返回 (状态码, 响应体原文)
async fn send_with_attachments(
    app: &axum::Router,
    sid: &str,
    cid: &str,
    message: &str,
    attachments: Value,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "session_id": sid,
                "character_id": cid,
                "message": message,
                "agent_mode": "fast",
                "attachments": attachments,
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// 能力闸门:默认连接未勾「视觉输入」→ 400 可操作文案(指路设置),不落盘不写消息
#[tokio::test]
async fn attachment_requires_vision_capability() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_connection(app, false).await;
    let cid = upload_character(app, "视觉闸门.json").await;
    let sid = create_session(app, &cid).await;

    let (status, text) = send_with_attachments(
        app,
        &sid,
        &cid,
        "看这张图",
        json!([{ "name": "a.png", "mime": "image/png", "data_url": png_data_url() }]),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "text={text}");
    assert!(text.contains("视觉输入"), "文案应指路能力位:{text}");
    assert!(text.contains("连接配置"), "文案应指路设置位置:{text}");

    // 未写消息(校验全在落盘与消息写入之前)
    let (_, history, _) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let has_image_msg = history["messages"]
        .as_array()
        .map(|msgs| {
            msgs.iter().any(|m| {
                m["role"] == "user"
                    && m["extra"]["image_refs"]
                        .as_array()
                        .is_some_and(|r| !r.is_empty())
            })
        })
        .unwrap_or(false);
    assert!(!has_image_msg, "闸门拒绝后不得留下带图消息:{history}");
}

/// 全链:勾选视觉 → 附件落盘 → extra 只存引用(正文不含 base64)→
/// GET /api/images 按魔数回图像;不存在 → 404
#[tokio::test]
async fn attachment_roundtrip_and_images_route() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_connection(app, true).await;
    let cid = upload_character(app, "视觉往返.json").await;
    let sid = create_session(app, &cid).await;

    let (status, text) = send_with_attachments(
        app,
        &sid,
        &cid,
        "看这张图\n\n[图片: 图标.png]",
        json!([{ "name": "图标.png", "mime": "image/png", "data_url": png_data_url() }]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "text={text}");
    assert!(text.contains("data:"), "SSE 应有事件:{text}");

    // 历史:正文是 [图片: 名称] 标记,不含 base64;extra.image_refs 只存引用
    let (status, history, raw) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!raw.contains("base64"), "历史里不得回传 base64 像素:{raw}");
    let msgs = history["messages"].as_array().unwrap();
    let user = msgs
        .iter()
        .find(|m| {
            m["role"] == "user"
                && m["extra"]["image_refs"]
                    .as_array()
                    .is_some_and(|r| !r.is_empty())
        })
        .unwrap_or_else(|| panic!("应存在带图 user 消息:{history}"));
    assert!(
        user["content"]
            .as_str()
            .unwrap()
            .contains("[图片: 图标.png]"),
        "正文应保留 [图片: 名称] 标记:{user}"
    );
    let image_id = user["extra"]["image_refs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(image_id.ends_with(".png"), "{image_id}");
    assert_eq!(user["extra"]["image_refs"][0]["name"], "图标.png");
    assert!(
        user["extra"]["image_refs"][0].get("data_url").is_none(),
        "extra 引用不得携带 data_url:{user}"
    );

    // 路由:字节与 Content-Type 按魔数给出
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/images/{image_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/png")
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], PNG_1PX, "回传字节应与上传一致");

    // 不存在 → 404
    let req = Request::builder()
        .method("GET")
        .uri("/api/images/does-not-exist.png")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// 非法载荷:张数超限 / 类型白名单 / 魔数不识别 / 重发路径带附件 —— 一律 400
#[tokio::test]
async fn attachment_rejects_invalid_payloads() {
    let _guard = test_lock().await;
    let app = test_app();
    reset_connection(app, true).await;
    let cid = upload_character(app, "视觉非法载荷.json").await;
    let sid = create_session(app, &cid).await;

    // 5 张 > 4 张上限
    let five = (0..5)
        .map(|i| json!({ "name": format!("{i}.png"), "mime": "image/png", "data_url": png_data_url() }))
        .collect::<Vec<_>>();
    let (status, text) = send_with_attachments(app, &sid, &cid, "x", json!(five)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "text={text}");
    assert!(text.contains("最多"), "{text}");

    // 白名单外类型
    let (status, text) = send_with_attachments(
        app,
        &sid,
        &cid,
        "x",
        json!([{ "name": "a.bmp", "mime": "image/bmp",
                 "data_url": format!("data:image/bmp;base64,{}",
                     base64::engine::general_purpose::STANDARD.encode(b"BM\x00\x00")) }]),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "text={text}");
    assert!(text.contains("不支持"), "{text}");

    // 魔数不识别(声明 png 实际文本字节)
    let (status, text) = send_with_attachments(
        app,
        &sid,
        &cid,
        "x",
        json!([{ "name": "fake.png", "mime": "image/png",
                 "data_url": format!("data:image/png;base64,{}",
                     base64::engine::general_purpose::STANDARD.encode(b"hello world")) }]),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "text={text}");
    assert!(text.contains("不是有效图像"), "{text}");

    // 重发锚点路径拒绝附件载荷(原图随历史自动携带,不在锚点路径重复接收)
    let (status, text) = send_with_attachments(
        app,
        &sid,
        &cid,
        "x",
        json!([{ "name": "a.png", "mime": "image/png", "data_url": png_data_url() }]),
    )
    .await;
    // 上面这条本身是合法常规发送(此处只作对照,确认基线可用)
    assert_eq!(status, StatusCode::OK, "合法附件应放行:{text}");

    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "session_id": sid,
                "character_id": cid,
                "message": "x",
                "resend_message_id": 1,
                "attachments": [{ "name": "a.png", "mime": "image/png", "data_url": png_data_url() }],
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("不接收附件"), "{text}");
}
