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

/// 选择目标:桌面盘符 / Android 绝对路径根(二者互斥,root 优先)。
enum Target {
    Drive(String),
    Root(String),
}

/// GET /api/data-location
pub async fn get(State(state): State<Arc<AppState>>) -> Response {
    Json(state.data_location.status()).into_response()
}

#[derive(Deserialize)]
pub struct SelectRequest {
    /// 盘符(桌面;接受 `d` / `D` / `D:` / `D:\`)
    #[serde(default)]
    pub drive: Option<String>,
    /// 绝对路径根(Android 路径模式;默认 `/storage/emulated/0/Download/Kedai-Data`)
    #[serde(default)]
    pub root: Option<String>,
}

/// PUT /api/data-location:选择盘符(桌面)或绝对路径(Android)
/// → 建根 → 迁移旧数据 → 写指针。
pub async fn put(
    State(state): State<Arc<AppState>>,
    JsonBody(req): JsonBody<SelectRequest>,
) -> Response {
    let svc = state.data_location.clone();
    // root 优先:桌面旧客户端只发 drive,行为不变;Android 发 root。
    let target = match (req.root, req.drive) {
        (Some(root), _) => Target::Root(root),
        (None, Some(drive)) => Target::Drive(drive),
        (None, None) => {
            return validation("缺少 drive 或 root:请提供盘符或数据根路径");
        }
    };
    let joined = tokio::task::spawn_blocking(move || match target {
        Target::Root(root) => svc.apply_root_path(&root),
        Target::Drive(drive) => svc.apply(&drive),
    })
    .await;
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
