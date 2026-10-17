// 回退恢复与生成的互斥(端点级,codex 复审两轮 409 争用证据的闭合)。
//
// 两个方向各一条,都用「真实的那一方」:
//   ① 前向(restore_returns_409_while_generation_lock_held):会话锁被**生成侧形态**的
//      OwnedMutexGuard 持有时,restore 端点 409 且零改动(快照保留);释放后同一快照 200
//      恢复、快照被精确清理。用例同时钉死 engine.is_active 为假——api/undo.rs 的两个
//      409 出口文案相同,不能只靠文案区分归属,要把「不是生成中 gate」显式断言掉。
//   ② 反向(send_returns_409_while_real_restore_in_flight):**真正的 restore 在执行中**
//      持锁时,chat/send 必须 409(「回退进行中」,该文案只出自会话锁 gate)。
//      确定性布点:restore 的 vars 逆操作经 SessionService::save_assistant_vars 走
//      db.write()——编排线程先持写锁,restore 线程就会停在「已持会话锁、被写锁阻塞」处;
//      轮询确认会话锁确被持有后才发 send,且不释放写锁 restore 就不可能完成,全程无
//      sleep 竞态。写锁握在 spawn_blocking 线程上(不占 tokio 运行线程),send 在运行
//      线程上走「占位前无写库」的既有路径(api/chat.rs「占位必须发生在任何消息写入
//      之前」),因此不会自锁。
//
// 为什么自建 AppState:共享 OnceLock 的 build_test_app 不回传 state,拿不到会话锁与 db;
// 装配形态与 tests/task_file_changes.rs 一致(独立数据目录,进程级隔离)。
// 两个用例共用一个 app/库,文件内以 tokio Mutex 串行,互不干扰。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::api::app_state::AppState;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{Mutex, MutexGuard};
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

/// 文件内串行:两个用例共享同一 app/库,且反向用例会短暂持有 db 写锁。
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

/// 直插一条 vars 快照:形状与服务层单测相同(已证可过预检);vars 逆操作写
/// session_assistant_vars,是反向用例拿来做「持锁 + 阻塞」布点的写点。
fn insert_vars_snapshot(state: &AppState, sid: &str, snap_id: &str) {
    let conn = state.db.write();
    conn.execute(
        "INSERT INTO undo_snapshots (id, session_id, tool_name, label, payload, created_at, seq) VALUES (?1, ?2, 'update_variables', '更新变量树', '{\"kind\":\"vars\",\"before\":\"{}\"}', '2026-01-01T00:00:00Z', 1)",
        rusqlite::params![snap_id, sid],
    )
    .expect("插快照行失败");
}

#[tokio::test]
async fn restore_returns_409_while_generation_lock_held() {
    let _serial = test_lock().await;
    let (state, app) = test_state();
    let cid = upload_character(app, "锁争用").await;
    let sid = create_session(app, &cid).await;
    insert_vars_snapshot(state, &sid, "snap-409");

    // 生成侧形态持锁(与 /api/chat/send 的 try_lock_owned 同款,guard 自持 Arc)
    let guard = state
        .undo
        .session_lock(&sid)
        .try_lock_owned()
        .expect("空闲锁应可获取");
    // 归属钉死:本用例没启动生成 → engine.is_active 为假 → 后面的 409 只可能出自
    // 会话锁 gate(api/undo.rs 的 is_active gate 与锁 gate 文案相同)
    assert!(
        !state.engine.is_active(&sid),
        "本用例不启动生成,engine 不应为 active"
    );
    let (status, body) = send_json(app, "POST", "/api/undo/snap-409/restore", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "持锁期间恢复应 409: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("正在生成中"),
        "409 应说明与生成冲突: {body}"
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
    // 同锁另一侧:发送也必须被拒(「回退进行中」文案只出自会话锁 gate)
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
        "发送侧 409 应出自会话锁 gate(区别于「生成中」gate): {body}"
    );

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

#[tokio::test]
async fn send_returns_409_while_real_restore_in_flight() {
    let _serial = test_lock().await;
    let (state, app) = test_state();
    let cid = upload_character(app, "反向争用").await;
    let sid = create_session(app, &cid).await;
    insert_vars_snapshot(state, &sid, "snap-rev");

    // 编排:先持写锁 → 起真 restore 线程 → 轮询到会话锁确被持有 → 报 ready → 等放行。
    // restore 被写锁钉在 apply_inverse 的写点,不释放写锁就不会完成 → 无竞态。
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<bool>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let sid_orch = sid.clone();
    let orch = tokio::task::spawn_blocking(move || {
        let db_guard = state.db.write();
        let svc = state.undo.clone();
        let worker = std::thread::spawn(move || svc.restore("snap-rev"));
        let mut held = false;
        for _ in 0..500 {
            if state.undo.session_lock(&sid_orch).try_lock().is_err() {
                held = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = ready_tx.send(held);
        if held {
            // 等主线程放行(超时兜底:即使主线程断言失败也不永久占锁)
            let _ = release_rx.recv_timeout(Duration::from_secs(30));
        }
        drop(db_guard);
        worker.join().expect("restore 线程不应 panic")
    });

    let held = ready_rx.await.expect("编排线程不应提前退出");
    assert!(
        held,
        "真实 restore 应已持有会话锁(等待超时,restore 未进入执行)"
    );

    // 此刻真实 restore 正在执行中(持会话锁 + 阻塞于写锁):send 必须 409
    let (status, body) = send_json(
        app,
        "POST",
        "/api/chat/send",
        json!({ "session_id": sid, "character_id": cid, "message": "反向争用探针", "agent_mode": "agent" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "回退执行中发送应 409: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("回退进行中"),
        "发送侧 409 应出自会话锁 gate(区别于「生成中」gate): {body}"
    );

    // 放行:释放写锁 → 真实 restore 走完(它整程未被干扰),回执应在
    let _ = release_tx.send(());
    let receipt = orch
        .await
        .expect("编排线程不应 panic")
        .expect("restore 不应报错")
        .expect("快照应存在");
    assert_eq!(receipt.restored_count, 1, "真实 restore 应完成整批恢复");
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
