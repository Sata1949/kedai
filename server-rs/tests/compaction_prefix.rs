//! auto 压缩全链路集成测试(harness 补强 HB-5,2026-09-18)。
//!
//! ## 为什么单独一份文件
//!
//! `compaction.rs` 的既有单测止步于纯函数与 DB 存取层(`should_auto_compact` /
//! `compaction_split` / `merge_incremental_summary` / `save_compaction`),
//! **auto 模式的引擎侧触发链**(`maybe_compact` → 增量段 → `generate_text` → 落库
//! → 下一轮投影)没有端到端用例。前缀稳定性是本仓库在缓存上的既有投入方向,
//! 这条链路一旦回归(摘要重写前缀、原文被删、尾部被吞),损失直接体现在费用上。
//!
//! ## 触发口径(实现约束,不是测试选择)
//!
//! `max_context_tokens` 被钳在 65536 下限(`secret.rs` / `api/settings.rs`),
//! `compaction_threshold` 下限 0.5 → 触发 auto 压缩**最少**需要 32768 历史 token。
//! 本用例把长消息定在 33000~38000 token 区间:高于触发线、低于 snip 水位
//! (0.6 × 65536 = 39321),使断言聚焦压缩路径本身(snip 投影由 `should_snip` /
//! `snip_tuples` 的单测覆盖)。文本按段变化——单字重复会被 BPE 压成极少 token。
//!
//! ## 断言口径
//!
//! ① 摘要落 `session_compactions`;② 旧摘要行字节冻结、新一轮只做尾部追加;
//! ③ 原文一条不删(可逆投影设计);④ 下一轮下发的消息数组里摘要槽前缀未被扰动
//! (借 `llm_request_log` 快照读「模型到底看到了什么」,而非重启推演)。
//!
//! 库直读沿用 `tests/agent_trace.rs` 的做法(测试数据目录是 `build_test_app` 的
//! 进程级共享单例,路径可由 pid 推出),不新造接口。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
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

/// 进程内串行:本用例改全局设置(压缩模式/窗口/快照开关),不得与其他用例并发。
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

/// 测试库路径:与 `build_test_app` 同款(%TEMP%/kedai-test-{pid}/kedai.db)。
fn test_db_path() -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("kedai-test-{}", std::process::id()));
    dir.push("kedai.db");
    dir
}

async fn upload_character(app: &axum::Router, name: &str, first_mes: &str) -> String {
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        json!({
            "spec": "chara_card_v2", "spec_version": "1.0",
            "name": name, "description": "压缩链路测试", "first_mes": first_mes,
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
    let ch: Value = serde_json::from_slice(&bytes).unwrap();
    ch["id"].as_str().expect("上传角色应返回 id").to_string()
}

async fn new_session(app: &axum::Router, cid: &str) -> String {
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    session["id"].as_str().expect("建会话应返回 id").to_string()
}

/// 发一轮 chat,读完整 SSE 流并解析为事件数组
async fn sse_events(app: &axum::Router, sid: &str, cid: &str, message: &str) -> Vec<Value> {
    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "session_id": sid, "character_id": cid,
                "message": message, "agent_mode": "fast",
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

/// 计数器:一段文本的 token 数(走产品自身的计数端点,不另养一套估算)
async fn token_total(app: &axum::Router, text: &str) -> i64 {
    let (status, v) = send_json(
        app,
        "POST",
        "/api/token/count",
        json!({ "messages": [{ "role": "user", "content": text }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "token 计数应 200");
    v["total"].as_i64().expect("token 计数应返回 total")
}

/// 构造 token 数落在 [33_000, 38_000] 的多样化长文本。
/// 段内固定套语 + 段号变化:整体可读、BPE 不塌缩,且每段内容不同。
async fn sized_message(app: &axum::Router) -> String {
    const UNIT: &str = "雨夜里,角色在旧书店的阁楼上翻找一封写给十年后自己的信,纸张泛黄,墨迹被潮气泡开,窗外有轨电车拖着长长的雨线驶过。";
    let build = |segments: usize| {
        (1..=segments)
            .map(|i| format!("第{i}段:{UNIT}\n"))
            .collect::<String>()
    };
    let mut segments = 900usize;
    for _ in 0..6 {
        let text = build(segments);
        let tokens = token_total(app, &text).await;
        if (33_000..=38_000).contains(&tokens) {
            return text;
        }
        // 线性外推:目标 35_500 token(区间中点)
        segments = ((segments as f64) * 35_500.0 / (tokens.max(1) as f64)) as usize + 5;
    }
    panic!("长消息定长失败(6 轮内未落入 33000~38000 token 区间)");
}

/// 读会话的压缩行:按 upto_message_id 升序返回 (upto, summary)
fn compactions(session_id: &str) -> Vec<(i64, String)> {
    let conn = rusqlite::Connection::open(test_db_path()).expect("打开测试库失败");
    let mut stmt = conn
        .prepare(
            "SELECT upto_message_id, summary FROM session_compactions \
             WHERE session_id = ?1 ORDER BY upto_message_id",
        )
        .expect("准备查询失败");
    let rows = stmt
        .query_map([session_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("查询压缩行失败");
    rows.filter_map(|r| r.ok()).collect()
}

/// 全部历史消息的 (id, role, content 字符数)
fn messages_digest(session_id: &str) -> Vec<(i64, String, usize)> {
    let conn = rusqlite::Connection::open(test_db_path()).expect("打开测试库失败");
    let mut stmt = conn
        .prepare("SELECT id, role, length(content) FROM messages WHERE session_id = ?1 ORDER BY id")
        .expect("准备查询失败");
    let rows = stmt
        .query_map([session_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as usize))
        })
        .expect("查询消息失败");
    rows.filter_map(|r| r.ok()).collect()
}

/// 最近一次「含摘要槽的请求」里摘要槽的完整内容(模型实际看到的那份)。
/// 摘要槽是独立 system 消息,以【早期对话摘要】开头;压缩请求自身不含摘要槽,故
/// 取到的必是压缩之后的正文请求。
fn last_summary_slot(session_id: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(test_db_path()).expect("打开测试库失败");
    let mut stmt = conn
        .prepare("SELECT payload FROM llm_requests WHERE session_id = ?1 ORDER BY id")
        .expect("准备查询失败");
    let rows = stmt
        .query_map([session_id], |r| r.get::<_, String>(0))
        .expect("查询请求快照失败");
    let mut found = None;
    for payload in rows.flatten() {
        let Ok(msgs) = serde_json::from_str::<Vec<Value>>(&payload) else {
            continue;
        };
        for m in msgs {
            let content = m["content"].as_str().unwrap_or_default();
            if content.starts_with("【早期对话摘要】") {
                found = Some(content.to_string());
            }
        }
    }
    found
}

#[tokio::test]
async fn auto_compaction_keeps_prefix_and_freezes_prior_summary() {
    let _guard = test_lock().await;
    let app = test_app();

    // 设置:auto 压缩(最低窗口 + 最低阈值)+ 请求快照(读「模型到底看到了什么」)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "compaction_mode": "auto",
            "compaction_threshold": 0.5,
            "max_context_tokens": 65_536,
            "compaction_keep_recent": 4,
            "llm_request_log": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "压缩相关设置应保存成功");

    // 开场白带 [[reply:]] 钩子:压缩请求把历史段拼成**一条 user 消息**,钩子命中 →
    // 首轮摘要内容确定。第三轮起该段已被摘要覆盖、不再进压缩段,摘要增量走默认回复。
    let cid = upload_character(app, "压缩前缀特征化.json", "开场白 [[reply:压缩机摘要-首]]").await;
    let sid = new_session(app, &cid).await;
    let big = sized_message(app).await;
    // 落库前 chat/send 会对消息做 trim(api/chat.rs),比对口径与库里一致
    let big_chars = big.trim().chars().count();

    // 轮 1:开场白(1 条)→ 不触发
    let e1 = sse_events(app, &sid, &cid, &big).await;
    assert!(
        e1.iter().any(|e| e["type"] == "finish"),
        "首轮应有 finish: {:?}",
        e1.iter()
            .filter_map(|e| e["type"].as_str())
            .collect::<Vec<_>>()
    );
    assert!(compactions(&sid).is_empty(), "历史不足时不得压缩");

    // 轮 2:历史 3 条(≤ compaction_keep_recent=4,compaction_split 返回 None)→ 不触发
    let _ = sse_events(app, &sid, &cid, "第二条").await;
    assert!(
        compactions(&sid).is_empty(),
        "历史条数不足以保留尾部原文时不得压缩"
    );

    // 轮 3:历史 5 条 → 压缩首段(开场白)
    let _ = sse_events(app, &sid, &cid, "第三条").await;
    let rows = compactions(&sid);
    assert_eq!(rows.len(), 1, "首轮 auto 压缩应落一行摘要: {rows:?}");
    let (upto1, summary1) = rows[0].clone();
    assert!(
        summary1.contains("压缩机摘要-首"),
        "首轮摘要应来自开场白段的钩子回复: {summary1}"
    );
    let slot3 = last_summary_slot(&sid).expect("压缩后的请求应含摘要槽");
    assert_eq!(
        slot3,
        format!("【早期对话摘要】\n{summary1}"),
        "摘要槽内容应等于落库摘要(模型看到的与库里的一致)"
    );

    // 轮 4:历史 7 条 → 增量压缩(段 = 长消息 + 首轮回复)
    let before_tail = messages_digest(&sid);
    let _ = sse_events(app, &sid, &cid, "第四条").await;
    let rows2 = compactions(&sid);
    assert_eq!(rows2.len(), 2, "第二轮应新增一行摘要(不是覆盖旧行)");
    assert_eq!(
        rows2[0],
        (upto1, summary1.clone()),
        "旧摘要行必须字节冻结(重写前缀 = 前缀缓存全 miss)"
    );
    let (upto2, summary2) = rows2[1].clone();
    assert!(upto2 > upto1, "新摘要覆盖点应前移: {upto2} > {upto1}");
    assert!(
        summary2.starts_with(&summary1) && summary2.len() > summary1.len(),
        "新一轮摘要只能在旧摘要尾部追加:\n旧={summary1:?}\n新={summary2:?}"
    );

    // 原文一条不删(可逆投影:删摘要行即恢复完整历史),尾部 keep_recent 条原文完整
    let after_tail = messages_digest(&sid);
    assert_eq!(
        before_tail.len() + 2,
        after_tail.len(),
        "一轮问答应新增 2 条消息(用户 + 助手),其余不得被删"
    );
    for (id, role, chars) in &before_tail {
        let same = after_tail
            .iter()
            .find(|(i, _, _)| i == id)
            .unwrap_or_else(|| panic!("消息 {id} 被压缩删除"));
        assert_eq!((&same.1, same.2), (role, *chars), "消息 {id} 原文被改写");
    }
    let big_row = after_tail
        .iter()
        .find(|(_, role, chars)| role == "user" && *chars == big_chars)
        .expect("长消息原文必须仍在库中");
    assert_eq!(big_row.2, big_chars, "长消息原文不得被裁剪/改写");

    // 摘要槽前缀未被扰动:第二轮请求里的摘要槽以第一轮内容为前缀(只追加)
    let slot4 = last_summary_slot(&sid).expect("第二轮请求仍应含摘要槽");
    assert_eq!(
        slot4,
        format!("【早期对话摘要】\n{summary2}"),
        "摘要槽应等于新的落库摘要"
    );
    assert!(
        slot4.starts_with(&slot3) && slot4.len() > slot3.len(),
        "下一轮构建的摘要槽必须以旧内容为前缀(只追加):\n旧={slot3:?}\n新={slot4:?}"
    );
}
