//! TQUAL P0：用确定的上游工具序列走真实任务引擎，复现正常探索/修复被守卫截断。
use axum::{http::StatusCode, routing::post, Json, Router};
use kedai_server::utils::test_support::TempDataDir;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

mod common;
use common::{send_json, test_app_with_prompt_isolation, test_lock, wait_terminal_with_attempts};

struct Provider(tokio::task::JoinHandle<()>);
impl Drop for Provider {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn call(name: &str, arguments: Value) -> Value {
    json!({"name": name, "arguments": arguments.to_string()})
}

async fn run_sequence(ws: &TempDataDir, sequence: Vec<Value>) -> (Value, Vec<Value>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let provider = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let requests = seen.clone();
            let sequence = sequence.clone();
            async move {
                let n = body["messages"].as_array().unwrap().iter()
                    .filter(|m| m["role"] == "tool").count();
                requests.lock().unwrap().push(body);
                let (delta, reason) = match sequence.get(n) {
                    Some(function) => (json!({"content":"执行当前验证步骤。", "tool_calls":[{
                        "index":0, "id":format!("quality-{n}"), "type":"function", "function":function
                    }]}), "tool_calls"),
                    None => (json!({"content":"验收完成，文件已交付。"}), "stop"),
                };
                let chunk = json!({"id":"quality", "choices":[{"index":0,"delta":delta,"finish_reason":null}]});
                let finish = json!({"id":"quality", "choices":[{"index":0,"delta":{},"finish_reason":reason}],
                    "usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}});
                ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n"))
            }
        }),
    );
    let _provider = Provider(tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap()
    }));
    let app = test_app_with_prompt_isolation();
    let (status, settings) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "openai_base_url":format!("http://{address}/v1"), "openai_api_key":"test-only-key",
            "model":"quality-fixture", "task_coding_bundle_enabled":true,
            "task_step_budget_secs":120, "task_total_budget_secs":120,
            "max_context_tokens":65536, "session_token_budget":0,
            "loop_guard_semantic_min_calls":12
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "设置失败：{settings}");
    let (status, created) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title":"阅读工作区实现并修复，交付 out.txt，运行验证后按要求收尾",
            "task_mode":"solo", "workspace":ws.path().to_string_lossy(), "tool_authorization":"auto"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建失败：{created}");
    let id = created["task"]["id"].as_str().unwrap();
    let (status, started) =
        send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "运行失败：{started}");
    let (_, detail) = wait_terminal_with_attempts(app, id, 300).await;
    let captured = requests.lock().unwrap().clone();
    (detail, captured)
}

#[tokio::test]
async fn task_quality_p0_reads_twenty_files_before_writing() {
    let _lock = test_lock().await;
    let ws = TempDataDir::new("quality-explore");
    let mut sequence = Vec::new();
    for n in 0..20 {
        let path = format!("module-{n}.txt");
        // 同内容的不同文件也是不同读取事实，不能被按工具名聚合误杀。
        std::fs::write(ws.join(&path), "共享接口定义\n").unwrap();
        sequence.push(call("fs_read", json!({"path":path})));
    }
    sequence.push(call(
        "fs_write",
        json!({"path":"out.txt","content":"完整实现"}),
    ));
    let (detail, requests) = run_sequence(&ws, sequence).await;
    assert_eq!(
        detail["task"]["status"], "done",
        "必要探索不得被截断：{detail}"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("out.txt")).unwrap(),
        "完整实现"
    );
    let system = requests[0]["messages"][0]["content"].as_str().unwrap();
    for forbidden in [
        "第一个工具调用就写",
        "最多先读 3 个",
        "自测通过即收尾",
        "每轮只做一个动作",
    ] {
        assert!(
            !system.contains(forbidden),
            "执行提示词残留误导：{forbidden}"
        );
    }
}

#[tokio::test]
async fn task_quality_p0_retests_after_real_edits() {
    let _lock = test_lock().await;
    let ws = TempDataDir::new("quality-repair");
    std::fs::write(ws.join("out.txt"), "0").unwrap();
    std::fs::write(ws.join("check.cjs"), "const v=require('fs').readFileSync('out.txt','utf8');console.log(v==='3'?'PASS':'FAIL');process.exitCode=v==='3'?0:1;").unwrap();
    let mut sequence = vec![call("fs_read", json!({"path":"out.txt"}))];
    for n in 0..3 {
        sequence.push(call("bash", json!({"command":"node check.cjs"})));
        sequence.push(call(
            "fs_edit",
            json!({"path":"out.txt","old":n.to_string(),"new":(n+1).to_string()}),
        ));
    }
    sequence.push(call("bash", json!({"command":"node check.cjs"})));
    let (detail, _) = run_sequence(&ws, sequence).await;
    assert_eq!(
        detail["task"]["status"], "done",
        "修复后的复测不得误熔断：{detail}"
    );
    assert_eq!(std::fs::read_to_string(ws.join("out.txt")).unwrap(), "3");
}

#[tokio::test]
async fn task_quality_p0_unchanged_retries_still_stop() {
    let _lock = test_lock().await;
    let ws = TempDataDir::new("quality-idle");
    std::fs::write(ws.join("out.txt"), "原值").unwrap();
    let sequence = vec![call("fs_read", json!({"path":"out.txt"})); 20];
    let (detail, requests) = run_sequence(&ws, sequence).await;
    assert_eq!(
        detail["task"]["status"], "partial",
        "原地重试必须收口：{detail}"
    );
    assert!(detail["task"]["error"]
        .as_str()
        .unwrap_or("")
        .contains("守卫截断"));
    assert!(requests.len() < 20, "不得用放开所有守卫修误杀");
}

#[tokio::test]
async fn task_quality_p0_identical_writes_do_not_reset_repetition() {
    let _lock = test_lock().await;
    let ws = TempDataDir::new("quality-empty-write");
    std::fs::write(ws.join("out.txt"), "原值").unwrap();
    let mut sequence = vec![call("fs_read", json!({"path":"out.txt"}))];
    sequence.extend(vec![
        call(
            "fs_write",
            json!({"path":"out.txt","content":"原值"})
        );
        20
    ]);
    let (detail, requests) = run_sequence(&ws, sequence).await;
    assert_eq!(
        detail["task"]["status"], "partial",
        "同内容重写不是进展：{detail}"
    );
    assert!(detail["task"]["error"]
        .as_str()
        .unwrap_or("")
        .contains("守卫截断"));
    assert!(requests.len() < 20);
}

#[tokio::test]
async fn task_quality_p0_prompt_preview_requires_acceptance_evidence() {
    let _lock = test_lock().await;
    let app = test_app_with_prompt_isolation();
    let (_, preview) = send_json(
        app,
        "GET",
        "/api/settings/prompt-preview?mode=task",
        json!({}),
    )
    .await;
    let layers = preview["layers"].as_array().unwrap();
    let text = layers
        .iter()
        .map(|l| l["content"].as_str().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "第一个工具调用就写",
        "最多先读 3 个",
        "自测通过即收尾",
        "每轮只做一个动作",
    ] {
        assert!(!text.contains(forbidden), "预览残留误导：{forbidden}");
    }
    assert!(text.contains("验收") && text.contains("证据"));
}
