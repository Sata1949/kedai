//! 数据位置接口:查询当前/候选盘符,选择盘符并迁移旧数据(2026-11 批次)。
//!
//! 迁移是阻塞 IO,放 `spawn_blocking`;错误映射:非法盘符/目标冲突 → 400,
//! 有任务在跑 → 409,其余 IO 失败 → 500(原文透传,便于用户按路径/权限自行处理)。
use crate::api::app_state::AppState;
use crate::api::json_body::JsonBody;
use crate::api::{conflict, err_status, validation};
use crate::services::data_location_service::DataLocationError;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;

/// GET /api/data-location
pub async fn get(State(state): State<Arc<AppState>>) -> Response {
    Json(state.data_location.status()).into_response()
}

#[derive(Deserialize)]
pub struct SelectRequest {
    /// 盘符(接受 `d` / `D` / `D:` / `D:\`)
    pub drive: String,
}

/// PUT /api/data-location:选择盘符 → 建 `<盘>:\Kedai-Data` → 迁移旧数据 → 写指针。
pub async fn put(
    State(state): State<Arc<AppState>>,
    JsonBody(req): JsonBody<SelectRequest>,
) -> Response {
    let svc = state.data_location.clone();
    let drive = req.drive;
    let joined = tokio::task::spawn_blocking(move || svc.apply(&drive)).await;
    match joined {
        Ok(Ok(outcome)) => Json(outcome).into_response(),
        Ok(Err(DataLocationError::BadRequest(msg))) => validation(msg),
        Ok(Err(DataLocationError::Busy(msg))) => conflict(msg),
        Ok(Err(DataLocationError::Internal(detail))) => {
            err_status(detail, StatusCode::INTERNAL_SERVER_ERROR)
        }
        Err(e) => err_status(
            format!("数据位置迁移任务失败: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}
