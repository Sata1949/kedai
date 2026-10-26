// MCP 管理面端点集成测试(PLGM 3.1):状态列表形状 / 未知服务器 404 / 无服务器空态 /
// 手动 start 不受 enabled 阻断且失败如实落状态。
//
// 测试辅助函数已收敛到 `tests/common`(2026-10-25 C-2E):`test_app` / `send_json` /
// `test_lock` / `wait_terminal` 只保留一份实现;每个测试文件 `mod common;` 仍编译为
// 独立测试二进制,进程级 app 与串行锁语义不变。`test_lock()` 仍只在本二进制内串行。
use axum::http::StatusCode;
use serde_json::json;
mod common;
use common::{send_json, test_app, test_lock};

/// 管理面顺序用例(单条,自洽不依赖执行顺序;共享 DATA_DIR/运行时台账):
/// ① 空态重置 → servers 空 + mcp_enabled=false;② 配置一台(禁用、命令不存在)→ 形状与派生;
/// ③ 未知服务器 404;④ 手动 start 不受 enabled 阻断、失败如实落状态;⑤ stop → stopped。
#[tokio::test]
async fn mcp_servers_state_shape_and_actions() {
    let app = test_app();
    let _guard = test_lock().await;

    // ① 空态重置(HTTP 层自重置:不依赖别处未写设置;本二进制此前无组装,台账为空)
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({ "mcp_enabled": false, "mcp_servers": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send_json(app, "GET", "/api/mcp/servers", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mcp_enabled"], json!(false), "重置后总开关关: {body}");
    assert!(
        body["servers"].as_array().is_some_and(|a| a.is_empty()),
        "重置后无服务器: {body}"
    );

    // ② 配置一台 disabled 的服务器(不会在启动装配里起进程)→ 列表如实回形状
    let (status, _) = send_json(
        app,
        "PUT",
        "/api/settings",
        json!({
            "mcp_enabled": true,
            "mcp_servers": [ { "name": "demo", "command": "kedai-no-such-cmd-9z8y7x", "args": [], "enabled": false } ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send_json(app, "GET", "/api/mcp/servers", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let servers = body["servers"].as_array().expect("servers 应为数组");
    let demo = servers
        .iter()
        .find(|s| s["name"] == json!("demo"))
        .expect("应列出配置条目");
    assert_eq!(demo["enabled"], json!(false));
    assert_eq!(
        demo["state"],
        json!("disabled"),
        "无实况时按设置派生: {demo}"
    );
    assert_eq!(demo["tool_count"], json!(0));
    assert!(demo["tools"].as_array().is_some_and(|a| a.is_empty()));

    // ③ 未知服务器:restart/stop/start 均 404
    for action in ["restart", "stop", "start"] {
        let (status, body) = send_json(
            app,
            "POST",
            &format!("/api/mcp/servers/nope/{action}"),
            json!({}),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{action} 未知服务器应 404: {body}"
        );
    }

    // ④ 手动 start:不受该条 enabled 阻断;命令不存在 → 200 + state=failed(启动失败不算 HTTP 错误)
    let (status, body) = send_json(app, "POST", "/api/mcp/servers/demo/start", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["state"],
        json!("failed"),
        "启动失败应如实回状态: {body}"
    );
    assert_eq!(body["tool_count"], json!(0));
    // GET:运行实况优先于设置派生,failed 与 last_error 可见
    let (_, body) = send_json(app, "GET", "/api/mcp/servers", json!({})).await;
    let demo = body["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == json!("demo"))
        .unwrap();
    assert_eq!(demo["state"], json!("failed"), "运行实况应优先: {demo}");
    assert!(
        demo["last_error"]
            .as_str()
            .unwrap_or("")
            .contains("启动失败"),
        "失败原因应可见: {demo}"
    );

    // ⑤ 幂等 stop:命中服务器 → 200 + state=stopped(工具已注销,配置保留)
    let (status, body) = send_json(app, "POST", "/api/mcp/servers/demo/stop", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("stopped"));
}
