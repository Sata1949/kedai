// 任务执行轨迹读取端点集成测试(TMSHOW-4,2026-11-04)。
//
// 为什么单独一个文件:`GET /api/tasks/{id}/trace` 自 TMSHOW-1 落地以来**零端点级测试**——
// 服务层的 `scan_trace` 有 7 条单测,但「路由是否注册、404 语义、limit 夹取、响应信封形状」
// 这几层完全没有机检。同形的 `GET /api/tasks/{id}/events` 有完整端点测试,
// 本文件照它的套路补齐。
//
// 播种方式与 `agent_trace.rs` 一致:先用真实 HTTP 建任务(让任务进内存缓存 + DB),
// 再直连测试库写 `task_trace` 行(写路径已由 `sink.rs` 与 `trace.rs` 单测覆盖,
// 这里只验证读取契约)。
use axum::http::StatusCode;
use serde_json::{json, Value};
mod common;
use common::{send_json, test_app, test_lock};

/// 测试库路径:与 `build_test_app` 同款(%TEMP%/kedai-test-{pid}/kedai.db)。
/// 该数据目录是**进程级共享**单例,生命周期等于测试进程。
fn test_db_path() -> std::path::PathBuf {
    let mut data_dir = std::env::temp_dir();
    data_dir.push(format!("kedai-test-{}", std::process::id()));
    data_dir.push("kedai.db");
    data_dir
}

/// 建任务并返回 id(该任务同时进内存缓存与 DB,轨迹外键才成立)。
async fn create_task(app: &axum::Router, title: &str) -> String {
    let (status, json) = send_json(app, "POST", "/api/tasks", json!({ "title": title })).await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 直接往 `task_trace` 播种若干行,返回播种条数。seq 从 1 递增。
fn seed_trace(task_id: &str, rows: &[(&str, Option<&str>, &str)]) {
    let conn = rusqlite::Connection::open(test_db_path()).expect("打开测试库失败");
    for (i, (kind, name, content)) in rows.iter().enumerate() {
        conn.execute(
            "INSERT INTO task_trace \
               (task_id, seq, phase, step_index, kind, name, content, truncated, created_at) \
             VALUES (?1, ?2, 'agent', 0, ?3, ?4, ?5, 0, '2026-11-04T00:00:00.000Z')",
            rusqlite::params![task_id, (i as i64) + 1, kind, name, content],
        )
        .expect("播种 task_trace 失败");
    }
}

/// 读轨迹端点,返回 (状态码, JSON)。
async fn get_trace(app: &axum::Router, task_id: &str, query: &str) -> (StatusCode, Value) {
    let path = if query.is_empty() {
        format!("/api/tasks/{task_id}/trace")
    } else {
        format!("/api/tasks/{task_id}/trace?{query}")
    };
    send_json(app, "GET", &path, json!({})).await
}

/// 任务不存在 → 404(不得回 200 空数组:前端会把「缺失」当「没有过程」)。
#[tokio::test]
async fn trace_404_for_unknown_task() {
    let _guard = test_lock().await;
    let app = test_app();
    let (status, json) = get_trace(app, "no-such-task", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "未知任务应 404: {json}");
}

/// 空轨迹 → `{ trace: [], truncated: false }`,**不是 404**(与 `/events` 同口径)。
#[tokio::test]
async fn trace_empty_list_for_task_without_trace() {
    let _guard = test_lock().await;
    let app = test_app();
    let id = create_task(app, "空轨迹任务").await;
    let (status, json) = get_trace(app, &id, "").await;
    assert_eq!(status, StatusCode::OK, "有任务无轨迹应 200: {json}");
    assert_eq!(json["trace"].as_array().map(|a| a.len()), Some(0));
    assert_eq!(json["truncated"], json!(false));
}

/// 按 seq 升序返回,且字段逐个对齐后端 `TaskTraceRecord` 线格式。
#[tokio::test]
async fn trace_returns_rows_in_seq_order_with_wire_fields() {
    let _guard = test_lock().await;
    let app = test_app();
    let id = create_task(app, "轨迹顺序任务").await;
    seed_trace(
        &id,
        &[
            ("reasoning", None, "先想一下"),
            ("tool_call", Some("bash"), r#"{"cmd":"ls"}"#),
            ("tool_result", Some("bash"), "a.txt"),
        ],
    );

    let (status, json) = get_trace(app, &id, "").await;
    assert_eq!(status, StatusCode::OK, "读轨迹应 200: {json}");
    let rows = json["trace"].as_array().expect("trace 应为数组");
    assert_eq!(rows.len(), 3, "三条轨迹应全部返回");
    let seqs: Vec<i64> = rows.iter().filter_map(|r| r["seq"].as_i64()).collect();
    assert_eq!(seqs, vec![1, 2, 3], "必须按 seq 升序");
    assert_eq!(rows[0]["kind"], json!("reasoning"));
    assert_eq!(rows[1]["kind"], json!("tool_call"));
    assert_eq!(rows[1]["name"], json!("bash"), "工具名应原样带出");
    assert_eq!(rows[2]["content"], json!("a.txt"));
    assert_eq!(rows[0]["truncated"], json!(false));
    assert_eq!(
        json["truncated"],
        json!(false),
        "起点落在窗口内不应标记截断"
    );
    // 十个字段一个不少:前端档 2 / 档 3 直接消费整行,漏字段会静默丢一类条目
    for field in [
        "id",
        "task_id",
        "seq",
        "phase",
        "step_index",
        "kind",
        "name",
        "content",
        "truncated",
        "created_at",
    ] {
        assert!(
            rows[0].get(field).is_some(),
            "线格式缺字段 {field}: {rows:?}"
        );
    }
}

/// `after` 增量过滤:只返回 `seq > after` 的行。
#[tokio::test]
async fn trace_after_filters_by_seq() {
    let _guard = test_lock().await;
    let app = test_app();
    let id = create_task(app, "轨迹增量任务").await;
    seed_trace(
        &id,
        &[
            ("reasoning", None, "一"),
            ("reasoning", None, "二"),
            ("reasoning", None, "三"),
        ],
    );

    let (status, json) = get_trace(app, &id, "after=1").await;
    assert_eq!(status, StatusCode::OK);
    let rows = json["trace"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "after=1 应只返回后两条");
    assert_eq!(rows[0]["seq"], json!(2));
}

/// `after` 越过最新 → 空数组 + `truncated=false`,**不是 404**。
#[tokio::test]
async fn trace_after_beyond_latest_returns_empty_array() {
    let _guard = test_lock().await;
    let app = test_app();
    let id = create_task(app, "轨迹越界任务").await;
    seed_trace(&id, &[("reasoning", None, "只有一条")]);

    let (status, json) = get_trace(app, &id, "after=99").await;
    assert_eq!(status, StatusCode::OK, "越界 after 应 200 而非 404: {json}");
    assert_eq!(json["trace"].as_array().map(|a| a.len()), Some(0));
    assert_eq!(json["truncated"], json!(false), "窗口内越界不算截断");
}

/// `limit` 夹取到 `1..=1000`:0 被抬到 1(而不是「不限」,更不是空)。
#[tokio::test]
async fn trace_limit_is_clamped_to_at_least_one() {
    let _guard = test_lock().await;
    let app = test_app();
    let id = create_task(app, "轨迹 limit 任务").await;
    seed_trace(
        &id,
        &[
            ("reasoning", None, "一"),
            ("reasoning", None, "二"),
            ("reasoning", None, "三"),
        ],
    );

    let (status, json) = get_trace(app, &id, "limit=0").await;
    assert_eq!(status, StatusCode::OK);
    let rows = json["trace"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "limit=0 应夹取到 1,不是 0 也不是不限: {rows:?}"
    );
    assert_eq!(rows[0]["seq"], json!(1), "夹取后仍从窗口最早一条起");
}

/// 轨迹只属于自己:同一个库里的另一个任务读不到别人的行。
#[tokio::test]
async fn trace_is_scoped_to_own_task() {
    let _guard = test_lock().await;
    let app = test_app();
    let a = create_task(app, "任务甲").await;
    let b = create_task(app, "任务乙").await;
    seed_trace(&a, &[("reasoning", None, "甲的思考")]);

    let (status, json) = get_trace(app, &b, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["trace"].as_array().map(|r| r.len()),
        Some(0),
        "乙不应读到甲的行: {json}"
    );
}
