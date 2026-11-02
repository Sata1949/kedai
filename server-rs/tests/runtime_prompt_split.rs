//! 运行时主提示词按模式拆两份(RTSPLIT-1,2026-11-02)。
//!
//! 背景:此前 `AGENTS_RUNTIME.md` 是 `DATA_DIR` 下单文件、双模式共用,而任务侧
//! **完全不读它**——设置页把这一框摆在标注「按模式独立存储」的系统提示词正下方,
//! 于是切模式时它必然一字不变,用户看到的就是「提示词不随模式切换」。
//!
//! 本文件锁三件事:
//! ① 两个端点(`GET`/`PUT /api/settings/agent-prompt`)按 `?mode=` 操作各自文件;
//! ② 预览的 `runtime_prompt` 层随模式取对应文件,且响应回显 `mode`;
//! ③ 任务侧装配真的会注入任务那份,角色扮演侧绝不含它。
//!
//! 隔离:`common::test_app_with_prompt_isolation` 把 `KEDAI_RUNTIME_PROMPT_DIR` 指向
//! 空目录(关闭内置默认),因此「未写入时两侧都读不到内容」是确定的起点。
mod common;

use axum::http::StatusCode;
use common::{send_json, test_app_with_prompt_isolation, test_lock, wait_terminal};
use serde_json::{json, Value};

/// 创建任务并返回 id
async fn create_task(app: &axum::Router, title: &str) -> String {
    let (status, json) = send_json(app, "POST", "/api/tasks", json!({ "title": title })).await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应返回 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 取某一层的内容(无该层返回 None)
fn layer_content<'a>(resp: &'a Value, source: &str) -> Option<&'a str> {
    resp["layers"]
        .as_array()?
        .iter()
        .find(|l| l["source"].as_str() == Some(source))?
        .get("content")?
        .as_str()
}

/// 清理本次写入的两份文件,避免污染同二进制内的其他用例
async fn clear_both(app: &axum::Router) {
    for mode in ["roleplay", "task"] {
        let _ = send_json(
            app,
            "PUT",
            &format!("/api/settings/agent-prompt?mode={mode}"),
            json!({ "content": "" }),
        )
        .await;
    }
}

#[tokio::test]
async fn agent_prompt_endpoint_follows_mode() {
    let _guard = test_lock().await;
    let app = test_app_with_prompt_isolation();

    // 起点:两侧都空(隔离目录无文件)
    let (status, rp0) = send_json(
        app,
        "GET",
        "/api/settings/agent-prompt?mode=roleplay",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rp0["mode"], "roleplay", "响应必须回显模式");
    assert!(
        rp0["path"]
            .as_str()
            .unwrap_or("")
            .ends_with("AGENTS_RUNTIME.md"),
        "角色扮演应读 AGENTS_RUNTIME.md:{}",
        rp0["path"]
    );

    let (status, task0) = send_json(
        app,
        "GET",
        "/api/settings/agent-prompt?mode=task",
        json!({}),
    )
    .await;
    // 未配置 ≠ 读取失败:任务侧刻意无内置默认,首次打开设置页时文件根本还不存在。
    // 若回 404,前端会把它显示成「读取失败」(第一版实现正是踩了这个坑)。
    assert_eq!(status, StatusCode::OK, "未配置时应 200 + 空内容,而不是 404");
    assert_eq!(task0["content"], "", "未配置时内容应为空串");
    assert_eq!(task0["mode"], "task");
    assert!(
        task0["path"]
            .as_str()
            .unwrap_or("")
            .ends_with("AGENTS_RUNTIME.task.md"),
        "任务模式应读 AGENTS_RUNTIME.task.md:{}",
        task0["path"]
    );
    assert_ne!(rp0["path"], task0["path"], "两份文件路径必须不同");

    // 各写各的
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings/agent-prompt?mode=roleplay",
        json!({ "content": "RP-SENTINEL-角色扮演主提示词" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings/agent-prompt?mode=task",
        json!({ "content": "TASK-SENTINEL-任务主提示词" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 各读各的:互不串
    let (_, rp) = send_json(
        app,
        "GET",
        "/api/settings/agent-prompt?mode=roleplay",
        json!({}),
    )
    .await;
    assert_eq!(rp["content"], "RP-SENTINEL-角色扮演主提示词");
    let (_, task) = send_json(
        app,
        "GET",
        "/api/settings/agent-prompt?mode=task",
        json!({}),
    )
    .await;
    assert_eq!(task["content"], "TASK-SENTINEL-任务主提示词");

    // 缺省(不带 mode)= roleplay,兼容旧客户端
    let (_, bare) = send_json(app, "GET", "/api/settings/agent-prompt", json!({})).await;
    assert_eq!(bare["content"], "RP-SENTINEL-角色扮演主提示词");
    assert_eq!(bare["mode"], "roleplay");

    clear_both(app).await;
}

#[tokio::test]
async fn prompt_preview_runtime_layer_follows_mode() {
    let _guard = test_lock().await;
    let app = test_app_with_prompt_isolation();

    send_json(
        app,
        "PUT",
        "/api/settings/agent-prompt?mode=roleplay",
        json!({ "content": "RP-PREVIEW-SENTINEL" }),
    )
    .await;
    send_json(
        app,
        "PUT",
        "/api/settings/agent-prompt?mode=task",
        json!({ "content": "TASK-PREVIEW-SENTINEL" }),
    )
    .await;

    let (status, rp) = send_json(
        app,
        "GET",
        "/api/settings/prompt-preview?mode=roleplay",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rp["mode"], "roleplay",
        "预览必须回显模式(RTSPLIT-2 前端据此判定时效)"
    );
    assert_eq!(
        layer_content(&rp, "runtime_prompt"),
        Some("RP-PREVIEW-SENTINEL")
    );

    let (status, task) = send_json(
        app,
        "GET",
        "/api/settings/prompt-preview?mode=task",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(task["mode"], "task");
    assert_eq!(
        layer_content(&task, "runtime_prompt"),
        Some("TASK-PREVIEW-SENTINEL"),
        "任务模式预览必须取任务那份文件,不得展示创作向文本"
    );

    // 缺省 = roleplay(旧客户端零变化)
    let (_, bare) = send_json(app, "GET", "/api/settings/prompt-preview", json!({})).await;
    assert_eq!(bare["mode"], "roleplay");
    assert_eq!(
        layer_content(&bare, "runtime_prompt"),
        Some("RP-PREVIEW-SENTINEL")
    );

    clear_both(app).await;
}

/// 任务侧未写文件时:预览不得凭空出现 runtime 层(任务侧缺省为空 = 零注入)。
/// 这条是「存量任务实际 system 逐字节不变」的可见性证据。
#[tokio::test]
async fn task_preview_has_no_runtime_layer_when_unset() {
    let _guard = test_lock().await;
    let app = test_app_with_prompt_isolation();
    clear_both(app).await;

    let (status, task) = send_json(
        app,
        "GET",
        "/api/settings/prompt-preview?mode=task",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        layer_content(&task, "runtime_prompt"),
        None,
        "任务侧未自定义时不应有运行时主提示词层"
    );
    // 但三层固定提示词仍在——留空不等于「任务提示词全没了」
    assert!(layer_content(&task, "task_executor_prompt").is_some());
    assert!(layer_content(&task, "task_planner_prompt").is_some());
}

/// 装配侧:任务那份写了内容 → 执行者 system 里真的出现,且按任务占位符渲染、
/// 以不可信来源边界包裹。用 mock 钩子 `[[floors]]` 让执行者回显它收到的完整消息。
#[tokio::test]
async fn task_executor_system_includes_task_runtime_prompt_only() {
    let _guard = test_lock().await;
    let app = test_app_with_prompt_isolation();
    clear_both(app).await;

    let sentinel = "TASK-RUNTIME-SENTINEL";
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings/agent-prompt?mode=task",
        json!({ "content": format!("{sentinel} 目标:{{{{lastUserMessage}}}} 用户:{{{{user}}}}") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 规划器只回 1 步;步骤 goal 带 [[floors]] 钩子让执行器回显实际消息序列
    let title = r#"[[reply:[{"name":"回显步骤","goal":"\u005b\u005bfloors\u005d\u005d"}] ]]"#;
    let id = create_task(app, title).await;
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成,详情: {detail}");

    let echo = detail["subtasks"][0]["result"].as_str().unwrap_or("");
    assert!(
        echo.contains(sentinel),
        "任务执行者 system 必须注入 AGENTS_RUNTIME.task.md 的内容,实际: {echo}"
    );
    assert!(
        echo.contains(r#"<UNTRUSTED_PROMPT_SOURCE source="runtime_prompt">"#),
        "运行时主提示词属外部来源,必须 untrusted 包裹,实际: {echo}"
    );
    // 占位符必须真展开:原文残留即为接线错误
    assert!(
        !echo.contains("{{lastUserMessage}}") && !echo.contains("{{user}}"),
        "占位符原文不得泄漏进 LLM 上下文,实际: {echo}"
    );
    // 三层固定提示词照常在内(运行时提示词是**追加**,不是替换)
    assert!(
        echo.contains("任务执行者"),
        "内置执行者指令仍应在场: {echo}"
    );

    clear_both(app).await;
}

/// 反向:角色扮演那份的内容**不得**出现在任务执行者 system 里。
/// 这是「拆两份」的核心价值——创作向文本不会污染任务执行。
#[tokio::test]
async fn task_executor_system_never_includes_roleplay_runtime_prompt() {
    let _guard = test_lock().await;
    let app = test_app_with_prompt_isolation();
    clear_both(app).await;

    let rp_sentinel = "ROLEPLAY-RUNTIME-ONLY-SENTINEL";
    send_json(
        app,
        "PUT",
        "/api/settings/agent-prompt?mode=roleplay",
        json!({ "content": rp_sentinel }),
    )
    .await;

    let title = r#"[[reply:[{"name":"回显步骤","goal":"\u005b\u005bfloors\u005d\u005d"}] ]]"#;
    let id = create_task(app, title).await;
    send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    let (st, detail) = wait_terminal(app, &id).await;
    assert_eq!(st, "done", "任务应完成,详情: {detail}");

    let echo = detail["subtasks"][0]["result"].as_str().unwrap_or("");
    assert!(
        !echo.contains(rp_sentinel),
        "角色扮演的运行时主提示词绝不得进任务执行者 system,实际: {echo}"
    );

    clear_both(app).await;
}
