// API 集成测试 · 角色卡 / 会话 / 消息 / 导入导出（含多问候与锚点截断）。
//
// 本文件由原 `tests/api_integration.rs`（137KB / 62 用例）按 `/api/*` 路由域拆分而来
// （QUALITY-FIX Q1-2,2026-09-27）：纯移动、不改行为。**拆分的实质收益是测试隔离**——
// `tests/` 下每个 `.rs` 是独立测试进程，`build_test_app()` 的 `DATA_DIR`
// （`%TEMP%\kedai-test-<pid>`）与 `OnceLock` 单例 app 随文件独立，原先「同进程共享
// settings.json / 设置覆盖层 / 流程库」的顺序耦合由此消失（既有 flake `TEST-ISO-1` 正是这一土壤）。
// **别再把它们合回一个文件。**
//
// 测试辅助函数已收敛到 `tests/common`(2026-10-25 C-2E):`test_app` / `send_json` /
// `test_lock` / `wait_terminal` 只保留一份实现;每个测试文件 `mod common;` 仍编译为
// 独立测试二进制,进程级 app 与串行锁语义不变。`test_lock()` 仍只在本二进制内串行。
//
// 共用前提：与 Node 版 `server/tests/api.test.ts` 对齐；mock 连接器 + 临时数据目录 + 免鉴权。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
mod common;
use common::{send_json, test_app};

async fn send_empty(app: &axum::Router, method: &str, path: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    resp.status()
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

#[tokio::test]
async fn character_crud() {
    let app = test_app();
    let (status, char) = upload_character(app, "测试卡.json").await;
    assert_eq!(status, StatusCode::CREATED, "上传失败: {char}");
    let id = char["id"].as_str().unwrap().to_string();
    assert_eq!(char["chara_name"], json!("测试角色"));
    assert_eq!(char["name"], json!("测试卡"));
    assert_eq!(char["data_raw"]["custom_field"]["unknown"], json!(true));
    // 列表(不含 data_raw)
    let (_, list) = send_json(app, "GET", "/api/characters", json!({})).await;
    assert!(!list["characters"].as_array().unwrap().is_empty());
    assert!(list["characters"][0].get("data_raw").is_none());
    let (status, detail) = send_json(app, "GET", &format!("/api/characters/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(detail["data_raw"].is_object());
    let (status, updated) = send_json(
        app,
        "PUT",
        &format!("/api/characters/{id}"),
        json!({ "chara_name": "新名字", "description": "新描述" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["chara_name"], json!("新名字"));
    assert_eq!(updated["data_raw"]["name"], json!("新名字"));
    let status = send_empty(app, "DELETE", &format!("/api/characters/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // 删除后 404
    let (status, _) = send_json(app, "GET", &format!("/api/characters/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn session_and_messages() {
    let app = test_app();
    let (_, char) = upload_character(app, "会话测试.json").await;
    let cid = char["id"].as_str().unwrap().to_string();

    // 创建会话
    let (status, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid, "title": "我的会话" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let sid = session["id"].as_str().unwrap().to_string();
    assert_eq!(session["title"], json!("我的会话"));

    // 列表
    let (_, list) = send_json(
        app,
        "GET",
        &format!("/api/chat/sessions?character_id={cid}"),
        json!({}),
    )
    .await;
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);

    // 历史:角色 first_mes 作为首条 assistant 消息注入(开场白)
    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let msgs = history["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["role"], json!("assistant"));
    assert_eq!(msgs[0]["content"], json!("你好,我是测试角色"));
    assert_eq!(msgs[0]["extra"]["first_mes"], json!(true));

    // 导入消息
    let (status, imp) = send_json(
        app,
        "POST",
        "/api/import/chat",
        json!({ "session_id": sid, "messages": [
            { "role": "user", "content": "第一条" },
            { "role": "assistant", "content": "回复一" },
            { "role": "system", "content": "记忆", "extra": { "kind": "memory" } }
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(imp["imported"], json!(3));

    // 历史
    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let msgs = history["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0]["role"], json!("user"));
    assert!(msgs[0]["id"].is_number());

    // 更新消息
    let mid = msgs[0]["id"].as_i64().unwrap();
    let (status, updated) = send_json(
        app,
        "PUT",
        &format!("/api/chat/messages/{mid}?session_id={sid}"),
        json!({ "content": "改过的" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["content"], json!("改过的"));
    assert_eq!(updated["extra"]["edited"], json!(true));

    // 删除消息
    let status = send_empty(
        app,
        "DELETE",
        &format!("/api/chat/messages/{mid}?session_id={sid}"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 导出
    let (status, exp) = send_json(
        app,
        "GET",
        &format!("/api/export/chat?session_id={sid}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(exp["session_id"], json!(sid));
    assert_eq!(exp["messages"].as_array().unwrap().len(), 2);

    // 清空
    let (status, clear) =
        send_json(app, "POST", "/api/chat/clear", json!({ "session_id": sid })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(clear["ok"], json!(true));
    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    assert_eq!(history["messages"].as_array().unwrap().len(), 0);

    // 删除会话
    let status = send_empty(app, "DELETE", &format!("/api/chat/sessions/{sid}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// 消息截断端点(「编辑用户消息后重发」的支撑):保留 anchor 消息,删除其后所有消息
#[tokio::test]
async fn truncate_messages_after_anchor() {
    let app = test_app();
    let (_, char) = upload_character(app, "截断测试.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    // 导入 4 条消息(role 任意,id 自增有序)
    let (status, imp) = send_json(
        app,
        "POST",
        "/api/import/chat",
        json!({ "session_id": sid, "messages": [
            { "role": "user", "content": "m1" },
            { "role": "assistant", "content": "r1" },
            { "role": "user", "content": "m2" },
            { "role": "assistant", "content": "r2" }
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(imp["imported"], json!(4));

    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let msgs = history["messages"].as_array().unwrap().clone();
    assert_eq!(msgs.len(), 4);
    let anchor = msgs[1]["id"].as_i64().unwrap(); // 第二条(assistant r1)为锚点

    // 截断:删除 id > anchor 的消息(m2 / r2)
    let (status, res) = send_json(
        app,
        "POST",
        &format!("/api/chat/sessions/{sid}/truncate"),
        json!({ "anchor_id": anchor }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(res["ok"], json!(true));
    assert_eq!(res["deleted"], json!(2));

    // 历史只剩前两条,锚点消息本身保留
    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let msgs = history["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["content"], json!("m1"));
    assert_eq!(msgs[1]["content"], json!("r1"));

    // 会话不存在 → 404
    let (status, _) = send_json(
        app,
        "POST",
        "/api/chat/sessions/no-such/truncate",
        json!({ "anchor_id": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 多开场:角色卡 alternate_greetings 提取、建会话按 greeting_index 播种、
/// 越界回退、regreet 会话内切换、PUT 更新备用开场列表。
#[tokio::test]
async fn multi_greeting_seed_and_switch() {
    let app = test_app();
    // 上传带备用开场的角色卡(含一个空串条目,应被过滤)
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"多开场.json\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        json!({
            "spec": "chara_card_v2",
            "spec_version": "1.0",
            "name": "多开场角色",
            "description": "多开场测试",
            "first_mes": "主开场文本",
            "alternate_greetings": ["备用开场A", "备用开场B", "", "备用开场C"]
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
    let char: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(char["chara_name"], json!("多开场角色"));
    let cid = char["id"].as_str().unwrap().to_string();
    assert_eq!(
        char["alternate_greetings"],
        json!(["备用开场A", "备用开场B", "备用开场C"]),
        "空串备用开场应被过滤"
    );

    // greeting_index=0 → 主开场
    let (_, s0) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid, "greeting_index": 0 }),
    )
    .await;
    let (_, h0) = send_json(
        app,
        "GET",
        &format!(
            "/api/chat/history?session_id={}",
            s0["id"].as_str().unwrap()
        ),
        json!({}),
    )
    .await;
    let m0 = h0["messages"].as_array().unwrap();
    assert_eq!(m0.len(), 1);
    assert_eq!(m0[0]["content"], json!("主开场文本"));

    // greeting_index=2 → 第三条备用开场(下标对齐 alternate_greetings,不含空串)
    let (_, s2) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid, "greeting_index": 2 }),
    )
    .await;
    let (_, h2) = send_json(
        app,
        "GET",
        &format!(
            "/api/chat/history?session_id={}",
            s2["id"].as_str().unwrap()
        ),
        json!({}),
    )
    .await;
    let m2 = h2["messages"].as_array().unwrap();
    assert_eq!(m2.len(), 1);
    assert_eq!(m2[0]["content"], json!("备用开场B"));

    // 越界 index 回退到最后一个开场
    let (_, s99) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid, "greeting_index": 99 }),
    )
    .await;
    let (_, h99) = send_json(
        app,
        "GET",
        &format!(
            "/api/chat/history?session_id={}",
            s99["id"].as_str().unwrap()
        ),
        json!({}),
    )
    .await;
    let m99 = h99["messages"].as_array().unwrap();
    assert_eq!(m99[0]["content"], json!("备用开场C"));

    // 会话内切换(regreet):先发一条用户消息,再切换为备用开场A
    let sid = s0["id"].as_str().unwrap().to_string();
    let (status, _) = send_json(
        app,
        "POST",
        "/api/chat/send",
        json!({ "session_id": sid, "character_id": cid, "message": "你好" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, regreet) = send_json(
        app,
        "POST",
        &format!("/api/chat/sessions/{sid}/regreet"),
        json!({ "greeting_index": 1 }),
    )
    .await;
    assert_eq!(regreet["ok"], json!(true));
    // 清空后重新播种:历史仅剩新开场一条
    let (_, hr) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let mr = hr["messages"].as_array().unwrap();
    assert_eq!(mr.len(), 1, "regreet 应清空会话并按新开场重新播种: {hr}");
    assert_eq!(mr[0]["content"], json!("备用开场A"));
    assert_eq!(mr[0]["extra"]["first_mes"], json!(true));

    // PUT 更新备用开场列表(清空 = 删除字段)
    let (status, updated) = send_json(
        app,
        "PUT",
        &format!("/api/characters/{cid}"),
        json!({ "alternate_greetings": ["新备用X"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["alternate_greetings"], json!(["新备用X"]));
    let (_, cleared) = send_json(
        app,
        "PUT",
        &format!("/api/characters/{cid}"),
        json!({ "alternate_greetings": [] }),
    )
    .await;
    assert!(
        cleared.get("alternate_greetings").is_none(),
        "空数组应清空备用开场: {cleared}"
    );
}

// ==================== KEDAI-01:导入原子性(审查修复批 A)====================

/// 建角色 + 会话并返回 (cid, sid, 开场白消息 id)。开场白由建会话自动播种,是会话首条消息。
async fn session_with_greeting(app: &axum::Router, name: &str) -> (String, String, i64) {
    let (_, char) = upload_character(app, name).await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let mid = history["messages"].as_array().unwrap()[0]["id"]
        .as_i64()
        .unwrap();
    (cid, sid, mid)
}

async fn history_of(app: &axum::Router, sid: &str) -> Vec<Value> {
    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    history["messages"].as_array().unwrap().clone()
}

/// KEDAI-01:导入失败必须**整体不生效**——旧消息与 message 作用域变量原样保留。
/// 修复前:import_chat 先清空再逐条校验后写入,第二条 role 非法时
/// 旧记录已被删、只剩半截新消息(2026-10-15 隔离实例端到端复现)。
#[tokio::test]
async fn import_failure_is_atomic_for_messages_and_vars() {
    let app = test_app();
    let (_cid, sid, mid) = session_with_greeting(app, "导入原子性.json").await;

    // 给开场白挂 message 作用域变量(scope_variables 镜像):失败导入必须不动它
    let (status, res) = send_json(
        app,
        "PUT",
        "/api/variables",
        json!({
            "session_id": sid, "scope": "message", "scope_id": mid.to_string(),
            "data": { "hp": 7 },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写 message 作用域变量应 200: {res}");

    // 失败导入:第二条 role 非法
    let (status, err) = send_json(
        app,
        "POST",
        "/api/import/chat",
        json!({ "session_id": sid, "messages": [
            { "role": "user", "content": "新消息一" },
            { "role": "invalid-role", "content": "新消息二" },
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非法 role 应 400: {err}");

    // 消息原样:仍是开场白一条(修复前此处只剩「新消息一」)
    let msgs = history_of(app, &sid).await;
    assert_eq!(msgs.len(), 1, "失败导入不得改变原消息: {msgs:?}");
    assert_eq!(msgs[0]["content"], json!("你好,我是测试角色"));
    assert_eq!(msgs[0]["id"], json!(mid), "原消息应连 id 一起保留");

    // 变量原样(修复前该行被 clear 的变量清理一并删掉)
    let (status, vars) = send_json(
        app,
        "GET",
        &format!("/api/variables?session_id={sid}&scope=message&scope_id={mid}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(vars["scope"], json!("message"));
    assert_eq!(vars["scope_id"], json!(mid.to_string()));
    assert_eq!(
        vars["data"]["hp"],
        json!(7),
        "失败导入不得清掉旧消息的作用域变量: {vars}"
    );
}

/// KEDAI-01:中间消息正文为空同样**整体拒绝**,不残留任何新消息(旧数据不动)。
#[tokio::test]
async fn import_failure_on_empty_content_keeps_old_messages() {
    let app = test_app();
    let (_cid, sid, mid) = session_with_greeting(app, "导入空正文.json").await;

    let (status, err) = send_json(
        app,
        "POST",
        "/api/import/chat",
        json!({ "session_id": sid, "messages": [
            { "role": "user", "content": "有效一条" },
            { "role": "assistant", "content": "" },
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空正文应 400: {err}");

    let msgs = history_of(app, &sid).await;
    assert_eq!(msgs.len(), 1, "不得残留任何新消息: {msgs:?}");
    assert_eq!(msgs[0]["id"], json!(mid));
}

/// KEDAI-01:成功导入照旧替换(条数/归一化/extra 保留);空数组 = 清空——
/// 这是**现有契约**,修复只加原子性、不顺手改变它(显式锁定)。
#[tokio::test]
async fn import_success_and_empty_array_semantics() {
    let app = test_app();
    let (_cid, sid, _mid) = session_with_greeting(app, "导入成功.json").await;

    let (status, imp) = send_json(
        app,
        "POST",
        "/api/import/chat",
        json!({ "session_id": sid, "messages": [
            { "role": "User", "content": "a" },
            { "role": "assistant", "content": "b", "extra": { "kind": "memory" } },
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(imp["imported"], json!(2));

    let msgs = history_of(app, &sid).await;
    assert_eq!(msgs.len(), 2, "成功导入应完整替换: {msgs:?}");
    assert_eq!(msgs[0]["role"], json!("user"), "role 应归一化为小写");
    assert_eq!(msgs[1]["extra"]["kind"], json!("memory"), "extra 应保留");

    // 空数组 = 清空(imported=0)
    let (status, imp0) = send_json(
        app,
        "POST",
        "/api/import/chat",
        json!({ "session_id": sid, "messages": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(imp0["imported"], json!(0));
    assert_eq!(
        history_of(app, &sid).await.len(),
        0,
        "空数组 = 清空(现有语义,不得改变)"
    );
}
