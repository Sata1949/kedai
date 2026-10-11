// 全局正则库 API(IMPHUB ⑥,2026-10-09):GET/PUT /api/regex-scripts(整体读写)。
//
// 语义与 /api/prompt-inject 同款:**全量替换**——综合导入⑥、启停、删除都走同一个 PUT,
// 不另开单条 CRUD(条目少、前端整表持有一份,整体写天然没有部分更新的竞态)。
// 条目**宽进**由服务层归一(`services/regex_service.rs`),本层只管取锁与落盘。
use crate::api::app_state::AppState;
use crate::api::json_body::JsonBody;
use crate::api::{err_with_code, ErrorCode};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

/// GET /api/regex-scripts:读全局正则库(整体)
pub async fn get_regex_scripts(State(state): State<Arc<AppState>>) -> Response {
    let scripts = state
        .regex
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get()
        .scripts
        .clone();
    Json(json!({ "ok": true, "scripts": scripts })).into_response()
}

/// PUT /api/regex-scripts:全量替换全局正则库。
/// 入参收 `{scripts: [...]}` 与裸数组两种顶层形态;条目逐项归一,取不到查找模式的
/// 条目跳过并在 `skipped` 里如实回报(不整单拒绝——同 prompt-inject 的宽进口径)。
pub async fn update_regex_scripts(
    State(state): State<Arc<AppState>>,
    JsonBody(body): JsonBody<serde_json::Value>,
) -> Response {
    let svc = state.regex.clone();
    let result = state
        .db_call(move || {
            let mut svc = svc.lock().unwrap_or_else(|e| e.into_inner());
            svc.set(body)
                .map(|(count, skipped)| (count, skipped, svc.get().scripts.clone()))
        })
        .await;
    match result {
        Ok(Ok((count, skipped, scripts))) => Json(json!({
            "ok": true,
            "count": count,
            "skipped": skipped,
            "scripts": scripts,
        }))
        .into_response(),
        // 泄露封堵(同 prompt-inject):写盘错误原文(可能含盘符路径)只进日志,响应给稳定文案 + code
        Ok(Err(e)) | Err(e) => {
            tracing::error!(error = %e, "保存全局正则库失败");
            err_with_code(
                ErrorCode::Internal,
                "保存失败,详情见服务端日志",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        }
    }
}
