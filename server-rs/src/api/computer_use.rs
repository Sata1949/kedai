// 电脑操作治理端点(CU-1,2026-10-06):
//   POST   /api/computer-use/stop          置位急停(前端「停止操作电脑」按钮)
//   POST   /api/computer-use/resume        解除急停(仅用户显式操作)
//   GET    /api/computer-use/status        急停状态(前端据此显示按钮形态)
//   GET    /api/computer-use/audit         操作审计列表(时间倒序,limit 上限 500)
//   DELETE /api/computer-use/audit         清空审计
//
// 安全说明:全部随应用整体受 bootstrap token 鉴权保护(同其它 /api 端点,见 security::guard);
// 审计含范围描述与图像引用名,**不含屏幕像素**(纪律见 services/computer_use.rs)。
use crate::api::app_state::AppState;
use crate::api::errors::{err_with_code, ErrorCode};
use crate::services::computer_use;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// 审计查询参数(与 exec 审计同口径)。
#[derive(Debug, Deserialize)]
pub struct CuAuditQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

/// POST /api/computer-use/stop:置位急停(幂等)。
pub async fn stop(State(state): State<Arc<AppState>>) -> Response {
    state.guards.cu_control.stop();
    Json(json!({ "stopped": true })).into_response()
}

/// POST /api/computer-use/resume:解除急停(幂等)。
pub async fn resume(State(state): State<Arc<AppState>>) -> Response {
    state.guards.cu_control.resume();
    Json(json!({ "stopped": false })).into_response()
}

/// GET /api/computer-use/status:当前急停状态。
pub async fn status(State(state): State<Arc<AppState>>) -> Response {
    Json(json!({ "stopped": state.guards.cu_control.is_stopped() })).into_response()
}

/// GET /api/computer-use/audit:操作审计列表(时间倒序)。
pub async fn list_audit(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CuAuditQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(100);
    let rows = computer_use::list(&state.db, limit);
    Json(json!({ "entries": rows })).into_response()
}

/// DELETE /api/computer-use/audit:清空审计(设置面板「清空」按钮)。
pub async fn clear_audit(State(state): State<Arc<AppState>>) -> Response {
    match computer_use::clear(&state.db) {
        Ok(n) => Json(json!({ "ok": true, "deleted": n })).into_response(),
        Err(e) => err_with_code(ErrorCode::Db, e, StatusCode::INTERNAL_SERVER_ERROR),
    }
}
