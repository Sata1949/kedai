// 任务附件端点(ATTACH-1,2026-11-03)。
//
// 三个端点都挂在单个任务下,任务不存在一律 404(不猜「附件不存在」还是「任务不存在」):
//   POST   /api/tasks/{id}/attachments        —— multipart 上传(可多文件,每文件一个 part)
//   GET    /api/tasks/{id}/attachments        —— 清单
//   DELETE /api/tasks/{id}/attachments/{aid}  —— 删除单个
//
// **复用 `api/characters.rs` 的 multipart 解析器**(`parse_boundary` / `parse_multipart`,
// `pub(crate)`):它是全仓唯一实现,world_books / plugins / prompt_inject 已在此之上复用。
// 它返回 `Vec<MultipartPart>`,天然支持多文件——这正是「导入附件」与「导入角色卡」的差别:
// 后者只取第一个带 filename 的 part。
//
// 上限在**服务层**校验(`MAX_ATTACHMENT_BYTES` / `MAX_ATTACHMENTS_PER_TASK`),不在这里
// 重复:个数上限要读库才知道,放服务层才能被单测直接覆盖。
use crate::api::app_state::AppState;
use crate::api::characters::{parse_boundary, parse_multipart};
use crate::api::{db_err, err_status, not_found};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

/// 字段名约定:每个文件一个 `file` part(与四个既有上传端点一致);
/// 另接受任意带 filename 的 part——浏览器多选时字段名固定为 `file`,
/// 但第三方客户端可能用 `file[]` 之类,按 filename 判定更宽容。
const FILE_FIELD: &str = "file";

/// POST /api/tasks/{id}/attachments:上传一个或多个附件。
///
/// 成功 → `201` + `{ ok: true, attachments: [...] }`(只含**本次**新入库的条目;
/// 前端把它并入本地清单,不重拉详情)。部分文件失败 → 整批 400 并点名失败原因:
/// 部分成功会让「我传了 3 个怎么只有 2 个」变成静默数据丢失。
pub async fn attachment_upload(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(task) = state.tasks.get(&id) else {
        return not_found("任务不存在");
    };
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(boundary) = parse_boundary(content_type) else {
        return err_status("缺少文件字段(file)", StatusCode::BAD_REQUEST);
    };
    let parts: Vec<(String, Option<String>, Vec<u8>)> = parse_multipart(&body, &boundary)
        .into_iter()
        .filter(|p| p.field == FILE_FIELD || p.filename.is_some())
        .map(|p| (p.field, p.filename, p.content))
        .collect();
    if parts.is_empty() {
        return err_status("缺少文件字段(file)", StatusCode::BAD_REQUEST);
    }

    let tasks = state.tasks.clone();
    let uploaded = state
        .db_call(move || {
            let mut out = Vec::new();
            for (field, filename, content) in parts {
                // 无名 part(纯文本字段)不是文件,跳过而不是报错:multipart 里常
                // 附带 `task_id` 之类元数据字段,为此整批失败太脆。
                let Some(name) = filename else {
                    let _ = field;
                    continue;
                };
                let mime = sniff_mime(&name);
                out.push(tasks.store_attachment(&task, &name, mime.as_deref(), &content)?);
            }
            if out.is_empty() {
                return Err("缺少文件字段(file)".to_string());
            }
            Ok(out)
        })
        .await;
    match uploaded {
        Err(e) => db_err(&e),
        // 内层 Err 是业务原因(空文件 / 超限 / 个数上限 / 写盘失败),全是用户可纠正的
        // 输入问题 → 400 并把中文原因原样上屏(与四个既有上传端点同口径)。
        Ok(Err(reason)) => err_status(reason, StatusCode::BAD_REQUEST),
        Ok(Ok(list)) => (
            StatusCode::CREATED,
            Json(json!({ "ok": true, "attachments": list })),
        )
            .into_response(),
    }
}

/// GET /api/tasks/{id}/attachments:附件清单(按上传顺序)。
pub async fn attachment_list(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if state.tasks.get(&id).is_none() {
        return not_found("任务不存在");
    }
    let tasks = state.tasks.clone();
    let list_id = id.clone();
    match state
        .db_call(move || tasks.list_attachments(&list_id))
        .await
    {
        Err(e) => db_err(&e),
        Ok(Err(e)) => crate::api::internal(e),
        Ok(Ok(list)) => Json(json!({ "attachments": list })).into_response(),
    }
}

/// DELETE /api/tasks/{id}/attachments/{attachment_id}:删除单个附件。
///
/// 删除是幂等的:记录已不存在 → 404(而不是 204),因为「删一个不存在的附件」
/// 通常是前端清单过期,静默成功会让用户以为删掉了别的东西。
pub async fn attachment_delete(
    State(state): State<Arc<AppState>>,
    Path((id, attachment_id)): Path<(String, String)>,
) -> Response {
    let Some(task) = state.tasks.get(&id) else {
        return not_found("任务不存在");
    };
    let tasks = state.tasks.clone();
    match state
        .db_call(move || tasks.delete_attachment(&task, &attachment_id))
        .await
    {
        Err(e) => db_err(&e),
        // db_call 的外层是「线程池执行失败」,内层才是业务结果:内层 Err 是 SQL/IO 原因,
        // 与其它 handler 同口径走 500(internal),不伪装成 404。
        Ok(Err(e)) => crate::api::internal(e),
        Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(false)) => not_found("附件不存在"),
    }
}

/// 按扩展名给一个**保守**的 MIME 提示。
///
/// 只覆盖「文本 vs 图像」这一条判定真正需要的类别,其余一律 `None`(如实留空):
/// 目标注入段要用它决定写 `fs_read` 还是 `view_image`。用扩展名而不是嗅探魔数,
/// 是因为这里不需要安全判定(附件本来就允许任意类型),只需要给模型一个读法建议。
fn sniff_mime(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e)?;
    let mime = match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "ts" => "text/typescript",
        "rs" => "text/rust",
        "py" => "text/x-python",
        "toml" => "text/toml",
        "yaml" | "yml" => "text/yaml",
        "xml" => "text/xml",
        "pdf" => "application/pdf",
        _ => return None,
    };
    Some(mime.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 扩展名 → MIME 只覆盖判读所需的类别,未知一律 `None`(不猜)。
    #[test]
    fn mime_sniff_covers_text_and_image_only() {
        assert_eq!(sniff_mime("a.PNG").as_deref(), Some("image/png"));
        assert_eq!(sniff_mime("笔记.md").as_deref(), Some("text/markdown"));
        assert_eq!(sniff_mime("x.json").as_deref(), Some("application/json"));
        assert_eq!(sniff_mime("noext"), None);
        assert_eq!(sniff_mime("a.unknownext"), None);
    }
}
