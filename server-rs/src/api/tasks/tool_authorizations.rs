// 任务待确认工具调用快照端点（2026-11-06 三档授权；自 tasks.rs 拆出）。
//
// **为什么需要它**：待授权事件是瞬态广播（不落 `task_events`）——重放旧的待授权会让
// 用户对早已超时的调用点头。刷新/断线重连后，前端靠本端点恢复队列，决议仍走
// `POST /api/agent/tool-permissions/resolve`。
use crate::api::app_state::AppState;
use crate::api::{db_err, not_found};
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

/// GET /api/tasks/{id}/tool-authorizations
///
/// 任务不存在回 404（不返回空数组把「任务没了」伪装成「没有待确认」）。
pub async fn list_tool_authorizations(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let svc = state.tasks.clone();
    let tid = id.clone();
    match state.db_call(move || svc.get(&tid)).await {
        Err(e) => db_err(&e),
        Ok(None) => not_found("任务不存在"),
        Ok(Some(_)) => {
            let pending = state.tool_registry.permissions().pending_for_task(&id);
            Json(json!({ "pending": pending })).into_response()
        }
    }
}
