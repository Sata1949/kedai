// MCP 管理面 API(PLGM 3.1):状态查询 + 单台启停/重启。
//
// 端点:GET /api/mcp/servers、POST /api/mcp/servers/{name}/{restart|stop|start}。
// 语义:MCP 是**进程级全局能力**(不支持模式级覆盖,见 docs/契约-协议与配置.md);
// restart/start 读**当前扁平设置快照**取该条配置——「改配置 → 一键重启生效」由此成立
// (替代旧版「改设置需重启应用」)。手动 start/stop 属显式动作,不受该条 `enabled` 阻断。
use crate::api::app_state::AppState;
use crate::api::err_status;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

/// GET /api/mcp/servers:设置快照 × 运行时台账的状态列表。
///
/// `state` 四值:`running / failed / stopped / disabled`。派生规则:运行实况
/// (`running`/`failed`)优先于设置派生;无实况(或实况为 stopped)时,总开关关或该条
/// `enabled=false` 报 `disabled`,否则报 `stopped`。
pub async fn list_servers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let snapshot = state.settings_snapshot();
    let runtimes: std::collections::HashMap<String, crate::mcp::ServerSnapshot> = state
        .mcp
        .snapshot()
        .into_iter()
        .map(|s| (s.name.clone(), s))
        .collect();
    let mut servers: Vec<serde_json::Value> = snapshot
        .mcp_servers
        .iter()
        .map(|cfg| {
            let rt = runtimes.get(&cfg.name);
            let st = match rt.map(|r| r.state) {
                // 运行实况优先:手动启停过的已停用条目也能看到真实状态
                Some(s) if s == "running" || s == "failed" => s,
                _ => {
                    if !snapshot.mcp_enabled || !cfg.enabled {
                        "disabled"
                    } else {
                        rt.map(|r| r.state).unwrap_or("stopped")
                    }
                }
            };
            json!({
                "name": cfg.name,
                "enabled": cfg.enabled,
                "state": st,
                "tool_count": rt.map(|r| r.tool_names.len()).unwrap_or(0),
                "tools": rt.map(|r| r.tool_names.clone()).unwrap_or_default(),
                "last_error": rt.and_then(|r| r.last_error.clone()),
            })
        })
        .collect();
    // 反向补列:设置里已删但进程仍在(未重启前)——如实展示,不静默隐藏
    let mut extra: Vec<serde_json::Value> = runtimes
        .iter()
        .filter(|(name, _)| !snapshot.mcp_servers.iter().any(|c| &c.name == *name))
        .map(|(name, rt)| {
            json!({
                "name": name,
                "enabled": false,
                "state": rt.state,
                "tool_count": rt.tool_names.len(),
                "tools": rt.tool_names.clone(),
                "last_error": rt.last_error.clone(),
            })
        })
        .collect();
    extra.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    servers.extend(extra);
    Json(json!({ "servers": servers, "mcp_enabled": snapshot.mcp_enabled }))
}

/// POST /api/mcp/servers/{name}/restart:以当前设置快照重启单台(改配置一键生效)。
/// 响应回最新状态;「服务器启动失败」不算 HTTP 错误(原因在 last_error,由 GET 呈现)。
pub async fn restart(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    let Some(cfg) = find_config(&state, &name) else {
        return err_status("未找到该 MCP 服务器(请核对名称)", StatusCode::NOT_FOUND);
    };
    state.mcp.restart(cfg).await;
    status_response(&state, &name)
}

/// POST /api/mcp/servers/{name}/stop:停单台并注销其工具(state=stopped,配置保留)。
pub async fn stop(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    if !state.mcp.stop(&name).await {
        return err_status("未找到该 MCP 服务器(请核对名称)", StatusCode::NOT_FOUND);
    }
    status_response(&state, &name)
}

/// POST /api/mcp/servers/{name}/start:显式启动(幂等:已在运行即回当前状态)。
pub async fn start(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    let Some(cfg) = find_config(&state, &name) else {
        return err_status("未找到该 MCP 服务器(请核对名称)", StatusCode::NOT_FOUND);
    };
    state.mcp.start_manual(cfg).await;
    status_response(&state, &name)
}

/// 当前扁平设置里的服务器配置(restart/start 的「现行值」来源)
fn find_config(
    state: &AppState,
    name: &str,
) -> Option<crate::models::tool_policy::McpServerConfig> {
    state
        .settings_snapshot()
        .mcp_servers
        .iter()
        .find(|s| s.name == name)
        .cloned()
}

/// 单台最新状态响应(ok/name/state/tool_count)
fn status_response(state: &AppState, name: &str) -> Response {
    let rt = state.mcp.snapshot().into_iter().find(|s| s.name == name);
    Json(json!({
        "ok": true,
        "name": name,
        "state": rt.as_ref().map(|r| r.state).unwrap_or("stopped"),
        "tool_count": rt.as_ref().map(|r| r.tool_names.len()).unwrap_or(0),
    }))
    .into_response()
}
