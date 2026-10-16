// 批次 6.1「回退快照/undo」集成测试:写工具(write/replace/create/update_variables/
// memory_write)执行前落逆操作快照,POST /api/undo/{id}/restore 逆序恢复。
// 链路:chat/send(mock [[tool:...]] 钩子)→ 工具执行(run_tool 两段式快照)→
// GET /api/chat/sessions/{id}/undo 列表 → restore 断言状态复原。
// mock 连接器不依赖角色内容;文件类断言直读 data_dir 下角色文件区。
// 全文件共享 app(临时目录按进程隔离)+ 全局锁串行(快照/settings 为进程级共享态)。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

fn test_app() -> &'static axum::Router {
    static APP: OnceLock<axum::Router> = OnceLock::new();
    APP.get_or_init(|| {
        std::env::set_var("CONNECTOR", "mock");
        kedai_server::build_test_app().expect("构建测试应用失败")
    })
}

async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

/// 测试数据目录(与 lib.rs build_test_app 同规则:temp/kedai-test-{pid})。
/// build_test_app 的数据目录是**进程级共享**单例,生命周期等于测试进程,
/// 不能套 RAII 守卫(会删掉后续用例要用的库)。
fn data_dir() -> PathBuf {
    std::env::temp_dir().join(format!("kedai-test-{}", std::process::id()))
}

fn char_file(character_id: &str, rel: &str) -> PathBuf {
    data_dir()
        .join("character_files")
        .join(character_id)
        .join(rel)
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
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}.json\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
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

/// 会话级授权危险工具(write/replace/create/update_variables/memory_write 均为 Dangerous)
async fn authorize(app: &axum::Router, sid: &str, tool: &str) {
    let (status, res) = send_json(
        app,
        "POST",
        "/api/agent/tool-permissions",
        json!({ "session_id": sid, "tool": tool, "scope": "session" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "授权 {tool} 应 200: {res}");
}

/// 发送携带 [[tool:...]] 钩子的 chat 消息并消费完整 SSE;断言正常完成。
/// 返回完整 SSE 文本(批 D-1 起测试据此断言工具结果事件里的快照绑定字段)。
async fn send_tool_chat(app: &axum::Router, sid: &str, cid: &str, marker: &str) -> String {
    // agent 模式:工具循环所在(fast/deep 不执行工具,deep 会把 mock ToolCall 当空输出重试)
    let body = json!({
        "session_id": sid, "character_id": cid,
        "message": marker, "agent_mode": "agent",
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert_eq!(status, StatusCode::OK, "chat/send 应 200: {text}");
    assert!(
        text.contains("\"type\":\"finish\"") || text.contains("\"type\": \"finish\""),
        "应收到 finish 事件: {text}"
    );
    text
}

/// 会话快照列表(新→旧)
async fn list_undo(app: &axum::Router, sid: &str) -> Vec<Value> {
    let (status, res) = send_json(
        app,
        "GET",
        &format!("/api/chat/sessions/{sid}/undo"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "undo 列表应 200: {res}");
    res["snapshots"].as_array().cloned().unwrap_or_default()
}

async fn restore(app: &axum::Router, id: &str) -> (StatusCode, Value) {
    send_json(app, "POST", &format!("/api/undo/{id}/restore"), json!({})).await
}

/// 造会话:上传角色 + 建会话 + 授权指定工具
async fn setup(app: &axum::Router, name: &str, tools: &[&str]) -> (String, String) {
    let cid = upload_character(app, name).await;
    let sid = create_session(app, &cid).await;
    for t in tools {
        authorize(app, &sid, t).await;
    }
    (cid, sid)
}

/// ① write(file) 后 restore → 文件内容复原;旧快照(写入前文件不存在)restore → 文件删除
#[tokio::test]
async fn write_file_restore_recovers_content() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 写入", &["write"]).await;
    let file = char_file(&cid, "undo_notes.md");

    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_notes.md","content":"v1 内容"}]]"#,
    )
    .await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1 内容");
    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_notes.md","content":"v2 内容"}]]"#,
    )
    .await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2 内容");

    let snaps = list_undo(app, &sid).await;
    assert_eq!(snaps.len(), 2, "两次 write 应产生 2 条快照: {snaps:?}");
    assert_eq!(snaps[0]["tool_name"], json!("write"));
    assert!(
        snaps[0]["label"]
            .as_str()
            .unwrap()
            .contains("undo_notes.md"),
        "label 应为中文可读简述: {}",
        snaps[0]["label"]
    );
    // anchor = 快照时该会话 max(messages.id)(已有多条消息,应为正 id)
    assert!(snaps[0]["anchor_message_id"].as_i64().unwrap() > 0);

    // 回退 v2 写入 → 内容复原为 v1
    let (status, res) = restore(app, snaps[0]["id"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "restore 应 200: {res}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "v1 内容",
        "restore 后应复原为 v1"
    );

    // 再回退 v1 写入(写入前文件不存在,逆操作 = 删除)
    let (status, res) = restore(app, snaps[1]["id"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "restore 应 200: {res}");
    assert!(!file.exists(), "写入前文件不存在,restore 后应删除");
}

/// ② create 新文件后 restore → 文件被删
#[tokio::test]
async fn create_file_restore_deletes_file() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 新建", &["create"]).await;
    let file = char_file(&cid, "undo_new.txt");

    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:create {"items":[{"type":"file","path":"undo_new.txt","content":"新建内容"}]}]]"#,
    )
    .await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "新建内容");

    let snaps = list_undo(app, &sid).await;
    assert_eq!(snaps.len(), 1, "create 应产生 1 条快照: {snaps:?}");
    assert_eq!(snaps[0]["tool_name"], json!("create"));

    let (status, res) = restore(app, snaps[0]["id"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "restore 应 200: {res}");
    assert!(!file.exists(), "create 的逆操作应删除新建文件");
}

/// ③ update_variables 后 restore → 变量树复原
#[tokio::test]
async fn update_variables_restore_recovers_tree() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 变量", &["update_variables"]).await;

    // 预置变量树(走 PUT 整树覆写)
    let (status, res) = send_json(
        app,
        "PUT",
        "/api/variables",
        json!({ "session_id": sid, "scope": "chat", "data": { "score": 5 } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PUT variables 应 200: {res}");

    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:update_variables {"patches":[{"op":"replace","path":"/score","value":99}]}]]"#,
    )
    .await;
    let (_, vars) = send_json(
        app,
        "GET",
        &format!("/api/variables?session_id={sid}"),
        json!({}),
    )
    .await;
    assert_eq!(vars["data"]["score"], json!(99), "工具应已把 score 改为 99");

    let snaps = list_undo(app, &sid).await;
    assert_eq!(
        snaps.len(),
        1,
        "update_variables 应产生 1 条快照: {snaps:?}"
    );
    assert_eq!(snaps[0]["tool_name"], json!("update_variables"));
    assert_eq!(snaps[0]["label"], json!("更新变量树"));

    let (status, res) = restore(app, snaps[0]["id"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "restore 应 200: {res}");
    let (_, vars) = send_json(
        app,
        "GET",
        &format!("/api/variables?session_id={sid}"),
        json!({}),
    )
    .await;
    assert_eq!(vars["data"]["score"], json!(5), "restore 后变量树应复原");
}

/// ④ undo_enabled=false(PUT settings)→ 不再产生新快照
#[tokio::test]
async fn undo_disabled_produces_no_snapshot() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 开关", &["write"]).await;

    let (status, res) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "undo_enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PUT settings 应 200: {res}");

    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_off.md","content":"x"}]]"#,
    )
    .await;
    assert!(
        char_file(&cid, "undo_off.md").exists(),
        "开关只影响快照,不影响工具正常执行"
    );
    let snaps = list_undo(app, &sid).await;
    assert!(
        snaps.is_empty(),
        "undo_enabled=false 时不应产生快照: {snaps:?}"
    );

    // 恢复开关(共享 app,避免污染本 target 其它用例)
    let (status, res) =
        send_json(app, "PUT", "/api/settings", json!({ "undo_enabled": true })).await;
    assert_eq!(status, StatusCode::OK, "恢复 undo_enabled 应 200: {res}");
}

/// ⑤ 回退旧快照 → 目标及后续快照**逆序整体恢复**(批 D-2 新语义:「回到所选时点」;
/// 旧实现的「只撤所选一条 + 清后续快照」被改写的对照见变更史 2026-10-16 章)。
/// 断言**文件状态**:两次 write 回退较早一次,两个文件都应回到该时点(均不存在)。
#[tokio::test]
async fn restore_older_snapshot_reverts_target_and_newer() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 逆序", &["write"]).await;
    let f1 = char_file(&cid, "undo_p1.md");
    let f2 = char_file(&cid, "undo_p2.md");

    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_p1.md","content":"一"}]]"#,
    )
    .await;
    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_p2.md","content":"二"}]]"#,
    )
    .await;
    assert!(f1.exists() && f2.exists(), "两次 write 后两个文件都应存在");

    let snaps = list_undo(app, &sid).await;
    assert_eq!(snaps.len(), 2, "两次 write 应产生 2 条快照: {snaps:?}");
    let newer = snaps[0]["id"].as_str().unwrap().to_string();
    let older = snaps[1]["id"].as_str().unwrap().to_string();

    // 回退较早的快照:数据整体回到「undo_p1.md 写入前」——两个文件都不该再存在
    let (status, res) = restore(app, &older).await;
    assert_eq!(status, StatusCode::OK, "restore 应 200: {res}");
    assert_eq!(
        res["restored_count"],
        json!(2),
        "应整体恢复 2 条快照: {res}"
    );
    assert!(
        !f1.exists(),
        "回退旧快照后 p1(目标本身)也应回到写入前:文件应被删除"
    );
    assert!(
        !f2.exists(),
        "回退旧快照后 p2(后续快照)应一并逆序撤销:文件应被删除"
    );
    let snaps = list_undo(app, &sid).await;
    assert!(
        snaps.is_empty(),
        "回退成功后已处理快照应被精确清理: {snaps:?}"
    );

    let (status, _) = restore(app, &newer).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "已随批清理的快照再 restore 应 404"
    );
    let (status, _) = restore(app, &older).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "已回退的快照再 restore 应 404"
    );
}

/// ⑤b 同毫秒快照按 seq 确定性排序(批 D-2):把两条快照的 created_at 改为同一时刻,
/// 回退较早一次仍应整体恢复两条(旧实现按 created_at>= 界定会与毫秒并列纠缠)。
#[tokio::test]
async fn restore_same_millisecond_uses_seq_order() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 同毫秒", &["write"]).await;
    let f1 = char_file(&cid, "undo_ms1.md");
    let f2 = char_file(&cid, "undo_ms2.md");

    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_ms1.md","content":"1"}]]"#,
    )
    .await;
    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_ms2.md","content":"2"}]]"#,
    )
    .await;
    let snaps = list_undo(app, &sid).await;
    assert_eq!(snaps.len(), 2, "应有 2 条快照: {snaps:?}");
    let older = snaps[1]["id"].as_str().unwrap().to_string();

    // 直连库把本会话两条快照的 created_at 改成同一毫秒(模拟并列)
    {
        let db = data_dir().join("kedai.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        let n = conn
            .execute(
                "UPDATE undo_snapshots SET created_at = '2026-01-01T00:00:00.000Z' WHERE session_id = ?1",
                rusqlite::params![sid],
            )
            .unwrap();
        assert_eq!(n, 2, "应改到本会话的 2 条快照");
    }

    let (status, res) = restore(app, &older).await;
    assert_eq!(status, StatusCode::OK, "restore 应 200: {res}");
    assert_eq!(res["restored_count"], json!(2));
    assert!(
        !f1.exists() && !f2.exists(),
        "同毫秒并列下也应整体恢复到目标时点"
    );
    assert!(list_undo(app, &sid).await.is_empty(), "已处理快照应清空");
}

/// ⑤c 预检拒绝:批内有截断快照 → 整批拒绝,文件与快照零变化(批 D-2)。
#[tokio::test]
async fn restore_precheck_rejects_truncated_snapshot() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 截断", &["write"]).await;
    let big = char_file(&cid, "undo_big.txt");
    let small = char_file(&cid, "undo_small.txt");

    // 预置 >256KB 的文件(不经工具):随后对它 write 时,快照项被标 truncated=true
    std::fs::create_dir_all(big.parent().unwrap()).unwrap();
    std::fs::write(&big, "A".repeat(300 * 1024)).unwrap();

    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_big.txt","content":"x"}]]"#,
    )
    .await;
    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_small.txt","content":"s"}]]"#,
    )
    .await;
    let snaps = list_undo(app, &sid).await;
    assert_eq!(snaps.len(), 2, "应有 2 条快照: {snaps:?}");
    let older = snaps[1]["id"].as_str().unwrap().to_string();

    let (status, res) = restore(app, &older).await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "预检失败应 500: {res}"
    );
    let msg = res["error"].as_str().unwrap_or_default();
    assert!(msg.contains("预检未通过"), "应说明预检拒绝: {msg}");
    assert!(msg.contains("256KB"), "应点明截断原因: {msg}");

    // 零改动:big 仍是被工具改写后的内容;small 未被撤销
    assert_eq!(std::fs::read_to_string(&big).unwrap(), "x");
    assert!(small.exists(), "预检拒绝时后续快照不得被撤销(文件仍在)");
    // 零删除:两条快照都在
    let snaps = list_undo(app, &sid).await;
    assert_eq!(snaps.len(), 2, "预检拒绝时快照不得删除: {snaps:?}");

    // 幂等:重复回退仍被同一预检拒绝
    let (status, _) = restore(app, &older).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

/// ⑤d 中途失败保留快照、修复后可原样重试(批 D-2):
/// 把后续快照指向的文件换成同名目录 →「删除文件」必然失败 → 500 且快照保留;
/// 清障后重试成功。
#[tokio::test]
async fn restore_midway_failure_keeps_snapshots_then_retry_succeeds() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 重试", &["write"]).await;
    let f1 = char_file(&cid, "undo_retry1.md");
    let f2 = char_file(&cid, "undo_retry2.md");

    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_retry1.md","content":"r1"}]]"#,
    )
    .await;
    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_retry2.md","content":"r2"}]]"#,
    )
    .await;
    let snaps = list_undo(app, &sid).await;
    let older = snaps[1]["id"].as_str().unwrap().to_string();

    // 制造中途失败:后续快照的逆操作(删除文件)必然失败——同名路径变成目录
    std::fs::remove_file(&f2).unwrap();
    std::fs::create_dir(&f2).unwrap();

    let (status, res) = restore(app, &older).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "应失败: {res}");
    let msg = res["error"].as_str().unwrap_or_default();
    assert!(msg.contains("已恢复 0/2"), "应报告进度: {msg}");
    assert!(msg.contains("快照记录已保留"), "应说明快照保留: {msg}");
    // 快照保留(失败不丢恢复依据);目标本身尚未被撤销(逆序从最新开始)
    let snaps = list_undo(app, &sid).await;
    assert_eq!(snaps.len(), 2, "失败后快照应全部保留: {snaps:?}");
    assert!(f1.exists(), "目标自身尚未处理,文件应保留");

    // 清障后原样重试 → 成功(逆操作幂等)
    std::fs::remove_dir(&f2).unwrap();
    let (status, res) = restore(app, &older).await;
    assert_eq!(status, StatusCode::OK, "清障后重试应成功: {res}");
    assert!(
        !f1.exists() && !f2.exists(),
        "重试后两文件都应回到目标时点(删除)"
    );
    assert!(list_undo(app, &sid).await.is_empty(), "成功后快照应清空");
}

/// ⑤e 文件与变量混合恢复(批 D-2):回退较早的 update_variables 快照,
/// 变量树与后续 write 的文件两类状态都应正确。
#[tokio::test]
async fn restore_mixed_variables_and_file() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 混合", &["write", "update_variables"]).await;
    let file = char_file(&cid, "undo_mix.md");

    // 预置变量树 score=5
    let (status, res) = send_json(
        app,
        "PUT",
        "/api/variables",
        json!({ "session_id": sid, "scope": "chat", "data": { "score": 5 } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PUT variables 应 200: {res}");

    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:update_variables {"patches":[{"op":"replace","path":"/score","value":99}]}]]"#,
    )
    .await;
    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_mix.md","content":"混合"}]]"#,
    )
    .await;
    assert!(file.exists(), "write 后文件应存在");
    let snaps = list_undo(app, &sid).await;
    assert_eq!(snaps.len(), 2, "应有 2 条快照: {snaps:?}");
    let vars_snap = snaps[1]["id"].as_str().unwrap().to_string();

    let (status, res) = restore(app, &vars_snap).await;
    assert_eq!(status, StatusCode::OK, "restore 应 200: {res}");
    // 变量树回到 5;后续 write 的文件一并撤销
    let (_, vars) = send_json(
        app,
        "GET",
        &format!("/api/variables?session_id={sid}"),
        json!({}),
    )
    .await;
    assert_eq!(vars["data"]["score"], json!(5), "变量树应回到目标时点");
    assert!(!file.exists(), "后续 write 的文件应在同一批中撤销");
    assert!(list_undo(app, &sid).await.is_empty(), "已处理快照应清空");
}

/// ⑥ 非写工具(calculator,Safe 无需授权)不产生快照
#[tokio::test]
async fn readonly_tool_produces_no_snapshot() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 只读", &[]).await;

    send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:calculator {"expression":"1+2"}]]"#,
    )
    .await;
    let snaps = list_undo(app, &sid).await;
    assert!(snaps.is_empty(), "calculator 不应产生快照: {snaps:?}");
}

/// ⑦ 快照绑定工具调用身份(批 D-1):同一 id 三处一致——快照行(列表)、
/// 工具结果 SSE 事件的可选元数据、tool_calls 落库(历史重载精确匹配的地基)。
#[tokio::test]
async fn snapshot_binds_to_tool_call_event_and_trace() {
    let _guard = test_lock().await;
    let app = test_app();
    let (cid, sid) = setup(app, "undo 绑定", &["write"]).await;

    let sse = send_tool_chat(
        app,
        &sid,
        &cid,
        r#"[[tool:write {"target":"file","path":"undo_bind.md","content":"绑定"}]]"#,
    )
    .await;

    let snaps = list_undo(app, &sid).await;
    assert_eq!(snaps.len(), 1, "应产生 1 条快照: {snaps:?}");
    let snap_id = snaps[0]["id"].as_str().unwrap().to_string();
    // 列表带调用身份(call_id):mock 的工具调用 id 形如 mock-call-0
    assert!(
        snaps[0]["call_id"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "快照列表应带 call_id 身份: {snaps:?}"
    );
    // SSE 工具结果事件携带同一快照 id(加性字段)
    assert!(
        sse.contains(&format!("\"snapshot_id\":\"{snap_id}\"")),
        "ToolResult 事件应携带快照 id {snap_id}: {sse}"
    );
    // 历史重载:tool_calls 落库同一 id
    let (status, trace) = send_json(
        app,
        "GET",
        &format!("/api/chat/sessions/{sid}/agent/trace"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "trace 应 200: {trace}");
    let calls = trace["trace"]["tool_calls"].as_array().unwrap();
    let write_call = calls
        .iter()
        .rev()
        .find(|c| c["name"] == json!("write"))
        .expect("trace 应含 write 工具调用");
    assert_eq!(
        write_call["snapshot_id"],
        json!(snap_id),
        "tool_calls 落库应绑定同一快照 id: {write_call:?}"
    );

    // 收尾:回退该次写入,避免污染共享 app 的后续用例状态
    let (status, res) = restore(app, &snap_id).await;
    assert_eq!(status, StatusCode::OK, "回退清理应 200: {res}");
}
