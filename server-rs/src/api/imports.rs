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
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dir = state.config.imports_dir.clone();
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return internal(format!("创建导入归档目录失败: {e}"));
    }
    // DATA-LOC-4:归档写盘是同步文件 IO,放到阻塞线程里执行,避免占着 async worker。
    let content = part.content.clone();
    let written = match tokio::task::spawn_blocking(move || {
        write_archive_with_stamp(&dir, &safe, &content, &stamp)
    })
    .await
    {
        Ok(Ok(name)) => name,
        Ok(Err(e)) => return internal(format!("写导入归档失败: {e}")),
        Err(e) => return internal(format!("写导入归档失败: 归档任务异常 {e}")),
    };
    Json(json!({ "ok": true, "name": written })).into_response()
}

/// 归档一次导入文件，返回实际落盘的文件名（DATA-LOC-4）。
///
/// 时间戳只到秒，同秒上传同名文件必然撞名。原实现直接 `fs::write`，
/// 两次都返回 200 却只剩一份——丢的是用户导入的原始归档。这里改为
/// `create_new` 独占创建 + 碰撞时追加唯一后缀重试：先到的那份永不改写。
fn write_archive_with_stamp(
    dir: &std::path::Path,
    safe: &str,
    content: &[u8],
    stamp: &str,
) -> std::io::Result<String> {
    for attempt in 0..64u32 {
        let name = if attempt == 0 {
            format!("{stamp}-{safe}")
        } else {
            format!("{stamp}-{attempt}-{safe}")
        };
        let path = dir.join(&name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                use std::io::Write as _;
                if let Err(e) = f.write_all(content) {
                    // 只清理本请求新建的残件，不碰既有文件。
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(name);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "同秒同名归档过多,放弃落盘",
    ))
}

#[cfg(test)]
mod tests {
    use super::{sanitize_import_name, write_archive_with_stamp};
    /// 测试用唯一后缀（不引入新依赖）：进程内计数 + 纳秒时间。
    fn next_test_id() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("{nanos}-{n}")
    }

    #[test]
    fn sanitize_strips_paths_and_illegal_chars() {
        assert_eq!(sanitize_import_name("C:\\tmp\\a.json"), "a.json");
        assert_eq!(sanitize_import_name("../evil.json"), "evil.json");
        assert_eq!(sanitize_import_name("a<b>c:d.json"), "a_b_c_d.json");
        assert_eq!(sanitize_import_name("  "), "import.json");
        assert_eq!(sanitize_import_name("角色 卡.json"), "角色 卡.json");
    }

    /// DATA-LOC-4：同秒归档两个同名文件，两次都必须成功且各自落成独立文件。
    ///
    /// 缺陷形态：`{秒级时间戳}-{清洗名}` 直接 `fs::write`，同秒同名必然覆盖，
    /// 两次都返回 200 却只剩一份，丢失的是用户导入的原始归档。
    #[test]
    fn same_second_same_name_archives_are_kept_separately() {
        let dir = std::env::temp_dir().join(format!("kedai-import-dup-{}", next_test_id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");

        let stamp = "20260101-120000";
        let first = write_archive_with_stamp(&dir, "report.json", b"first payload", stamp)
            .expect("第一次归档应成功");
        let second = write_archive_with_stamp(&dir, "report.json", b"second payload", stamp)
            .expect("第二次归档应成功");

        assert_ne!(first, second, "同秒同名归档必须落到两个不同文件");
        assert_eq!(
            std::fs::read(dir.join(&first)).expect("读第一份"),
            b"first payload"
        );
        assert_eq!(
            std::fs::read(dir.join(&second)).expect("读第二份"),
            b"second payload",
            "第二次归档不得覆盖第一次"
        );
        assert!(first.contains("20260101-120000"), "时间戳应保留: {first}");
        assert!(first.contains("report.json"), "可读原名应保留: {first}");
        assert!(second.contains("report.json"), "可读原名应保留: {second}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 预置的碰撞文件不得被改写（不能靠先 truncate 再写来让位）。
    #[test]
    fn colliding_archive_file_is_left_intact() {
        let dir = std::env::temp_dir().join(format!("kedai-import-clash-{}", next_test_id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let stamp = "20260101-130000";
        let first = write_archive_with_stamp(&dir, "a.txt", b"original", stamp).expect("首次归档");
        std::fs::write(dir.join(&first), b"external edit").expect("模拟外部改动");
        let second =
            write_archive_with_stamp(&dir, "a.txt", b"new content", stamp).expect("再次归档");
        assert_ne!(first, second);
        assert_eq!(
            std::fs::read(dir.join(&first)).expect("读回"),
            b"external edit",
            "已存在的归档文件不得被新归档覆盖"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_limits_length() {
        let long = format!("{}.json", "a".repeat(300));
        let out = sanitize_import_name(&long);
        assert_eq!(out.chars().count(), 120);
    }
}
