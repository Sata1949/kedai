//! 数据迁移维护态入口；鉴权在外层完成后才领取操作租约。
use crate::api::{app_state::AppState, conflict};
use axum::{
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

pub(super) async fn guard(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let query_only = request.method() == Method::GET
        && matches!(
            path,
            "/api/health" | "/api/bootstrap" | "/api/data-location" | "/api/tasks/events"
        );
    let migrating = request.method() == Method::PUT && path == "/api/data-location";
    if !path.starts_with("/api/") || query_only || migrating {
        return next.run(request).await;
    }
    let lease = match state.db.data_activity.enter() {
        Ok(lease) => lease,
        Err(message) => return conflict(message),
    };
    crate::models::data_activity::scope(lease, next.run(request)).await
}
