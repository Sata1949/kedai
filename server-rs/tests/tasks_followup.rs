// 任务模式集成测试 · 任务追加指令（followup：追加 / 状态闸 / 保留部分成果 / 替换）。
//
// 本文件由原 `tests/tasks.rs`（176KB / 65 用例）按 API 域拆分而来（QUALITY-FIX Q1-1,2026-09-27）：
// 纯移动、不改行为。**拆分的实质收益是测试隔离**——`tests/` 下每个 `.rs` 是一个独立测试进程，
// `build_test_app()` 的 `DATA_DIR`（`%TEMP%\kedai-test-<pid>`）与 `OnceLock` 单例 app 随
// 文件独立，原先「同一进程内共享 settings.json / 流程库」的顺序耦合由此消失。**别再把它们合回一个文件。**
//
// 测试辅助函数按「谁用谁带」复制（仓库既有体例：40 个测试文件里 36 个各自持有 `test_app`），
// 刻意不建 `tests/common` 共享层：那会让 count-tests 的集成文件数口径虚高，也会给多会话并行
// 改测试制造新的争用点，与本次拆分的用意相悖。
//
// 共用前提：`build_test_app()`（mock 连接器 + 临时数据目录 + 免鉴权）；mock 钩子 `[[reply:内容]]`
// 让规划器返回 JSON 计划、子智能体返回指定结果，实现确定性端到端。

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

/// 轮询任务详情直到终态(done/partial/error/ended)或超时;返回最终 status 与详情
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    for _ in 0..50 {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let st = json["task"]["status"].as_str().unwrap_or("").to_string();
        if st == "done" || st == "partial" || st == "error" || st == "ended" {
            return (st, json);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("任务 {id} 未在超时内到达终态");
}

/// 创建指定模式的任务并返回 id
async fn create_task_with_mode(app: &axum::Router, title: &str, mode: &str) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": mode }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 轮询任务详情直到出现目标状态或超时
async fn wait_status(app: &axum::Router, id: &str, target: &str) -> Value {
    for _ in 0..50 {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        if json["task"]["status"].as_str() == Some(target) {
            return json;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("任务 {id} 未在超时内到达状态 {target}");
}

/// followup 全链路:solo 任务完成后追加指令 → 任务回 running 续跑(用户输入落
/// task_messages 后 spawn solo 续跑:原目标 + 上轮 result + 追加指令)→ 终态回 done;
/// result 追加「**追加 1:**」段且保留首轮成果;第二轮追加段序号为「追加 2」。
/// 实跑问题 1 后 messages 语义扩展为「完整对话记录」:创建落 user(kind=goal)、
/// 首轮产出落 assistant(kind=result)、每轮 followup 落 user+assistant(kind=followup),
/// 故一轮追加后 messages = 4 行、两轮追加累计 6 行。
/// 剧本说明:首轮 title 不带钩子(产出 = mock 默认回复),followup 指令带
/// [[reply:...]] 钩子 —— 续跑 user 消息(原目标 + 上轮 result + 追加指令)中
/// 首个 [[reply: 即本论钩子(mock reply 钩子按首个出现命中),产出确定。
#[tokio::test]
async fn task_followup_appends_to_result_and_messages() {
    let app = test_app();

    let id = create_task_with_mode(app, "写一段关于秋天的短文", "solo").await;
    // 实跑问题 1:创建即落 user 目标消息(对话记录首条)
    let (_, created) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    let created_msgs = created["messages"].as_array().expect("详情应含 messages");
    assert_eq!(created_msgs.len(), 1, "创建应落 1 条目标消息: {created}");
    assert_eq!(created_msgs[0]["role"], "user");
    assert_eq!(created_msgs[0]["kind"], "goal");
    assert_eq!(created_msgs[0]["content"], "写一段关于秋天的短文");

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "首轮应完成: {detail}");
    let first_result = detail["task"]["result"].as_str().unwrap_or("").to_string();
    assert!(
        first_result.contains("模拟回复"),
        "首轮产出应为 mock 默认回复: {first_result}"
    );
    // 实跑问题 1:首轮成果落 assistant 消息(kind=result),供前端逐轮气泡渲染
    let after_run = detail["messages"].as_array().expect("详情应含 messages");
    assert_eq!(
        after_run.len(),
        2,
        "首轮后应为「目标 + 成果」两行: {detail}"
    );
    assert_eq!(after_run[0]["kind"], "goal");
    assert_eq!(after_run[1]["role"], "assistant");
    assert_eq!(after_run[1]["kind"], "result");
    assert_eq!(
        after_run[1]["content"], first_result,
        "成果消息应存首轮产出全文: {detail}"
    );

    // ===== 第一轮追加 =====
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/followup"),
        json!({ "content": "[[reply:追加成果甲]] 再补充一点秋色" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "终态任务 followup 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "追加续跑后应回 done: {detail}");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains(&first_result),
        "result 应保留首轮成果: {result}"
    );
    assert!(
        result.contains("**追加 1:**"),
        "result 应含「追加 1」段标: {result}"
    );
    assert!(
        result.contains("再补充一点秋色"),
        "段标后应带指令概要: {result}"
    );
    assert!(
        result.contains("追加成果甲"),
        "result 应含本轮追加产出: {result}"
    );
    // 段序:首轮成果在前,追加段在后
    let first_idx = result.find(&first_result).unwrap_or(usize::MAX);
    let append_idx = result.find("**追加 1:**").unwrap_or(usize::MAX);
    assert!(first_idx < append_idx, "首轮成果应在追加段之前: {result}");

    // messages 四行:目标 + 首轮成果 + user 指令 + assistant 产出(created_at 升序)
    let messages = detail["messages"]
        .as_array()
        .expect("详情应含 messages 数组");
    assert_eq!(messages.len(), 4, "一轮追加应为 4 行消息: {messages:?}");
    assert_eq!(messages[0]["kind"], "goal");
    assert_eq!(messages[1]["kind"], "result");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["kind"], "followup");
    assert!(
        messages[2]["content"]
            .as_str()
            .unwrap_or("")
            .contains("再补充一点秋色"),
        "user 消息应存指令原文: {messages:?}"
    );
    assert_eq!(messages[3]["role"], "assistant");
    assert_eq!(messages[3]["kind"], "followup");
    assert_eq!(
        messages[3]["content"], "追加成果甲",
        "assistant 消息应存产出全文: {messages:?}"
    );

    // ===== 第二轮追加(序号递增;第二轮不带钩子,产出为 mock 默认回复) =====
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/followup"),
        json!({ "content": "再润色一遍结尾" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "第二轮 followup 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "第二轮追加后应回 done: {detail}");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("**追加 2:**"),
        "第二轮段标应为「追加 2」: {result}"
    );
    assert!(
        result.contains("再润色一遍结尾"),
        "第二轮段标应带本轮指令概要: {result}"
    );
    let messages = detail["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 6, "两轮追加应累计 6 行消息: {messages:?}");
    assert_eq!(messages[4]["role"], "user");
    assert_eq!(messages[4]["kind"], "followup");
    assert_eq!(messages[5]["role"], "assistant");

    // 删除任务级联清消息(FK ON DELETE CASCADE;DB 层断言见 migration.rs 专测)
    let status = send_empty(app, "DELETE", &format!("/api/tasks/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// followup 状态门禁:仅终态(done/partial/error/ended)可追加;
/// pending/running/planned 409(code=CONFLICT);不存在 404(NOT_FOUND);空指令 400(VALIDATION)。
/// ended 终态可追加(stop 后仍可继续对话)。
#[tokio::test]
async fn task_followup_state_gate() {
    let app = test_app();

    // 404:任务不存在
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks/不存在/followup",
        json!({ "content": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "不存在应 404: {json}");
    assert_eq!(json["code"], "NOT_FOUND");

    // 400:空指令(校验先于状态门禁)
    let id = create_task_with_mode(app, "门禁测试目标", "solo").await;
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/followup"),
        json!({ "content": "   " }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空指令应 400: {json}");
    assert_eq!(json["code"], "VALIDATION");

    // 400:非法 mode(严格解析,与 task_mode 同口径;前置于状态门禁)
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/followup"),
        json!({ "content": "补充", "mode": "rewrite" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非法 mode 应 400: {json}");
    assert_eq!(json["code"], "VALIDATION");

    // 409:pending(未启动)
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/followup"),
        json!({ "content": "补充" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "pending 应 409: {json}");
    assert_eq!(json["code"], "CONFLICT");

    // 409:running(默认慢速回复留出窗口)
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let mut saw_running = false;
    for _ in 0..100 {
        let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        if detail["task"]["status"].as_str() == Some("running") {
            saw_running = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(saw_running, "应观测到 running 态");
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/followup"),
        json!({ "content": "补充" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "running 应 409: {json}");
    assert_eq!(json["code"], "CONFLICT");

    // stop → ended 终态可追加(stop 后仍可继续对话;产出 = mock 默认回复)
    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (st, _) = wait_terminal(app, &id).await;
    assert_eq!(st, "ended", "stop 后应 ended");
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/followup"),
        json!({ "content": "结束后继续写" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ended 终态应可追加: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "ended 追加续跑后应回 done: {detail}");
    assert!(
        detail["task"]["result"]
            .as_str()
            .unwrap_or("")
            .contains("**追加 1:**"),
        "ended 追加应产出追加段: {detail}"
    );

    // 409:planned(plan 模式待批准)
    let title = r#"[[reply:[{"name":"步骤一","goal":"写第一段"}] ]]"#;
    let pid = create_task_with_mode(app, title, "plan").await;
    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{pid}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    wait_status(app, &pid, "planned").await;
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{pid}/followup"),
        json!({ "content": "补充" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "planned 应 409(待批准请走批准/对话): {json}"
    );
    assert_eq!(json["code"], "CONFLICT");
}

/// partial 历史不被抹平:原 partial 任务(含失败步骤)追加续跑完成后仍回 partial
///(失败步骤历史仍存),追加段照常入 result;done/error/ended 追加后回 done
///(done/ended 路径见上两例)。
#[tokio::test]
async fn task_followup_keeps_partial_status() {
    let app = test_app();

    // legacy 两步一成一败 → partial。规划钩子用 reply_if(仅匹配规划器 system
    // 「任务规划器」):followup 续跑轮的 system 是「任务执行者」不命中它,本轮
    // 唯一命中的是追加指令里的 [[reply:]] —— 若用无条件 [[reply:]] 出计划,
    // 续跑 user 消息(含 title 原文)会先命中旧钩子,追加产出被污染(与
    // task_plan_resume_step_error_carries_reason_text 剧本说明同款规避)。
    let title = concat!(
        r#"[[reply_if:任务规划器|[{"name":"成功步","goal":"写一段正常内容"},"#,
        r#"{"name":"失败步","goal":"\u005b\u005bfail:上游抖动\u005d\u005d"}] ]]"#,
        " partial 追加目标"
    );
    let id = create_task_with_mode(app, title, "legacy").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, _) = wait_terminal(app, &id).await;
    assert_eq!(st, "partial", "前置应为 partial");

    // 追加(钩子续跑产出确定文本;追加期间 running,完成回 partial)
    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/followup"),
        json!({ "content": "[[reply:partial追加成果]] 把失败步补上" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "partial 应可追加: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "partial",
        "含失败步骤历史的任务追加后仍应为 partial: {detail}"
    );
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("**追加 1:**"),
        "result 应含追加段: {result}"
    );
    assert!(
        result.contains("partial追加成果"),
        "result 应含追加产出: {result}"
    );
    let messages = detail["messages"].as_array().unwrap();
    assert_eq!(
        messages.len(),
        4,
        "目标 + 首轮成果 + 一轮追加两行 = 4 行: {messages:?}"
    );
}

// ==================== 批次 R2b:批准环节规划对话(plan-chat) ====================

/// 2026-09-10 六模式实跑修复(F5):followup replace 模式整体替换结果。
/// 背景:append 语义下「压缩到 200 字」这类指令无法表达——新产出以追加段附加,
/// 原文仍在(实测 result 反而变长)。replace 模式用新产出整体替换 result(段标
/// 「修订 N」),旧内容不保留;messages 仍按 user/assistant 各一行落库。
/// 用两个独立任务分别验证 replace 与默认 append,避免 title 内的回复钩子
/// 在多轮之间互相污染(mock [[reply:]] 按最后一条 user 消息中首个命中)。
#[tokio::test]
async fn task_followup_replace_mode_replaces_result() {
    let app = test_app();

    // ===== 任务 A:replace 整体替换 =====
    // 首轮无钩子 → mock 默认回复(内含目标前 60 字,作为「旧内容」标记)
    let a = create_task_with_mode(app, "独有标记QAQ 写一段内容", "solo").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{a}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &a).await;
    assert_eq!(st, "done");
    assert!(
        detail["task"]["result"]
            .as_str()
            .unwrap_or("")
            .contains("独有标记QAQ"),
        "首轮应含旧内容标记: {detail}"
    );

    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{a}/followup"),
        json!({ "content": "[[reply:替换后成果]]", "mode": "replace" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "replace 追加应 200: {json}");
    let (st, detail) = wait_terminal(app, &a).await;
    assert_eq!(st, "done", "replace 完成后应为 done: {detail}");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("**修订 1:**"),
        "replace 模式应用「修订 N」段标: {result}"
    );
    assert!(
        result.contains("替换后成果"),
        "result 应含新版产出: {result}"
    );
    assert!(
        !result.contains("独有标记QAQ"),
        "replace 模式旧结果不得保留: {result}"
    );
    let messages = detail["messages"].as_array().unwrap();
    // 目标 + 首轮成果 + 一轮追加两行 = 4 行
    assert_eq!(
        messages.len(),
        4,
        "目标 + 首轮成果 + 一轮追加两行 = 4 行: {messages:?}"
    );
    assert_eq!(messages[0]["kind"], "goal");
    assert_eq!(messages[1]["kind"], "result");

    // ===== 任务 B:缺省 mode = append,累积而非替换 =====
    let b = create_task_with_mode(app, "第二标记QBQ 写点东西", "solo").await;
    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{b}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (st, _) = wait_terminal(app, &b).await;
    assert_eq!(st, "done");

    let (status, json) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{b}/followup"),
        json!({ "content": "[[reply:追加内容段]]" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "缺省 mode 追加应 200: {json}");
    let (st, detail) = wait_terminal(app, &b).await;
    assert_eq!(st, "done");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("**追加 1:**") && result.contains("第二标记QBQ"),
        "缺省 append 应保留旧结果并附加段: {result}"
    );
    assert!(
        result.contains("追加内容段"),
        "append 应含本轮产出: {result}"
    );
}
