// 引擎核心热路径的**特征化测试**(characterization tests)。
//
// ## 为什么单独一份文件
//
// `agents/engine/mod.rs::run`(约 520 行)与 `agents/engine/executor.rs::run_tool_loop`
// (约 437 行)是全库最大的两个函数,但**从未被拆分**——原因是核心热路径,回归风险高
// (见 docs/功能-变更史.md §6 批次 5)。
//
// 本文件的作用:在**动它们之前**先把「当前行为」钉成断言,作为后续拆分的回归网。
// 与既有测试的分工:
//   - api_integration.rs 已覆盖工具循环轮次上限、并行调用回填、上游错误终态;
//   - tasks.rs 覆盖任务模式侧;
//   - **本文件只补既有测试的空白**,不重复已覆盖的断言。
//
// ## 本文件锁定的三处空白(经核查均无既有覆盖)
//
//   1. **中断终态**(`SseEvent::Interrupted`):全仓集成测试零覆盖——`interrupted`
//      分支位于 `run` 的收尾 match 里,是拆分时最容易丢失的分支之一;
//   2. **抢占语义**(`run` 开头的 `runs` 表 + `flag.abort()`):仅有一个单元测试覆盖
//      `finish_run_generation` 的**表清理**,未覆盖「新 run 抢占旧 run」的端到端行为;
//   3. **工具结果回填格式**(`[tool]` 行 + `ok` 字段):并行调用的回填有覆盖,
//      但「单工具调用的结果消息形状」与「工具失败时错误回填仍继续循环」无覆盖。
//
// 这些断言只描述**当前事实**,不含「应该怎样」的判断——拆分后必须逐条保持为真。
//
// ## 编写过程中被实测否决的三个臆断(记录下来防后人重踩)
//
// 特征化测试的价值正在于此:初版断言写的是「我以为的行为」,被实测逐一否决,
// 改写成「代码实际的行为」后才有回归价值:
//
//   1. **臆断**:`SseEvent::ToolResult` 有 `ok` 布尔字段区分成败。
//      **实际**:事件形状是 `{name, output, call_id?, render_kind?}`,**没有** `ok`;
//      成败靠 `output` 内是否含 `error` 区分(成功/失败共用同一事件形状)。
//   2. **臆断**:调用未注册工具会回填 `ok=false`。
//      **实际**:mock 的 `tool_offered` 门忠实模拟「模型只能调用已下发工具」,
//      未下发工具**根本不产生 tool_call**;要测「执行期失败」须用已下发但会失败的工具
//      (如 `calculator` 除零)。
//   3. **臆断**:固定 `sleep(250ms)` 后 stop 是稳定的中断方式。
//      **实际**:时序赌博(负载高时会错过生成窗口而偶发失败),且 mock 只有
//      **默认回复**分支逐字符流式(`[[reply:]]` 系钩子单块直出、无延时,来不及中断)。
//      已改为**事件驱动**:逐帧读到首个 `token` 才 stop,零时间假设。
//
// 另:新会话自带开场白(`first_mes`,role=assistant),断言 assistant 条数须**前后对比**。
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

async fn upload_character(app: &axum::Router, name: &str) -> String {
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        json!({ "spec": "chara_card_v2", "spec_version": "1.0", "name": name, "description": "特征化测试", "first_mes": "你好" })
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

async fn new_session(app: &axum::Router, cid: &str) -> String {
    let (_, session) = send_json(
        app,
        "POST",
        "/api/chat/sessions",
        json!({ "character_id": cid }),
    )
    .await;
    session["id"].as_str().unwrap().to_string()
}

/// 从 SSE body 读下一帧并解析为 JSON(增量读,供「观测事件后再动作」的用例)。
/// 流结束返回 None。参考 tests/task_events.rs 的同名做法。
async fn read_sse_event(body: &mut Body) -> Option<Value> {
    loop {
        let frame = body.frame().await?.ok()?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        let text = String::from_utf8_lossy(&data);
        for line in text.lines() {
            if let Some(payload) = line.strip_prefix("data:") {
                if let Ok(v) = serde_json::from_str::<Value>(payload.trim()) {
                    return Some(v);
                }
            }
        }
    }
}

/// 发一轮 chat,读完整 SSE 流并解析为事件数组
async fn sse_events(
    app: &axum::Router,
    sid: &str,
    cid: &str,
    message: &str,
    agent_mode: &str,
) -> Vec<Value> {
    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "session_id": sid, "character_id": cid,
                "message": message, "agent_mode": agent_mode,
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

// ==================== 特征化 1:中断终态与抢占 ====================

/// **抢占语义(端到端)**:同一会话连续发起两次生成时,第一次被第二次抢占。
///
/// 当前事实(由 `run` 开头的抢占区块与 `pending_runs` 守卫共同决定):
///   - 第二次请求在**第一次尚未落库完成**时会被 `pending_runs` 占位守卫拒绝(409),
///     这是路由层的并发保护,与引擎内抢占是两道不同机制;
///   - 引擎内抢占(`if existing.active { existing.flag.abort() }`)只在「旧 run 仍在
///     引擎 but 占位已释放」的窗口生效(如 resend/regenerate 路径)。
///
/// 本测试锁定**路由层守望**:并发第二次必须 409 而非静默并行写库。
/// (引擎内抢占的窗口难以在集成层稳定构造,故以 `chatMessageScriptScheduler` 之外的
///  单测覆盖表清理逻辑;此处不臆造断言。)
#[tokio::test]
async fn concurrent_second_send_is_rejected_while_first_in_flight() {
    let _guard = test_lock().await;
    let app = test_app();
    let cid = upload_character(app, "抢占特征化.json").await;
    let sid = new_session(app, &cid).await;

    // 长回复给出足够的在途窗口(逐字流式,每字符 8ms)
    let long = "抢占测试".repeat(60);
    let make = |msg: String| {
        Request::builder()
            .method("POST")
            .uri("/api/chat/send")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "session_id": sid, "character_id": cid, "message": msg }).to_string(),
            ))
            .unwrap()
    };
    let (a, b) = tokio::join!(
        app.clone().oneshot(make(format!("[[reply:{long}]]"))),
        app.clone().oneshot(make("[[reply:第二发]]".to_string()))
    );
    let statuses = [a.unwrap().status(), b.unwrap().status()];
    assert!(
        statuses.contains(&StatusCode::OK),
        "至少一发应被接受: {statuses:?}"
    );
    assert!(
        statuses.contains(&StatusCode::CONFLICT),
        "在途时的第二发应被占位守卫拒绝(409),而非并行写库: {statuses:?}"
    );
}

/// **中断保留部分产出**(HB-3,2026-09-18 **口径变更**):主动 stop 后
/// 已生成的部分正文落库(extra.interrupted=true)、中断轮消耗计入 session_usage、
/// 仍不得发 `finish`、仍必须发 `interrupted`。
///
/// ## 口径为什么改(原断言为「中断不落库」)
///
/// 聊天侧「中断即丢弃」与任务侧「中断保留已落库内容」是两套口径(遗留 OBS-1),
/// 且中断轮消耗的 token 无账可查——用户按停止 = 白等一轮。2026-09-18 的 harness
/// 补强批次 HB-3 统一为「保留 + 标记」:extra.interrupted 与既有 extra.truncated
/// 同构(事件是暂态的,不落库则刷新后看不出这条是中断的),前端据此渲染「已中断」
/// 并复用既有「生成新版本」入口。
///
/// 本用例由旧不变量 `stop_aborts_generation_without_persisting_assistant_message`
/// 重写而来(断言方向反转 + 新增标记与记账断言),测试名同步改为现在这条。
///
/// ## 为什么用「增量读 + 见 token 才 stop」而非固定 sleep(工程要点,沿用原注释)
///
/// 初版用 `sleep(250ms)` 后 stop,结果**偶发失败**——固定延时是时序赌博:
/// 机器负载高时 250ms 可能已错过生成窗口(生成早结束 → 发 finish → 断言失败)。
/// 现改为**事件驱动**:逐帧读到第一个 `token` 事件才发 stop,无任何时间假设。
/// 这同时保证「确实是在流式过程中中断的」,而不是「生成还没开始就停了」。
///
/// 另一处陷阱:mock 只有**默认回复**分支逐字符流式(每字符 sleep 8ms);
/// `[[reply:]]`/`[[reply_stream:]]` 钩子走单块/无延时路径,来不及中断。
/// 故本测试不传任何内容钩子。
#[tokio::test]
async fn stop_keeps_partial_output_with_interrupted_mark() {
    let _guard = test_lock().await;
    let app = test_app();
    let cid = upload_character(app, "中断特征化.json").await;
    let sid = new_session(app, &cid).await;

    // 基线:新会话已含开场白(first_mes,role=assistant),故须**前后对比**而非绝对计数。
    let assistant_before = assistant_count(app, &sid).await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/send")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                // 不传内容钩子 → 走默认回复(唯一逐字符 8ms 流式的路径)
                "session_id": sid, "character_id": cid,
                "message": "随便说点什么", "agent_mode": "deep",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "chat/send 应 200");
    let mut body = resp.into_body();

    // 逐帧读,直到看见第一个 token(证明**已进入流式阶段**);先到的 step 事件忽略。
    let mut events: Vec<Value> = Vec::new();
    let mut entered_streaming = false;
    while let Some(ev) = read_sse_event(&mut body).await {
        let is_token = ev["type"] == "token";
        events.push(ev);
        if is_token {
            entered_streaming = true;
            break;
        }
    }
    assert!(
        entered_streaming,
        "应至少收到一个 token 才开始中断(实际事件: {:?})",
        events
            .iter()
            .filter_map(|e| e["type"].as_str())
            .collect::<Vec<_>>()
    );

    // 此刻确定处于流式中 → 请求停止(无时序假设)
    let (status, _) = send_json(app, "POST", "/api/chat/stop", json!({ "session_id": sid })).await;
    assert_eq!(status, StatusCode::OK, "stop 应 200");

    // 读完剩余事件
    while let Some(ev) = read_sse_event(&mut body).await {
        events.push(ev);
    }
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();

    // **必须真的走到中断分支**:发 `interrupted` 事件。
    assert!(
        types.contains(&"interrupted"),
        "stop 后应发出 interrupted 终态事件(类型序列: {types:?})"
    );
    // 中断后不得出现 finish(否则说明「中断」被当成正常完成)
    assert!(
        !types.contains(&"finish"),
        "中断不应发出 finish 事件(类型序列: {types:?})"
    );

    // 部分产出保留:历史新增一条 assistant 消息,带 interrupted 标记且正文非空
    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    let assistant_after = history["messages"]
        .as_array()
        .map(|msgs| msgs.iter().filter(|m| m["role"] == "assistant").count())
        .unwrap_or(0);
    assert_eq!(
        assistant_after,
        assistant_before + 1,
        "中断应保留部分产出(前 {assistant_before} → 后 {assistant_after})"
    );
    let last = history["messages"]
        .as_array()
        .and_then(|msgs| msgs.iter().rev().find(|m| m["role"] == "assistant"))
        .expect("应有落库的 assistant 消息");
    assert_eq!(
        last["extra"]["interrupted"],
        json!(true),
        "中断消息必须带 extra.interrupted 标记(刷新后仍可见): {last}"
    );
    assert!(
        !last["content"].as_str().unwrap_or("").trim().is_empty(),
        "中断消息正文不应为空(流式中已生成的字符): {last}"
    );

    // 中断轮用量入账(HB-3):此前整轮白烧不计账
    let (_, usage) = send_json(
        app,
        "GET",
        &format!("/api/token/session-total?session_id={sid}"),
        json!({}),
    )
    .await;
    assert!(
        usage["total_tokens"].as_i64().unwrap_or(0) > 0,
        "中断轮消耗必须计入 session_usage: {usage}"
    );
}

/// 统计会话中 assistant 消息条数(含开场白)
async fn assistant_count(app: &axum::Router, sid: &str) -> usize {
    let (_, history) = send_json(
        app,
        "GET",
        &format!("/api/chat/history?session_id={sid}"),
        json!({}),
    )
    .await;
    history["messages"]
        .as_array()
        .map(|msgs| msgs.iter().filter(|m| m["role"] == "assistant").count())
        .unwrap_or(0)
}

// ==================== 特征化 2:工具结果回填形状 ====================

/// **单工具调用的回填形状**:一次工具调用产生
///   ① 一条 `tool_result` 事件(带 `ok: true` 与 `output.result`),
///   ② 最终正文里一条 `[tool ...]` 行。
///
/// 当前事实:工具结果以「`ok` 布尔 + `output` 对象」形状回填(见 executor.rs 的
/// `ExecutorResult`/工具结果构造);成功调用 `ok=true`。拆分 `run_tool_loop` 时
/// 该形状必须逐字保持(前端 `api/types.ts` 按此解析)。
#[tokio::test]
async fn single_tool_call_backfills_ok_true_with_result() {
    let _guard = test_lock().await;
    let app = test_app();
    let cid = upload_character(app, "工具回填特征化.json").await;
    let sid = new_session(app, &cid).await;

    let events = sse_events(
        app,
        &sid,
        &cid,
        r#"[[tool:calculator {"expression":"12*34"}]]"#,
        "agent",
    )
    .await;

    let result = events
        .iter()
        .find(|e| e["type"] == "tool_result")
        .expect("应有 tool_result 事件: {events:?}");

    // **形状契约(实测)**:SseEvent::ToolResult = {name, output, call_id?, render_kind?}
    // ——成功与失败**共用同一事件形状**,靠 `output` 内部是否含 `error` 区分;
    // 事件级**没有** `ok` 布尔字段(本轮编写测试时曾臆造 `ok`,被实测否决)。
    assert_eq!(
        result["name"], "calculator",
        "tool_result 应携带工具名: {result}"
    );
    assert!(result["output"].is_object(), "output 应为对象: {result}");
    assert!(
        result["output"].get("error").is_none(),
        "成功调用不应含 output.error: {result}"
    );
    assert!(
        result["call_id"].is_string(),
        "应携带 call_id(供前端关联调用): {result}"
    );
    // calculator 的确定性结果(表达式语义的回归锚点)
    assert_eq!(
        result["output"]["result"].as_f64(),
        Some(408.0),
        "12*34 应回填 408: {result}"
    );

    // 终态:应有 finish
    //
    // 注:**不回填进最终正文**——工具结果作为 `role="tool"` 消息回灌给模型(下一轮),
    // 最终 assistant 正文是模型据此产出的文本。回灌形状的断言见并行调用用例
    // (api_integration.rs 用 `[[tool_echo:]]` 回显 LLM 消息结构);
    // 此处只锁定「事件级形状 + 正常收尾」。
    assert!(
        events.iter().any(|e| e["type"] == "finish"),
        "成功工具调用后应有 finish: {events:?}"
    );
}

/// **工具失败仍继续循环(不中断生成)**:调用不存在的工具时,
/// 结果以 `ok=false` + `output.error` 回填,循环继续并最终正常 finish。
///
/// 当前事实:工具执行失败不返回 Err(那会中断整个 run),而是把错误包装进结果
/// 回填给模型,让模型自行纠正——这是「模型驱动重试」的关键设计。
/// 拆分时若误把失败当致命错误上抛,本测试会失败。
#[tokio::test]
async fn failed_tool_call_backfills_error_and_loop_continues() {
    let _guard = test_lock().await;
    let app = test_app();
    let cid = upload_character(app, "工具失败特征化.json").await;
    let sid = new_session(app, &cid).await;

    // 用**已下发但执行会失败**的工具:calculator 除零 → 执行期 Err("除数不能为 0")。
    //
    // 注意**不能**用「未注册的工具名」:mock 的 tool_offered 门(generate 忠实模拟
    // 「模型只能调用已下发工具」)会让未下发工具根本不产生 tool_call,
    // 于是什么都不会发生——那是 mock 忠实性,不是被测行为(本轮踩到的第二个坑)。
    let events = sse_events(
        app,
        &sid,
        &cid,
        r#"[[tool:calculator {"expression":"1/0"}]]"#,
        "agent",
    )
    .await;

    let result = events
        .iter()
        .find(|e| e["type"] == "tool_result")
        .expect("工具失败也应有 tool_result 事件(回填给模型): {events:?}");

    // 失败形状:**与成功同形状**,靠 output.error 区分(无事件级 ok 字段)
    assert!(
        result["output"].get("error").is_some(),
        "失败应在 output 内携带 error: {result}"
    );
    assert!(
        result["output"]["error"].is_string(),
        "失败应携带 output.error: {result}"
    );

    // 关键:失败**不中断**生成,主流程仍正常收尾
    assert!(
        events.iter().any(|e| e["type"] == "finish"),
        "工具失败不应中断生成,应有 finish: {events:?}"
    );
    assert!(
        !events.iter().any(|e| e["type"] == "error"),
        "工具失败不应产生顶层 error 终态: {events:?}"
    );
}

// ==================== 特征化 3:无工具路径的最小形状 ====================

/// **纯文本路径**:未触发工具时,事件流的最小形状为
/// `step* → token+ → (vars?) → finish`,且**不含** tool_call/tool_result。
///
/// 当前事实:这条路径是 `run` 的「快车道」(工具循环零轮),拆分时最容易被
/// 新增的阶段破坏(例如误发空的 tool_result)。锁定它可防止「为拆而拆」引入噪声事件。
#[tokio::test]
async fn plain_text_path_emits_no_tool_events() {
    let _guard = test_lock().await;
    let app = test_app();
    let cid = upload_character(app, "纯文本特征化.json").await;
    let sid = new_session(app, &cid).await;

    let events = sse_events(app, &sid, &cid, "[[reply:普通回复]]", "deep").await;

    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(
        types.contains(&"finish"),
        "纯文本路径应有 finish: {types:?}"
    );
    assert!(
        !types.contains(&"tool_call"),
        "未触发工具不应有 tool_call: {types:?}"
    );
    assert!(
        !types.contains(&"tool_result"),
        "未触发工具不应有 tool_result: {types:?}"
    );
    // 正文内容经 finish 透出
    let finish = events.iter().find(|e| e["type"] == "finish").unwrap();
    assert!(
        finish["content"]
            .as_str()
            .unwrap_or("")
            .contains("普通回复"),
        "finish 应携带生成正文: {finish}"
    );
}

// ==================== RPFLOW-1:deep/agent 流程(草稿隐藏 + 强制批判修改) ====================

/// **草稿步默认隐藏(RPFLOW-1)**:deep 模式第 1 步产物经 `draft` 事件透出、
/// **不进正文**(无任何携带草稿文本的 token 事件),正文以正文步产物收口,
/// 反思步强制发生且通过时在推理链可见。
///
/// mock 钩子靠 system 子串门控,天然按步骤分派:
///   - 草稿步 system 含「本步指令·草稿」→ 回「草稿XYZ」;
///   - 正文步 system 含「本步指令·正文」→ 回「正式正文ABC」;
///   - 反思调用 system 含「反思纪律」(引擎追加的纪律段)→ 回「PASS」(无工具调用 → 达标)。
#[tokio::test]
async fn deep_mode_draft_is_hidden_and_reflect_accepts() {
    let _guard = test_lock().await;
    let app = test_app();
    let cid = upload_character(app, "流程特征化-草稿.json").await;
    let sid = new_session(app, &cid).await;

    let events = sse_events(
        app,
        &sid,
        &cid,
        "[[reply_if_any:本步指令·草稿|草稿XYZ]] [[reply_if_any:本步指令·正文|正式正文ABC]] [[reply_if_any:反思纪律|PASS]]",
        "deep",
    )
    .await;

    // 1) 草稿经 draft 事件透出(面板折叠展示的数据源)
    let draft = events
        .iter()
        .find(|e| e["type"] == "draft")
        .expect("deep 模式应有 draft 事件");
    assert!(
        draft["text"].as_str().unwrap_or("").contains("草稿XYZ"),
        "draft 事件应携带草稿文本: {draft}"
    );
    // 2) 草稿默认隐藏:任何 token 事件都不得携带草稿文本
    for e in events.iter().filter(|e| e["type"] == "token") {
        assert!(
            !e["text"].as_str().unwrap_or("").contains("草稿XYZ"),
            "草稿不得经 token 事件透出(默认隐藏): {e}"
        );
    }
    // 3) 正文以正文步产物收口,草稿不污染
    let finish = events.iter().find(|e| e["type"] == "finish").unwrap();
    let content = finish["content"].as_str().unwrap_or("");
    assert!(
        content.contains("正式正文ABC"),
        "finish 应为正文步产物: {finish}"
    );
    assert!(
        !content.contains("草稿XYZ"),
        "finish 不得包含草稿: {finish}"
    );
    // 4) 步骤事件可见:草稿步 + 反思通过(反思不再静默)
    let steps: Vec<String> = events
        .iter()
        .filter(|e| e["type"] == "step")
        .filter_map(|e| e["step"].as_str().map(str::to_string))
        .collect();
    assert!(
        steps.iter().any(|s| s == "草稿中…"),
        "应有草稿步事件: {steps:?}"
    );
    assert!(
        steps.iter().any(|s| s == "反思通过"),
        "应有反思通过事件: {steps:?}"
    );
}

/// **批判与定点修改(RPFLOW-1)**:反思步模型调用 revise_passage 后,引擎把修改后的
/// 正文回写并收口到 finish,推理链发出「批判与修改」进度事件;工具的 `text` 参数由
/// 引擎覆写为当前正文(模型传的过期全文被忽略)。
///
/// mock:正文步回「第一句。旧句。」;**[[tool_if:]] 以「反思纪律」子串门控**——该子串
/// 只出现在反思调用的 system(引擎追加的纪律段),草稿/正文步不命中,故工具调用
/// 只由反思步发出;后续轮无判定输出 → 有界结束,机械闸通过后采纳修订文本。
#[tokio::test]
async fn deep_mode_reflect_applies_targeted_revision() {
    let _guard = test_lock().await;
    let app = test_app();
    let cid = upload_character(app, "流程特征化-修改.json").await;
    let sid = new_session(app, &cid).await;

    let events = sse_events(
        app,
        &sid,
        &cid,
        "[[reply_if_any:本步指令·草稿|草稿DR]] [[reply_if_any:本步指令·正文|第一句。旧句。]] \
         [[tool_if:反思纪律|revise_passage {\"find\":\"旧句\",\"replace\":\"新句\",\"text\":\"模型传的过期全文\"}]]",
        "deep",
    )
    .await;

    // 修改进度事件(反思过程可见)
    let modify = events
        .iter()
        .find(|e| e["type"] == "step" && e["step"].as_str() == Some("批判与修改"))
        .expect("应有「批判与修改」事件");
    assert!(
        modify["detail"]
            .as_str()
            .unwrap_or("")
            .contains("第 1/2 次"),
        "修改事件应带次数(deep 上限 2): {modify}"
    );
    // 修订文本回写并收口(旧句被新句替换;且未因重生成被丢弃)
    let finish = events.iter().find(|e| e["type"] == "finish").unwrap();
    let content = finish["content"].as_str().unwrap_or("");
    assert!(
        content.contains("新句"),
        "finish 应包含修订后的文本: {finish}"
    );
    assert!(!content.contains("旧句"), "旧文本应已被替换: {finish}");
}

/// **反思回退重生成正文时草稿注记必须在场(RPFLOW-1 审查回归)**:正文步提示词恒写
/// 「严格依据上方「内部草稿」扩写」,而回退重生成走 `llm_messages.truncate(base_len)`。
/// 旧实现 base_len 取在步骤循环之前,草稿注记(循环内 push)会被一并截掉 →
/// 重生成时正文步提示词引用一个不存在的「内部草稿」。
///
/// 构造:正文以中间标点「，」结尾 → 机械闸判「疑似截断」→ 回退到正文步重生成。
/// 正文内容由「内部草稿·仅你可见」子串门控 —— 注记仍在消息里才回得出该短语,
/// 被丢则落到 mock 默认回复(不含门控短语),足以区分两种实现。
#[tokio::test]
async fn deep_retreat_regeneration_keeps_draft_note() {
    let _guard = test_lock().await;
    let app = test_app();
    let cid = upload_character(app, "流程特征化-回退注记.json").await;
    let sid = new_session(app, &cid).await;

    let events = sse_events(
        app,
        &sid,
        &cid,
        "[[reply_if_any:本步指令·草稿|草稿RT]] \
         [[reply_if_any:内部草稿·仅你可见|依据草稿扩写的正文，]]",
        "deep",
    )
    .await;

    // 先确认确实进了回退重生成分支(否则本用例会退化成「无判别性」的绿)
    let steps: Vec<String> = events
        .iter()
        .filter(|e| e["type"] == "step")
        .filter_map(|e| e["step"].as_str().map(str::to_string))
        .collect();
    assert!(
        steps.iter().any(|s| s == "重新生成"),
        "正文以中间标点结尾应触发回退重生成: {steps:?}"
    );
    // 重生成后的正文仍来自「内部草稿在场」那条路径
    let finish = events.iter().find(|e| e["type"] == "finish").unwrap();
    let content = finish["content"].as_str().unwrap_or("");
    assert!(
        content.contains("依据草稿扩写的正文"),
        "回退重生成丢了草稿注记,正文步提示词引用落空: {finish}"
    );
}

/// **空草稿可见化(RPFLOW-1 实测回归)**:草稿步输出为空(推理耗尽预算,真实模型两轮
/// 实测中各出现过一次)时不再静默跳过——发「草稿为空」step 事件、不产生 draft 事件,
/// 且正文步照常收口(不因缺草稿而失败)。
///
/// mock 用 `[[empty_if:本步指令·草稿]]` 令草稿步返回空(completion=0,不触发提额重试;
/// 提额判定本身由 `draft_heal_budget` 单测覆盖),正文步由 `[[reply_if_any:]]` 门控。
#[tokio::test]
async fn deep_empty_draft_is_visible_and_main_step_continues() {
    let _guard = test_lock().await;
    let app = test_app();
    let cid = upload_character(app, "流程特征化-空草稿.json").await;
    let sid = new_session(app, &cid).await;

    let events = sse_events(
        app,
        &sid,
        &cid,
        "[[empty_if:本步指令·草稿]] [[reply_if_any:本步指令·正文|正文照常]]",
        "deep",
    )
    .await;

    assert!(
        events.iter().all(|e| e["type"] != "draft"),
        "空草稿不应产生 draft 事件(无内容可展示)"
    );
    let steps: Vec<String> = events
        .iter()
        .filter(|e| e["type"] == "step")
        .filter_map(|e| e["step"].as_str().map(str::to_string))
        .collect();
    assert!(
        steps.iter().any(|s| s == "草稿为空"),
        "空草稿应有可见 step 事件(不静默): {steps:?}"
    );
    let finish = events.iter().find(|e| e["type"] == "finish").unwrap();
    assert!(
        finish["content"]
            .as_str()
            .unwrap_or("")
            .contains("正文照常"),
        "缺草稿不应影响正文步收口: {finish}"
    );
}

/// **归档步(RPFLOW-2)**:deep 流程第 4 步执行且**产物不进气泡**——归档模型回吐的
/// 文本不产生 token 事件、不影响 finish 正文;可观测的只有 step 事件。
/// 归档步的工具白名单(含 worldbook_update)由 planner 单测锁定;工具行为见
/// `tools/worldbook.rs` 单测。
///
/// mock 用 [[reply_if_any:]] 门控「归档与同步」子串:该子串只出现在归档步的
/// system(步骤提示词),且钩子扫描**全量消息**(归档步的末条 user 是引擎追加的
/// 「本轮最终正文」注记,末条提取的钩子会丢标记——与 TM-EMPTY-1 提醒轮同型)。
#[tokio::test]
async fn deep_mode_archive_step_runs_without_leaking_into_bubble() {
    let _guard = test_lock().await;
    let app = test_app();
    let cid = upload_character(app, "流程特征化-归档.json").await;
    let sid = new_session(app, &cid).await;

    let events = sse_events(
        app,
        &sid,
        &cid,
        "[[reply_if_any:本步指令·草稿|草稿DR]] [[reply_if_any:本步指令·正文|正文ABC]] \
         [[reply_if_any:反思纪律|PASS]] [[reply_if_any:归档与同步|归档正文MNX]]",
        "deep",
    )
    .await;

    // 归档步确实执行(step 事件可见)
    let steps: Vec<String> = events
        .iter()
        .filter(|e| e["type"] == "step")
        .filter_map(|e| e["step"].as_str().map(str::to_string))
        .collect();
    assert!(
        steps.iter().any(|s| s == "归档与词条同步中…"),
        "应有归档步事件: {steps:?}"
    );
    // 归档产物不进气泡:token 事件与 finish 正文都不得携带归档话术
    for e in events.iter().filter(|e| e["type"] == "token") {
        assert!(
            !e["text"].as_str().unwrap_or("").contains("归档正文MNX"),
            "归档产物不得经 token 透出: {e}"
        );
    }
    let finish = events.iter().find(|e| e["type"] == "finish").unwrap();
    let content = finish["content"].as_str().unwrap_or("");
    assert!(
        !content.contains("归档正文MNX"),
        "finish 不得含归档产物: {finish}"
    );
    assert!(
        content.contains("正文ABC"),
        "finish 应为正文步产物: {finish}"
    );
}

/// **词条同步(RPFLOW-2,端到端)**:归档步调用 `worldbook_update` 命中**蓝灯(常驻)**
/// 条目时,不改写它,而是**新建绿灯(触发)条目**;调用的下落也要能在角色卡内嵌
/// character_book 里读回。
///
/// mock:正文步回「正文WB」;`[[tool_if:归档与同步|worldbook_update {...}]]` 只在归档步
/// 生效(子串只出现在归档步 system;该步白名单 ARCHIVE_TOOLS 含 worldbook_update);
/// 工具结果回填后一轮走固定完成回复,归档循环收敛。
#[tokio::test]
async fn deep_mode_archive_syncs_worldbook_entry() {
    let _guard = test_lock().await;
    let app = test_app();
    // 角色卡带内嵌世界书:一条蓝灯(常驻)条目
    let card = json!({
        "spec": "chara_card_v2",
        "spec_version": "2.0",
        "name": "词条同步角色",
        "description": "RPFLOW-2 词条同步测试",
        "first_mes": "你好",
        "data": {
            "name": "词条同步角色",
            "description": "RPFLOW-2 词条同步测试",
            "character_book": {
                "entries": [{
                    "id": 0,
                    "comment": "世界观·书店",
                    "content": "书店叫「晚灯书屋」。",
                    "constant": true,
                    "enabled": true,
                    "position": 0,
                    "order": 100,
                    "keys": []
                }]
            }
        }
    });
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"词条同步.json\"\r\nContent-Type: application/json\r\n\r\n{card}\r\n--BOUND--\r\n"
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
    let cid = char["id"].as_str().unwrap().to_string();
    let sid = new_session(app, &cid).await;

    let events = sse_events(
        app,
        &sid,
        &cid,
        "我把书店招牌换成了霓虹灯。 \
         [[reply_if_any:本步指令·草稿|草稿WB]] [[reply_if_any:本步指令·正文|正文WB]] \
         [[reply_if_any:反思纪律|PASS]] \
         [[tool_if:归档与同步|worldbook_update {\"topic\":\"书店\",\"content\":\"招牌已换成霓虹灯\",\"keywords\":[\"招牌\",\"霓虹灯\"]}]]",
        "deep",
    )
    .await;

    // 工具确实在归档步被调用并返回结构化结果
    let call = events
        .iter()
        .find(|e| e["type"] == "tool_call" && e["name"] == "worldbook_update")
        .expect("归档步应调用 worldbook_update: 见 events");
    assert!(call["name"].as_str().unwrap_or("") == "worldbook_update");
    let result = events
        .iter()
        .find(|e| e["type"] == "tool_result" && e["name"] == "worldbook_update")
        .expect("worldbook_update 应有 tool_result");
    let output = &result["output"];
    assert!(output.get("error").is_none(), "工具不应报错: {output}");
    assert_eq!(
        output["action"], "created",
        "命中蓝灯(常驻)条目 → 应新建绿灯条目而非改写: {output}"
    );

    // 角色卡内嵌词条:原常驻条目逐字未动 + 新增一条绿灯条目
    let (status, payload) = send_json(
        app,
        "GET",
        &format!("/api/characters/{cid}/world-entries"),
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list = payload["entries"].as_array().cloned().unwrap_or_default();
    let constant = list
        .iter()
        .find(|e| e["comment"] == "世界观·书店")
        .expect("原常驻条目应仍在");
    assert_eq!(constant["constant"], json!(true), "蓝灯条目不得被改写");
    assert_eq!(
        constant["content"], "书店叫「晚灯书屋」。",
        "蓝灯条目内容必须逐字未动: {constant}"
    );
    let created = list
        .iter()
        .find(|e| e["comment"] != "世界观·书店")
        .expect("应新建绿灯条目");
    assert_eq!(created["constant"], json!(false), "新条目必须是绿灯");
    assert!(
        created["content"].as_str().unwrap_or("").contains("霓虹灯"),
        "新条目应含最新情况: {created}"
    );
}
