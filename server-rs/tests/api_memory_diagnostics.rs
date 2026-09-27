// API 集成测试 · 记忆库（蒸馏 / 检索 / 剪枝 / 注入）/ 诊断缓存。
//
// 本文件由原 `tests/api_integration.rs`（137KB / 62 用例）按 `/api/*` 路由域拆分而来
// （QUALITY-FIX Q1-2,2026-09-27）：纯移动、不改行为。**拆分的实质收益是测试隔离**——
// `tests/` 下每个 `.rs` 是独立测试进程，`build_test_app()` 的 `DATA_DIR`
// （`%TEMP%\kedai-test-<pid>`）与 `OnceLock` 单例 app 随文件独立，原先「同进程共享
// settings.json / 设置覆盖层 / 流程库」的顺序耦合由此消失（既有 flake `TEST-ISO-1` 正是这一土壤）。
// **别再把它们合回一个文件。**
//
// 测试辅助函数按「谁用谁带」复制（仓库既有体例：40 个测试文件里 36 个各自持有 `test_app`），
// 刻意不建 `tests/common` 共享层：那会让 count-tests 的集成文件数口径虚高，也会给多会话并行
// 改测试制造新的争用点。`test_lock()` 是**本二进制内**的串行锁，随文件带走即可。
//
// 共用前提：与 Node 版 `server/tests/api.test.ts` 对齐；mock 连接器 + 临时数据目录 + 免鉴权。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tower::ServiceExt;

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

/// 直插 llm_requests 缓存统计行(绕过引擎,精确控制命中/未命中数据)
fn insert_cache_rows(sid: &str) {
    // build_test_app 的进程级共享数据目录(只读/直插,不受本批次守卫管理)
    let db_path = std::env::temp_dir()
        .join(format!("kedai-test-{}", std::process::id()))
        .join("kedai.db");
    let conn = rusqlite::Connection::open(&db_path).expect("打开测试库失败");
    for (seq, hit, miss, prompt, completion) in
        [(1, 700, 300, 1000, 200), (2, 600, 400, 1000, 1000)]
    {
        conn.execute(
            "INSERT INTO llm_requests
               (session_id, run_id, seq, payload, model, created_at,
                prompt_cache_hit_tokens, prompt_cache_miss_tokens, prompt_tokens, completion_tokens)
             VALUES (?1, 'run-diag', ?2, '', 'mock', ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                sid,
                seq,
                format!("2026-08-16T00:00:0{seq}Z"),
                hit,
                miss,
                prompt,
                completion
            ],
        )
        .expect("插入缓存统计行失败");
    }
}

/// 泄露守卫:响应体不得包含内部实现细节
#[track_caller]
fn assert_no_internal_leak(body: &Value) {
    let text = body.to_string();
    for needle in [
        "SQLite",
        "no such column",
        "no such table",
        "Failed to deserialize",
        "missing field",
        r"C:\",
        "D:\\",
        "rusqlite",
        "panicked at",
        "os error",
    ] {
        assert!(
            !text.contains(needle),
            "响应体泄露内部细节「{needle}」: {text}"
        );
    }
}

#[tokio::test]
async fn diagnostics_cache_reports_hit_rate_and_pricing() {
    let app = test_app();
    let (_, char) = upload_character(app, "缓存诊断.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid, "title": "缓存诊断会话" }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    insert_cache_rows(&sid);

    let (_, body) = send_json(
        app,
        "GET",
        &format!("/api/diagnostics/cache?session_id={sid}"),
        json!({}),
    )
    .await;
    let totals = &body["totals"];
    assert_eq!(totals["count"], json!(2), "应统计 2 条: {body}");
    assert_eq!(totals["total_hit"], json!(1300));
    assert_eq!(totals["total_miss"], json!(700));
    let rate = totals["hit_rate"].as_f64().expect("hit_rate 应为数值");
    assert!((rate - 0.65).abs() < 1e-9, "加权命中率 0.65,实际 {rate}");
    // 费用 = (1300*0.27 + 700*2 + 1200*8)/1e6 = 0.011351(默认 DeepSeek 参考价)
    let cost = body["cost"].as_f64().unwrap();
    assert!((cost - 0.011351).abs() < 1e-9, "费用估算: {cost}");
    let saved = body["saved"].as_f64().unwrap();
    assert!((saved - 0.002249).abs() < 1e-9, "节省估算: {saved}");
    // 明细:时间正序(seq 1 在前),含 session/时间/hit/miss
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["hit"], json!(700));
    assert_eq!(entries[1]["miss"], json!(400));
    assert_eq!(entries[0]["session_id"], json!(sid));
    // 单价结构体透出(每百万 token,不做货币换算)
    assert_eq!(body["pricing"]["cache_hit_per_m"], json!(0.27));
    assert_eq!(body["pricing"]["input_per_m"], json!(2.0));
    assert_eq!(body["pricing"]["output_per_m"], json!(8.0));
    // 水位报告字段齐全(档位合法集合;具体档位随并行测试的 settings 变化,不精确断言)
    let level = body["watermark"]["level"].as_str().unwrap_or("");
    assert!(
        ["ok", "soft", "snip", "compact", "force", "unknown"].contains(&level),
        "水位档位应合法: {body}"
    );
    assert!(body["watermark"]["max_context_tokens"].is_u64());

    // 窗口过滤:window=1 只取最新一条(seq 2)
    let (_, body) = send_json(
        app,
        "GET",
        &format!("/api/diagnostics/cache?session_id={sid}&window=1"),
        json!({}),
    )
    .await;
    assert_eq!(body["totals"]["count"], json!(1));
    assert_eq!(body["entries"][0]["hit"], json!(600));
}

/// 回归:不带 session_id 时必须成功(全会话合并统计)。
///
/// 该分支曾因 SQL 沿用 `LIMIT ?2` 而只绑定一个参数,恒定返回
/// `{"error":"... Wrong number of parameters ..."}`;且错误分支用 200 返回,
/// 于是前端 `CacheHealthPanel`(默认不传 sessionId)把 `{error}` 当成
/// `CacheDiagnostics`,访问 `data.totals.hit_rate` 抛 TypeError。
/// 本测试锁定「200 + 正常结构」,不允许错误体出现在成功路径上。
#[tokio::test]
async fn diagnostics_cache_without_session_id_returns_all_sessions() {
    let app = test_app();
    let (_, char) = upload_character(app, "缓存诊断无会话.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid, "title": "无会话过滤" }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();
    insert_cache_rows(&sid);

    // window=500(上限)避开与本文件其它用例的时间序竞争
    let (status, body) =
        send_json(app, "GET", "/api/diagnostics/cache?window=500", json!({})).await;
    assert_eq!(status, StatusCode::OK, "无 session_id 应正常返回: {body}");
    assert!(
        body.get("error").is_none(),
        "成功路径不得出现 error 字段: {body}"
    );
    // 结构完整:totals 为对象且统计到至少本用例插入的两条
    assert!(body["totals"].is_object(), "totals 应为对象: {body}");
    let count = body["totals"]["count"].as_u64().unwrap_or(0);
    assert!(
        count >= 2,
        "全会话统计应包含本用例的 2 条,实际 {count}: {body}"
    );
    // entries 与 count 自洽,且每行字段齐全(校验无会话分支的列映射未错位)。
    // 这里不断言「本用例的会话一定在窗口内」:本文件所有用例共用一个库并行执行,
    // 窗口截断取决于其它用例插入的行数,依赖它会引入时序 flake。
    let entries = body["entries"].as_array().expect("entries 应为数组");
    assert_eq!(
        entries.len() as u64,
        count,
        "entries 条数应与 totals.count 一致: {body}"
    );
    let first = entries.first().expect("至少有 1 条明细");
    for key in ["session_id", "created_at", "hit", "miss", "prompt_tokens"] {
        assert!(first.get(key).is_some(), "明细缺少字段 {key}: {first}");
    }
    // 缺省 session_id 回传 null(前端据此区分「全会话」与「单会话」)
    assert_eq!(body["session_id"], json!(null));
    assert!(body["watermark"]["max_context_tokens"].is_u64());
}

// ===== 跨会话记忆蒸馏(落地项 2) =====

/// 记忆库全流程:设置开关 → 蒸馏(mock [[reply:]] 钩子按行拆分)→ 列表 → 手动添加
/// → 编辑 → 删除;默认未开启蒸馏时端点拒绝。
#[tokio::test]
async fn memory_distill_crud_flow() {
    let app = test_app();
    let (_, char) = upload_character(app, "记忆角色.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    // 默认蒸馏关闭:端点拒绝并给出开启指引(先显式复位,规避并行用例的设置残留)
    let _ = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_distill_enabled": false }),
    )
    .await;
    let (status, err) = send_json(
        app,
        "POST",
        "/api/memory/distill",
        json!({ "session_id": sid }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未开启应 400: {err}");
    assert!(err["error"]
        .as_str()
        .unwrap()
        .contains("memory_distill_enabled"));

    // 开启蒸馏
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_distill_enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 导入带 [[reply:]] 钩子的历史:mock 连接器回显标记内文本 → 按行拆 2 条
    let (status, _) = send_json(
        app,
        "POST",
        "/api/import/chat",
        json!({ "session_id": sid, "messages": [
            { "role": "user", "content": "我们聊聊吧[[reply:用户喜欢下雪天\n角色害怕打雷]]" },
            { "role": "assistant", "content": "好呀。" }
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send_json(
        app,
        "POST",
        "/api/memory/distill",
        json!({ "session_id": sid }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "蒸馏失败: {body}");
    assert_eq!(body["inserted"], json!(2), "应按行拆 2 条: {body}");
    assert_eq!(body["character_id"], json!(cid));

    // 列表:全字段(id/kind/source_session_id/content/usage_count/selected)
    let (status, list) = send_json(
        app,
        "GET",
        &format!("/api/memory?character_id={cid}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let memories = list["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 2);
    let m0 = &memories[0];
    assert!(m0["id"].is_i64());
    assert_eq!(m0["character_id"], json!(cid));
    assert_eq!(m0["kind"], json!("distilled"));
    assert_eq!(m0["source_session_id"], json!(sid));
    assert_eq!(m0["usage_count"], json!(0));
    assert_eq!(m0["selected"], json!(true));
    assert!(m0["created_at"].is_string());
    let contents: Vec<&str> = memories
        .iter()
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert!(contents.contains(&"用户喜欢下雪天"), "内容: {contents:?}");
    assert!(contents.contains(&"角色害怕打雷"), "内容: {contents:?}");

    // 手动添加
    let (status, created) = send_json(
        app,
        "POST",
        "/api/memory",
        json!({ "character_id": cid, "content": "  用户养了一只猫  " }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "手动添加失败: {created}");
    assert_eq!(created["memory"]["kind"], json!("manual"));
    assert_eq!(created["memory"]["content"], json!("用户养了一只猫"));
    let manual_id = created["memory"]["id"].as_i64().unwrap();

    // 编辑:content 与 selected
    let (status, updated) = send_json(
        app,
        "PATCH",
        &format!("/api/memory/{manual_id}"),
        json!({ "content": "用户养了两只猫", "selected": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "编辑失败: {updated}");
    assert_eq!(updated["memory"]["content"], json!("用户养了两只猫"));
    assert_eq!(updated["memory"]["selected"], json!(false));
    // 空白 content 拒绝
    let (status, _) = send_json(
        app,
        "PATCH",
        &format!("/api/memory/{manual_id}"),
        json!({ "content": "   " }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // 不存在的 id → 404
    let (status, _) = send_json(
        app,
        "PATCH",
        "/api/memory/999999",
        json!({ "selected": true }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 删除:204;重复删除 404
    let status = send_empty(app, "DELETE", &format!("/api/memory/{manual_id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let status = send_empty(app, "DELETE", &format!("/api/memory/{manual_id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // 删除后列表只剩 2 条蒸馏记忆
    let (_, list) = send_json(
        app,
        "GET",
        &format!("/api/memory?character_id={cid}"),
        json!({}),
    )
    .await;
    assert_eq!(list["memories"].as_array().unwrap().len(), 2);
    // 还原蒸馏开关,避免污染并行用例
    let _ = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_distill_enabled": false }),
    )
    .await;
}

/// GET /api/memory/search:中文 FTS 命中、缺参 400、limit 上限钳制;
/// POST /api/memory/prune:硬删除 selected=0 归档条目并返回条数
#[tokio::test]
async fn memory_search_and_prune_endpoints() {
    let app = test_app();
    let (_, char) = upload_character(app, "记忆检索角色.json").await;
    let cid = char["id"].as_str().unwrap().to_string();

    // 落两条记忆(其中一条含「图书馆」)
    let (status, created) = send_json(
        app,
        "POST",
        "/api/memory",
        json!({ "character_id": cid, "content": "用户与角色在图书馆初识" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let lib_id = created["memory"]["id"].as_i64().unwrap();
    let (_, other) = send_json(
        app,
        "POST",
        "/api/memory",
        json!({ "character_id": cid, "content": "角色害怕打雷" }),
    )
    .await;
    let other_id = other["memory"]["id"].as_i64().unwrap();

    // 检索:中文命中
    let (status, body) = send_json(
        app,
        "GET",
        &format!("/api/memory/search?character_id={cid}&q=%E5%9B%BE%E4%B9%A6%E9%A6%86"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "检索失败: {body}");
    let memories = body["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 1, "应仅命中含「图书馆」的记忆: {body}");
    assert_eq!(memories[0]["id"], json!(lib_id));
    assert_eq!(memories[0]["pinned"], json!(false), "条目应含 pinned 字段");

    // 缺参 400
    let (status, _) = send_json(
        app,
        "GET",
        &format!("/api/memory/search?character_id={cid}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "缺 q 应 400");
    let (status, _) = send_json(app, "GET", "/api/memory/search?q=test", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "缺 character_id 应 400");

    // limit 超上限钳制到 100(不报错)
    let (status, body) = send_json(
        app,
        "GET",
        &format!("/api/memory/search?character_id={cid}&q=%E5%9B%BE%E4%B9%A6%E9%A6%86&limit=9999"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["memories"].as_array().unwrap().len(), 1);

    // prune:两条都 selected=1 → 删除 0 条
    let (status, body) = send_json(
        app,
        "POST",
        "/api/memory/prune",
        json!({ "character_id": cid }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], json!(0), "无归档条目应删 0 条");

    // 归档一条后 prune 硬删除它,列表只剩一条
    let (status, _) = send_json(
        app,
        "PATCH",
        &format!("/api/memory/{other_id}"),
        json!({ "selected": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send_json(
        app,
        "POST",
        "/api/memory/prune",
        json!({ "character_id": cid }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"], json!(1), "应硬删除 1 条归档条目: {body}");
    let (_, list) = send_json(
        app,
        "GET",
        &format!("/api/memory?character_id={cid}"),
        json!({}),
    )
    .await;
    let remaining = list["memories"].as_array().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0]["id"], json!(lib_id));

    // 缺参 400
    let (status, _) = send_json(app, "POST", "/api/memory/prune", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// 记忆设置白名单透出:GET 默认 关闭/上限 8;PUT 可写,越界(>50)忽略、0 合法
#[tokio::test]
async fn memory_settings_exposed_and_clamped() {
    let app = test_app();
    let (status, s) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        s["memory_distill_enabled"].is_boolean(),
        "设置应透出 memory_distill_enabled: {s}"
    );
    assert_eq!(s["memory_inject_limit"], json!(8), "注入上限默认 8: {s}");
    assert_eq!(
        s["memory_inject_char_budget"],
        json!(2000),
        "字符预算默认 2000: {s}"
    );
    assert_eq!(s["memory_max_entries"], json!(200), "容量默认 200: {s}");

    // 越界值忽略(保持默认);0 合法(关闭注入/不限制/不淘汰)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_inject_limit": 999 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, s) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(s["memory_inject_limit"], json!(8), "越界值应被忽略");

    // 新增两字段:越界忽略、合法值生效
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_inject_char_budget": 999_999, "memory_max_entries": 999_999 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, s) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(
        s["memory_inject_char_budget"],
        json!(2000),
        "越界预算应忽略"
    );
    assert_eq!(s["memory_max_entries"], json!(200), "越界容量应忽略");
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_inject_char_budget": 500, "memory_max_entries": 10 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, s) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(s["memory_inject_char_budget"], json!(500), "合法预算应生效");
    assert_eq!(s["memory_max_entries"], json!(10), "合法容量应生效");
    // 还原
    let _ = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_inject_char_budget": 2000, "memory_max_entries": 200 }),
    )
    .await;

    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_inject_limit": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, s) = send_json(app, "GET", "/api/settings", json!({})).await;
    assert_eq!(s["memory_inject_limit"], json!(0), "0 应合法(关闭注入)");
    // 还原注入上限默认,避免影响并行用例
    let _ = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_inject_limit": 8 }),
    )
    .await;
}

/// 端到端:记忆槽随 chat/send 注入消息数组([[floors]] 回显断言),响应完成后
/// touch 回写 usage_count;inject_limit=0 时等价关闭注入。
#[tokio::test]
async fn memory_slot_injected_and_touched() {
    let app = test_app();
    let (_, char) = upload_character(app, "记忆注入角色.json").await;
    let cid = char["id"].as_str().unwrap().to_string();
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid = session["id"].as_str().unwrap().to_string();

    // 手动添加一条记忆(默认 inject_limit=8 > 0,即注入)
    let (status, created) = send_json(
        app,
        "POST",
        "/api/memory",
        json!({ "character_id": cid, "content": "用户偏爱雨天" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let memory_id = created["memory"]["id"].as_i64().unwrap();

    // 发送一轮:[[floors]] 回显完整 LLM 消息数组 → 记忆槽应作为独立 system 消息出现
    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "session_id": sid, "character_id": cid, "message": "看记忆 [[floors]]" })
                .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let events: Vec<Value> = text
        .split("\n\n")
        .filter_map(|block| {
            let block = block.trim();
            if block.is_empty() {
                return None;
            }
            let data_line = block.lines().find(|l| l.starts_with("data: "))?;
            serde_json::from_str(&data_line[6..]).ok()
        })
        .collect();
    let finish = events
        .iter()
        .find(|e| e["type"] == "finish")
        .expect("应有 finish 事件");
    let echoed = finish["content"].as_str().unwrap();
    assert!(
        echoed.contains("[system] 【角色长期记忆】"),
        "回显应含记忆槽 system 消息: {}",
        &echoed[..echoed.len().min(300)]
    );
    assert!(
        echoed.contains("- 用户偏爱雨天"),
        "记忆槽应逐条一行注入: {}",
        &echoed[..echoed.len().min(300)]
    );

    // 响应完成后 touch 生效:usage_count+1、last_usage 落时间戳,content 不变
    let (_, list) = send_json(
        app,
        "GET",
        &format!("/api/memory?character_id={cid}"),
        json!({}),
    )
    .await;
    let entry = list["memories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_i64() == Some(memory_id))
        .expect("应能查到记忆条目");
    assert_eq!(entry["usage_count"], json!(1), "注入后应回写计数: {entry}");
    assert!(
        entry["last_usage"].is_string(),
        "last_usage 应有值: {entry}"
    );

    // inject_limit=0 等价关闭注入:新会话(避免上轮回显文本残留在历史)不再出现记忆槽
    let _ = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_inject_limit": 0 }),
    )
    .await;
    let (_, session2) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    let sid2 = session2["id"].as_str().unwrap().to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "session_id": sid2, "character_id": cid, "message": "再看 [[floors]]" })
                .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let events: Vec<Value> = text
        .split("\n\n")
        .filter_map(|block| {
            let block = block.trim();
            if block.is_empty() {
                return None;
            }
            let data_line = block.lines().find(|l| l.starts_with("data: "))?;
            serde_json::from_str(&data_line[6..]).ok()
        })
        .collect();
    let finish = events
        .iter()
        .find(|e| e["type"] == "finish")
        .expect("应有 finish 事件");
    let echoed2 = finish["content"].as_str().unwrap();
    assert!(
        !echoed2.contains("【角色长期记忆】"),
        "limit=0 时不应注入记忆槽: {}",
        &echoed2[..echoed2.len().min(300)]
    );
    // 计数不再增长(第二轮未注入)
    let (_, list) = send_json(
        app,
        "GET",
        &format!("/api/memory?character_id={cid}"),
        json!({}),
    )
    .await;
    let entry = list["memories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_i64() == Some(memory_id))
        .unwrap();
    assert_eq!(entry["usage_count"], json!(1), "未注入不应回写计数");

    // 还原设置,避免污染并行用例
    let _ = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "memory_inject_limit": 8 }),
    )
    .await;
}

// ==================== 工具插件文件名净化策略统一(优化项 B-4) ====================

/// 向量化连接测试:测试失败属「业务上报」而非 HTTP 错误,保留 200 + ok:false,
/// 补 code 供程序化分支(未配置 → VALIDATION)
#[tokio::test]
async fn embedding_test_reports_code_without_http_error() {
    let app = test_app();
    let (status, body) = send_json(app, "POST", "/api/settings/embedding/test", json!({})).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "测试结果上报不应变成 HTTP 错误: {body}"
    );
    assert_eq!(body["ok"], json!(false));
    assert_eq!(body["code"], json!("VALIDATION"));
    // message 保留(用户需要据此修正自己的配置),且不含内部实现细节
    assert!(
        body["message"].as_str().is_some_and(|s| !s.is_empty()),
        "保留可读 message: {body}"
    );
    assert_no_internal_leak(&body);
}
