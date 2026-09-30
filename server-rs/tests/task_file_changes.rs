// 任务文件变更台账集成测试(2026-09-30 批次 4,PRODCAP-4「交付可审计」)。
//
// 覆盖三件事,每件都是「台账少记一行」或「把不可用装成没改动」这类会被误判为正常的形态:
//   1. 清单端点按发生顺序返回,空清单是空数组(前端负责显示「本轮未改动文件」);
//   2. diff 端点的三种基线形态:有基线 / 新建 / 基线不可用 —— **不可用必须说得出来,
//      不能返回空 diff**(空 diff 与「没改」在用户眼里一模一样);
//   3. 回滚端点:恢复到改动前、新建的文件回滚即删除、基线不可用时**不动手**、
//      且回滚自身再记一条 `op=rollback`(不隐身)。
//
// 记账的写入侧(fs_write/fs_edit 落盘前后 capture 基线)是同一 `record` 函数,
// 由 `services::task_change_service` 的落库口径 + 这里的读出侧共同覆盖;
// 端到端「跑一个真任务再看点」需要 mock 连接器下发 fs_* 且绑定工作区,
// 该形态已有 `tests/task_events.rs::workspace_less_task_binds_scratch_and_offers_fs_tools`
// 覆盖到「工具确实下发」这一层,本文件不重复起任务。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::api::app_state::AppState;
use kedai_server::services::task_change_service as tcs;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn test_state() -> (&'static Arc<AppState>, &'static axum::Router) {
    static STATE: OnceLock<Arc<AppState>> = OnceLock::new();
    static ROUTER: OnceLock<axum::Router> = OnceLock::new();
    let state = STATE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("kedai-chg-test-{}", std::process::id()));
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

async fn request(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    let req = match body {
        Some(b) => builder.body(Body::from(b.to_string())).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// 建一个绑定**独立**工作区目录的任务,返回 (task_id, 工作区路径)。
///
/// 每例一个目录 + 单调计数后缀:本文件的用例会真跑 bash 并对工作区做**全树扫描**,
/// 共用目录会让并发用例互相看见/删掉对方的文件(2026-09-30 批次 4b 用例实测踩到过:
/// 旧版 `remove_dir_all` + 共用路径一度让另一个用例的扫描产出假删除行)。
async fn task_with_workspace(app: &axum::Router) -> (String, std::path::PathBuf) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let ws = std::env::temp_dir().join(format!(
        "kedai-chg-ws-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&ws).expect("建工作区失败");
    let (status, json) = request(
        app,
        "POST",
        "/api/tasks",
        Some(json!({
            "title": "变更台账用例",
            "task_mode": "solo",
            "workspace": ws.to_string_lossy(),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "建任务应成功: {json}");
    let id = json["task"]["id"].as_str().unwrap_or_default().to_string();
    assert!(!id.is_empty(), "响应里没有任务 id: {json}");
    (id, ws)
}

/// 清单:两条(新建 + 修改)按顺序返回;空清单是空数组。
#[tokio::test]
async fn changes_list_returns_rows_in_order_and_empty_is_empty_array() {
    let (state, app) = test_state();
    let (id, ws) = task_with_workspace(app).await;
    let db = state.db.clone();

    // 先测空清单(还没记过账)
    let (status, json) = request(app, "GET", &format!("/api/tasks/{id}/changes"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["changes"].as_array().map(|a| a.len()),
        Some(0),
        "空清单必须是空数组而非 null:{json}"
    );

    // 新建:基线为空(文件不存在)
    let rel = "src/new.rs";
    std::fs::create_dir_all(ws.join("src")).unwrap();
    let abs = ws.join("src").join("new.rs");
    let before_create = tcs::Baseline::capture(&abs);
    std::fs::write(&abs, "fn a() {}\n").unwrap();
    tcs::record(&db, &id, rel, "create", "tool", &before_create, Some(&abs));

    // 修改:基线是上一版正文
    let before_edit = tcs::Baseline::capture(&abs);
    std::fs::write(&abs, "fn a() {}\nfn b() {}\n").unwrap();
    tcs::record(&db, &id, rel, "modify", "tool", &before_edit, Some(&abs));

    let (status, json) = request(app, "GET", &format!("/api/tasks/{id}/changes"), None).await;
    assert_eq!(status, StatusCode::OK);
    let rows = json["changes"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{json}");
    assert_eq!(rows[0]["op"], "create");
    assert_eq!(rows[1]["op"], "modify");
    assert_eq!(rows[0]["path"], rel, "清单只给相对路径,不外泄本机绝对路径");
    assert!(
        rows[0]["before_hash"].is_null(),
        "新建不该有 before_hash:{}",
        rows[0]
    );
    assert!(
        rows[1]["has_baseline"].as_bool() == Some(true),
        "修改应有可回滚基线:{}",
        rows[1]
    );
    assert!(
        serde_json::to_string(&rows[0])
            .unwrap()
            .find(&ws.to_string_lossy().to_string())
            .is_none(),
        "响应里不得出现工作区绝对路径"
    );
}

/// diff:有基线给逐行 diff;新建给「全量新增 + note」;没记过账的说得出来(不是空 diff)。
#[tokio::test]
async fn diff_endpoint_distinguishes_baseline_new_and_missing() {
    let (state, app) = test_state();
    let (id, ws) = task_with_workspace(app).await;
    let db = state.db.clone();
    let rel = "a.rs";
    let abs = ws.join("a.rs");

    // ① 没记过账 → available:false + 原因(空 diff 会被读成「没改动」,这是本用例要防的)
    let (status, json) = request(
        app,
        "GET",
        &format!("/api/tasks/{id}/changes/diff?path={rel}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["available"], json!(false), "{json}");
    assert!(
        json["reason"].as_str().unwrap_or("").contains("台账"),
        "要说清为什么没有 diff:{json}"
    );

    // ② 新建 → available:true + note,diff 全是新增行
    std::fs::write(&abs, "line1\nline2\n").unwrap();
    let before = tcs::Baseline::capture(&std::path::Path::new(&ws).join("nope.rs"));
    tcs::record(&db, &id, rel, "create", "tool", &before, Some(&abs));
    let (_, json) = request(
        app,
        "GET",
        &format!("/api/tasks/{id}/changes/diff?path={rel}"),
        None,
    )
    .await;
    assert_eq!(json["available"], json!(true), "{json}");
    let d = json["diff"].as_str().unwrap_or_default();
    assert!(
        d.contains("+line1") && d.contains("+line2"),
        "新建应全按新增行:{d}"
    );
    assert!(
        json["note"].as_str().unwrap_or("").contains("新建"),
        "{json}"
    );

    // ③ 有基线 → 逐行 - / +
    let before_edit = tcs::Baseline::capture(&abs);
    std::fs::write(&abs, "line1\ntouched\n").unwrap();
    tcs::record(&db, &id, rel, "modify", "tool", &before_edit, Some(&abs));
    let (_, json) = request(
        app,
        "GET",
        &format!("/api/tasks/{id}/changes/diff?path={rel}"),
        None,
    )
    .await;
    let d = json["diff"].as_str().unwrap_or_default();
    assert!(
        d.contains("-line2") && d.contains("+touched"),
        "应有逐行增删:{d}"
    );
    assert!(d.contains("--- a/a.rs"), "unified 头:{d}");
}

/// 基线不可用(超出 256KB 留存上限):diff 说「基线不可用」,回滚**不动手**。
#[tokio::test]
async fn oversized_baseline_is_reported_unavailable_and_blocks_rollback() {
    let (state, app) = test_state();
    let (id, ws) = task_with_workspace(app).await;
    let db = state.db.clone();
    let rel = "big.bin.txt";
    let abs = ws.join(rel);
    // 先写一份超限的「改动前」正文,capture 后应当 truncated 且不留 blob
    let big = "x".repeat((tcs::MAX_BASELINE_BYTES as usize) + 16);
    std::fs::write(&abs, &big).unwrap();
    let before = tcs::Baseline::capture(&abs);
    assert!(before.truncated, "超上限必须标 truncated");
    std::fs::write(&abs, "shrunk\n").unwrap();
    tcs::record(&db, &id, rel, "modify", "tool", &before, Some(&abs));

    let (_, json) = request(
        app,
        "GET",
        &format!("/api/tasks/{id}/changes/diff?path={rel}"),
        None,
    )
    .await;
    assert_eq!(json["available"], json!(false), "{json}");
    assert!(
        json["reason"].as_str().unwrap_or("").contains("基线不可用"),
        "{json}"
    );

    let (status, json) = request(
        app,
        "POST",
        &format!("/api/tasks/{id}/changes/rollback?path={rel}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "不可用不是服务端错误:{json}");
    assert_eq!(json["ok"], json!(false), "{json}");
    // 关键:正文**没被动过**(不能「恢复成空文件」装作案发现场)
    assert_eq!(std::fs::read_to_string(&abs).unwrap(), "shrunk\n");
    let (_, list) = request(app, "GET", &format!("/api/tasks/{id}/changes"), None).await;
    assert_eq!(
        list["changes"].as_array().map(|a| a.len()),
        Some(1),
        "回滚未执行,不该多记一条:{}",
        list["changes"]
    );
}

/// 回滚:改过的文件恢复原正文,并**再记一条 `op=rollback`**;
/// 新建的文件回滚 = 删除。
#[tokio::test]
async fn rollback_restores_content_and_records_itself() {
    let (state, app) = test_state();
    let (id, ws) = task_with_workspace(app).await;
    let db = state.db.clone();
    let abs = ws.join("keep.rs");
    std::fs::write(&abs, "original\n").unwrap();
    let before = tcs::Baseline::capture(&abs);
    std::fs::write(&abs, "mangled\n").unwrap();
    tcs::record(&db, &id, "keep.rs", "modify", "tool", &before, Some(&abs));

    let (status, json) = request(
        app,
        "POST",
        &format!("/api/tasks/{id}/changes/rollback?path=keep.rs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["ok"], json!(true), "{json}");
    assert_eq!(std::fs::read_to_string(&abs).unwrap(), "original\n");

    let (_, list) = request(app, "GET", &format!("/api/tasks/{id}/changes"), None).await;
    let rows = list["changes"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "回滚须再记一条,不隐身:{rows:?}");
    assert_eq!(rows[1]["op"], "rollback");
    assert_eq!(rows[1]["source"], "rollback");

    // 新建的文件:回滚 = 删除
    let created = ws.join("was-new.rs");
    let before_new = tcs::Baseline::capture(&created); // 不存在 → exists=false
    std::fs::write(&created, "brand new\n").unwrap();
    tcs::record(
        &db,
        &id,
        "was-new.rs",
        "create",
        "tool",
        &before_new,
        Some(&created),
    );
    let (_, json) = request(
        app,
        "POST",
        &format!("/api/tasks/{id}/changes/rollback?path=was-new.rs"),
        None,
    )
    .await;
    assert_eq!(json["ok"], json!(true), "{json}");
    assert_eq!(json["removed"], json!(true), "新建回滚应删除文件:{json}");
    assert!(!created.exists(), "文件应已删除");
}

/// 越界路径被工作区闸门拒绝(400),而且不回滚任何东西。
#[tokio::test]
async fn traversal_path_is_rejected() {
    let (_state, app) = test_state();
    let (id, _ws) = task_with_workspace(app).await;
    let (status, json) = request(
        app,
        "GET",
        &format!("/api/tasks/{id}/changes/diff?path=../../etc/passwd"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert!(
        json["error"].as_str().unwrap_or("").contains("越界"),
        "{json}"
    );
}

/// 单测侧的 diff 预算与脚本正确性(服务内部函数,不起 HTTP)。
#[test]
fn service_diff_helpers_behave() {
    // 任务 id 解析:子 agent 层叠会话必须收敛到根 id(否则外键不匹配、记账静默丢失)
    assert_eq!(
        tcs::task_id_of("task:abc:main:2:sub:def").as_deref(),
        Some("abc")
    );
    assert_eq!(tcs::task_id_of("session-1"), None);
    // 大文件小改动:只吐变化处,不吐全文
    let before: String = (0..1_200).map(|i| format!("l{i}\n")).collect();
    let after: String = (0..1_200)
        .map(|i| {
            if i == 900 {
                "CHANGED\n".to_string()
            } else {
                format!("l{i}\n")
            }
        })
        .collect();
    let d = tcs::unified_diff(&before, &after, "x.txt");
    assert!(d.contains("-l900") && d.contains("+CHANGED"), "{d}");
    assert!(
        d.lines().count() < 40,
        "小改动不该输出全文:{}",
        d.lines().count()
    );
    // 超限:明说不生成
    let big_a: String = (0..3_000).map(|i| format!("a{i}\n")).collect();
    let big_b: String = (0..3_000).map(|i| format!("b{i}\n")).collect();
    assert!(tcs::unified_diff(&big_a, &big_b, "big.txt").contains("改动过大"));
}

// ==================== 批次 4b:bash 侧启发式检出(2026-09-30)====================
//
// 这一组用**真实工具调用**驱动(不走 HTTP 建任务时的 mock 连接器):
// `state.tool_registry.execute_with_decision` 与引擎的「已裁决放行」路径同形,
// bash 的 command 交给平台 shell 真跑。判据都落在「清单里有没有那一行」上——
// 扫描是旁路观测,任何一条断言失败都意味着用户看到的「改了什么」是错账。

use kedai_server::models::types::ToolContext;
use kedai_server::tools::permissions::{PermissionDecision, ToolRisk};

/// 任务上下文(与任务引擎同形:虚拟 session + 工作区作用域)
fn task_ctx(task_id: &str, ws: &std::path::Path) -> ToolContext {
    let scope = kedai_server::tools::workspace_guard::scope_for_task(Some(&ws.to_string_lossy()))
        .expect("作用域构造不应失败")
        .expect("绑定工作区必有作用域");
    ToolContext {
        session_id: format!("task:{task_id}"),
        character_id: String::new(),
        agent_depth: 0,
        scope: Some(scope),
    }
}

/// 「已裁决放行」的许可(授权路径本身由 permissions 的既有用例覆盖)
fn allow() -> PermissionDecision {
    PermissionDecision {
        allowed: true,
        risk: ToolRisk::Dangerous,
        reason: "测试放行".into(),
    }
}

async fn call_tool(
    state: &Arc<AppState>,
    name: &str,
    args: Value,
    ctx: ToolContext,
) -> Result<String, String> {
    state
        .tool_registry
        .execute_with_decision(name, &args.to_string(), ctx, &allow())
        .await
}

/// 跑一条真命令(顺带打开命令执行总开关——它默认关闭,与生产一致)
async fn run_bash(
    state: &Arc<AppState>,
    task_id: &str,
    ws: &std::path::Path,
    command: &str,
) -> Result<String, String> {
    state.settings.lock().unwrap().exec_enabled = true;
    call_tool(
        state,
        "bash",
        json!({ "command": command }),
        task_ctx(task_id, ws),
    )
    .await
}

async fn fs_write(
    state: &Arc<AppState>,
    task_id: &str,
    ws: &std::path::Path,
    rel: &str,
    content: &str,
) -> Result<String, String> {
    call_tool(
        state,
        "fs_write",
        json!({ "path": rel, "content": content }),
        task_ctx(task_id, ws),
    )
    .await
}

async fn changes_of(app: &axum::Router, id: &str) -> Value {
    let (status, json) = request(app, "GET", &format!("/api/tasks/{id}/changes"), None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    json
}

/// ① bash 改文件 → 清单出现 `source='bash'` 的行(create 与 modify 两条)
#[tokio::test]
async fn bash_change_is_recorded_with_source_bash() {
    let (state, app) = test_state();
    let (id, ws) = task_with_workspace(app).await;

    run_bash(state, &id, &ws, "echo hello > f.txt")
        .await
        .expect("命令应执行成功");
    let list = changes_of(app, &id).await;
    let rows = list["changes"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{list}");
    assert_eq!(rows[0]["source"], "bash");
    assert_eq!(rows[0]["op"], "create");
    assert_eq!(rows[0]["path"], "f.txt");
    assert!(
        rows[0]["after_bytes"].as_u64().unwrap_or(0) >= 5,
        "应记下改动后大小:{list}"
    );
    assert_eq!(list["undected"], json!(false), "完整扫描不该挂横幅:{list}");

    run_bash(state, &id, &ws, "echo world >> f.txt")
        .await
        .expect("命令应执行成功");
    let list = changes_of(app, &id).await;
    let rows = list["changes"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{list}");
    assert_eq!(rows[1]["op"], "modify");
    assert_eq!(rows[1]["source"], "bash");
    assert_eq!(
        rows[1]["has_baseline"],
        json!(true),
        "D6=(a):bash 行也要有可回滚基线:{list}"
    );
}

/// ② 扫描缺项是**顶层字段**而不是清单里的假行:清单长度语义不被污染
#[tokio::test]
async fn scan_mark_is_surfaced_without_polluting_changes() {
    let (state, app) = test_state();
    let (id, _ws) = task_with_workspace(app).await;
    let db = state.db.clone();

    let list = changes_of(app, &id).await;
    assert_eq!(list["undected"], json!(false), "没有标记就是 false:{list}");
    assert!(list["undected_reason"].is_null(), "{list}");

    assert!(tcs::record_scan_mark(
        &db,
        &id,
        "后扫描未完成(扫到 20000 项即撞预算),已省略删除类改动"
    ));
    let list = changes_of(app, &id).await;
    assert_eq!(list["undected"], json!(true), "{list}");
    assert!(
        list["undected_reason"]
            .as_str()
            .unwrap_or("")
            .contains("后扫描"),
        "原因要原样下发(前端按它分档):{list}"
    );
    assert_eq!(
        list["changes"].as_array().map(|a| a.len()),
        Some(0),
        "标记不得混进改动清单:{list}"
    );
}

/// ③ 同一路径双来源:fs_write 之后 bash 再改 → bash 只记自己那一次(不重报 tool 的改动)
#[tokio::test]
async fn fs_write_then_bash_same_file_records_one_bash_row() {
    let (state, app) = test_state();
    let (id, ws) = task_with_workspace(app).await;

    fs_write(state, &id, &ws, "dual.txt", "a\n")
        .await
        .expect("fs_write 应成功");
    run_bash(state, &id, &ws, "echo b >> dual.txt")
        .await
        .expect("命令应执行成功");

    let list = changes_of(app, &id).await;
    let rows = list["changes"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "两条独立改动,各记一行:{list}");
    assert_eq!(rows[0]["source"], "tool");
    assert_eq!(rows[0]["op"], "create");
    assert_eq!(rows[1]["source"], "bash");
    assert_eq!(
        rows[1]["op"], "modify",
        "bash 侧不得把 fs_write 建的文件误记成 create:{list}"
    );
}

/// ④ bash 删文件 → op=delete 且 after_bytes=0
#[tokio::test]
async fn bash_delete_records_delete_row_with_zero_bytes() {
    let (state, app) = test_state();
    let (id, ws) = task_with_workspace(app).await;

    fs_write(state, &id, &ws, "gone.txt", "bye\n")
        .await
        .expect("fs_write 应成功");
    run_bash(state, &id, &ws, "del /q gone.txt")
        .await
        .expect("命令应执行成功");

    let list = changes_of(app, &id).await;
    let rows = list["changes"].as_array().unwrap();
    let last = rows.last().unwrap();
    assert_eq!(last["op"], "delete", "{list}");
    assert_eq!(last["source"], "bash");
    assert_eq!(last["after_bytes"], json!(0));
}

/// ⑤ 聊天路径(无工作区作用域)跑 bash → 零记账、零标记
#[tokio::test]
async fn chat_path_bash_records_nothing() {
    let (state, app) = test_state();
    let (id, _ws) = task_with_workspace(app).await;

    state.settings.lock().unwrap().exec_enabled = true;
    let ctx = ToolContext {
        session_id: "chat-session-1".into(),
        character_id: String::new(),
        agent_depth: 0,
        scope: None,
    };
    call_tool(state, "bash", json!({ "command": "echo chat" }), ctx)
        .await
        .expect("聊天路径命令应能执行(缺省 cwd = 数据目录)");

    let list = changes_of(app, &id).await;
    assert_eq!(list["changes"].as_array().map(|a| a.len()), Some(0));
    assert_eq!(list["undected"], json!(false));
}

/// ⑥ D6=(a) 的验收主体:bash 检出的改动点开 diff **真给逐行内容**
#[tokio::test]
async fn bash_detected_change_yields_real_diff() {
    let (state, app) = test_state();
    let (id, ws) = task_with_workspace(app).await;

    run_bash(state, &id, &ws, "echo one > g.txt")
        .await
        .expect("命令应执行成功");
    run_bash(state, &id, &ws, "echo two >> g.txt")
        .await
        .expect("命令应执行成功");

    let (status, json) = request(
        app,
        "GET",
        &format!("/api/tasks/{id}/changes/diff?path=g.txt"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["available"], json!(true), "有基线就该给 diff:{json}");
    let d = json["diff"].as_str().unwrap_or_default();
    assert!(d.contains("+two"), "应看到追加行:{d}");
    assert!(d.contains("--- a/g.txt"), "unified 头:{d}");
}

/// ⑦ 双来源同一路径的回滚取**最新一行**:bash 行的基线 = 命令执行前(= fs_write 的产物)
#[tokio::test]
async fn rollback_after_dual_source_returns_to_pre_command_state() {
    let (state, app) = test_state();
    let (id, ws) = task_with_workspace(app).await;

    fs_write(state, &id, &ws, "dual2.txt", "A\n")
        .await
        .expect("fs_write 应成功");
    run_bash(state, &id, &ws, "echo B >> dual2.txt")
        .await
        .expect("命令应执行成功");

    let (status, json) = request(
        app,
        "POST",
        &format!("/api/tasks/{id}/changes/rollback?path=dual2.txt"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["ok"], json!(true), "{json}");
    let content = std::fs::read_to_string(ws.join("dual2.txt")).unwrap();
    assert_eq!(
        content, "A\n",
        "必须回到「命令执行前」而不是「fs_write 之前」(那是错误的时间点)"
    );
    let list = changes_of(app, &id).await;
    let last = list["changes"].as_array().unwrap().last().cloned().unwrap();
    assert_eq!(last["op"], "rollback", "回滚自身要留痕:{list}");
}

/// ⑧ 改动后超 256KB → 不逐行记,但扫描级标记必须说出来(不是「没有变更」)
#[tokio::test]
async fn oversized_change_is_marked_instead_of_rowed() {
    let (state, app) = test_state();
    let (id, ws) = task_with_workspace(app).await;

    let big = "x".repeat((tcs::MAX_BASELINE_BYTES as usize) + 64);
    fs_write(state, &id, &ws, "huge.bin", &big)
        .await
        .expect("fs_write 应成功");
    run_bash(state, &id, &ws, "echo tail >> huge.bin")
        .await
        .expect("命令应执行成功");

    let list = changes_of(app, &id).await;
    let rows = list["changes"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "只该有 fs_write 那一行:{list}");
    assert_eq!(rows[0]["source"], "tool");
    assert_eq!(list["undected"], json!(true), "必须明说没记全:{list}");
    assert!(
        list["undected_reason"]
            .as_str()
            .unwrap_or("")
            .contains("超过留存上限"),
        "原因要说清是超上限而非预算打满:{list}"
    );
}
