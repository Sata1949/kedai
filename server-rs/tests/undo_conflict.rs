// 回退恢复与生成的互斥(端点级,codex 复审二轮 P1-1 要求的「真实 409 争用」证据)。
//
// 场景:生成侧持有**会话语义锁**(与 /api/chat/send 的 try_lock_owned 同一把)期间,
// POST /api/undo/{id}/restore 必须 409 且零改动(快照保留),POST /api/chat/send 也必须
// 409(回退进行中);释放后同一快照可 200 恢复、快照被精确清理。两个方向共用同一把锁,
// 本文件把两侧的 409 拒绝都钉在端点层。服务层的单侧争用另有单测
// (undo_service::tests::restore_rejects_while_session_lock_held);本文件补的是
// **端点映射**:同样的争用经 HTTP 出口必须落到 409(而不是被吞成 500 或照常执行)。
//
// 为什么自建 AppState:共享 OnceLock 的 build_test_app 不回传 state,拿不到会话锁;
// 装配形态与 tests/task_file_changes.rs 一致(独立数据目录,进程级隔离)。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::api::app_state::AppState;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn test_state() -> (&'static Arc<AppState>, &'static axum::Router) {
    static STATE: OnceLock<Arc<AppState>> = OnceLock::new();
    static ROUTER: OnceLock<axum::Router> = OnceLock::new();
    let state = STATE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("kedai-undo-conflict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建数据目录失败");
        std::env::set_var("CONNECTOR", "mock");
        std::env::set_var("DATA_DIR", &dir);
        std::env::set_var("LOG_LEVEL", "error");
        let mut config = kedai_server::config::AppConfig::from_env();
        config.auth_required = false;
        AppState::new(config).expect("构造 AppState 失败")
    });
    let router = ROUTER.get_or_init(|| kedai_server::api::build_router(state.clone()));
    (state, router)
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

/// 上传空角色卡(mock 连接器不依赖角色内容;name 仅作标识)
async fn upload_character(app: &axum::Router, name: &str) -> String {
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}.json\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--",
        json!({ "spec": "chara_card_v2", "spec_version": "1.0", "name": name, "description": "测试", "first_mes": "你好" })
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
    char["id"].as_str().unwrap().to_string()
}

async fn create_session(app: &axum::Router, cid: &str) -> String {
    let (status, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "建会话应 201: {session}");
    session["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn restore_returns_409_while_generation_lock_held() {
    let (state, app) = test_state();
    let cid = upload_character(app, "锁争用").await;
    let sid = create_session(app, &cid).await;

    // 直插一条 vars 快照(形状与 undo_service 单测相同,已证可过预检)
    {
        let conn = state.db.write();
        conn.execute(
            "INSERT INTO undo_snapshots (id, session_id, tool_name, label, payload, created_at, seq) VALUES ('snap-409', ?1, 'update_variables', '更新变量树', '{\"kind\":\"vars\",\"before\":\"{}\"}', ?2, 1)",
            rusqlite::params![sid, "2026-01-01T00:00:00Z"],
        )
        .expect("插快照行失败");
    }

    // 生成侧持锁期间:① 恢复端点 409(本用例没有在跑生成,engine.is_active 为假,
    // 所以这个 409 只可能来自会话锁争用这条路径);② 发送端点也 409(反向拒绝,
    // 与 ① 共用同一把锁)
    let lock = state.undo.session_lock(&sid);
    let guard = lock.try_lock().expect("空闲锁应可获取");
    let (status, body) = send_json(app, "POST", "/api/undo/snap-409/restore", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "持锁期间恢复应 409: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("正在生成中"),
        "409 应说明与生成冲突: {body}"
    );
    let (status, body) = send_json(
        app,
        "POST",
        "/api/chat/send",
        json!({ "session_id": sid, "character_id": cid, "message": "锁争用探针", "agent_mode": "agent" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "持锁期间发送也应 409: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("回退进行中"),
        "发送侧 409 应说明回退占位(区别于已生成中的 gate): {body}"
    );
    // 零改动:争用拒绝不得删除快照
    let alive: i64 = state
        .db
        .read()
        .expect("读库失败")
        .query_row(
            "SELECT COUNT(*) FROM undo_snapshots WHERE id = 'snap-409'",
            [],
            |r| r.get(0),
        )
        .expect("查快照失败");
    assert_eq!(alive, 1, "409 不得删除快照");

    // 释放后:同一快照恢复成功,且快照被精确清理
    drop(guard);
    let (status, body) = send_json(app, "POST", "/api/undo/snap-409/restore", json!({})).await;
    assert_eq!(status, StatusCode::OK, "释放后应可恢复: {body}");
    assert_eq!(body["restored_count"], json!(1), "应恢复 1 条: {body}");
    let (status, list) = send_json(
        app,
        "GET",
        &format!("/api/chat/sessions/{sid}/undo"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "列表应 200: {list}");
    assert_eq!(
        list["snapshots"].as_array().map(|a| a.len()),
        Some(0),
        "恢复成功后该快照应被清理: {list}"
    );
}
