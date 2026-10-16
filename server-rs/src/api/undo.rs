// 回退快照 API(批次 6.1「undo」):
//   GET  /api/chat/sessions/{id}/undo  列出会话回退快照(新→旧,不含 payload)
//   POST /api/undo/{id}/restore        按快照逆序执行逆操作恢复
// 快照产生点在 tools/registry.rs run_tool(两段式);本模块只做列表与恢复入口。
// 同步 DB/文件操作经 state.db_call 挪进阻塞线程池(DB 并发改造惯例)。
use crate::api::app_state::AppState;
use crate::api::{db_err, err_status};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

/// GET /api/chat/sessions/{id}/undo:快照列表(新→旧)
pub async fn list_snapshots(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Response {
    if session_id.trim().is_empty() {
        return err_status("缺少 session_id", StatusCode::BAD_REQUEST);
    }
    let undo = state.undo.clone();
    match state.db_call(move || undo.list(&session_id)).await {
        Err(e) => db_err(&e),
        Ok(snapshots) => Json(json!({ "snapshots": snapshots })).into_response(),
    }
}

/// POST /api/undo/{id}/restore:按快照恢复(批 D-2 语义 = 回到所选快照**之前**的状态:
/// 取目标及同会话后续全部快照,逆序应用逆操作;成功后精确清理已处理快照)。
/// 生成进行中拒绝(见下)——恢复与在途写入并发会互相踩;失败返回 500 中文错误,
/// 快照记录保留(逆操作幂等,可修复原因后原样重试)。
pub async fn restore_snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if id.trim().is_empty() {
        return err_status("缺少快照 id", StatusCode::BAD_REQUEST);
    }
    // 冲突协调(批 D-2 的第一道,进程内):该会话正在生成时不允许回退——生成中的
    // 工具写与恢复的逆操作会交错。跨会话同角色的文件写冲突为已登记边界(见遗留)。
    let undo = state.undo.clone();
    let id_for_lookup = id.clone();
    let session_id = state
        .db_call(move || undo.session_of(&id_for_lookup))
        .await
        .ok()
        .flatten();
    if let Some(sid) = session_id {
        if state.engine.is_active(&sid) {
            return err_status("该会话正在生成中,请先停止生成再回退", StatusCode::CONFLICT);
        }
    }
    let undo = state.undo.clone();
    match state.db_call(move || undo.restore(&id)).await {
        Err(e) => db_err(&e),
        Ok(Err(e)) => err_status(&e, StatusCode::INTERNAL_SERVER_ERROR),
        Ok(Ok(None)) => err_status("快照不存在或已被回退", StatusCode::NOT_FOUND),
        Ok(Ok(Some(receipt))) => Json(json!({
            "ok": true,
            "restored": receipt.label,
            "restored_count": receipt.restored_count,
        }))
        .into_response(),
    }
}
