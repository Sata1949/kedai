//! 综合导入文件归档:把用户加载的原始 bundle/预设文件落到 `<root>/imports/`。
//!
//! 为什么由前端在「解析并导入成功」后回调:现有综合导入是前端按段分发到各业务端点,
//! 后端没有「整包」入口;若在后端另建解析路径,会出现两套导入语义。本端点只负责
//! **留档**,不参与解析,失败也不影响导入本身(前端只记提示)。
//!
//! multipart 解析复用 `api/characters.rs` 的全仓唯一实现(与角色卡/世界书/插件上传同源)。
use crate::api::app_state::AppState;
use crate::api::characters::{parse_boundary, parse_multipart};
use crate::api::{internal, validation};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

/// 单个归档文件上限(与前端综合导入件的实际量级匹配;超过直接拒绝,避免塞满磁盘)
const MAX_IMPORT_ARCHIVE_BYTES: usize = 20 * 1024 * 1024;

/// 文件名清洗:只留最后一段,替换 Windows 非法字符,限长;保留扩展名。
pub(crate) fn sanitize_import_name(raw: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("import.json")
        .trim();
    let mut out = String::with_capacity(base.len());
    for ch in base.chars() {
        let keep = ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_' | '(' | ')' | ' ' | '·');
        out.push(if keep { ch } else { '_' });
    }
    let out = out.trim_matches([' ', '.']).to_string();
    if out.is_empty() {
        return "import.json".to_string();
    }
    if out.chars().count() > 120 {
        let mut cut = String::new();
        for ch in out.chars().take(120) {
            cut.push(ch);
        }
        cut
    } else {
        out
    }
}

/// POST /api/imports/archive(multipart,字段 `file` 或任意带 filename 的 part)
pub async fn archive(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(boundary) = parse_boundary(content_type) else {
        return validation("缺少文件字段(file)");
    };
    let parts = match parse_multipart(&body, &boundary) {
        Ok(parts) => parts,
        Err(e) => return validation(e),
    };
    let Some(part) = parts
        .into_iter()
        .find(|p| p.field == "file" || p.filename.is_some())
    else {
        return validation("缺少文件字段(file)");
    };
    if part.content.is_empty() {
        return validation("导入文件为空");
    }
    if part.content.len() > MAX_IMPORT_ARCHIVE_BYTES {
        return validation("导入文件超过 20MB 上限,未归档");
    }
    let original = part.filename.unwrap_or_else(|| "import.json".to_string());
    let safe = sanitize_import_name(&original);
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = format!("{stamp}-{safe}");
    let dir = state.config.imports_dir.clone();
    let archive_name = name.clone();
    let saved = state
        .db_call(move || {
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建导入归档目录失败: {e}"))?;
            std::fs::write(dir.join(archive_name), part.content)
                .map_err(|e| format!("写导入归档失败: {e}"))
        })
        .await;
    match saved {
        Ok(Ok(())) => {}
        Ok(Err(error)) | Err(error) => return internal(error),
    }
    Json(json!({ "ok": true, "name": name })).into_response()
}

#[cfg(test)]
mod tests {
    use super::sanitize_import_name;

    #[test]
    fn sanitize_strips_paths_and_illegal_chars() {
        assert_eq!(sanitize_import_name("C:\\tmp\\a.json"), "a.json");
        assert_eq!(sanitize_import_name("../evil.json"), "evil.json");
        assert_eq!(sanitize_import_name("a<b>c:d.json"), "a_b_c_d.json");
        assert_eq!(sanitize_import_name("  "), "import.json");
        assert_eq!(sanitize_import_name("角色 卡.json"), "角色 卡.json");
    }

    #[test]
    fn sanitize_limits_length() {
        let long = format!("{}.json", "a".repeat(300));
        let out = sanitize_import_name(&long);
        assert_eq!(out.chars().count(), 120);
    }
}
