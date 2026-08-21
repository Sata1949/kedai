// 路由子模块共享的响应工具(自 api/mod.rs 迁入,路径经 mod.rs 再导出保持不变)
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// 便捷:带状态码的 JSON 响应(各路由子模块 use super::WithStatus)
pub trait WithStatus {
    fn with_status(self, code: StatusCode) -> Response;
}

impl WithStatus for Response {
    fn with_status(mut self, code: StatusCode) -> Response {
        *self.status_mut() = code;
        self
    }
}

/// DB 阻塞任务失败统一 500 响应(2026-08 DB 并发改造:
/// spawn_blocking JoinError / 连接池错误 / 服务内 String 错误)
pub(crate) fn db_err(e: &str) -> Response {
    Json(json!({ "error": e }))
        .into_response()
        .with_status(StatusCode::INTERNAL_SERVER_ERROR)
}
