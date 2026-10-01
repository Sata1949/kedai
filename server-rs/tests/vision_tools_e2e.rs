// 视觉工具三件 · 端到端闸门与真执行(视觉能力包 D4,2026-10-02)。
//
// 两问:**未开视觉能力位时** mock 的 `[[tool:view_image]]` 钩子不会命中(view_image 不在
// 本轮下发工具面)→ 事件流里没有该工具调用、不产生图像文件;**开启后**同一题面真跑一次
// view_image → 事件流出现「调用/返回」且图像落入 DATA_DIR/images。
//
// 为什么单独一个文件:本用例要翻转连接能力位,而连接与设置是进程内共享全局
// (仓库既有裁定:共享全局状态的用例拆文件 + 独立 DATA_DIR,见 coding_pack_patch_tool.rs 头注)。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use kedai_server::utils::test_support::TempDataDir;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;
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

/// 打开任务事件流(体例照 `task_events.rs`)
async fn open_event_stream(app: &axum::Router) -> Body {
    let req = Request::builder()
        .method("GET")
        .uri("/api/tasks/events")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    resp.into_body()
}

/// 从 SSE body 读下一条 data 帧并解析(体例照 `task_events.rs`)
async fn read_event(body: &mut Body) -> Option<Value> {
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

/// 收齐该任务到终态的事件(体例照 `task_events.rs`)
async fn collect_until_terminal(body: &mut Body, task_id: &str) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut seq: Vec<Value> = Vec::new();
    loop {
        let got = tokio::time::timeout_at(deadline, read_event(body)).await;
        match got {
            Ok(Some(ev)) => {
                if ev["type"].as_str() == Some("task") && ev["task_id"].as_str() == Some(task_id) {
                    let terminal = ev["kind"].as_str() == Some("status")
                        && matches!(
                            ev["status"].as_str(),
                            Some("done") | Some("partial") | Some("error") | Some("ended")
                        );
                    seq.push(ev);
                    if terminal {
                        return seq;
                    }
                }
            }
            _ => panic!("等待任务 {task_id} 事件超时或流中断,已收 {} 条", seq.len()),
        }
    }
}

/// 建任务 → 跑 → 收事件到终态;返回 (任务 id, 事件序列)
async fn run_task(app: &axum::Router, title: &str, ws: &str) -> (String, Vec<Value>) {
    let mut body = open_event_stream(app).await;
    let (status, created) = send_json(
        app,
        "POST",
        "/api/tasks",
        json!({ "title": title, "task_mode": "solo", "workspace": ws }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建任务失败: {created}");
    let id = created["task"]["id"].as_str().unwrap().to_string();
    let (status, r) = send_json(app, "POST", &format!("/api/tasks/{id}/run"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "run 应 200: {r}");
    let seq = collect_until_terminal(&mut body, &id).await;
    let last = seq.last().cloned().unwrap_or(Value::Null);
    assert!(
        matches!(last["status"].as_str(), Some("done") | Some("partial")),
        "任务应正常完成: {last}"
    );
    (id, seq)
}

/// 事件序列里是否出现「调用/返回该工具」的进展(排除策略拒绝形态)
fn tool_engaged(seq: &[Value], name: &str) -> bool {
    seq.iter().any(|e| {
        e["detail"].as_str().is_some_and(|d| {
            (d.contains(&format!("调用工具 {name}")) || d.contains(&format!("工具 {name}")))
                && !d.contains("被任务策略拒绝")
        })
    })
}

/// DATA_DIR(与 `build_test_app` 同一公式:`%TEMP%\kedai-test-<pid>`)
fn test_data_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("kedai-test-{}", std::process::id()))
}

/// 统计 DATA_DIR/images 顶层的图像文件数(派生目录不计)
fn count_image_files() -> usize {
    std::fs::read_dir(test_data_dir().join("images"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .count()
        })
        .unwrap_or(0)
}

/// 1x1 PNG(真实魔数**且 CRC 合法**——D4 起图像要真解码,魔数对但 CRC 错的字节会被
/// `image` 解码器拒绝;此字节由 zlib 正确编码生成)
const PNG_1PX: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x38, 0x21, 0x27, 0x07,
    0x00, 0x02, 0xB6, 0x01, 0x05, 0x0A, 0x5B, 0xA6, 0x06, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[tokio::test]
async fn view_image_gated_by_vision_capability_and_runs_when_enabled() {
    let app = test_app();
    let ws_guard = TempDataDir::new("d4-ws");
    let ws = ws_guard.path().to_string_lossy().into_owned();
    std::fs::write(ws_guard.path().join("shot.png"), PNG_1PX).unwrap();

    let title = r#"[[tool:view_image {"path":"shot.png"}]] 查看图片"#;

    // ① 连接未开「视觉输入」(基线为 mock 无能力位):view_image 不在工具面 →
    //    mock 钩子不命中 → 事件流无该工具、任务照常 done、零图像落盘
    let before = count_image_files();
    let (_, seq_off) = run_task(app, title, &ws).await;
    assert!(
        !tool_engaged(&seq_off, "view_image"),
        "未开视觉位时 view_image 不得进入工具循环: {seq_off:?}"
    );
    assert_eq!(
        count_image_files(),
        before,
        "未开视觉位时 view_image 不得被执行(不应产生图像文件)"
    );

    // ② 开启视觉能力位:同一题面真跑 view_image → 事件流出现调用/返回 + 图像落入 images
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({"connections": [{
            "name": "视觉基线", "connector_type": "mock",
            "base_url": "", "model": "", "enabled": true,
            "supports_vision": true
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "开启视觉位失败: {body}");
    let (_, seq_on) = run_task(app, title, &ws).await;
    assert!(
        tool_engaged(&seq_on, "view_image"),
        "开启视觉位后 view_image 应进入工具循环: {seq_on:?}"
    );
    assert!(
        count_image_files() > before,
        "开启视觉位后 view_image 应真执行并把图像写入 DATA_DIR/images;事件: {seq_on:?}"
    );
}
