// SKY-1 确定性部分 / SKY-4 / SKY-5 回归(2026-10-24)。
//
// 覆盖四类终态判据:
//   ① 声明文件里的命名导入没有被目标文件导出 → 交付内容体检 partial(不再等终审);
//   ② 目标要求单文件交付,但 HTML 仍 `<script src>` 本地分片 → partial;
//   ③ 健康的单文件内联(仅 CDN 依赖)→ done,不得误伤;
//   ④ 审计打回后补做仍留下 `@@` 槽位 → 补做轮末执行器复查直接判 partial,
//      不再消耗一次终审调用。
//
// 独立成文件的原因同其它 task_*.rs:设置/数据目录按进程隔离。
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
    let dir =
        std::env::temp_dir().join(format!("kedai-wiring-it-{tag}-{}-{ns}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

/// 构造「规划器拓扑 + 审计 + 汇总」的 mock 标题。
///
/// `goals` 是 (子目标名, 负责文件, goal 文本) 列表;工具指令已由调用方拼进 goal。
/// 任务目标就是 title(创建接口没有 goal 字段),所以末段 `goal_suffix` 同时承担
/// 「单文件交付」等目标级判据与 mock 的默认末尾文本。
fn team_title(goals: &[(&str, &[&str], String)], audit: Value, goal_suffix: &str) -> String {
    let goals_json: Vec<Value> = goals
        .iter()
        .map(|(name, files, goal)| {
            json!({
                "name": name,
                "goal": goal,
                "files": files,
            })
        })
        .collect();
    let topology = json!({
        "mains": [{
            "name": "接线主",
            "goals": goals_json,
        }]
    })
    .to_string();
    // goal 文本里的 `[[tool:...]]` 必须转成 JSON 的 `\u005b`/`\u005d` 转义:
    // mock 的 `[[reply_if:...|...]]` 以第一个 `]]` 结束,嵌套方括号会截断回复。
    // 规划器解析 JSON 时这些转义会还原成真实工具标记(与既有用例同口径)。
    let topology = topology
        .replace("[[", "\\u005b\\u005b")
        .replace("]]", "\\u005d\\u005d");
    format!(
        "[[reply_if:团队规划器|{}]]\n[[reply_if:团队审计员|{}]]\n[[reply_if:任务汇总者|团队最终成果]]\n{}",
        topology, audit, goal_suffix
    )
}

fn write_goal(path: &str, content: &str, label: &str) -> String {
    let args = json!({ "path": path, "content": content });
    format!("[[tool:fs_write {args}]] {label}")
}

async fn run_task(app: &axum::Router, title: &str, ws: &Path) -> String {
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

fn pass_audit() -> Value {
    json!({"通过": true, "打回": [], "结论": "通过"})
}

/// ① 命名导入与导出不符:引擎必须静态抓住,终态 partial 且点名具体名字。
#[tokio::test]
async fn missing_export_downgrades_to_partial() {
    let app = test_app();
    let ws = temp_workspace("missing-export");
    let title = team_title(
        &[
            (
                "写分片",
                &["parts/a.js"],
                write_goal("parts/a.js", "export const A = 1;\n", "写分片"),
            ),
            (
                "写入口",
                &["index.html"],
                write_goal(
                    "index.html",
                    "<html><script type=\"module\">import { B } from './parts/a.js'; console.log(B);</script></html>",
                    "写入口",
                ),
            ),
        ],
        pass_audit(),
        " 接线一致性用例",
    );
    let id = run_task(app, &title, &ws).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "partial", "未导出导入不得判 done: {detail}");
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("交付内容校验未通过") && error.contains("B"),
        "error 应点名未导出的导入名: {error:?}"
    );
    let issues = detail["delivery"]["content_issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        issues.iter().any(|v| {
            let s = v.as_str().unwrap_or("");
            s.contains("index.html") && s.contains("B") && s.contains("未被目标文件导出")
        }),
        "交付摘要应下发静态接线问题: {detail}"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

/// ② 单文件目标仍外链本地分片:确定性判定为未完成,不依赖审计主观判断。
#[tokio::test]
async fn single_file_external_ref_downgrades_to_partial() {
    let app = test_app();
    let ws = temp_workspace("single-file-ref");
    let title = team_title(
        &[
            (
                "写分片",
                &["parts/a.js"],
                write_goal("parts/a.js", "export const A = 1;\n", "写分片"),
            ),
            (
                "写入口",
                &["index.html"],
                write_goal(
                    "index.html",
                    "<html><script src=\"./parts/a.js\"></script></html>",
                    "写入口",
                ),
            ),
        ],
        pass_audit(),
        " 最终只交付一个 HTML 文件",
    );
    let id = run_task(app, &title, &ws).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "partial", "单文件外链不得判 done: {detail}");
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("单文件交付不得引用本地外部资源"),
        "error 应说明单文件外链问题: {error:?}"
    );
    let issues = detail["delivery"]["content_issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        issues.iter().any(|v| v
            .as_str()
            .unwrap_or("")
            .contains("单文件交付不得引用本地外部资源")),
        "交付摘要应下发单文件外链问题: {detail}"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

/// ③ 健康单文件(内联脚本 + 仅 CDN import)不得被确定性检查误伤。
#[tokio::test]
async fn healthy_single_file_inline_stays_done() {
    let app = test_app();
    let ws = temp_workspace("single-file-healthy");
    let title = team_title(
        &[(
            "写入口",
            &["index.html"],
            write_goal(
                "index.html",
                "<html><script type=\"module\">import * as THREE from \"https://cdn.example/three.module.js\"; const A = 1; console.log(THREE, A);</script></html>",
                "写入口",
            ),
        )],
        pass_audit(),
        " 单文件交付健康用例",
    );
    let id = run_task(app, &title, &ws).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "done", "健康单文件不得误伤: {detail}");
    assert_eq!(detail["task"]["error"].as_str().unwrap_or(""), "");
    let issues = detail["delivery"]["content_issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(issues.is_empty(), "健康交付不应有内容问题: {detail}");

    let _ = std::fs::remove_dir_all(&ws);
}

/// ④ 补做轮末执行器复查:审计打回后仍留 `@@` → 直接 partial,不再调用终审。
#[tokio::test]
async fn redo_recheck_skips_final_audit_and_returns_partial() {
    let app = test_app();
    let ws = temp_workspace("redo-recheck");
    let title = team_title(
        &[(
            "写入口",
            &["index.html"],
            write_goal("index.html", "<html>@@BODY@@</html>", "写入口"),
        )],
        json!({
            "通过": false,
            "打回": [{"main": 1, "instruction": "补齐占位符"}],
            "结论": "入口仍是模板"
        }),
        " 补做复查用例",
    );
    let id = run_task(app, &title, &ws).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "partial", "补做后仍有占位符不得判 done: {detail}");
    let result = detail["task"]["result"].as_str().unwrap_or("");
    assert!(
        result.contains("补做后交付内容复查未通过"),
        "审计结论段应说明补做复查失败: {result}"
    );
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("交付内容校验未通过") && error.contains("@@"),
        "error 应点名占位符问题: {error:?}"
    );

    // 事件可解释性:补做复查失败必须透出,而不是静默跳过终审。
    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=2000"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let joined = events["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|e| e["detail"].as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("补做后交付内容复查未通过"),
        "应透出补做复查事件: {joined}"
    );

    // 终审被跳过:调用记录里不应出现 final_audit。
    let (status, calls) = send_json(app, "GET", &format!("/api/tasks/{id}/calls"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let has_final = calls["calls"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|c| c["phase"] == "final_audit"));
    assert!(!has_final, "确定性复查失败后不应再调用终审: {calls}");

    let _ = std::fs::remove_dir_all(&ws);
}

/// ⑤ 全局命名接线不一致（SKY-1，2026-10-25）：入口 `requireGlobal('BlockEngine')`，
/// 内联脚本只导出 `MC_BLOCK_ENGINE` → 静态闸门判 partial，并同时点名两侧名字。
#[tokio::test]
async fn global_name_mismatch_downgrades_to_partial() {
    let app = test_app();
    let ws = temp_workspace("global-name-mismatch");
    let html = "<html><script>global.MC_BLOCK_ENGINE = {};\nvar engine = requireGlobal('BlockEngine');</script></html>";
    let title = team_title(
        &[(
            "写入口",
            &["index.html"],
            write_goal("index.html", html, "写入口"),
        )],
        pass_audit(),
        " 单文件交付(命名一致性用例)",
    );
    let id = run_task(app, &title, &ws).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "partial", "全局命名不一致不得判 done: {detail}");
    let error = detail["task"]["error"].as_str().unwrap_or("");
    assert!(
        error.contains("全局命名不一致")
            && error.contains("BlockEngine")
            && error.contains("MC_BLOCK_ENGINE"),
        "error 应同时点名消费者与生产者: {error:?}"
    );
    let issues = detail["delivery"]["content_issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        issues.iter().any(|v| {
            let s = v.as_str().unwrap_or("");
            s.contains("index.html") && s.contains("requireGlobal('BlockEngine')")
        }),
        "交付摘要应下发全局命名问题: {detail}"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

/// ⑥ 交付卫生（SKY-1，2026-10-25）：根目录预置未声明的 `_check.js` 只进提示，
/// 不把健康交付降成 partial；摘要与事件都要能看到。
#[tokio::test]
async fn scratch_files_are_reported_without_downgrading() {
    let app = test_app();
    let ws = temp_workspace("hygiene-note");
    std::fs::write(ws.join("_check.js"), "console.log('scratch');\n").expect("写临时脚本失败");
    let title = team_title(
        &[(
            "写入口",
            &["index.html"],
            write_goal(
                "index.html",
                "<html><script>window.App={ready:true};</script></html>",
                "写入口",
            ),
        )],
        pass_audit(),
        " 单文件交付(交付卫生用例)",
    );
    let id = run_task(app, &title, &ws).await;
    let (st, detail) = wait_terminal(app, &id).await;

    assert_eq!(st, "done", "临时脚本只作提示,不得降终态: {detail}");
    let hygiene = detail["delivery"]["hygiene_issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        hygiene
            .iter()
            .any(|v| v.as_str().unwrap_or("").contains("_check.js")),
        "交付摘要应下发卫生提示: {detail}"
    );

    let (status, events) = send_json(
        app,
        "GET",
        &format!("/api/tasks/{id}/events?after=0&limit=2000"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let joined = events["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|e| e["detail"].as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("交付卫生提示"),
        "应透出卫生提示事件: {joined}"
    );

    let _ = std::fs::remove_dir_all(&ws);
}
