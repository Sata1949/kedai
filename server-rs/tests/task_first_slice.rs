//! TQUAL-22：推理独占输出预算时，用同一份已核对事实推进下一步，不重新放大预算。
use axum::{http::StatusCode, routing::post, Json, Router};
use kedai_server::utils::test_support::TempDataDir;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

mod common;
use common::{send_json, test_app_with_prompt_isolation, test_lock, wait_terminal};

struct Provider(tokio::task::JoinHandle<()>);
impl Drop for Provider {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn stream(delta: Value, finish: &str, completion_tokens: u32) -> String {
    let delta = json!({"choices":[{"index":0,"delta":delta,"finish_reason":null}]});
    let finish = json!({"choices":[{"index":0,"delta":{},"finish_reason":finish}],
        "usage":{"prompt_tokens":10,"completion_tokens":completion_tokens,
            "total_tokens":completion_tokens+10}});
    format!("data: {delta}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

fn tool(name: &str, args: Value) -> String {
    stream(
        json!({"tool_calls":[{"index":0,"id":format!("slice-{name}"),"type":"function",
            "function":{"name":name,"arguments":args.to_string()}}]}),
        "tool_calls",
        12,
    )
}

#[tokio::test]
async fn reasoning_only_cutoff_retries_with_a_bounded_action_and_preserves_facts() {
    exercise(true).await;
}

#[tokio::test]
async fn exhausted_action_retry_does_not_generate_a_spurious_summary() {
    exercise(false).await;
}

async fn exercise(recover: bool) {
    let _lock = test_lock().await;
    let workspace = TempDataDir::new("first-slice");
    std::fs::write(workspace.join("interface.txt"), "GROUND_FACT_846\n").unwrap();
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests = captured.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let requests = requests.clone();
            async move {
                let turn = {
                    let mut requests = requests.lock().unwrap();
                    requests.push(body);
                    requests.len()
                };
                let data = match turn {
                    1 => tool("fs_read", json!({"path":"interface.txt"})),
                    2 | 3 if turn == 2 || !recover => stream(
                        json!({"reasoning_content":"PRIVATE_UNFINISHED_REASONING"}),
                        "length",
                        4096,
                    ),
                    3 => tool(
                        "fs_write",
                        json!({"path":"slice.txt","content":"GROUND_FACT_846: 可运行切片\n"}),
                    ),
                    _ => stream(json!({"content":"已提交切片，保留接口约定。"}), "stop", 12),
                };
                ([("content-type", "text/event-stream")], data)
            }
        }),
    );
    let _provider = Provider(tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap()
    }));
    let app = test_app_with_prompt_isolation();
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "connections":[{"id":"slice","name":"切片桩","enabled":true,
                "connector_type":"openai-compatible","base_url":format!("http://{address}/v1"),
                "api_key":"test-only-key","model":"slice-model"}],
            "task_coding_bundle_enabled":true,"task_tool_policy":"allowlist",
            "task_tool_allowlist":["fs_read","fs_write"],"default_max_tokens":4096,
            "session_token_budget":0,"task_step_budget_secs":30
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let connection_id = body["settings"]["connections"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // 编码能力包会切换任务缺省值；锁定任务档本身，避免把聊天档的 4096 当成实发预算。
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/settings?mode=task",
        json!({"default_max_tokens":4096}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({"title":"按照 interface.txt 实现 slice.txt","task_mode":"solo",
            "workspace":workspace.path().to_string_lossy(),
            "connection_id":connection_id}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["task"]["id"].as_str().unwrap();
    let (status, body) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (state, detail) = wait_terminal(app, id).await;
    if recover {
        assert_eq!(state, "done", "{detail}");
        assert_eq!(
            std::fs::read_to_string(workspace.join("slice.txt")).unwrap(),
            "GROUND_FACT_846: 可运行切片\n"
        );
    } else {
        assert_eq!(state, "error", "{detail}");
        assert!(detail["task"]["error"]
            .as_str()
            .unwrap()
            .contains("聚焦重试仍未返回可执行动作"));
        assert!(!workspace.join("slice.txt").exists());
    }
    let requests = captured.lock().unwrap();
    assert_eq!(
        requests.len(),
        if recover { 4 } else { 3 },
        "只应对推理截断重试一次"
    );
    let last_prompt = |turn: usize| {
        requests[turn]["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["content"]
            .as_str()
            .unwrap()
    };
    assert!(!last_prompt(0).contains("【引擎：当前一轮】"));
    assert!(last_prompt(1).contains("【引擎：当前一轮】"));
    if recover {
        assert!(!last_prompt(3).contains("【引擎：当前一轮】"));
    }
    assert_eq!(requests[1]["max_tokens"], 4096);
    assert_eq!(
        requests[2]["max_tokens"], 4096,
        "聚焦重试不放大本轮输出额度"
    );
    let history = requests[2]["messages"].as_array().unwrap();
    let last = history.last().unwrap()["content"].as_str().unwrap();
    assert!(last.contains("【引擎：继续实施】"), "{last}");
    assert!(
        history
            .iter()
            .any(|m| m["role"] == "tool"
                && m["content"].as_str().unwrap().contains("GROUND_FACT_846"))
    );
    assert!(!requests[2]
        .to_string()
        .contains("PRIVATE_UNFINISHED_REASONING"));
    assert!(
        requests.iter().all(|r| r.get("reasoning_effort").is_none()),
        "未知兼容网关不得收到未声明支持的参数"
    );
}
