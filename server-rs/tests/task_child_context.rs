//! TQUAL-19/20：子 agent 的连接、工作区和只读工具面必须与父任务一致。
use axum::{http::StatusCode, routing::post, Json, Router};
use kedai_server::utils::test_support::TempDataDir;
use serde_json::{json, Value};
use std::sync::atomic::AtomicUsize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

mod common;
use common::{send_json, test_app_with_prompt_isolation, test_lock, wait_terminal_with_attempts};

struct Provider(tokio::task::JoinHandle<()>);
impl Drop for Provider {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn response(delta: Value, reason: &str) -> String {
    response_with_usage(delta, reason, 10)
}

fn response_with_usage(delta: Value, reason: &str, prompt_tokens: u32) -> String {
    let chunk = json!({"id":"context", "choices":[{"index":0,"delta":delta,"finish_reason":null}]});
    let finish = json!({"id":"context", "choices":[{"index":0,"delta":{},"finish_reason":reason}],
        "usage":{"prompt_tokens":prompt_tokens,"completion_tokens":5,"total_tokens":prompt_tokens+5}});
    format!("data: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

fn tool(name: &str, args: Value) -> String {
    tool_with_usage(name, args, 10)
}

fn tool_with_usage(name: &str, args: Value, prompt_tokens: u32) -> String {
    response_with_usage(
        json!({"content":"核对任务上下文。", "tool_calls":[{
            "index":0,"id":"context-call","type":"function",
            "function":{"name":name,"arguments":args.to_string()}
        }]}),
        "tool_calls",
        prompt_tokens,
    )
}

async fn configure(app: &Router, url: &str, default_vision: bool, selected_vision: bool) -> String {
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "connections":[
                {"id":"context-default","name":"默认","connector_type":"openai-compatible",
                 "base_url":url,"api_key":"test-only-key","model":"default-model","enabled":true,
                 "supports_vision":default_vision},
                {"id":"context-selected","name":"任务","connector_type":"openai-compatible",
                 "base_url":url,"api_key":"test-only-key","model":"selected-model","enabled":true,
                 "supports_vision":selected_vision}
            ],
            "task_tool_policy":"allowlist",
            "task_tool_allowlist":["agentgo","fs_read","fs_glob","fs_grep","view_image"],
            "task_step_budget_secs":30,"task_total_budget_secs":60,
            "session_token_budget":0,"task_coding_bundle_enabled":false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "配置失败：{body}");
    body["settings"]["connections"][1]["id"]
        .as_str()
        .unwrap()
        .into()
}

fn tools(body: &Value) -> Vec<&str> {
    body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn task_cannot_finish_done_before_required_browser_verification() {
    let _lock = test_lock().await;
    let workspace = TempDataDir::new("verification-budget");
    std::fs::write(
        workspace.join(".kedai-verify.json"),
        r##"{"entry":"index.html","actions":[{"action":"assert_visible","selector":"#done"}]}"##,
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let provider = Router::new().route("/v1/chat/completions", post(|| async {
        ([("content-type", "text/event-stream")], tool_with_usage("fs_write", json!({"path":"index.html","content":"<!doctype html><p id='done'>已写文件，尚未验收</p>"}), 1024))
    }));
    let _provider = Provider(tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap()
    }));
    let app = test_app_with_prompt_isolation();
    let connection = configure(app, &url, false, false).await;
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "task_coding_bundle_enabled":true,"task_tool_allowlist":["fs_write"],
            "session_token_budget":1024,"session_budget_action":"stop"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, created) = send_json(app, "POST", "/api/tasks", json!({
        "title":"交付带验收契约的网页", "task_mode":"solo", "workspace":workspace.path().to_string_lossy(), "connection_id":connection
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["task"]["id"].as_str().unwrap();
    let (status, body) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, detail) = wait_terminal_with_attempts(app, id, 200).await;
    assert!(
        workspace.join("index.html").is_file(),
        "必须先有真实产物再核对验收终态"
    );
    assert_eq!(
        detail["task"]["status"], "partial",
        "预算结束时不能用已有产物代替必要验收：{detail}"
    );
    assert_eq!(detail["delivery"]["verification"]["status"], "not_run");
    assert_eq!(detail["delivery"]["verification"]["browser_required"], true);
}

#[tokio::test]
async fn child_inherits_selected_connection_and_workspace_readonly_tools() {
    let _lock = test_lock().await;
    let workspace = TempDataDir::new("child-context");
    std::fs::write(workspace.join("sentinel.txt"), "WORKSPACE_SENTINEL_492").unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = requests.clone();
    let child_finished = Arc::new(AtomicBool::new(false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let provider = Router::new().route("/v1/chat/completions", post(move |Json(body): Json<Value>| {
        let captured = captured.clone();
        let child_finished = child_finished.clone();
        async move {
            captured.lock().unwrap().push(body.clone());
            let messages = body["messages"].as_array().unwrap();
            let child = messages.iter().any(|m| m["role"]=="user" && m["content"].as_str().unwrap_or("").starts_with("CHILD-READ:"));
            let result = messages.iter().rev().find(|m| m["role"]=="tool");
            let stream = if child {
                match result {
                    None => tool("fs_read", json!({"path":"sentinel.txt"})),
                    Some(result) => {
                        child_finished.store(true, Ordering::Release);
                        response(json!({"content":result["content"].as_str().unwrap()}), "stop")
                    }
                }
            } else if result.is_none() {
                tool("agentgo", json!({"tasks":[{"name":"核对文件","instruction":"CHILD-READ: 读取 sentinel.txt，返回其中标记；不得写文件。","max_tokens":512}]}))
            } else {
                for _ in 0..100 {
                    if child_finished.load(Ordering::Acquire) { break; }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                response(json!({"content":"子任务已结束。"}), "stop")
            };
            ([("content-type","text/event-stream")], stream)
        }
    }));
    let _provider = Provider(tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap()
    }));
    let app = test_app_with_prompt_isolation();
    let connection = configure(app, &url, false, true).await;
    let (status, created) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({
            "title":"委派子代理核对工作区文件，只读分析，不要求写入文件。","task_mode":"multi",
            "connection_id":connection,"workspace":workspace.path().to_string_lossy()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["task"]["id"].as_str().unwrap();
    let (status, _) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (_, detail) = wait_terminal_with_attempts(app, id, 200).await;
    let requests = requests.lock().unwrap();
    let child = requests
        .iter()
        .find(|r| {
            r["messages"].as_array().unwrap().iter().any(|m| {
                m["role"] == "user"
                    && m["content"]
                        .as_str()
                        .unwrap_or("")
                        .starts_with("CHILD-READ:")
            })
        })
        .expect("子调用应真正发出");
    assert_eq!(
        child["model"], "selected-model",
        "子 agent 不得掉回默认连接"
    );
    let offered = tools(child);
    assert!(
        offered.contains(&"fs_read"),
        "子 agent 必须能读父工作区：{offered:?}"
    );
    for forbidden in ["fs_write", "fs_edit", "bash", "agentgo"] {
        assert!(
            !offered.contains(&forbidden),
            "子 agent 不得扩大权限：{forbidden}"
        );
    }
    assert!(
        detail["subtasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["result"]
                .as_str()
                .unwrap_or("")
                .contains("WORKSPACE_SENTINEL_492")),
        "应读到实际工作区内容：{detail}"
    );
}

#[tokio::test]
async fn task_tools_use_selected_connection_vision_capability() {
    let _lock = test_lock().await;
    let workspace = TempDataDir::new("connection-vision");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let provider = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            captured.lock().unwrap().push(body);
            async {
                (
                    [("content-type", "text/event-stream")],
                    response(json!({"content":"只读核对完成。"}), "stop"),
                )
            }
        }),
    );
    let _provider = Provider(tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap()
    }));
    let app = test_app_with_prompt_isolation();
    for (default_vision, selected_vision) in [(true, false), (false, true)] {
        let connection = configure(app, &url, default_vision, selected_vision).await;
        let (status, created) = send_json(
            app,
            "POST",
            "/api/tasks",
            json!({"title":"只读核对，不要求文件交付","task_mode":"solo",
            "workspace":workspace.path().to_string_lossy(),"connection_id":connection}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = created["task"]["id"].as_str().unwrap();
        send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
        let (_, detail) = wait_terminal_with_attempts(app, id, 100).await;
        assert_eq!(
            detail["delivery"]["verification"]["status"], "not_run",
            "未验收的任务必须明确未执行：{detail}"
        );
        let seen = seen.lock().unwrap();
        let actual = tools(seen.last().unwrap()).contains(&"view_image");
        assert_eq!(
            actual, selected_vision,
            "工具能力必须来自实际任务连接，不能跟随默认连接"
        );
    }
}

#[tokio::test]
async fn selected_vision_connection_receives_real_view_image_parts_and_persists_image() {
    let _lock = test_lock().await;
    let workspace = TempDataDir::new("selected-view-image");
    image::RgbImage::from_pixel(1, 1, image::Rgb([29, 127, 213]))
        .save(workspace.join("shot.png"))
        .unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = requests.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let provider = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            captured.lock().unwrap().push(body);
            let stream = match calls.fetch_add(1, Ordering::SeqCst) {
                0 => tool("view_image", json!({"path":"shot.png"})),
                _ => response(
                    json!({"content":"图片读取完成；这是只读观察，不要求文件交付。"}),
                    "stop",
                ),
            };
            async { ([("content-type", "text/event-stream")], stream) }
        }),
    );
    let _provider = Provider(tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap()
    }));
    let app = test_app_with_prompt_isolation();
    let connection = configure(app, &url, false, true).await;
    // build_test_app 的进程级 DATA_DIR；不读取真实用户目录，也不另套短生命周期守卫。
    let images = std::env::temp_dir()
        .join(format!("kedai-test-{}", std::process::id()))
        .join("images");
    let before: std::collections::HashSet<_> = match std::fs::read_dir(&images) {
        Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(error) => panic!("读取测试图像目录失败：{error}"),
    };
    let (status, created) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({"title":"只读查看 shot.png，不修改或生成文件。","task_mode":"solo",
            "workspace":workspace.path().to_string_lossy(),"connection_id":connection}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["task"]["id"].as_str().unwrap();
    let (status, started) =
        send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let (_, detail) = wait_terminal_with_attempts(app, id, 200).await;
    assert_eq!(
        detail["task"]["status"], "done",
        "真实图像回灌后应完成：{detail}"
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "应先调用 view_image，再读取实际图像回复");
    assert!(tools(&requests[0]).contains(&"view_image"));
    for request in requests.iter() {
        assert_eq!(
            request["model"], "selected-model",
            "视觉调用必须使用所选连接"
        );
    }
    let messages = requests[1]["messages"].as_array().unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message["role"] == "tool" && message["tool_call_id"] == "context-call"),
        "工具返回必须与首轮调用配对"
    );
    let image_parts: Vec<&Value> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "image_url")
        .collect();
    assert_eq!(
        image_parts.len(),
        1,
        "第二次请求必须带一张真实图像，而非只有文字说明"
    );
    let data_url = image_parts[0]["image_url"]["url"].as_str().unwrap();
    assert!(data_url.starts_with("data:image/png;base64,"));
    let (mime, bytes) =
        kedai_server::services::image_service::ImageService::decode_data_url("shot.png", data_url)
            .unwrap();
    assert_eq!(mime, "image/png");
    let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
    assert_eq!(decoded.dimensions(), (1, 1));
    assert_eq!(decoded.get_pixel(0, 0).0, [29, 127, 213]);
    let persisted = std::fs::read_dir(&images)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && !before.contains(path))
        .collect::<Vec<_>>();
    assert_eq!(persisted.len(), 1, "view_image 应新增一张持久化图像");
    assert_eq!(
        std::fs::read(&persisted[0]).unwrap(),
        bytes,
        "模型拿到的像素必须来自实际落盘文件"
    );
}

#[tokio::test]
async fn verification_failure_repairs_inside_original_live_context() {
    let _lock = test_lock().await;
    let workspace = TempDataDir::new("live-verification");
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let provider = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            captured.lock().unwrap().push(body);
            let response = match calls.fetch_add(1, Ordering::SeqCst) {
                0 => tool(
                    "fs_write",
                    json!({"path":".kedai-verify.json","content":"{\"entry\":42}"}),
                ),
                1 => response(json!({"content":"准备交付，尚未校验验收配置。"}), "stop"),
                2 => tool("fs_read", json!({"path":".kedai-verify.json"})),
                3 => tool(
                    "fs_write",
                    json!({"path":".kedai-verify.json","content":"{\"actions\":[]}"}),
                ),
                _ => response(
                    json!({"content":"已修复验收配置；没有浏览器入口，功能仍未验收。"}),
                    "stop",
                ),
            };
            async { ([("content-type", "text/event-stream")], response) }
        }),
    );
    let _provider = Provider(tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap()
    }));
    let app = test_app_with_prompt_isolation();
    let connection = configure(app, &url, false, false).await;
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"task_coding_bundle_enabled":true,"task_tool_allowlist":["fs_read","fs_write"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let goal = "ORIGINAL_ACCEPTANCE_81：创建可解析的验收配置，不能伪报网页已通过";
    let (status,created)=send_json(app,"POST","/api/tasks",json!({"title":goal,"task_mode":"solo","workspace":workspace.path().to_string_lossy(),"connection_id":connection})).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["task"]["id"].as_str().unwrap();
    send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    let (_, detail) = wait_terminal_with_attempts(app, id, 200).await;
    assert_eq!(detail["task"]["status"], "done", "{detail}");
    assert!(
        count.load(Ordering::SeqCst) >= 5,
        "必须在首轮结束后收到失败并修复"
    );
    {
        let requests = seen.lock().unwrap();
        let repair = &requests[2]["messages"].as_array().unwrap();
        assert!(repair.iter().any(
            |m| m["role"] == "user" && m["content"].as_str().is_some_and(|s| s.contains(goal))
        ));
        assert!(
            repair.iter().any(|m| m["role"] == "tool"),
            "原工具历史必须继续存活"
        );
        assert!(repair.iter().any(|m| m["content"]
            .as_str()
            .is_some_and(|s| s.contains("验收步骤格式错误"))));
        let feedback = repair
            .iter()
            .filter_map(|m| m["content"].as_str())
            .find(|s| s.starts_with("[独立验收证据]"))
            .unwrap();
        assert!(
            !feedback.contains(goal),
            "原目标已完整保留，不应把它作为新指令再次下发"
        );
        assert!(
            feedback.contains("不是新的任务"),
            "明确只处理新证据，不重跑已完成步骤"
        );
    }
    assert_eq!(detail["delivery"]["verification"]["status"], "not_run");
    assert_eq!(
        std::fs::read_to_string(workspace.join(".kedai-verify.json")).unwrap(),
        "{\"actions\":[]}"
    );
    // 续跑必须冻结本轮用户指令，不能从 tasks.title 重新取旧要求而抹掉修订。
    let (status, body) = send_json(
        app,
        "POST",
        &format!("/api/tasks/{id}/followup"),
        json!({"content":"NEW_REQUIREMENT_92：只核对已有配置并说明未验证事项，不新增文件。"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, followup) = wait_terminal_with_attempts(app, id, 200).await;
    assert!(
        followup["delivery"]["verification"]["original_goal"]
            .as_str()
            .is_some_and(|goal| goal.contains("NEW_REQUIREMENT_92")),
        "验收目标必须包含本轮追加指令：{followup}"
    );
}
