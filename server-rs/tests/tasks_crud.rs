// 任务模式集成测试 · 任务 CRUD / 终态 / 停止 / 汇总回退 / usage / 执行者库。
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

/// 创建任务并返回 id
async fn create_task(app: &axum::Router, title: &str) -> String {
    let (status, json) = send_json(app, "POST", "/api/tasks", json!({ "title": title })).await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
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

/// 上传四段人设字段各带唯一标记的角色卡(R3a 测试夹具),返回角色 id。
/// 平铺(V1 风格)顶层字段:persona_style 读取口径为 data_raw 顶层
/// (description 入 CharacterRecord.description,其余经 data_raw 顶层取;
/// V2 卡的 data 包装层不在本批读取语义内,不动既有行为)。
async fn upload_persona_marked_character(app: &axum::Router) -> String {
    let card = json!({
        "name": "人设标记角色",
        "description": "R3A-DESC-MARK 人设正文",
        "personality": "R3A-PERS-MARK 人格",
        "scenario": "R3A-SCEN-MARK 情境",
        "mes_example": "R3A-MESEX-MARK 文风示例",
        "first_mes": "你好"
    });
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"persona-mark.json\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        card
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
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "上传角色卡应 2xx,实际 {status}: {json}"
    );
    json["character"]["id"]
        .as_str()
        .or_else(|| json["id"].as_str())
        .expect("上传响应应含角色 id")
        .to_string()
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

/// 断言 usage_total 与 task_llm_calls 逐行 token 求和一致(修复后为强不变量)。
fn assert_usage_matches_calls(detail: &Value, calls: &Value) {
    let sum = |field: &str| -> i64 {
        calls["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c[field].as_i64().unwrap_or(0))
            .sum()
    };
    let usage = &detail["usage_total"];
    assert_eq!(
        usage["prompt_tokens"].as_i64().unwrap_or(-1),
        sum("prompt_tokens"),
        "usage_total.prompt_tokens 应等于调用明细求和: usage={usage} calls={calls}"
    );
    assert_eq!(
        usage["completion_tokens"].as_i64().unwrap_or(-1),
        sum("completion_tokens"),
        "usage_total.completion_tokens 应等于调用明细求和: usage={usage} calls={calls}"
    );
}

/// 建一个执行者,返回 id
async fn create_executor(app: &axum::Router, name: &str, instruction: &str) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/task-executors",
        json!({ "config": { "name": name, "instruction": instruction } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建执行者应 200: {json}");
    json["saved"]["id"]
        .as_str()
        .expect("响应应含 saved.id")
        .to_string()
}

#[tokio::test]
async fn task_crud_roundtrip() {
    let app = test_app();

    // 空标题被拒
    let (status, json) = send_json(app, "POST", "/api/tasks", json!({ "title": "   " })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空标题应 400: {json}");

    // 创建
    let id = create_task(app, "写一篇短文").await;

    // 列表包含
    let (status, json) = send_json(app, "GET", "/api/tasks", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = json["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert!(ids.contains(&id.as_str()), "列表应包含新任务");

    // 详情:status=pending,plan 空,subtasks 空
    let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["task"]["status"], "pending");
    assert_eq!(json["task"]["plan"].as_array().unwrap().len(), 0);
    assert_eq!(json["subtasks"].as_array().unwrap().len(), 0);

    // 删除
    let status = send_empty(app, "DELETE", &format!("/api/tasks/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 删除后 404
    let (status, _) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 端到端:mock 规划器返回 2 步,各子智能体返回结果,汇总后 status=done。
#[tokio::test]
async fn task_run_to_done() {
    let app = test_app();

    // title 内嵌 [[reply:...]] 让 mock 规划器直接返回 JSON 计划(2 步,goal 为普通文本);
    // 步骤 goal 无特殊标记,mock 子智能体返回默认非空回复。
    // 注意:mock reply 钩子在首个 "]]" 截断,故数组闭合 "]" 与标记闭合 "]]" 之间须留空格。
    let title =
        r#"[[reply:[{"name":"步骤一","goal":"写第一段"},{"name":"步骤二","goal":"写第二段"}] ]]"#;
    let id = create_task(app, title).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成,详情: {detail}");

    // 计划 2 步均 done
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2, "计划应拆为 2 步");
    for step in plan {
        assert_eq!(step["status"], "done");
    }

    // 子任务 2 条均 done 且结果非空
    let subtasks = detail["subtasks"].as_array().unwrap();
    assert_eq!(subtasks.len(), 2);
    for st in subtasks {
        assert_eq!(st["status"], "done");
        assert!(!st["result"].as_str().unwrap().is_empty());
    }

    // 最终结果非空
    assert!(!detail["task"]["result"].as_str().unwrap().is_empty());
}

/// 空输出重试(WP2):步骤 goal 触发 [[empty]](mock 只回 Usage 无 Token),
/// 执行器空内容自动重试一次仍空 → 步骤 error 且文案含「空内容」与 finish_reason;
/// 汇总照常产出 → 任务终态 partial(WP3:含 error 步骤但成果已产出)。
#[tokio::test]
async fn task_step_empty_output_retries_then_errors() {
    let app = test_app();

    // 规划器返回 1 步;goal 内 [[empty]] 须 unicode 转义:
    // 一是避免 mock reply 钩子在首个 "]]" 提前截断计划 JSON,
    // 二是避免规划/汇总消息原文含 "[[empty]]" 误触发空输出钩子(钩子按各自消息分别匹配)。
    let title = r#"[[reply:[{"name":"空步骤","goal":"\u005b\u005bempty\u005d\u005d"}] ]]"#;
    let id = create_task(app, title).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "partial",
        "空步骤 error + 汇总成功应为部分完成,详情: {detail}"
    );

    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0]["status"], "error", "空输出步骤应 error: {detail}");
    let result = plan[0]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("空内容") && result.contains("finish_reason"),
        "步骤错误文案应含「空内容」与 finish_reason,实际: {result}"
    );
    // 提交 3 · D6:文案必须可操作(用户知道该调什么),且思考占比不可伪造
    assert!(
        result.contains("提高单次生成上限") && result.contains("非推理模型"),
        "错误文案应给出可操作建议,实际: {result}"
    );
    assert!(
        result.contains("思考占比未知"),
        "mock 未上报 completion token → 应明说未知而不是报假的 0%: {result}"
    );

    // 子任务同样 error 且带原因
    let subtasks = detail["subtasks"].as_array().unwrap();
    assert_eq!(subtasks.len(), 1);
    assert_eq!(subtasks[0]["status"], "error");
    assert!(
        subtasks[0]["error"]
            .as_str()
            .unwrap_or("")
            .contains("空内容"),
        "子任务错误应含「空内容」,实际: {detail}"
    );
}

/// 部分完成终态(WP3):两步中一步失败([[fail:]] 钩子)、一步成功,
/// 汇总仍产出 → 任务终态 partial 而非 done/error。
#[tokio::test]
async fn task_partial_when_step_fails() {
    let app = test_app();

    // [[fail:...]] 同样须 unicode 转义(避免 reply 截断 + 避免规划消息误触发 fail 钩子)
    let title = r#"[[reply:[{"name":"成功步","goal":"写一段正常内容"},{"name":"失败步","goal":"\u005b\u005bfail:上游抖动\u005d\u005d"}] ]]"#;
    let id = create_task(app, title).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "partial", "部分步骤失败应为部分完成,详情: {detail}");

    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0]["status"], "done", "第一步应成功: {detail}");
    assert_eq!(plan[1]["status"], "error", "第二步应失败: {detail}");
    assert!(
        plan[1]["result"]
            .as_str()
            .unwrap_or("")
            .contains("上游抖动"),
        "失败步骤应保留上游错误原文,实际: {detail}"
    );
    // 汇总成果照常产出
    assert!(!detail["task"]["result"].as_str().unwrap_or("").is_empty());
}

/// 任务 usage 落库与聚合(WP4):一次完整执行(规划+2 步骤+汇总)后,
/// 详情接口 usage_total 聚合非零,全局累计接口口径一致且不小于单任务值。
#[tokio::test]
async fn task_usage_recorded_and_aggregated() {
    let app = test_app();

    let title =
        r#"[[reply:[{"name":"步骤一","goal":"写第一段"},{"name":"步骤二","goal":"写第二段"}] ]]"#;
    let id = create_task(app, title).await;
    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成,详情: {detail}");

    // 单任务累计:规划+步骤+汇总均有 prompt/completion 消耗(mock 按字符数估算,恒 > 0)
    let usage = &detail["usage_total"];
    let p = usage["prompt_tokens"].as_i64().unwrap_or(0);
    let c = usage["completion_tokens"].as_i64().unwrap_or(0);
    assert!(
        p > 0,
        "任务 prompt_tokens 应 > 0(规划/步骤/汇总均已落库): {detail}"
    );
    assert!(c > 0, "任务 completion_tokens 应 > 0: {detail}");
    assert_eq!(
        usage["reasoning_tokens"].as_i64().unwrap_or(-1),
        0,
        "mock 无推理 token"
    );

    // 全局累计接口:存在且不小于该任务累计(同进程其他测试也可能写入,共享 DB)
    let (status, total) = send_json(app, "GET", "/api/tasks/usage-total", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let gp = total["usage_total"]["prompt_tokens"].as_i64().unwrap_or(0);
    let gc = total["usage_total"]["completion_tokens"]
        .as_i64()
        .unwrap_or(0);
    assert!(
        gp >= p && gc >= c,
        "全局累计应不小于单任务累计: {total} vs {detail}"
    );
}

/// 汇总路径空输出 + 步骤已产出(提交 2 部分成果兜底):
/// 规划与步骤正常、汇总返回空([[empty_if:任务汇总者]] 钩子:仅当 system 含
/// 「任务汇总者」时返回空,规划器/执行者 system 不含该子串故不受影响)→ 汇总按
/// finish_reason 分级重试一次仍空。
/// 旧行为(批次 WP6):走 set_error → error 终态 + result 空,已完成步骤的产出被整体
/// 丢弃(真实模型实测 legacy/写作 就是这样丢掉约 3900 字)。现行为:能拼出成果即
/// partial + result = 已完成步骤的确定性拼装,error 仍保留汇总失败原因。
/// 对照用例 task_summary_failure_without_step_outputs_stays_error:无产出时不得伪造。
#[tokio::test]
async fn task_summary_failure_falls_back_to_step_outputs() {
    let app = test_app();

    // [[empty_if:]] 置于 [[reply:]] 之后:reply 仍在首个 "]]" 截断出计划 JSON,
    // empty_if 不参与截断;汇总 user 消息含整个 title(带钩子)故被命中。
    let title = r#"[[reply:[{"name":"步骤一","goal":"写第一段"}] ]][[empty_if:任务汇总者]]"#;
    let id = create_task(app, title).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "partial",
        "汇总失败但有已完成步骤应部分完成(成果不得丢),详情: {detail}"
    );

    // 原因仍可诊断:error 保留汇总失败文案(含「空内容」与 finish_reason)
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("空内容") && error.contains("finish_reason"),
        "汇总失败原因应保留在 error,实际: {error}"
    );

    // 步骤本身成功(done);成果 = 「## 步骤名」+ 该步产出
    let plan = detail["task"]["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0]["status"], "done", "步骤应成功,失败在汇总: {detail}");
    let step_result = plan[0]["result"].as_str().unwrap_or("");
    assert!(!step_result.is_empty(), "步骤应有产出: {detail}");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("## 步骤一"),
        "result 应含步骤标题: {result}"
    );
    assert!(
        result.contains(step_result),
        "result 应含步骤产出: {result}"
    );
    assert!(
        !result.contains("## 最终计划"),
        "模式脚手架段不得混进成果: {result}"
    );

    let subtasks = detail["subtasks"].as_array().unwrap();
    assert_eq!(subtasks.len(), 1);
    assert_eq!(subtasks[0]["status"], "done", "子任务应成功: {detail}");
}

/// 汇总失败且**无任何已完成步骤**时不得伪造成果(提交 2 的反面对照):
/// 步骤全失败(空输出)+ 汇总同样失败 → 终态仍是 error、result 仍为空,
/// 与旧行为逐字一致。两条合起来钉住「有成果才 partial,没成果不假装」。
/// 钩子设计:步骤失败用 **goal 内嵌 `[[empty]]`**(转义写入规划 JSON,serde 还原;
/// 规划器 user 消息只见转义序列故不受影响),汇总失败用 `[[empty_if:任务汇总者]]`,
/// 二者互不干扰——mock 的 `[[empty_if:]]` 只认首个标记,故不能靠两条 empty_if 区分阶段。
#[tokio::test]
async fn task_summary_failure_without_step_outputs_stays_error() {
    let app = test_app();

    let title = concat!(
        r#"[[reply:[{"name":"步骤一","goal":"\u005b\u005bempty\u005d\u005d 写第一段"},"#,
        r#"{"name":"步骤二","goal":"\u005b\u005bempty\u005d\u005d 写第二段"}] ]]"#,
        "[[empty_if:任务汇总者]]"
    );
    let id = create_task(app, title).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(
        st, "error",
        "无任何已完成步骤时仍应为 error(不得伪造成果),详情: {detail}"
    );
    let plan = detail["task"]["plan"].as_array().unwrap();
    for step in plan {
        assert_eq!(step["status"], "error", "步骤应全部失败: {detail}");
    }
    assert!(
        detail["task"]["result"].as_str().unwrap_or("").is_empty(),
        "无成果不得伪造 result: {detail}"
    );
}

/// 停止运行中任务(WP6):任务进入 running 且首个子任务执行中时 stop →
/// 任务终态 ended;执行中子任务一并 ended,不得残留 running/pending。
/// (task_stop_pending 仅覆盖 pending 态,本测试覆盖 running 态。)
/// 时序依据:步骤 goal 无钩子,mock 走逐字流式默认回复(约 8ms/字符,单步 1s+),
/// 为「轮询观测 running 子任务 → stop」留出充足时间窗。
#[tokio::test]
async fn task_stop_running() {
    let app = test_app();

    let title =
        r#"[[reply:[{"name":"步骤一","goal":"写第一段"},{"name":"步骤二","goal":"写第二段"}] ]]"#;
    let id = create_task(app, title).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    // 轮询直到任务 running 且有子任务进入 running(执行中)
    let mut saw_running_subtask = false;
    for _ in 0..200 {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let task_running = json["task"]["status"].as_str() == Some("running");
        let sub_running = json["subtasks"]
            .as_array()
            .map(|s| s.iter().any(|st| st["status"].as_str() == Some("running")))
            .unwrap_or(false);
        if task_running && sub_running {
            saw_running_subtask = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(saw_running_subtask, "应观测到 running 中的子任务再 stop");

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "running 态 stop 应 200");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "ended", "stop 后任务应 ended,详情: {detail}");
    let subtasks = detail["subtasks"].as_array().unwrap();
    assert!(!subtasks.is_empty(), "stop 时已存在执行中子任务: {detail}");
    for st in subtasks {
        let s = st["status"].as_str().unwrap_or("");
        assert!(
            s != "running" && s != "pending",
            "子任务不得残留 running/pending: {detail}"
        );
    }
    assert!(
        subtasks
            .iter()
            .any(|s| s["status"].as_str() == Some("ended")),
        "执行中子任务应被 stop 置为 ended: {detail}"
    );
}

/// 停止时保住已完成步骤的产出(提交 2 终态兜底 · finalize_run ended 分支):
/// 3 步任务,首步 done 后 stop → 终态仍是 ended(状态语义不变),但 result 已写入
/// 已完成步骤的确定性拼装,不再出现「点了停止 → 之前的产出全没了」。
#[tokio::test]
async fn task_stop_running_keeps_done_step_output() {
    let app = test_app();

    let title = r#"[[reply:[{"name":"步骤一","goal":"写第一段"},{"name":"步骤二","goal":"写第二段"},{"name":"步骤三","goal":"写第三段"}] ]]"#;
    let id = create_task(app, title).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    // 轮询直到首步 done(legacy 每步完成即回写 plan);步骤 goal 无钩子,mock 走逐字
    // 流式默认回复(约 8ms/字符 ≈ 1s/步),后续两步各留一个窗口,足够在第二步执行中 stop。
    let mut first_done = false;
    for _ in 0..200 {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        if json["task"]["plan"][0]["status"].as_str() == Some("done") {
            first_done = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(first_done, "应观测到首步 done 再 stop");

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "running 态 stop 应 200");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "ended", "stop 后任务应 ended,详情: {detail}");

    let plan = detail["task"]["plan"].as_array().unwrap();
    let step0 = plan[0]["result"].as_str().unwrap_or("");
    assert!(!step0.is_empty(), "首步应有产出: {detail}");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        !result.is_empty(),
        "停止后已完成步骤的产出不得丢(终态兜底): {detail}"
    );
    assert!(
        result.contains("## 步骤一"),
        "result 应含已完成步骤标题: {result}"
    );
    assert!(
        result.contains(step0),
        "result 应含已完成步骤产出: {result}"
    );
}

/// 停止一个 pending 任务:status 转 ended。
#[tokio::test]
async fn task_stop_pending() {
    let app = test_app();
    let id = create_task(app, "待停止的任务").await;

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/stop"), json!({})).await;
    assert_eq!(status, StatusCode::OK);

    let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["task"]["status"], "ended");
}

/// 不存在的任务:run/stop/get 均 4xx。
#[tokio::test]
async fn task_not_found() {
    let app = test_app();
    let (status, _) = send_json(app, "POST", "/api/tasks/不存在/run", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send_json(app, "POST", "/api/tasks/不存在/stop", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send_json(app, "GET", "/api/tasks/不存在", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 创建任务:未知 task_mode 严格拒绝(400),容错回退只用于 DB 读侧。
#[tokio::test]
async fn task_create_unknown_mode_rejected() {
    let app = test_app();
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": "某任务", "task_mode": "bogus" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未知模式应 400: {json}");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("未知任务模式"),
        "错误文案应含「未知任务模式」: {json}"
    );
}

// ==================== 批次 4.3b:六模式(multi / custom / team) ====================

/// 问题①:solo 任务经 mock [[finish:length|...]] 钩子模拟 max_tokens 截断,
/// 任务照样 done(半截文本也是产出),但 GET /api/tasks/{id}/calls 的 agent 行
/// 必须带 finish_reason="length" 标记——修复前该列不存在,截断与正常收尾无从区分。
/// 钩子内容取到首个 "]]"(与 [[reply:]] 同截断语义),故内容内不得含 "]]"。
#[tokio::test]
async fn task_llm_call_finish_reason_marks_length_truncation() {
    let app = test_app();

    let title = "[[finish:length|这段成果在 max_tokens 处被截断,后半]] 截断观测目标";
    let id = create_task_with_mode(app, title, "solo").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");

    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "截断产出仍应 done(半截文本也是产出): {detail}");
    assert!(
        detail["task"]["result"]
            .as_str()
            .unwrap_or("")
            .contains("这段成果在 max_tokens 处被截断,后半"),
        "结果应为半截文本: {detail}"
    );

    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let calls = calls["calls"].as_array().unwrap();
    let agent = calls
        .iter()
        .find(|c| c["phase"] == "agent")
        .expect("应有 phase=agent 调用行");
    assert_eq!(
        agent["finish_reason"].as_str(),
        Some("length"),
        "截断调用行必须带 finish_reason=length 标记: {agent}"
    );
}

/// 问题①(对照组):正常结束的调用 finish_reason="stop"(mock 文本路径补发 Finish{stop});
/// 同时锁定「旧行读取兼容」——finish_reason 列对旧数据默认 ''(未知),新行必有值。
/// legacy 三段式(planner/step/summarize)全部调用行逐一断言。
#[tokio::test]
async fn task_llm_call_finish_reason_stop_on_normal_completion() {
    let app = test_app();

    let title = r#"[[reply:[{"name":"步骤一","goal":"写第一段"}] ]]"#;
    let id = create_task_with_mode(app, title, "legacy").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, _) = wait_terminal(app, &id).await;
    assert_eq!(st, "done");

    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let calls = calls["calls"].as_array().unwrap();
    assert!(!calls.is_empty(), "应有调用追踪行");
    for c in calls {
        // 字段必须存在(旧客户端缺列时 serde 反序列化不得失败由服务端 String 保证)
        let reason = c["finish_reason"]
            .as_str()
            .unwrap_or_else(|| panic!("调用行缺 finish_reason 字段: {c}"));
        assert_eq!(
            reason,
            "stop",
            "正常完成的调用 finish_reason 应为 stop(phase={}): {c}",
            c["phase"].as_str().unwrap_or("?")
        );
    }
}

/// 2026-09-10 六模式实跑修复(F3):调用记账口径统一。
/// 验收标准:task_llm_calls 各行 token 求和 == 详情接口 usage_total(逐字段)。
/// 覆盖两个此前漏计的缺口:规划器侦察轮、截断自愈行。
#[tokio::test]
async fn task_usage_total_matches_call_rows_with_scout_and_heal() {
    let app = test_app();

    // 场景 A:规划器侦察轮(plan 模式,plan_scout_loop 记侦察轮 usage)
    let title = r#"[[tool:read {"type":"file","name":"notes.md"}]] [[reply:[{"name":"侦察步","goal":"基于收集的信息写作"}] ]] 侦察记账目标"#;
    let id = create_task_with_mode(app, title, "plan").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let _ = wait_status(app, &id, "planned").await;
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    let planner_rows = calls["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["phase"] == "planner")
        .count();
    assert!(planner_rows >= 2, "应含侦察轮与计划轮: {calls:?}");
    assert_usage_matches_calls(&detail, &calls);

    // 场景 B:截断自愈(solo 工具循环单轮被截断 → 翻倍重发;heal 行记 usage)
    let title = r#"[[tool_raw:read {"type":"file","name":"notes.md"}]] 截断自愈记账目标"#;
    let id = create_task_with_mode(app, title, "solo").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (_st, _) = wait_terminal(app, &id).await;
    let (_, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
    let (_, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert!(
        calls["calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["response_summary"]
                .as_str()
                .unwrap_or("")
                .contains("截断自愈")),
        "应有截断自愈标注行: {calls:?}"
    );
    assert_usage_matches_calls(&detail, &calls);
}

#[tokio::test]
async fn executor_library_crud_roundtrip() {
    let app = test_app();

    // 空名称 / 空指令被拒
    let (status, json) = send_json(
        app,
        "POST",
        "/api/task-executors",
        json!({ "config": { "name": "  ", "instruction": "指令" } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空名称应 400: {json}");
    let (status, _) = send_json(
        app,
        "POST",
        "/api/task-executors",
        json!({ "config": { "name": "名", "instruction": "  " } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空指令应 400");

    // 新建(带唯一名,不依赖库内初始状态)
    let name = "EXEC-CRUD-审稿员";
    let id = create_executor(app, name, "你是严苛的审稿人,逐条指出问题").await;

    // 列表包含且带指令原文
    let (status, json) = send_json(app, "GET", "/api/task-executors", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let found = json["executors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"].as_str() == Some(id.as_str()))
        .expect("列表应含刚创建的执行者");
    assert_eq!(found["name"], name);
    assert_eq!(found["instruction"], "你是严苛的审稿人,逐条指出问题");

    // 更新(同 id 就地改,不新增)
    let (status, json) = send_json(
        app,
        "POST",
        "/api/task-executors",
        json!({ "config": { "id": id, "name": "EXEC-CRUD-审稿员改", "instruction": "改后的指令" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "更新应 200: {json}");
    let count = json["executors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"].as_str() == Some(id.as_str()))
        .count();
    assert_eq!(count, 1, "同 id 更新不得产生重复条目");

    // 删除
    let (status, _) = send_json(
        app,
        "DELETE",
        &format!("/api/task-executors/{id}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "删除应 200");
    let (_, json) = send_json(app, "GET", "/api/task-executors", json!({})).await;
    assert!(
        !json["executors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"].as_str() == Some(id.as_str())),
        "删除后列表不应再含该执行者"
    );

    // 删除不存在的执行者 → 400(与 agent-flows 同款校验语义)
    let (status, _) = send_json(app, "DELETE", "/api/task-executors/not-exist-id", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "删除不存在的执行者应 400");
}

/// 解耦的核心断言:绑定执行者时,注入的是执行者指令而非角色卡人设。
/// 用 [[floors]] 钩子回显 mock 执行者实际收到的完整消息序列(system 含身份段)。
#[tokio::test]
async fn task_executor_replaces_character_persona() {
    let app = test_app();
    let cid = upload_persona_marked_character(app).await;
    let executor_id = create_executor(
        app,
        "EXEC-SEP-执行者",
        "EXECUTOR-INSTRUCTION-MARK 你的职责是核对数据",
    )
    .await;

    // ① 只绑执行者:注入执行者指令,不含角色卡任何人设标记
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": "[[floors]]", "task_mode": "solo", "executor_id": executor_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        json["task"]["executor_id"].as_str(),
        Some(executor_id.as_str()),
        "任务详情应回带 executor_id: {json}"
    );

    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "solo 任务应完成: {detail}");
    let echo = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        echo.contains("EXECUTOR-INSTRUCTION-MARK"),
        "应注入执行者指令段: {echo}"
    );
    for persona_mark in [
        "R3A-DESC-MARK",
        "R3A-PERS-MARK",
        "R3A-SCEN-MARK",
        "R3A-MESEX-MARK",
    ] {
        assert!(
            !echo.contains(persona_mark),
            "绑定执行者时不得注入角色卡人设 {persona_mark}(解耦契约): {echo}"
        );
    }

    // ② 兼容路径未回归:不绑执行者、只给 character_id(旧客户端形态)仍注入人设
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": "[[floors]]", "task_mode": "solo", "character_id": cid }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "兼容创建应 201: {json}");
    let legacy_id = json["task"]["id"].as_str().unwrap().to_string();
    let (status, _) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{legacy_id}/run"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "兼容路径 run 应 200");
    let (st, detail) = wait_terminal(app, &legacy_id).await;
    assert_eq!(st, "done", "兼容路径任务应完成: {detail}");
    let echo = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        echo.contains("R3A-DESC-MARK"),
        "旧路径(character_id)应仍注入角色人设: {echo}"
    );
    assert!(
        !echo.contains("EXECUTOR-INSTRUCTION-MARK"),
        "旧路径不得注入执行者指令: {echo}"
    );
}

/// TM-GEN-1:执行者「建议温度」接通——执行者库配置了温度时,solo 主 agent 与 legacy
/// 步骤生成都以它优先于任务缺省温度;未配置(或未绑执行者)沿用缺省(0.8)。
/// 观测手段:mock `[[echo_temp]]` 回显请求温度——请求参数不落库,只能靠回显断言;
/// 回显值同时锁定 f32→f64 拓宽噪声已被消除(0.42 不得变成 0.41999998…)。
#[tokio::test]
async fn executor_temperature_reaches_agent_and_step_generation() {
    let app = test_app();

    // 带温度的执行者(0.42)
    let (status, json) = send_json(
        app,
        "POST",
        "/api/task-executors",
        json!({ "config": { "name": "EXEC-TEMP-温度执行者", "instruction": "按要求产出", "temperature": 0.42 } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "建执行者应 200: {json}");
    let exec_id = json["saved"]["id"].as_str().unwrap().to_string();

    // ① solo 主 agent(经 run_agent_loop):命中执行者建议温度
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": "[[echo_temp]] solo 温度目标", "task_mode": "solo", "executor_id": exec_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();
    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "solo 任务应完成: {detail}");
    assert!(
        detail["task"]["result"]
            .as_str()
            .unwrap_or("")
            .contains("温度=0.42"),
        "solo 主 agent 应使用执行者建议温度(且无 f32 拓宽噪声): {detail}"
    );

    // ② 无执行者:solo 沿用任务缺省温度(温度字段不设任务上限——D4(a))
    let id2 = create_task_with_mode(app, "[[echo_temp]] 无执行者温度目标", "solo").await;
    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id2}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (st, detail) = wait_terminal(app, &id2).await;
    assert_eq!(st, "done", "无执行者 solo 应完成: {detail}");
    assert!(
        detail["task"]["result"]
            .as_str()
            .unwrap_or("")
            .contains("温度=0.8"),
        "无执行者应沿用任务缺省温度 0.8: {detail}"
    );

    // ③ legacy 步骤生成(经 generate_step):同解析式命中执行者温度
    //    规划器出 1 步;步骤 goal 内嵌 [[echo_temp]](unicode 转义写入计划 JSON,
    //    serde 还原为真实钩子——与既有 legacy 用例同款布局)
    let title = concat!(
        r#"[[reply:[{"name":"温度步","goal":"\u005b\u005becho_temp\u005d\u005d 回显温度"}]]]"#,
        " legacy 温度目标"
    );
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": "legacy", "executor_id": exec_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建 legacy 任务应 201: {json}");
    let id3 = json["task"]["id"].as_str().unwrap().to_string();
    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id3}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200");
    let (st, detail) = wait_terminal(app, &id3).await;
    assert_eq!(st, "done", "legacy 任务应完成: {detail}");
    let step_result = detail["task"]["plan"][0]["result"].as_str().unwrap_or("");
    assert!(
        step_result.contains("温度=0.42"),
        "legacy 步骤生成应使用执行者建议温度: {step_result}"
    );
}

/// 引用不存在的执行者 → 静默丢弃(任务照建,回退通用执行者),不报错。
/// 与「执行期查不到配置即回退通用执行者」同口径,避免创建期/执行期语义分叉。
#[tokio::test]
async fn unknown_executor_id_is_dropped_and_task_still_runs() {
    let app = test_app();
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": "[[floors]]", "task_mode": "solo", "executor_id": "no-such-executor" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "未知执行者不应阻断创建: {json}"
    );
    assert!(
        json["task"]["executor_id"].is_null(),
        "未知执行者应被丢弃为空: {json}"
    );

    let id = json["task"]["id"].as_str().unwrap().to_string();
    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (st, _) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "丢弃执行者后任务应以通用执行者跑完");
}
// ===== 批准执行方式(2026-09-17:plan 批准界面可选「用什么模式执行」) =====
//
// 契约:
//   - 缺省(不传 exec_mode)= approved_plan = 按计划逐步执行(改造前行为,回归锁);
//   - 可选 solo/multi/team/custom:以该模式执行,已批准计划作 goal 上下文;
//   - 未知值 400;legacy/plan 不在可选集内(前者重复规划、后者会回到 planned 死循环)。
//
// 测试写法说明(与既有 plan 测试同款):标题内嵌钩子让每次调用都以单 chunk 快速返回,
// 否则 mock 的默认回复会逐字符流式(8ms/字),多步任务会超出 wait_terminal 的轮询窗口。
// plan 步骤的 goal 用 \[ \] 转义钩子:规划 JSON 经 serde 还原为真实钩子,续跑时步骤
// 专属产出即该钩子文本,从而无需依赖默认回复。
//
// 未覆盖 custom 的端到端执行:流程库是共享全局状态,而既有
// `task_custom_mode_flow_steps_and_disabled_flow` 会改写/禁用当前流程,
// 并行执行时任何依赖流程状态的用例都是竞态(该用例注释已记载此约束)。
// custom 的分发正确性由 `approve_with_unknown_exec_mode_is_rejected` 的 400 断言
// (legacy/plan 拒绝)与 run_approved 的类型穷尽匹配共同保证。

/// 工具纪律段注入条件(提交 3 · D3-c):**有工具**的执行者 system 追加纪律句,
/// legacy(无工具)档不追加。
///
/// 观测手段:mock 的 `[[floors]]` 回显完整消息序列(`[角色] 内容`),于是**真实 system
/// 文本**直接落进步骤 result——正反两面都在同一份回显上断言,并以
/// 「回显里含『你是任务执行者』」作控制项:保证「没出现纪律句」不是因为压根没回显到
/// 系统提示词(mock 的其它条件钩子只匹配 system,回显是唯一能读全文的口子)。
#[tokio::test]
async fn executor_tool_discipline_injected_only_for_tool_executors() {
    let app = test_app();

    // ① solo:策略默认下发工具(fs_*/bash 等)→ 必须注入
    let id = create_task_with_mode(app, "[[floors]] 纪律注入断言(solo)", "solo").await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "回显即产出,应正常收尾: {detail}");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("你是任务执行者"),
        "回显应含执行者内置指令(控制项,否则本断言无意义): {result}"
    );
    assert!(
        result.contains("自测通过即收尾"),
        "有工具的执行者必须拿到工具纪律段: {result}"
    );

    // ② legacy:步骤走纯文本生成(无工具)→ 不注入
    //    [[floors]] 放在规划器产出的 step goal 里(JSON 内需 \u005d 转义 "]]"),
    //    规划器/汇总器的 user 消息里是被转义的形式,故只有步骤轮会命中回显。
    let title =
        r#"[[reply:[{"name":"步骤一","goal":"\u005b\u005bfloors\u005d\u005d 写第一段"}] ]]"#;
    let id = create_task(app, title).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (_st, detail) = wait_terminal(app, &id).await;
    let step_result = detail["task"]["plan"][0]["result"].as_str().unwrap_or("");
    assert!(
        step_result.contains("你是任务执行者"),
        "legacy 步骤回显应含执行者内置指令(控制项): {step_result}"
    );
    assert!(
        !step_result.contains("自测通过即收尾"),
        "legacy 步骤没有工具,不得注入工具纪律段: {step_result}"
    );
}
