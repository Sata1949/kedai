// 任务级空闲看守的集成用例(提交 3 · D7):客户端放弃 ≠ 任务永生。
//
// 为什么自建 AppState 而不是 `build_test_app()`:看守的测试缝(`scan_idle_tasks`)是
// `TaskService` 上的 pub 方法,而 `build_test_app()` 只返回 Router,拿不到 Arc<AppState>;
// 本文件因此自己设 DATA_DIR/scratch 并构造(cf. lib.rs 的注释:数据目录必须**不用**
// TempDataDir——它会在 drop 时删库,而本进程多个用例共享同一目录)。
//
// 判据两条路径都要覆盖,这是本文件的重点:
//   * **库回退**:直接插入一行 running 任务(没有心跳,模拟重启前遗留行)→ 必须被收尾;
//   * **心跳优先**:API 建的任务(创建即有心跳)但库里时间戳很旧 → **不得**被收尾——
//     这正是「长工具循环期间 task_llm_calls 无新行」的真实形态,只看库会误杀活跃任务。
use axum::http::StatusCode;
use kedai_server::api::app_state::AppState;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast::error::TryRecvError;
use tokio::sync::broadcast::Receiver;
use tokio::sync::{Mutex, MutexGuard};

/// 串行化:看守扫的是**全库** running/planning 任务,用例各自插入的行会互相进入
/// 对方的候选集(并行下「只收尾我这一行」的断言必然打架),故全文件排队。
async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

/// 本进程隔离数据目录 + 应用状态(env 只在首次构造前设一次)
fn test_state() -> (&'static Arc<AppState>, &'static axum::Router) {
    static STATE: OnceLock<Arc<AppState>> = OnceLock::new();
    static ROUTER: OnceLock<axum::Router> = OnceLock::new();
    let state = STATE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("kedai-idle-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建数据目录失败");
        let scratch =
            std::env::temp_dir().join(format!("kedai-idle-scratch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::env::set_var("CONNECTOR", "mock");
        std::env::set_var("DATA_DIR", &dir);
        std::env::set_var("KEDAI_TASK_SCRATCH_DIR", &scratch);
        std::env::set_var("LOG_LEVEL", "error");
        let mut config = kedai_server::config::AppConfig::from_env();
        config.auth_required = false;
        AppState::new(config).expect("构造 AppState 失败")
    });
    let router = ROUTER.get_or_init(|| kedai_server::api::build_router(state.clone()));
    (state, router)
}

fn db_path() -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("kedai-idle-test-{}", std::process::id()))
        .join("kedai.db")
}

/// 直插一行任务(绕过 API:**不产生心跳**,模拟「库里躺着但本进程没有执行」的形态)。
/// 只写 NOT NULL 列 + 状态;`plan` 留空数组 → 部分成果兜底无内容可拼(不得伪造)。
fn insert_task(id: &str, status: &str, updated_at: &str) {
    let conn = rusqlite::Connection::open(db_path()).expect("打开测试库失败");
    conn.execute(
        "INSERT INTO tasks (id, title, status, plan, result, error, created_at, updated_at, task_mode) \
         VALUES (?1, '空闲看守用例', ?2, '[]', '', '', ?3, ?3, 'solo')",
        rusqlite::params![id, status, updated_at],
    )
    .expect("插入任务行失败");
}

fn set_status_updated(id: &str, status: &str, updated_at: &str) {
    let conn = rusqlite::Connection::open(db_path()).expect("打开测试库失败");
    conn.execute(
        "UPDATE tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![status, updated_at, id],
    )
    .expect("更新任务行失败");
}

fn row_of(id: &str) -> (String, String, String) {
    let conn = rusqlite::Connection::open(db_path()).expect("打开测试库失败");
    conn.query_row(
        "SELECT status, error, updated_at FROM tasks WHERE id = ?1",
        rusqlite::params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .expect("读任务行失败")
}

/// 收集广播里已到达的事件(非阻塞:拿到空即停——看守的写入是同步完成的,
/// 事件在 `scan_idle_tasks` 返回前就已发出)
fn drain(rx: &mut Receiver<kedai_server::models::types::SseEvent>) -> Vec<Value> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(ev) => out.push(serde_json::to_value(&ev).unwrap_or(Value::Null)),
            Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => break,
            Err(TryRecvError::Lagged(_)) => continue, // 允许丢帧:断言只看「有/无」
        }
    }
    out
}

/// 本任务事件里 detail 含给定子串的条数(终态 status 事件与原因事件都在其中)
fn detail_hits(events: &[Value], task_id: &str, needle: &str) -> usize {
    events
        .iter()
        .filter(|e| e["type"] == "task" && e["task_id"].as_str() == Some(task_id))
        .filter(|e| e["detail"].as_str().is_some_and(|d| d.contains(needle)))
        .count()
}

/// 库回退路径:一行 running + 旧 updated_at(无心跳)→ 收尾 ended、原因可见、事件唯一。
#[tokio::test]
async fn idle_task_without_heartbeat_is_reaped_once() {
    let _guard = test_lock().await;
    let (state, _router) = test_state();
    let mut rx = state.tasks.subscribe();
    let id = "idle-direct-row";
    insert_task(id, "running", "2020-01-01T00:00:00.000Z");

    let reaped = state.tasks.scan_idle_tasks(60);
    assert_eq!(reaped, vec![id.to_string()], "库回退判据应收尾该任务");

    let (status, error, _) = row_of(id);
    assert_eq!(status, "ended", "空闲收尾的终态是 ended(与 stop 同源)");
    assert!(
        error.contains("空闲超时自动收尾"),
        "原因必须可见(用户得知道凭什么被停): {error}"
    );

    let events = drain(&mut rx);
    assert_eq!(
        detail_hits(&events, id, "空闲超时自动收尾"),
        1,
        "原因事件只发一次: {events:?}"
    );
    assert_eq!(
        detail_hits(&events, id, "任务状态更新为"),
        1,
        "终态写入也只发一次: {events:?}"
    );

    // 幂等:再扫一轮不得重复收尾,也不得再发事件
    let again = state.tasks.scan_idle_tasks(60);
    assert!(again.is_empty(), "已收尾的任务不再是候选: {again:?}");
    assert!(
        drain(&mut rx).is_empty(),
        "重复扫描不得产生任何新事件(否则前端会看到重复的收尾通知)"
    );
}

/// 心跳优先(核心防误杀):API 建的任务有新鲜心跳,但库里时间戳很旧
/// → **不得**收尾。这正是「长工具循环期间没有新 task_llm_calls 行」的真实形态:
/// 只看库里那两处口径会把正在干活的任务判成空闲(默认 900s 下十分钟的循环必中招)。
#[tokio::test]
async fn fresh_heartbeat_prevents_reaping_stale_db_row() {
    let _guard = test_lock().await;
    let (state, router) = test_state();
    let id = create_task_via_api(router).await;
    // 库里时间戳退到 2020(**不走 API**:直改库不刷新心跳)
    set_status_updated(&id, "running", "2020-01-01T00:00:00.000Z");
    let (st, _, updated) = row_of(&id);
    assert_eq!(
        (st.as_str(), updated.as_str()),
        ("running", "2020-01-01T00:00:00.000Z"),
        "前置:库里看起来「躺了很久」"
    );

    let reaped = state.tasks.scan_idle_tasks(60);
    // 只断言「本任务不在其中」而不要求整体为空:同文件的其它用例可能留下自己的
    // stale running 行(共享库),断言空集会让用例之间产生顺序依赖
    assert!(
        !reaped.contains(&id),
        "有心跳(创建事件刚刷新过)的任务不得被判空闲: {reaped:?}"
    );
    let (st, _, _) = row_of(&id);
    assert_eq!(st, "running", "不得被误杀");
}

/// 库时间戳很新(刚落过行)→ 不收尾;终态任务 → 不在候选集。
#[tokio::test]
async fn recent_activity_and_terminal_tasks_are_not_reaped() {
    let _guard = test_lock().await;
    let (state, _router) = test_state();
    let now = kedai_server::models::db::now_iso();
    let fresh = "idle-fresh-row";
    insert_task(fresh, "running", &now);
    let done = "idle-done-row";
    insert_task(done, "done", "2020-01-01T00:00:00.000Z");

    let reaped = state.tasks.scan_idle_tasks(60);
    assert!(
        !reaped.contains(&fresh.to_string()),
        "最近有活动(库时间戳新)不得收尾: {reaped:?}"
    );
    assert!(
        !reaped.contains(&done.to_string()),
        "终态任务跳过(候选集只看 running/planning): {reaped:?}"
    );
    assert_eq!(row_of(fresh).0, "running");
    assert_eq!(row_of(done).0, "done");
}

/// `timeout_secs = 0` = 关闭本闸门(整轮不扫,一行也不动)
#[tokio::test]
async fn zero_timeout_disables_watchdog() {
    let _guard = test_lock().await;
    let (state, _router) = test_state();
    let id = "idle-zero-row";
    insert_task(id, "running", "2020-01-01T00:00:00.000Z");
    assert!(
        state.tasks.scan_idle_tasks(0).is_empty(),
        "0 = 关:不得收尾任何任务"
    );
    assert_eq!(row_of(id).0, "running", "关闭态一行都不该动");
    // 收尾本用例的现场:这行是「陈旧的 running」,留着会被后续用例的扫描收掉,
    // 让它们看到本用例的 id(断言集合的用例就会莫名其妙失败)——用例之间不得有顺序依赖
    set_status_updated(id, "ended", "2020-01-01T00:00:00.000Z");
}

/// 经 HTTP 建任务(会发 created 事件 → 顺带刷新心跳;这正是「有心跳」的来源)
async fn create_task_via_api(router: &axum::Router) -> String {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let req = Request::builder()
        .method("POST")
        .uri("/api/tasks")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "title": "心跳用例", "task_mode": "solo" }).to_string(),
        ))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = {
        use http_body_util::BodyExt;
        resp.into_body().collect().await.unwrap().to_bytes()
    };
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    v["task"]["id"].as_str().unwrap().to_string()
}
