// SKY-6 第 3 项回归:声明交付物的内容体检(未替换 `@@` 槽位)必须进入终态门槛。
//
// 判别力(修复前必红):
//   * 规划器声明 index.html,执行器写出带 `@@BODY@@` 的模板 → 文件存在且有变更,
//     旧门槛只看存在/变更,任务记 done;
//   * 修复后:内容体检抓到未替换槽位 → 任务 partial,error 与交付摘要都点名该文件。
// 反向用例:健康文件不得被内容体检误伤(done + content_issues 为空)。
//
// 独立成文件的原因同其它 task_*.rs:设置/数据目录按进程隔离,便于将来扩展。
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
mod common;
use common::{send_json, test_app};

/// 任务终态轮询:沿用本文件历史的 300 次 × 100ms 超时,实现见 `common`.
async fn wait_terminal(app: &axum::Router, id: &str) -> (String, Value) {
    common::wait_terminal_with_attempts(app, id, 300).await
}

fn temp_workspace(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("kedai-sky6c-{tag}-{}-{ns}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

/// 单主单子目标:写 index.html,规划器把 index.html 声明为必需文件。
/// `content` 直接进 fs_write 参数(测试要控制占位符/健康内容两态)。
fn team_title(content: &str) -> String {
    concat!(
        r#"[[reply_if:团队规划器|{"mains":["#,
        r#"{"name":"写入口","goals":[{"name":"写 index.html","goal":"\u005b\u005btool:fs_write {\"path\":\"index.html\",\"content\":\"__CONTENT__\"}\u005d\u005d 写入口","files":["index.html"]}]}"#,
        r#" ]}]]"#,
        r#"[[reply_if:团队审计员|{"通过":true,"打回":[],"结论":"通过"}]]"#,
        r#"[[reply_if:任务汇总者|团队最终成果]]"#,
        " 交付内容校验用例"
    )
    .replace("__CONTENT__", content)
}

async fn run_team_task(app: &axum::Router, title: &str, ws: &Path) -> String {
    let (status, json) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title": title,
            "task_mode": "team",
            "workspace": ws.to_string_lossy(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    let id = json["task"]["id"].as_str().unwrap().to_string();
    let (status, json) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {json}");
    id
}

#[tokio::test]
async fn placeholder_in_declared_file_downgrades_to_partial() {
    let app = test_app();
    let ws = temp_workspace("placeholder");
    let id = run_team_task(app, &team_title("<html>@@BODY@@</html>"), &ws).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(
        st, "partial",
        "声明文件带未替换占位符时不得判 done: {detail}"
    );
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("交付内容校验未通过"),
        "error 应说明内容校验失败: {error:?}"
    );
    assert!(
        error.contains("index.html") && error.contains("@@"),
        "error 应点名文件与占位符问题: {error:?}"
    );
    let issues = detail["delivery"]["content_issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        issues
            .iter()
            .any(|v| v.as_str().unwrap_or("").contains("index.html")
                && v.as_str().unwrap_or("").contains("@@")),
        "交付摘要应下发内容体检项: {detail}"
    );
    assert!(
        detail["delivery"]["truncated"]
            .as_array()
            .is_none_or(|a| a.is_empty()),
        "本用例无守卫截断步骤: {detail}"
    );
    let written = std::fs::read_to_string(ws.join("index.html")).unwrap_or_default();
    assert!(written.contains("@@BODY@@"), "文件应原样落盘: {written:?}");

    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn healthy_declared_file_stays_done() {
    let app = test_app();
    let ws = temp_workspace("healthy");
    let id = run_team_task(app, &team_title("<html>ok</html>"), &ws).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "done", "健康交付物不得被内容体检误伤: {detail}");
    assert_eq!(
        detail["task"]["error"].as_str().unwrap_or(""),
        "",
        "done 时 error 应为空: {detail}"
    );
    let issues = detail["delivery"]["content_issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(issues.is_empty(), "健康文件不应有内容问题: {detail}");

    let _ = std::fs::remove_dir_all(&ws);
}
