// 用户设定 API(USERPERSONA-1):角色扮演专属的全局用户角色配置。
// GET/PUT /api/user-persona;配置独立落 DATA_DIR/user_persona.json。
use crate::api::app_state::AppState;
use crate::api::json_body::JsonBody;
use crate::api::{err_with_code, validation, ErrorCode};
use crate::services::user_persona_service::UserPersonaConfig;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 响应线格式:`{ ok, config }`。GET 即使 enabled=false 也返回已存文本,
/// 便于用户切换开关不丢内容。
#[derive(Serialize)]
pub struct UserPersonaResponse {
    pub ok: bool,
    pub config: UserPersonaConfig,
}

/// GET /api/user-persona:读取用户设定(总是返回当前落盘/内存值)
pub async fn get(State(state): State<Arc<AppState>>) -> Response {
    let config = state
        .user_persona
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get()
        .clone();
    Json(UserPersonaResponse { ok: true, config }).into_response()
}

/// PUT body:全量配置
#[derive(Debug, Deserialize)]
pub struct UpdateUserPersonaBody {
    pub config: UserPersonaConfig,
}

/// PUT /api/user-persona:归一化 + 校验(name ≤64 / description ≤4000)→ 原子落盘。
pub async fn update(
    State(state): State<Arc<AppState>>,
    JsonBody(body): JsonBody<UpdateUserPersonaBody>,
) -> Response {
    let svc = state.user_persona.clone();
    let result = state
        .db_call(move || {
            let mut svc = svc.lock().unwrap_or_else(|e| e.into_inner());
            svc.set(body.config).map(|()| svc.get().clone())
        })
        .await;
    match result {
        Ok(Ok(config)) => Json(UserPersonaResponse { ok: true, config }).into_response(),
        // 校验失败文案含字段名,便于用户定位;写盘错误原文(可能含路径)只进日志。
        Ok(Err(e)) => validation(e),
        Err(e) => {
            tracing::error!(error = %e, "保存用户设定失败");
            err_with_code(
                ErrorCode::Internal,
                "保存失败,详情见服务端日志",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        }
    }
}
