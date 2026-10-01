// 图像通道服务(视觉能力包 D2):`DATA_DIR/images/` 的写入、校验与 data URL 组装。
//
// 数据流:聊天附件(data URL)→ `decode_data_url`(声明 mime 白名单 + 魔数 + 单张上限)
// → `save`(uuid 文件名 + write_atomic)→ 消息 extra 里**只存引用**
// (`ImageRef{id,name,mime}`,data_url 留空);送模型前由引擎把引用 `resolve` 回 data URL
// (`resolve_recent` 同时执行「只带最近 4 张」的截断)。
//
// 读取端(`/api/images/{file}` 路由与本服务的 resolve)一律先过文件名白名单,
// 再按魔数定 Content-Type——与头像路由同一安全模型(该路由免 Bearer,
// 理由见 api/security.rs 白名单注释)。
use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::models::types::ImageRef;
use crate::utils::fs_atomic::write_atomic;
use crate::utils::image_sniff::{extension_for_mime, sniff_image_mime};

/// 单条消息附件张数上限(与前端 ChatInput 的提示文案同口径)
pub const MAX_IMAGES_PER_MESSAGE: usize = 4;
/// 单张解码后字节上限
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// 单条消息附件合计字节上限(解码后)
pub const MAX_TOTAL_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// data URL 的声明 mime 白名单(仅这些类型允许落盘;真实类型仍以魔数为准)
const UPLOAD_MIME_WHITELIST: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

pub struct ImageService {
    dir: PathBuf,
}

impl ImageService {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            dir: data_dir.join("images"),
        }
    }

    /// 图像目录(诊断用)
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 文件名白名单校验(与头像路由同规则);通过则给出磁盘路径,否则 None。
    pub fn file_path(&self, file: &str) -> Option<PathBuf> {
        if file.is_empty() {
            return None;
        }
        let safe: String = file
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
            .collect();
        if safe != file {
            return None;
        }
        Some(self.dir.join(file))
    }

    /// 解码与校验前端附件 data URL:返回 (魔数认定的 mime, 解码后字节)。
    /// 拒绝:非法 data URL / 声明 mime 不在白名单 / base64 解不开 / 空 /
    /// 超单张上限 / 魔数不识别(不是有效图像)。
    pub fn decode_data_url(name: &str, data_url: &str) -> Result<(String, Vec<u8>), String> {
        let (declared, payload) = split_data_url(data_url)
            .ok_or_else(|| format!("附件「{name}」不是有效的图像 data URL"))?;
        if !UPLOAD_MIME_WHITELIST.contains(&declared.as_str()) {
            return Err(format!(
                "附件「{name}」类型 {declared} 不支持(仅支持 PNG / JPEG / GIF / WebP)"
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|_| format!("附件「{name}」base64 解码失败"))?;
        if bytes.is_empty() {
            return Err(format!("附件「{name}」内容为空"));
        }
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(format!("附件「{name}」超过单张 10MB 限制"));
        }
        let mime = sniff_image_mime(&bytes)
            .ok_or_else(|| format!("附件「{name}」不是有效图像(魔数校验未通过)"))?;
        Ok((mime.to_string(), bytes))
    }

    /// 落盘:uuid 文件名(含扩展名)+ 原子写;返回可入 extra 的引用(data_url 留空)。
    pub fn save(&self, name: &str, mime: &str, bytes: &[u8]) -> Result<ImageRef, String> {
        let ext = extension_for_mime(mime).ok_or_else(|| format!("不支持的图像 mime:{mime}"))?;
        let id = format!("{}.{ext}", uuid::Uuid::new_v4());
        write_atomic(&self.dir.join(&id), bytes).map_err(|e| format!("图像落盘失败:{e}"))?;
        Ok(ImageRef {
            id,
            name: name.to_string(),
            mime: mime.to_string(),
            data_url: String::new(),
        })
    }

    /// 引用 → data URL(读盘 + 魔数嗅探;mime 以字节实际形态为准)。
    pub fn resolve_data_url(&self, id: &str) -> Result<String, String> {
        let path = self
            .file_path(id)
            .ok_or_else(|| "非法图像文件名".to_string())?;
        let bytes = std::fs::read(&path).map_err(|e| format!("图像读取失败:{e}"))?;
        let mime = sniff_image_mime(&bytes).ok_or_else(|| "图像魔数校验未通过".to_string())?;
        Ok(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        ))
    }

    /// 历史携带:只保留**最近** [`MAX_IMAGES_PER_MESSAGE`] 张(按消息从新到旧计数),
    /// 更早的引用就地丢弃;保留者解析成 data URL,失败的跳过并留 warn。
    /// 入参为与历史视图对齐的「每消息引用列表」。
    pub fn resolve_recent(&self, per_message: &mut [Vec<ImageRef>]) {
        let mut remaining = MAX_IMAGES_PER_MESSAGE;
        for refs in per_message.iter_mut().rev() {
            refs.retain_mut(|r| {
                if remaining == 0 {
                    return false;
                }
                match self.resolve_data_url(&r.id) {
                    Ok(url) => {
                        r.data_url = url;
                        remaining -= 1;
                        true
                    }
                    Err(e) => {
                        tracing::warn!(image_id = %r.id, error = %e, "历史图像解析失败,已跳过");
                        false
                    }
                }
            });
        }
    }
}

/// 拆 `data:<mime>;base64,<payload>`(mime 大小写归一到小写,`image/jpg` 归一为 jpeg;
/// 不接受缺 `;base64` 标记的形态——本通道只收 base64 图像)
fn split_data_url(data_url: &str) -> Option<(String, &str)> {
    let rest = data_url.trim().strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mime = meta.strip_suffix(";base64")?.trim().to_ascii_lowercase();
    let mime = if mime == "image/jpg" {
        "image/jpeg".to_string()
    } else {
        mime
    };
    Some((mime, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    /// 1x1 PNG(真实魔数,内容不重要)
    const PNG_1PX: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn png_data_url() -> String {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(PNG_1PX)
        )
    }

    #[test]
    fn decode_accepts_whitelisted_image_and_sniffs_mime() {
        let (mime, bytes) = ImageService::decode_data_url("a.png", &png_data_url()).unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, PNG_1PX);
        // 声明 jpeg 但实际 PNG:魔数为准(避免错误扩展名导致 Content-Type 漂移)
        let (mime, _) =
            ImageService::decode_data_url("a.jpg", &png_data_url().replace("png", "jpeg")).unwrap();
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn decode_rejects_bad_inputs_with_actionable_messages() {
        // 非 data URL
        let e = ImageService::decode_data_url("a.png", "https://x/a.png").unwrap_err();
        assert!(e.contains("a.png"), "{e}");
        // 白名单外类型
        let e = ImageService::decode_data_url(
            "a.bmp",
            &format!(
                "data:image/bmp;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(b"BM\x00\x00")
            ),
        )
        .unwrap_err();
        assert!(e.contains("不支持"), "{e}");
        // base64 解不开
        let e = ImageService::decode_data_url("a.png", "data:image/png;base64,!!!!").unwrap_err();
        assert!(e.contains("解码失败"), "{e}");
        // 大小写与 jpg 归一
        assert!(ImageService::decode_data_url("a.png", "data:IMAGE/PNG;base64,!!!!").is_err());
        // 魔数不识别(真实文本字节)
        let e = ImageService::decode_data_url(
            "a.png",
            &format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(b"hello world")
            ),
        )
        .unwrap_err();
        assert!(e.contains("不是有效图像"), "{e}");
        // 超过单张上限
        let big = vec![0x89u8, 0x50, 0x4E, 0x47];
        let mut oversize = big.clone();
        oversize.resize(MAX_IMAGE_BYTES + 1, 0);
        let e = ImageService::decode_data_url(
            "big.png",
            &format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&oversize)
            ),
        )
        .unwrap_err();
        assert!(e.contains("10MB"), "{e}");
    }

    #[test]
    fn save_and_resolve_roundtrip() {
        let dir = TempDataDir::new("image-svc");
        let svc = ImageService::new(dir.path().to_path_buf());
        let r = svc.save("照片.png", "image/png", PNG_1PX).unwrap();
        assert!(r.id.ends_with(".png"), "{}", r.id);
        assert!(r.data_url.is_empty(), "extra 引用只存引用,不带像素");
        let url = svc.resolve_data_url(&r.id).unwrap();
        assert_eq!(url, png_data_url());
        // 白名单:目录穿越与空名一律 None
        assert!(svc.file_path("../x.png").is_none());
        assert!(svc.file_path("a/b.png").is_none());
        assert!(svc.file_path("").is_none());
        assert!(svc.file_path("ok-1_2.png").is_some());
    }

    #[test]
    fn resolve_recent_keeps_only_latest_four_images() {
        let dir = TempDataDir::new("image-recent");
        let svc = ImageService::new(dir.path().to_path_buf());
        // 六条历史消息各一张图 → 只保留后 4 条(从新到旧)的图
        let mut per_message: Vec<Vec<ImageRef>> = (0..6)
            .map(|i| {
                vec![svc
                    .save(&format!("m{i}.png"), "image/png", PNG_1PX)
                    .unwrap()]
            })
            .collect();
        svc.resolve_recent(&mut per_message);
        assert!(per_message[0].is_empty(), "更早的引用应被丢弃");
        assert!(per_message[1].is_empty(), "更早的引用应被丢弃");
        let kept: Vec<usize> = per_message
            .iter()
            .enumerate()
            .filter(|(_, refs)| !refs.is_empty())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(kept, vec![2, 3, 4, 5], "只保留最近 4 张");
        assert!(per_message[5][0]
            .data_url
            .starts_with("data:image/png;base64,"));
    }
}
