// 任务附件 → 目标文本注入(ATTACH-1,2026-11-03,提交 2)。
//
// 这一层要回答一个只有集成测试能回答的问题:**模型真的看得到附件清单吗**。
// 单测(`task_engine::attachments`)证明拼接函数对,但不能证明它被接进了执行路径
// ——尤其 legacy 模式历史上读的是 `task.title` 而不是 `ctx.goal`,那处特例是
// 本批最容易静默失效的点。
//
// 手法沿用 `tasks_prompt.rs` 的 `[[floors]]` 钩子:mock 连接器会把**实际收到的
// 完整消息序列**回显成文字,于是「目标里有没有附件段」可以直接断言。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
mod common;
use common::{send_json, test_app, test_lock, wait_terminal};

async fn upload_attachment(app: &axum::Router, task_id: &str, name: &str, content: &[u8]) -> Value {
    let boundary = "KEDAIBOUND";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(content);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/tasks/{task_id}/attachments"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::CREATED, "附件上传应 201: {json}");
    json
}

async fn create_task(app: &axum::Router, title: &str, extra: Value) -> String {
    let mut body = json!({ "title": title });
    if let Value::Object(map) = extra {
        for (k, v) in map {
            body[&k] = v;
        }
    }
    let (status, json) = send_json(app, "POST", "/api/tasks", body).await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 取某任务全部 LLM 调用的 prompt_summary 拼接串——这是**模型实际看到的**消息内容,
/// 比步骤回显更权威(回显受 mock 钩子替换 user 消息内容的影响)。
async fn prompt_text(app: &axum::Router, task_id: &str) -> String {
    let (status, calls) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{task_id}/calls"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "取调用追踪应 200: {calls}");
    calls["calls"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["prompt_summary"].as_str())
                .collect::<Vec<_>>()
                .join(
                    "
",
                )
        })
        .unwrap_or_default()
}

/// `[[reply:...]]` 让规划器返回指定的单步计划,步骤 goal 用 `[[floors]]` 让执行器
/// 回显它收到的完整消息序列。
fn echo_title(step_name: &str) -> String {
    format!(r#"[[reply:[{{"name":"{step_name}","goal":"\u005b\u005bfloors\u005d\u005d"}}] ]]"#)
}

/// **legacy 模式**必须也能看到附件段。
///
/// 这条是本批最容易漏的点:legacy 的规划器此前读 `task.title` 而不是 `ctx.goal`,
/// 而附件的注入段只加在 `ctx.goal` 上。不改那处特例,附件对 legacy 完全无效,
/// 且**没有任何测试会红**——除非专门断言。
#[tokio::test]
async fn legacy_mode_sees_attachment_section_in_goal() {
    let _g = test_lock().await;
    let app = test_app();
    let title = echo_title("读附件");
    let id = create_task(app, &title, json!({ "task_mode": "legacy" })).await;
    upload_attachment(
        app,
        &id,
        "规格说明.txt",
        "附件正文标记 ATTACH-BODY-MARK".as_bytes(),
    )
    .await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成: {detail}");

    let prompt = prompt_text(app, &id).await;
    assert!(
        prompt.contains("## 附件"),
        "legacy 的目标里必须有附件段(此前的特例读 task.title,会漏掉): {prompt}"
    );
    assert!(
        prompt.contains("规格说明.txt"),
        "附件段应带展示名: {prompt}"
    );
    assert!(
        prompt.contains("attachments/"),
        "附件段应带相对路径: {prompt}"
    );
    // 只注入**清单**,不内联正文(附件可达 25MB)
    assert!(
        !prompt.contains("ATTACH-BODY-MARK"),
        "附件正文不得被内联进目标(会炸上下文): {prompt}"
    );
}

/// **solo 模式**同样能看到附件段,并且路径可被工具读到。
#[tokio::test]
async fn solo_mode_sees_attachment_section_in_goal() {
    let _g = test_lock().await;
    let app = test_app();
    let title = echo_title("读附件");
    let id = create_task(app, &title, json!({ "task_mode": "solo" })).await;
    upload_attachment(app, &id, "笔记.md", "md 正文".as_bytes()).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成: {detail}");

    let prompt = prompt_text(app, &id).await;
    assert!(prompt.contains("## 附件"), "solo 目标应含附件段: {prompt}");
    assert!(
        prompt.contains("fs_read"),
        "文本附件应提示用 fs_read: {prompt}"
    );
}

/// 无附件的任务:目标与改造前逐字节一致(不得出现空的附件段)。
#[tokio::test]
async fn tasks_without_attachments_are_unchanged() {
    let _g = test_lock().await;
    let app = test_app();
    let title = echo_title("无附件");
    let id = create_task(app, &title, json!({ "task_mode": "solo" })).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成: {detail}");

    let prompt = prompt_text(app, &id).await;
    assert!(
        !prompt.contains("## 附件"),
        "没有附件时不得出现附件段(无附件的任务行为必须零变化): {prompt}"
    );
}

/// 视觉未开启时,图像附件写「无法查看画面」而不是承诺 view_image。
#[tokio::test]
async fn image_attachment_hint_reflects_vision_flag() {
    let _g = test_lock().await;
    let app = test_app();
    // mock 连接器的 supports_vision 缺省为 false → 目标里应是「无法查看画面」
    let title = echo_title("看图");
    let id = create_task(app, &title, json!({ "task_mode": "solo" })).await;
    upload_attachment(app, &id, "shot.png", b"\x89PNG\r\n\x1a\n-fake").await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成: {detail}");

    let prompt = prompt_text(app, &id).await;
    assert!(
        prompt.contains("无法查看画面"),
        "视觉关闭时图像附件必须如实说明看不到: {prompt}"
    );
    assert!(
        !prompt.contains("view_image"),
        "视觉关闭时不得预告 view_image 可用: {prompt}"
    );
}

/// **legacy 的步骤执行与汇总也必须看到附件段**（ATTACH-1-L1，2026-11-03 真实模型实测抓到）。
///
/// 此前只修了规划器（它拿 `ctx.goal`），但 legacy 的 `generate_step` 读的是
/// **规划器写的 `step.goal`**、`summarize_task` 读的是 `task.title`——两者都不含
/// 附件清单。真实模型的表现是：规划器看得到附件，执行阶段却回答「我没有收到任何附件」。
#[tokio::test]
async fn legacy_step_and_summarize_see_attachment_section() {
    let _g = test_lock().await;
    let app = test_app();
    // 规划器返回一个普通步骤；步骤回显用 [[floors]] 让执行器回显实际收到的消息
    let title = echo_title("读附件");
    let id = create_task(app, &title, json!({ "task_mode": "legacy" })).await;
    upload_attachment(app, &id, "legacy说明.txt", "LEGACY-BODY-MARK".as_bytes()).await;

    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成: {detail}");

    // 逐 phase 检查：step 与 summarize 都必须带附件段
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let rows = calls["calls"].as_array().expect("calls 应为数组");
    let by_phase = |phase: &str| -> String {
        rows.iter()
            .filter(|r| r["phase"] == phase)
            .filter_map(|r| r["prompt_summary"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let step_prompt = by_phase("step");
    assert!(
        step_prompt.contains("## 附件"),
        "**步骤执行**必须看到附件段(否则模型会说「没收到附件」): {step_prompt}"
    );
    assert!(
        step_prompt.contains("legacy说明.txt"),
        "步骤提示应带附件展示名: {step_prompt}"
    );
    let summary_prompt = by_phase("summarize");
    assert!(
        summary_prompt.contains("## 附件"),
        "**汇总**也要看到附件段: {summary_prompt}"
    );
    // 正文不内联（只给清单）
    assert!(
        !step_prompt.contains("LEGACY-BODY-MARK"),
        "附件正文不得内联进步骤提示: {step_prompt}"
    );
}
