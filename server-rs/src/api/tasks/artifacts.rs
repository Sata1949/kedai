// 任务草稿产物与回收端点(PRODCAP-5)。
//
// 从 `api/tasks.rs` 搬出(ATTACH-1,纯代码移动,逻辑与线格式逐字不变):该文件被
// `tools/file-size-baseline.json` 锁在只降不升,而任务附件要新增三个端点。搬出的
// 三组端点彼此内聚(都只操作 scratch 目录),与主 CRUD 没有共享的私有 helper。
use crate::api::app_state::AppState;
use crate::api::{db_err, internal, not_found, validation};
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// GET /api/tasks/{id}/artifacts:任务 scratch 产物清单(PRODCAP-5)。
///
/// 只列**文件**(递归;目录不入清单),相对路径统一 `/` 分隔(跨平台线格式)。
/// 任务不存在 → 404;从未落过产物(目录不存在)→ `files: []`(空数组,不是 404,
/// 与变更清单同款「不猜语义」纪律)。条目上限 1000 / 深度 16,触顶 `truncated: true`。
pub async fn task_artifacts(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if state.tasks.get(&id).is_none() {
        return not_found("任务不存在");
    }
    let tasks = state.tasks.clone();
    let list_id = id.clone();
    match state
        .db_call(move || tasks.list_task_artifacts(&list_id))
        .await
    {
        Err(e) => db_err(&e),
        Ok((files, truncated)) => Json(json!({
            "files": files
                .into_iter()
                .map(|f| json!({
                    "path": f.path,
                    "size": f.size,
                    // mtime 取不到如实回 null,不猜(调用方显示「时间未知」)
                    "mtime": f.mtime.map(|t| t.to_rfc3339()),
                }))
                .collect::<Vec<_>>(),
            "truncated": truncated,
        }))
        .into_response(),
    }
}

#[derive(Deserialize)]
pub struct ArtifactDownloadQuery {
    /// 清单里的相对路径(整体 URL 编码)
    pub path: String,
}

/// GET /api/tasks/{id}/artifacts/download?path=:下载任务 scratch 单个产物(PRODCAP-5)。
///
/// 路径经与 `fs_*` 工具族**同一把**闸门(`tools::workspace_guard::safe_workspace_path`,
/// 根 = 该任务 scratch 目录):越界/符号链接/NUL → 400 + 原文原因(用户可纠正的输入问题),
/// 文件不存在 → 404。响应 `application/octet-stream` + `Content-Disposition`(RFC 5987:
/// 中文文件名走 `filename*=UTF-8''…`,裸 header 不得塞非 ASCII)。
pub async fn task_artifact_download(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<ArtifactDownloadQuery>,
) -> Response {
    if state.tasks.get(&id).is_none() {
        return not_found("任务不存在");
    }
    let Some(root) = state.tasks.existing_scratch_dir(&id) else {
        return not_found("该任务没有产物目录(尚未产生过程文件)");
    };
    let data_dir = state.config.data_dir.clone();
    let abs =
        match crate::tools::workspace_guard::safe_workspace_path(&root, &q.path, Some(&data_dir)) {
            Ok(p) => p,
            // 闸门拒绝是用户可纠正的输入问题(越界/符号链接/NUL),走 400(VALIDATION)
            Err(e) => return validation(e),
        };
    let file = state
        .db_call(move || -> Result<(std::path::PathBuf, Vec<u8>), String> {
            if !abs.is_file() {
                return Err(String::from("__not_found__"));
            }
            std::fs::read(&abs)
                .map(|bytes| (abs, bytes))
                .map_err(|e| format!("读取产物失败:{e}"))
        })
        .await;
    match file {
        Err(e) => db_err(&e),
        Ok(Err(reason)) if reason == "__not_found__" => not_found("产物文件不存在"),
        Ok(Err(e)) => internal(e),
        Ok(Ok((p, bytes))) => {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "artifact".into());
            (
                [
                    (
                        axum::http::header::CONTENT_TYPE,
                        String::from("application/octet-stream"),
                    ),
                    (
                        axum::http::header::CONTENT_DISPOSITION,
                        content_disposition(&name),
                    ),
                ],
                bytes,
            )
                .into_response()
        }
    }
}

/// RFC 5987 的 Content-Disposition:ASCII 兜底 + UTF-8 百分号编码(header 值须为可见 ASCII)
fn content_disposition(name: &str) -> String {
    let mut ascii = String::new();
    let mut enc = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_') {
            ascii.push(b as char);
            enc.push(b as char);
        } else {
            ascii.push('_');
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{enc}")
}

/// POST /api/tasks/scratch/cleanup:立即回收任务草稿(PRODCAP-5)。
///
/// 与空闲看守的自动清理**共用同一实现**;`KEDAI_TASK_SCRATCH_KEEP_DAYS = 0`(关)时
/// 不执行清理、原样回报 0(前端据此显示「保留策略已关闭」,而不是「已清理 0 个」)。
pub async fn scratch_cleanup(State(state): State<Arc<AppState>>) -> Response {
    let keep_days = state.config.task_scratch_keep_days;
    if keep_days == 0 {
        return Json(json!({
            "removed": [],
            "skipped_active": [],
            "kept_fresh": 0,
            "failed": [],
            "keep_days": 0,
        }))
        .into_response();
    }
    let tasks = state.tasks.clone();
    match state
        .db_call(move || tasks.cleanup_task_scratch(keep_days))
        .await
    {
        Err(e) => db_err(&e),
        Ok(r) => Json(json!({
            "removed": r.removed,
            "skipped_active": r.skipped_active,
            "kept_fresh": r.kept_fresh,
            "failed": r.failed,
            "keep_days": keep_days,
        }))
        .into_response(),
    }
}
