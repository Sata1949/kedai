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

async fn run_task(app: &axum::Router, id: &str) {
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
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
