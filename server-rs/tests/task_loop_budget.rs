// 任务侧工具循环「两道闸」的集成用例(提交 3 · D3):
//   ① 步骤墙钟预算(`task_step_budget_secs`):到点**带着已有产出收尾**——终态是 done
//      而不是 error,事件里说明原因;
//   ② 语义熔断任务侧钳制:用户把聊天侧三值设到 64/12/4(聊天侧合法),任务侧仍按
//      ≤8/≤4/≤2 生效——同工具输出无实质变化时**第 4 次**即熔断(未钳制要 12 次)。
//
// 观测手段(两条都靠 mock 的工具循环钩子,见 src/connectors/mock.rs):
//   * `[[tool_loop_text:sleep|N {json}]]`:前 N 轮「短正文 + ToolCall」,正文让循环被
//     收掉时仍有内容可落库(否则工具轮正文恒空,无法校验「保留产出」);
//   * 每轮注入 `_mock_round` 使参数指纹互不相同 → 只可能被**输出侧**的语义熔断抓住
//     (排除指纹熔断的干扰,这正是判别的关键)。
//
// 独立成文件的原因同其它 task_*.rs:设置是**进程级共享**的,本文件要改设置再复位,
// 单文件即单进程/单数据目录,文件内再统一 test_lock() 串行。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

fn test_app() -> &'static axum::Router {
    static APP: OnceLock<axum::Router> = OnceLock::new();
    APP.get_or_init(|| build_test_app().expect("构建测试应用失败"))
}

/// 串行化:本文件的用例都会写全局设置(进程级共享),必须互相排队
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

/// 创建任务(题面就是 title:任务目标口径,见 tools/bench/task-probe.mjs 文件头)
async fn create_task(app: &axum::Router, title: &str) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": "solo" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 绑定工作区创建任务(TMR-5 只读守卫需要「有写工具」的任务面;无工作区的任务
/// 没有 fs_write,守卫按设计不启用)。
async fn create_task_in_workspace(app: &axum::Router, title: &str, ws: &Path) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": title,
            "task_mode": "solo",
            "workspace": ws.to_string_lossy(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建工作区任务应 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 临时工作区:用进程 id + 纳秒做唯一后缀,测试结束显式清理。
fn temp_workspace(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("kedai-tmr-{tag}-{}-{ns}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

async fn run_task(app: &axum::Router, id: &str) {
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
}

/// 终态追加指令(followup):只接受终态任务,返回 200 后异步续跑。
async fn followup_task(app: &axum::Router, id: &str, content: &str) {
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/followup"),
        json!({ "content": content, "mode": "append" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "followup 应 200: {json}");
}

/// 轮询到终态,返回 (状态, 详情)
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let (_, d) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        let st = d["task"]["status"].as_str().unwrap_or("").to_string();
        if matches!(st.as_str(), "done" | "partial" | "error" | "ended") {
            return (st, d);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "任务未在 60s 内收敛,mock 用例不该这么慢: {d}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 订阅任务事件流,返回后台收集句柄(读到本任务的终态 status 即返回)。
/// 返回的事件按到达顺序,调用方按 task_id 过滤(同进程其它用例的事件不算失败)。
fn spawn_collector(app: &axum::Router) -> tokio::task::JoinHandle<Vec<Value>> {
    let req = Request::builder()
        .method("GET")
        .uri("/api/tasks/events")
        .body(Body::empty())
        .unwrap();
    let app = app.clone();
    tokio::spawn(async move {
        let resp = app.oneshot(req).await.unwrap();
        let mut body = resp.into_body();
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let frame = tokio::time::timeout_at(deadline, body.frame()).await;
            let Ok(Some(Ok(frame))) = frame else {
                break; // 超时/流结束:返回已收集的部分(断言在调用方)
            };
            let Ok(data) = frame.into_data() else {
                continue;
            };
            let text = String::from_utf8_lossy(&data);
            for line in text.lines() {
                let Some(payload) = line.strip_prefix("data:") else {
                    continue;
                };
                if let Ok(v) = serde_json::from_str::<Value>(payload.trim()) {
                    let terminal = v["kind"].as_str() == Some("status")
                        && matches!(
                            v["status"].as_str(),
                            Some("done") | Some("partial") | Some("error") | Some("ended")
                        );
                    out.push(v);
                    if terminal {
                        return out;
                    }
                }
            }
        }
        out
    })
}

/// 本任务的事件里,detail 命中某子串的 agent_status 明细文本(并集,广播 Lagged 允许丢帧)
fn status_details(events: &[Value], task_id: &str) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["type"] == "task" && e["task_id"].as_str() == Some(task_id))
        .filter(|e| e["kind"].as_str() == Some("agent_status"))
        .filter_map(|e| e["detail"].as_str().map(str::to_string))
        .collect()
}

/// 本任务的全部事件 detail(不限 kind):TMR-3 的「文件产物校验未通过」是 Status 事件,
/// 不像熔断事件那样走 agent_status,不能复用 `status_details`。
fn all_task_details(events: &[Value], task_id: &str) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["type"] == "task" && e["task_id"].as_str() == Some(task_id))
        .filter_map(|e| e["detail"].as_str().map(str::to_string))
        .collect()
}

/// 步骤墙钟预算(D3-a):预算 1s + 每轮真睡 1.2s → 第 1 轮末即到点。
/// 断言四件事:终态 **done**(不是 error)、正文是该轮产出、事件说明「墙钟预算用尽」、
/// 工具确实执行过(证明确实走到了「工具已执行完」的轮末闸门)。
#[tokio::test]
async fn step_wall_clock_budget_stops_loop_and_keeps_output() {
    let _guard = test_lock().await;
    let app = test_app();

    // 预算 1 秒(刻意远小于单次调用 300s 看门狗:合法语义 = 「第一轮结束就收尾」)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "task_step_budget_secs": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "预算设置应保存成功");

    let collector = spawn_collector(app);
    tokio::time::sleep(Duration::from_millis(300)).await; // 等 SSE 建连

    // 每轮睡 1.2s:第 1 轮的工具执行完就已超过 1s 预算
    let id = create_task(
        app,
        r#"[[tool_loop_text:sleep|6 {"ms":1200}]] 步骤预算用例"#,
    )
    .await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    let events = tokio::time::timeout(Duration::from_secs(10), collector)
        .await
        .expect("事件收集应收尾")
        .expect("收集任务不应 panic");

    assert_eq!(
        st, "done",
        "预算到点必须带着产出正常收尾(不得判失败): {detail}"
    );
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("第1轮说明"),
        "结果应是预算收尾那一轮的产出: {result:?}"
    );

    let details = status_details(&events, &id);
    let budget_evt = details
        .iter()
        .find(|d| d.contains("墙钟预算用尽"))
        .unwrap_or_else(|| panic!("应透出墙钟预算用尽的事件: {details:?}"));
    assert!(
        budget_evt.contains("预算 1s"),
        "事件应写明预算值,便于用户对照设置: {budget_evt}"
    );
    let sleep_calls = details
        .iter()
        .filter(|d| d.contains("调用工具 sleep"))
        .count();
    assert!(
        sleep_calls >= 1,
        "轮末闸门意味着工具已执行:应有 sleep 调用记录: {details:?}"
    );

    // 复位(同进程其它用例共享设置)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "task_step_budget_secs": 1200 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 语义熔断任务侧钳制(D3-b):用户把三值设到聊天侧宽松档(64/12/4),
/// 任务侧经 `clamp_for_task` 仍按 ≤8/≤4/≤2 执行 → 同工具输出无实质变化时第 4 次熔断。
/// 判别性:未钳制时同一份输入要 12 次才熔断,故「≤5 次」这条断言能钉住钳制接线。
#[tokio::test]
async fn semantic_guard_is_clamped_for_tasks() {
    let _guard = test_lock().await;
    let app = test_app();

    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_window": 64,
            "loop_guard_semantic_min_calls": 12,
            "loop_guard_semantic_max_distinct": 4,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "三值应可写(聊天侧区间内)");

    let collector = spawn_collector(app);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // calculator 恒返回 "2":输出去重数恒为 1 ≤ 2 → 只等「同工具次数」到点;
    // 参数每轮注入 _mock_round(指纹互不相同)→ 指纹熔断不会抢先触发
    let id = create_task(
        app,
        r#"[[tool_loop_text:calculator|20 {"expression":"1+1"}]] 熔断钳制用例"#,
    )
    .await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    let events = tokio::time::timeout(Duration::from_secs(10), collector)
        .await
        .expect("事件收集应收尾")
        .expect("收集任务不应 panic");

    assert_eq!(st, "done", "熔断是收尾不是失败: {detail}");
    let details = status_details(&events, &id);
    let calls = details
        .iter()
        .filter(|d| d.contains("调用工具 calculator"))
        .count();
    assert!(
        calls <= 5,
        "任务侧应在第 4 次同工具调用时熔断(未钳制要 12 次),实际 {calls} 次: {details:?}"
    );
    let break_evt = details
        .iter()
        .find(|d| d.contains("重复空转熔断"))
        .unwrap_or_else(|| panic!("应透出重复空转熔断事件: {details:?}"));
    assert!(
        break_evt.contains("去重后"),
        "熔断事件应说明判据(次数与去重数): {break_evt}"
    );

    // 复位
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_window": 16,
            "loop_guard_semantic_min_calls": 12,
            "loop_guard_semantic_max_distinct": 2,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 连续只读空转(TMR-5):绑定工作区的任务连续用 fs_read 读同一文件 20 轮,
/// 期间**关闭语义熔断**(否则第 4 次同工具同输出会先被 HB-2 收掉)。
/// 预期:第 8 次只读后出现「只读探索提醒」且循环继续;第 16 次后出现
/// 「连续只读空转熔断」并保留该轮正文;任务以 done 收尾(熔断是收尾不是失败)。
#[tokio::test]
async fn readonly_streak_nudges_then_breaks() {
    let _guard = test_lock().await;
    let app = test_app();

    // 关掉语义熔断:本用例要验证的是「只读计数」独立生效,不允许 HB-2 抢先熔断。
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "loop_guard_semantic_min_calls": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "应可关闭语义熔断");

    let ws = temp_workspace("readonly");
    std::fs::write(ws.join("a.txt"), "hello\n").expect("写测试文件失败");

    let collector = spawn_collector(app);
    tokio::time::sleep(Duration::from_millis(300)).await; // 等 SSE 建连

    // 20 轮 fs_read:用 any_text 变体(连续只读提醒会追加 user 消息,普通 tool_loop
    // 标记按既有语义在提醒轮失效;any 变体跨提醒轮持续)。mock 每轮注入 _mock_round
    // 使「工具名+参数」指纹互不相同,因此重复调用熔断不会抢跑;真正生效的是只读计数。
    let id = create_task_in_workspace(
        app,
        r#"[[tool_loop_any_text:fs_read|20 {"path":"a.txt"}]] 只读空转用例"#,
        &ws,
    )
    .await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    let events = tokio::time::timeout(Duration::from_secs(10), collector)
        .await
        .expect("事件收集应收尾")
        .expect("收集任务不应 panic");

    assert_eq!(
        st, "partial",
        "只读熔断应带着已有产出收尾;绑定工作区且零产物时,TMR-3 再把它降为 partial: {detail}"
    );
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("第16轮说明"),
        "应保留熔断那一轮的正文(恰在第 16 次只读后中止): {result:?}"
    );

    let details = status_details(&events, &id);
    let nudge = details
        .iter()
        .find(|d| d.contains("只读探索提醒"))
        .unwrap_or_else(|| panic!("应在第 8 次只读后透出提醒: {details:?}"));
    assert!(
        nudge.contains("8 次"),
        "提醒应写明触发阈值,便于用户理解: {nudge}"
    );
    let broke = details
        .iter()
        .find(|d| d.contains("连续只读空转熔断"))
        .unwrap_or_else(|| panic!("应在第二次达到阈值后熔断: {details:?}"));
    assert!(
        broke.contains("提醒后仍未落盘"),
        "熔断事件应说明提醒已发过: {broke}"
    );

    // 复位语义熔断(同进程其它用例共享设置)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "loop_guard_semantic_min_calls": 12 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = std::fs::remove_dir_all(&ws);
}

/// 2026-10-23 空岛 team 回归:同一文件的多段 `fs_edit` 成功/失败回执可以恒定,
/// 但参数每轮不同——HB-2 不得把这种正常总装当成空转熔断。
#[tokio::test]
async fn semantic_guard_keeps_repeated_edits_with_distinct_args() {
    let _guard = test_lock().await;
    let app = test_app();

    // 任务侧会把三值钳到 ≤8/≤4/≤2;显式写成钳制后的值,便于失败时读日志。
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_window": 8,
            "loop_guard_semantic_min_calls": 4,
            "loop_guard_semantic_max_distinct": 2,
            "task_step_budget_secs": 3600,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let ws = temp_workspace("edit-args");
    std::fs::write(ws.join("a.txt"), "hello\n").expect("写测试文件失败");

    let collector = spawn_collector(app);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 6 轮 fs_edit:mock 每轮注入 _mock_round 使参数不同;文件首轮被改后,
    // 后续 5 轮都是同一种「找不到旧文本」失败回执——恰是旧实现误杀的形状。
    let id = create_task_in_workspace(
        app,
        r#"[[tool_loop_text:fs_edit|6 {"path":"a.txt","old":"hello","new":"hello2"}]] fs_edit 参数判别用例"#,
        &ws,
    )
    .await;
    run_task(app, &id).await;
    let (st, _detail) = wait_terminal(app, &id).await;
    let events = tokio::time::timeout(Duration::from_secs(10), collector)
        .await
        .expect("事件收集应收尾")
        .expect("收集任务不应 panic");

    assert_ne!(st, "error", "编辑失败也不该让任务崩成 error");
    let details = status_details(&events, &id);
    assert!(
        !details.iter().any(|d| d.contains("重复空转熔断")),
        "不同编辑参数不得触发 HB-2: {details:?}"
    );
    // 用工具结果事件计数兜底:REST 调用行在 mock 工具循环里只落最终一行,
    // 不能用来判轮数;事件明细每轮一条「工具 fs_edit 已返回结果」。
    let edit_results = details
        .iter()
        .filter(|d| d.contains("工具 fs_edit 已返回结果"))
        .count();
    assert!(
        edit_results >= 6,
        "应跑满 6 轮编辑(旧实现第 4 轮熔断),实际 fs_edit 结果事件 {edit_results} 条: {details:?}"
    );

    // 复位
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_window": 16,
            "loop_guard_semantic_min_calls": 12,
            "loop_guard_semantic_max_distinct": 2,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = std::fs::remove_dir_all(&ws);
}

/// 2026-10-23 空岛 followup 回归:终态追加指令常需先通读既有产物再修,连续只读
/// 守卫在该路径关闭——20 次 fs_read 不得出现「只读探索提醒」或「连续只读空转熔断」。
#[tokio::test]
async fn followup_disables_readonly_streak_guard() {
    let _guard = test_lock().await;
    let app = test_app();

    // 关语义熔断(本用例只验证只读计数);顺带确保步骤预算不是别的用例留下的短值。
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_min_calls": 0,
            "task_step_budget_secs": 3600,
            "task_total_budget_secs": 0,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "应可关闭语义熔断");

    let ws = temp_workspace("followup-readonly");
    std::fs::write(ws.join("a.txt"), "hello\n").expect("写测试文件失败");

    // 首轮普通任务(无工具) → 终态后追加 20 轮只读指令。
    let id = create_task_in_workspace(app, "followup 只读守卫用例", &ws).await;
    run_task(app, &id).await;
    let _ = wait_terminal(app, &id).await;

    let collector = spawn_collector(app);
    tokio::time::sleep(Duration::from_millis(300)).await;
    followup_task(
        app,
        &id,
        r#"[[tool_loop_any_text:fs_read|20 {"path":"a.txt"}]] 先通读再修:只读续跑用例"#,
    )
    .await;
    let (st, detail) = wait_terminal(app, &id).await;
    let events = tokio::time::timeout(Duration::from_secs(20), collector)
        .await
        .expect("事件收集应收尾")
        .expect("收集任务不应 panic");

    assert_ne!(st, "error", "followup 不应失败: {detail}");
    let details = status_details(&events, &id);
    assert!(
        !details.iter().any(|d| d.contains("只读探索提醒")),
        "followup 不应触发只读提醒: {details:?}"
    );
    assert!(
        !details.iter().any(|d| d.contains("连续只读空转熔断")),
        "followup 不应触发只读熔断: {details:?}"
    );
    // 20 轮只读 + 首轮 + 收尾;若旧实现第 16 次熔断,fs_read 结果事件会停在 16 条。
    let read_results = details
        .iter()
        .filter(|d| d.contains("工具 fs_read 已返回结果"))
        .count();
    assert!(
        read_results >= 17,
        "followup 应跑满 20 轮只读(旧实现第 16 次熔断),实际 fs_read 结果事件 {read_results} 条: {details:?}"
    );

    // 复位
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "loop_guard_semantic_min_calls": 12 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = std::fs::remove_dir_all(&ws);
}

/// TMR-3:绑定工作区但只说话不落盘 → 终态由 done 降为 partial,error 与事件都说明原因。
#[tokio::test]
async fn workspace_task_without_file_changes_is_partial() {
    let _guard = test_lock().await;
    let app = test_app();
    let ws = temp_workspace("delivery-missing");

    let collector = spawn_collector(app);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 纯文本回复、不调用任何工具:执行器原本会返回 Done,TMR-3 门槛必须把它降为 partial。
    let id = create_task_in_workspace(app, r#"[[reply:只说不写的成果正文]] 交付门槛缺失用例"#, &ws)
        .await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    let events = tokio::time::timeout(Duration::from_secs(10), collector)
        .await
        .expect("事件收集应收尾")
        .expect("收集任务不应 panic");

    assert_eq!(
        st, "partial",
        "绑定工作区且零文件变更时不得判 done: {detail}"
    );
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("未检测到文件变更"),
        "error 应说明缺产物原因: {error:?}"
    );
    let details = all_task_details(&events, &id);
    assert!(
        details.iter().any(|d| d.contains("文件产物校验未通过")),
        "应透出文件产物校验未通过事件: {details:?}"
    );
    let id2 = create_task(app, r#"[[reply:纯文本任务]] 不绑定工作区"#).await;
    run_task(app, &id2).await;
    let (st2, detail2) = wait_terminal(app, &id2).await;
    assert_eq!(
        st2, "done",
        "未绑定工作区的纯文本任务不得被文件门槛误伤: {detail2}"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

/// TMR-3:写入走 bash 也计入产物——台账的 source=bash 启发式检出同样应满足门槛,
/// 避免「只有 fs_write 才算交付」的窄口径把命令型编码任务误判为 partial。
#[tokio::test]
async fn workspace_task_with_bash_file_change_stays_done() {
    let _guard = test_lock().await;
    let app = test_app();
    let ws = temp_workspace("delivery-bash");

    // 命令执行总开关默认关闭(与生产一致);本用例要验证 bash 产物计数,先显式打开。
    let (status, _) = send_json(app, "PUT", "/api/settings", json!({ "exec_enabled": true })).await;
    assert_eq!(status, StatusCode::OK, "应可打开命令执行总开关");

    let id = create_task_in_workspace(
        app,
        r#"[[tool:bash {"command":"echo bash-out > bash.txt"}]] bash 写文件任务"#,
        &ws,
    )
    .await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "done", "bash 写入应被台账识别为产物: {detail}");
    let written = ws.join("bash.txt");
    assert!(written.is_file(), "bash 应把文件落在工作区: {detail}");
    assert_eq!(
        std::fs::read_to_string(&written).unwrap_or_default().trim(),
        "bash-out"
    );

    // 复位(同进程其它用例共享设置)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "exec_enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = std::fs::remove_dir_all(&ws);
}

/// TMR-3:绑定工作区且真的写了文件 → 保持 done,文件内容落盘正确。
#[tokio::test]
async fn workspace_task_with_file_change_stays_done() {
    let _guard = test_lock().await;
    let app = test_app();
    let ws = temp_workspace("delivery-present");

    let id = create_task_in_workspace(
        app,
        r#"[[tool:fs_write {"path":"out.txt","content":"hello tmr"}]] 写文件任务"#,
        &ws,
    )
    .await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "done", "有文件产物时应保持 done: {detail}");
    let written = ws.join("out.txt");
    assert!(written.is_file(), "fs_write 应把文件落在工作区: {detail}");
    assert_eq!(
        std::fs::read_to_string(&written).unwrap_or_default(),
        "hello tmr",
        "文件内容应与工具参数一致"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

/// SKY-6 第 4 项:纯读工具(`fs_read/fs_glob/fs_grep`)的 HB-2 采用**整窗阈值**——
/// 同一纯读工具要占满最近一整窗(任务侧 8 次)且输出去重 ≤2 种才熔断。6 次同文件读
/// 未占满整窗,不得熔断;旧口径(同工具 4 次)会在第 4 次熔断,即全题复跑的 4/8/2 误伤。
#[tokio::test]
async fn repeated_reads_within_window_do_not_trip_semantic_guard() {
    let _guard = test_lock().await;
    let app = test_app();

    // 任务侧钳制后的口径:窗口 8 / 同工具 4 / 去重 2;显式设置便于失败时读日志。
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_window": 8,
            "loop_guard_semantic_min_calls": 4,
            "loop_guard_semantic_max_distinct": 2,
            "task_step_budget_secs": 3600,
            "task_total_budget_secs": 0,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let ws = temp_workspace("readonly-hb2-short");
    std::fs::write(ws.join("a.txt"), "hello\n").expect("写测试文件失败");

    let collector = spawn_collector(app);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 6 轮 fs_read 同一文件:输出恒同、参数每轮仅在 _mock_round 上变化。
    // any 变体保证只读提醒轮(本例不会到 8 次)之外的其它追加消息不影响循环。
    let id = create_task_in_workspace(
        app,
        r#"[[tool_loop_any_text:fs_read|6 {"path":"a.txt"}]] 纯读短序列用例"#,
        &ws,
    )
    .await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    let events = tokio::time::timeout(Duration::from_secs(10), collector)
        .await
        .expect("事件收集应收尾")
        .expect("收集任务不应 panic");

    assert_ne!(st, "error", "重复读不该把任务打成 error: {detail}");
    let details = status_details(&events, &id);
    assert!(
        !details.iter().any(|d| d.contains("重复空转熔断")),
        "6 次同文件读未占满整窗,不得触发 HB-2: {details:?}"
    );
    // 用「调用工具」事件计数证明循环真的跑了(否则断言无判别力);广播允许丢帧,
    // 故取 ≥5 而非严格 6——旧口径在第 4 次就断,本断言仍能区分。
    let reads = details
        .iter()
        .filter(|d| d.contains("调用工具 fs_read"))
        .count();
    assert!(
        reads >= 5,
        "应真实跑满约 6 次 fs_read(旧实现第 4 次熔断),实际 {reads} 条: {details:?}"
    );

    // 复位
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_window": 16,
            "loop_guard_semantic_min_calls": 12,
            "loop_guard_semantic_max_distinct": 2,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = std::fs::remove_dir_all(&ws);
}

/// SKY-6 第 4 项的反向判据:整窗阈值不是「关掉」——同一纯读工具占满一整窗
/// (8 次)且输出只有 1 种时仍须熔断,且恰在第 8 次(旧口径第 4 次)。
#[tokio::test]
async fn repeated_reads_at_full_window_still_trip_semantic_guard() {
    let _guard = test_lock().await;
    let app = test_app();

    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_window": 8,
            "loop_guard_semantic_min_calls": 4,
            "loop_guard_semantic_max_distinct": 2,
            "task_step_budget_secs": 3600,
            "task_total_budget_secs": 0,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let ws = temp_workspace("readonly-hb2-full");
    std::fs::write(ws.join("a.txt"), "hello\n").expect("写测试文件失败");

    let collector = spawn_collector(app);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let id = create_task_in_workspace(
        app,
        r#"[[tool_loop_any_text:fs_read|12 {"path":"a.txt"}]] 纯读整窗熔断用例"#,
        &ws,
    )
    .await;
    run_task(app, &id).await;
    let (st, detail) = wait_terminal(app, &id).await;
    let events = tokio::time::timeout(Duration::from_secs(10), collector)
        .await
        .expect("事件收集应收尾")
        .expect("收集任务不应 panic");

    assert_ne!(st, "error", "熔断是收尾不是失败: {detail}");
    let details = status_details(&events, &id);
    let break_at = details
        .iter()
        .position(|d| d.contains("重复空转熔断"))
        .unwrap_or_else(|| panic!("整窗同输出纯读仍须熔断: {details:?}"));
    let reads_before = details[..break_at]
        .iter()
        .filter(|d| d.contains("调用工具 fs_read"))
        .count();
    // 广播允许丢帧:允许少 1 帧;旧口径为 4,区间仍具判别力。
    assert!(
        (7..=8).contains(&reads_before),
        "整窗阈值应在第 8 次才熔断(旧口径为第 4 次),实际 {reads_before} 次: {details:?}"
    );
    let break_evt = &details[break_at];
    assert!(
        break_evt.contains("出现 8 次"),
        "熔断事件应说明「最近 8 次中出现 8 次」: {break_evt}"
    );

    // 复位
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "loop_guard_semantic_window": 16,
            "loop_guard_semantic_min_calls": 12,
            "loop_guard_semantic_max_distinct": 2,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = std::fs::remove_dir_all(&ws);
}
