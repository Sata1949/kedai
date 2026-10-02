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
/// 单张图像上限(**压缩字节**,base64 解码后的原始文件字节)
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// 单条消息附件合计字节上限(压缩字节)
pub const MAX_TOTAL_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// 解码尺寸硬上限:单边像素(2026-10-02 修复批次,防解压炸弹——压缩字节达标、
/// 展开后数 GB 的图会在解码时分配爆内存;8K 屏截图 7680 宽在限内)
pub const MAX_DECODE_SIDE: u32 = 32768;
/// 解码尺寸硬上限:总像素数(64MP;按头部尺寸判定,超限直接拒绝而非尝试分配)
pub const MAX_DECODE_PIXELS: u64 = 64 * 1024 * 1024;
/// 解码期分配上限(non-strict,image::Limits 二次防线;默认 512MiB 再收紧一档)
const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;

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
        // 尺寸硬校验(2026-10-02 修复批次):只读头部、不分配像素;超限在入口即拒
        // (压缩字节达标但展开巨大的「解压炸弹」不允许进入后续解码与下发路径)
        check_decode_limits(&bytes).map_err(|e| format!("附件「{name}」{e}"))?;
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
            label: None,
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

    /// 解析约定式工具返回(视觉能力包 D4):`{"text": "...", "images": [引用]}`。
    /// 命中形状 → (text 字段, 已解析 data URL 的图像引用)——图像先落 DATA_DIR/images
    /// (工具侧保存),此处只做引用→data URL;其它形状 → (原始 JSON 文本, 空),
    /// 既有工具零改动。引用解析失败的单张跳过(留 warn);文本与图像都为空时回退原文。
    pub fn parse_tool_output(&self, output: &serde_json::Value) -> (String, Vec<ImageRef>) {
        let fallback = || output.to_string();
        let Some(obj) = output.as_object() else {
            return (fallback(), Vec::new());
        };
        let Some(images_val) = obj.get("images") else {
            return (fallback(), Vec::new());
        };
        let Ok(raw) = serde_json::from_value::<Vec<ImageRef>>(images_val.clone()) else {
            return (fallback(), Vec::new());
        };
        if raw.is_empty() {
            return (fallback(), Vec::new());
        }
        let text = obj
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let mut images = Vec::with_capacity(raw.len());
        for r in raw {
            match self.resolve_data_url(&r.id) {
                Ok(url) => images.push(ImageRef { data_url: url, ..r }),
                Err(e) => {
                    tracing::warn!(image_id = %r.id, error = %e, "工具图像解析失败,已跳过");
                }
            }
        }
        if text.is_empty() && images.is_empty() {
            return (fallback(), Vec::new());
        }
        (text, images)
    }

    // ===== 大图自动拆分(视觉能力包 D3;仅 image_auto_split 连接在下发前调用)=====

    /// 大图自动拆分(2026-10-02 视觉能力包 D3):`image_auto_split` 连接在**下发前**
    /// 对每条消息调用。任一边超 [`AUTO_SPLIT_MAX_SIDE`] 的图像替换为
    /// 「全局总览 + 行/列块」的有序引用(`ImageRef.label` 携带标注文本);
    /// 尺寸达标 / 解码失败 / data_url 为空 → 原样保留,不阻断本轮。
    /// 派生块经 `derived/{key}/` 磁盘缓存复用(见 [`Self::split_large_image`])。
    pub fn expand_for_auto_split(&self, messages: &mut [crate::models::types::LlmMessage]) {
        for m in messages.iter_mut() {
            if m.images.is_empty() {
                continue;
            }
            let mut next: Vec<ImageRef> = Vec::with_capacity(m.images.len());
            for img in m.images.drain(..) {
                match self.split_large_image(&img) {
                    Some(parts) => next.extend(parts),
                    None => next.push(img),
                }
            }
            m.images = next;
        }
    }

    /// 单图拆分:返回 `Some(零件)` 表示已拆分(总览在最前),`None` = 无需/无法拆分。
    /// 网格:列/行 = ceil(w/S) / ceil(h/S)(等比均分);块数超上限先整图等比缩小;
    /// 块以 PNG 输出(无损),总览长边 ≤ S。整图解码失败(损坏/未知格式)原样透传。
    pub fn split_large_image(&self, img: &ImageRef) -> Option<Vec<ImageRef>> {
        if img.data_url.is_empty() {
            return None;
        }
        let bytes = decode_data_url_bytes(&img.data_url)?;
        // 头部尺寸先行(不分配像素,修复批次):小图免解码直接透传;超大图同路径拒绝
        let (sw, sh) = match read_dimensions(&bytes) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, image = %img.name, "大图拆分跳过(无法读取图像尺寸),原样下发");
                return None;
            }
        };
        if let Err(e) = check_limits_of(sw, sh) {
            tracing::warn!(error = %e, image = %img.name, "大图拆分跳过(图像超限),原样下发");
            return None;
        }
        let (scale, cols, rows) = split_plan(sw, sh)?;
        // 磁盘缓存:派生块只依赖源文件与参数,manifest 命中即复用(不重复解码/裁剪/编码;
        // 修复批次:命中检查提前到解码之前——此前先全量解码再查 manifest,与注释口径相悖)
        let key = cache_key_of(img);
        if let Some(hit) = self.load_derived(&key, &img.name) {
            return Some(hit);
        }
        // 有上限解码(修复批次):单边 >32768 或 >64MP 的图在分配像素缓冲前即拒
        let decoded = match decode_bounded(&bytes) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, image = %img.name, "大图拆分跳过(超限或不可解码),原样下发");
                return None;
            }
        };
        let (w, h) = (decoded.width(), decoded.height());
        let base = if scale < 1.0 {
            let (sw, sh) = scaled_dims(w, h, scale);
            decoded.resize(sw, sh, image::imageops::FilterType::Lanczos3)
        } else {
            decoded
        };
        let (bw, bh) = (base.width(), base.height());
        let mut parts: Vec<ImageRef> = Vec::with_capacity((cols * rows + 1) as usize);
        // 首块前附全局总览(长边 ≤ S;等比缩)
        let overview = base.resize(
            AUTO_SPLIT_MAX_SIDE,
            AUTO_SPLIT_MAX_SIDE,
            image::imageops::FilterType::Lanczos3,
        );
        parts.push(part_ref(
            &key,
            "overview",
            &img.name,
            &format!("（原图总览:{w}×{h}）"),
            &overview,
        )?);
        for r in 0..rows {
            for c in 0..cols {
                let x0 = c * bw / cols;
                let y0 = r * bh / rows;
                let x1 = ((c + 1) * bw / cols).min(bw);
                let y1 = ((r + 1) * bh / rows).min(bh);
                let block = base.crop_imm(x0, y0, x1 - x0, y1 - y0);
                parts.push(part_ref(
                    &key,
                    &format!("r{}c{}", r + 1, c + 1),
                    &img.name,
                    &format!(
                        "（大图拆分:第{}行/第{}列,共{}行×{}列）",
                        r + 1,
                        c + 1,
                        rows,
                        cols
                    ),
                    &block,
                )?);
            }
        }
        self.save_derived(&key, &parts);
        Some(parts)
    }

    /// 读派生缓存:`derived/{key}/manifest.json` 参数匹配且全部块文件可读 → 重建引用;
    /// 任一环节失败返回 `None`(调用方重算并覆盖)。
    fn load_derived(&self, key: &str, name: &str) -> Option<Vec<ImageRef>> {
        let dir = self.dir.join("derived").join(key);
        let manifest_raw = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
        let manifest: DerivedManifest = serde_json::from_str(&manifest_raw).ok()?;
        if manifest.version != DERIVED_MANIFEST_VERSION
            || manifest.max_side != AUTO_SPLIT_MAX_SIDE
            || manifest.max_blocks != AUTO_SPLIT_MAX_BLOCKS
        {
            return None;
        }
        let mut parts = Vec::with_capacity(manifest.parts.len());
        for p in manifest.parts {
            let bytes = std::fs::read(dir.join(&p.file)).ok()?;
            parts.push(ImageRef {
                id: p.id,
                name: name.to_string(),
                mime: "image/png".to_string(),
                data_url: format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&bytes)
                ),
                label: p.label,
            });
        }
        Some(parts)
    }

    /// 写派生缓存(块文件 + manifest;失败仅告警——缓存是加速,不是正确性前提)。
    fn save_derived(&self, key: &str, parts: &[ImageRef]) {
        let dir = self.dir.join("derived").join(key);
        let mut manifest_parts = Vec::with_capacity(parts.len());
        for (i, p) in parts.iter().enumerate() {
            let Some(bytes) = decode_data_url_bytes(&p.data_url) else {
                return;
            };
            let file = format!("p{i}.png");
            if let Err(e) = write_atomic(&dir.join(&file), &bytes) {
                tracing::warn!(error = %e, "派生图像缓存写入失败(下次将重算)");
                return;
            }
            manifest_parts.push(DerivedPart {
                id: p.id.clone(),
                label: p.label.clone(),
                file,
            });
        }
        let manifest = DerivedManifest {
            version: DERIVED_MANIFEST_VERSION,
            max_side: AUTO_SPLIT_MAX_SIDE,
            max_blocks: AUTO_SPLIT_MAX_BLOCKS,
            parts: manifest_parts,
        };
        match serde_json::to_string_pretty(&manifest) {
            Ok(text) => {
                if let Err(e) = write_atomic(&dir.join("manifest.json"), text.as_bytes()) {
                    tracing::warn!(error = %e, "派生图像 manifest 写入失败(下次将重算)");
                }
            }
            Err(e) => tracing::warn!(error = %e, "派生图像 manifest 序列化失败"),
        }
    }
}

/// 派生缓存格式版本(参数不匹配即重算;格式变更时递增)
const DERIVED_MANIFEST_VERSION: u32 = 1;

/// 大图拆分阈值(长边像素;DeepSeek 等对图像尺寸较严的端点)
pub const AUTO_SPLIT_MAX_SIDE: u32 = 1300;
/// 单图最大块数(超出先把整图等比缩小到块数 ≤ 此值)
pub const AUTO_SPLIT_MAX_BLOCKS: u32 = 12;

#[derive(serde::Serialize, serde::Deserialize)]
struct DerivedManifest {
    version: u32,
    max_side: u32,
    max_blocks: u32,
    parts: Vec<DerivedPart>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DerivedPart {
    id: String,
    label: Option<String>,
    file: String,
}

/// 拆分计划(纯函数,便于对极端尺寸做数学单测):
/// 返回 `None` = 任一边 ≤ S(无需拆分);`Some((scale, cols, rows))` 中 scale < 1.0
/// 表示需先整图等比缩小(块数超上限时按 0.9 步长收敛求最小缩放)。
fn split_plan(w: u32, h: u32) -> Option<(f64, u32, u32)> {
    if w <= AUTO_SPLIT_MAX_SIDE && h <= AUTO_SPLIT_MAX_SIDE {
        return None;
    }
    let mut scale = 1.0f64;
    loop {
        let (sw, sh) = scaled_dims(w, h, scale);
        let cols = sw.div_ceil(AUTO_SPLIT_MAX_SIDE);
        let rows = sh.div_ceil(AUTO_SPLIT_MAX_SIDE);
        if cols * rows <= AUTO_SPLIT_MAX_BLOCKS || scale < 0.05 {
            return Some((scale, cols, rows));
        }
        scale *= 0.9;
    }
}

/// 按比例缩放尺寸(至少 1 像素)
fn scaled_dims(w: u32, h: u32, scale: f64) -> (u32, u32) {
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

/// 组装一个派生部件(PNG 编码失败返回 None,整体降级为不拆分)
fn part_ref(
    key: &str,
    suffix: &str,
    name: &str,
    label: &str,
    img: &image::DynamicImage,
) -> Option<ImageRef> {
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    Some(ImageRef {
        id: format!("{key}#{suffix}"),
        name: name.to_string(),
        mime: "image/png".to_string(),
        data_url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&buf)
        ),
        label: Some(label.to_string()),
    })
}

/// 派生缓存键:优先用源文件 id(白名单外字符剔除);无可用 id 时退回 data_url 哈希。
fn cache_key_of(img: &ImageRef) -> String {
    let safe: String = img
        .id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .collect();
    if !safe.is_empty() {
        return safe;
    }
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(img.data_url.as_bytes());
    let hex = format!("{:x}", h.finalize());
    hex[..16].to_string()
}

/// data URL → 原始字节(base64 段;拆解或解码失败返回 None)
fn decode_data_url_bytes(data_url: &str) -> Option<Vec<u8>> {
    let (_, payload) = split_data_url(data_url)?;
    base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .ok()
}

/// 只读图像头部拿尺寸(**不分配像素缓冲**;修复批次)。
/// 各格式的头部解析由 image crate 提供,这里只做统一的错误文案。
fn read_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("图像格式识别失败:{e}"))?
        .into_dimensions()
        .map_err(|e| format!("不是有效的图像文件:{e}"))
}

/// 尺寸上限判定(纯函数,便于单测):单边与总像素都必须落在硬上限内
fn check_limits_of(w: u32, h: u32) -> Result<(), String> {
    if w > MAX_DECODE_SIDE || h > MAX_DECODE_SIDE || (w as u64) * (h as u64) > MAX_DECODE_PIXELS {
        return Err(format!(
            "图像尺寸过大({w}×{h};上限 {MAX_DECODE_SIDE}×{MAX_DECODE_SIDE} 且 {MAX_DECODE_PIXELS} 像素),已拒绝以防内存耗尽"
        ));
    }
    Ok(())
}

/// 头部尺寸校验(不分配;入口与拆分路径共用;修复批次)
fn check_decode_limits(bytes: &[u8]) -> Result<(), String> {
    let (w, h) = read_dimensions(bytes)?;
    check_limits_of(w, h)
}

/// 带上限解码(修复批次):先按头部尺寸硬校验(解压炸弹防线),再以 `image::Limits`
/// 作二次防线解码。全仓需解码用户/模型来源图像处统一走本函数,勿直接 `load_from_memory`。
pub fn decode_bounded(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    check_decode_limits(bytes)?;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("图像格式识别失败:{e}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    reader.decode().map_err(|e| format!("图像解码失败:{e}"))
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
    use image::GenericImageView;

    /// 1x1 PNG(真实魔数**且 CRC 合法**——D4 起图像会真解码,魔数对但 CRC 错的字节
    /// 会被 `image` 解码器拒绝;此字节由 zlib 正确编码生成)
    const PNG_1PX: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x38,
        0x21, 0x27, 0x07, 0x00, 0x02, 0xB6, 0x01, 0x05, 0x0A, 0x5B, 0xA6, 0x06, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
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

    /// 解压炸弹防线(2026-10-02 修复批次):头部尺寸超限在**分配像素缓冲之前**即拒;
    /// 限内小图正常解码
    #[test]
    fn decode_bounded_rejects_oversize_dimensions_before_alloc() {
        use crate::utils::test_support::png_header_with_dims;
        // 100000×100000(由 1×1 PNG 骨架改写 IHDR 尺寸):尺寸判定即拒,不会尝试分配像素
        let huge = png_header_with_dims(100_000, 100_000);
        let err = decode_bounded(&huge).unwrap_err();
        assert!(err.contains("尺寸过大"), "{err}");
        // 单边超限(32769)同样拒
        let wide = png_header_with_dims(MAX_DECODE_SIDE + 1, 8);
        assert!(decode_bounded(&wide).unwrap_err().contains("尺寸过大"));
        // 限内小图:正常解码且尺寸原样
        let mut png = Vec::new();
        image::DynamicImage::new_rgb8(4, 3)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(decode_bounded(&png).unwrap().dimensions(), (4, 3));
    }

    /// 附件入口尺寸校验(修复批次):超限 data URL 在 decode_data_url 阶段即拒(报错点名附件)
    #[test]
    fn decode_data_url_rejects_oversize_dimensions() {
        use crate::utils::test_support::png_header_with_dims;
        let huge = png_header_with_dims(100_000, 100_000);
        let url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&huge)
        );
        let e = ImageService::decode_data_url("huge.png", &url).unwrap_err();
        assert!(e.contains("huge.png") && e.contains("尺寸过大"), "{e}");
    }

    /// 块数上限的保证域(修复批次):在解码尺寸上限内(单边 ≤32768 且 ≤64MP)的任意
    /// 尺寸,「>12 块先缩」都在缩放下限 0.05 之前收敛(块数 ≤ 12)——0.05 只是远超
    /// 调用域的数学兜底,不参与真实路径
    #[test]
    fn split_plan_honours_block_cap_within_decode_limits() {
        let cases = [
            (32768u32, 32768u32),
            (32768, 1),
            (1, 32768),
            (8000, 8000), // = 64MP 上限
            (15360, 4320),
            (1301, 1301),
            (4096, 1301),
        ];
        for (w, h) in cases {
            let (scale, cols, rows) = split_plan(w, h).unwrap();
            assert!(scale >= 0.05, "{w}×{h} 不应触碰 0.05 下限:scale={scale}");
            assert!(
                cols * rows <= AUTO_SPLIT_MAX_BLOCKS,
                "{w}×{h} 块数 {} 超上限(scale={scale})",
                cols * rows
            );
        }
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

    // ===== 大图自动拆分(视觉能力包 D3)=====

    /// 生成指定尺寸的真实 PNG data URL(内容为渐变,编码快且非单调白)
    fn png_data_url_of(w: u32, h: u32, seed: u8) -> String {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 251) as u8, seed])
        });
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&buf)
        )
    }

    fn image_ref(id: &str, url: String) -> ImageRef {
        ImageRef {
            id: id.into(),
            name: format!("{id}.png"),
            mime: "image/png".into(),
            data_url: url,
            label: None,
        }
    }

    /// 网格数学(纯函数,极端尺寸不真编码):1300 直通;各档列/行;8192² 先缩到 ≤12 块
    #[test]
    fn split_plan_grid_math() {
        assert!(split_plan(1300, 1300).is_none(), "1300 及以下直通");
        assert!(split_plan(400, 1300).is_none());
        assert_eq!(split_plan(1301, 700), Some((1.0, 2, 1)));
        assert_eq!(split_plan(2600, 1300), Some((1.0, 2, 1)));
        assert_eq!(split_plan(2000, 2000), Some((1.0, 2, 2)));
        assert_eq!(split_plan(3840, 2160), Some((1.0, 3, 2)));
        let (scale, cols, rows) = split_plan(8192, 8192).unwrap();
        assert!(scale < 1.0, "块数超上限必须先整图缩小");
        assert!(
            cols * rows <= AUTO_SPLIT_MAX_BLOCKS,
            "块数 {cols}x{rows} 超上限"
        );
        assert!(cols * rows >= 2, "拆分应至少两块");
    }

    /// 端到端:1400×1400 → 总览 + 2×2 块;首件是总览;每件带行/列标注;统一 PNG 输出
    #[test]
    fn split_large_image_emits_overview_and_labeled_blocks() {
        let dir = TempDataDir::new("image-split");
        let svc = ImageService::new(dir.path().to_path_buf());
        let img = image_ref("src.png", png_data_url_of(1400, 1400, 7));
        let parts = svc.split_large_image(&img).expect("1400 应拆分");
        assert_eq!(parts.len(), 5, "总览 + 4 块,实际 {}", parts.len());
        assert!(
            parts[0].label.as_deref().unwrap().contains("总览"),
            "{:?}",
            parts[0].label
        );
        assert!(parts[1].label.as_deref().unwrap().contains("第1行/第1列"));
        assert!(parts[4].label.as_deref().unwrap().contains("第2行/第2列"));
        for p in &parts {
            assert!(
                p.data_url.starts_with("data:image/png;base64,"),
                "块统一 PNG"
            );
            assert!(p.id.starts_with("src.png#"), "派生 id 带源前缀:{}", p.id);
        }
        // derived 目录已落缓存
        assert!(dir
            .path()
            .join("images")
            .join("derived")
            .join("src.png")
            .join("manifest.json")
            .exists());
    }

    /// 缓存命中:篡改块文件后二次拆分读到的是缓存(证明未重算)
    #[test]
    fn split_cache_hit_reuses_derived_files() {
        let dir = TempDataDir::new("image-split-cache");
        let svc = ImageService::new(dir.path().to_path_buf());
        let img = image_ref("src2.png", png_data_url_of(1400, 700, 3));
        let first = svc.split_large_image(&img).unwrap();
        // 用另一张 8x8 有效 PNG 覆写第 1 个块文件
        let other = png_data_url_of(8, 8, 99);
        let other_bytes = decode_data_url_bytes(&other).unwrap();
        let block_file = dir
            .path()
            .join("images")
            .join("derived")
            .join("src2.png")
            .join("p1.png");
        std::fs::write(&block_file, &other_bytes).unwrap();
        let second = svc.split_large_image(&img).unwrap();
        assert_eq!(second.len(), first.len(), "缓存命中不改变件数");
        assert_eq!(second[1].data_url, other, "块内容应来自缓存文件(未重算)");
        assert_eq!(second[0].data_url, first[0].data_url, "总览照常复用");
    }

    /// 小图/坏图不拆:原样保留(「无需拆分 → 引用逐字段不变」的判别),不落 derived
    #[test]
    fn split_skips_small_and_broken_images() {
        let dir = TempDataDir::new("image-split-skip");
        let svc = ImageService::new(dir.path().to_path_buf());
        let small = image_ref("s.png", png_data_url_of(200, 200, 1));
        assert!(svc.split_large_image(&small).is_none(), "≤1300 直通");
        let broken = image_ref("b.png", "data:image/png;base64,!!!!".into());
        assert!(svc.split_large_image(&broken).is_none(), "解码失败原样透传");
        let empty = image_ref("e.png", String::new());
        assert!(svc.split_large_image(&empty).is_none(), "无 data_url 跳过");
        // expand_for_auto_split:含小图的消息原样(引用逐字段相等,无 label)
        let mut msgs = vec![crate::models::types::LlmMessage::plain("user", "x")];
        msgs[0].images = vec![small.clone()];
        svc.expand_for_auto_split(&mut msgs);
        assert_eq!(msgs[0].images, vec![small], "无需拆分时引用逐字段不变");
        assert!(
            !dir.path().join("images").join("derived").exists(),
            "未拆分不产生派生缓存"
        );
    }

    /// 工具图像通道(视觉能力包 D4):约定式 `{text, images}` 解析出 text 与已解析引用;
    /// 其它形状原样回退;引用解析失败跳过;images 空数组视为非约定形状。
    #[test]
    fn parse_tool_output_convention_and_fallback() {
        let dir = TempDataDir::new("tool-out");
        let svc = ImageService::new(dir.path().to_path_buf());
        let saved = svc.save("a.png", "image/png", PNG_1PX).unwrap();
        // 命中:返回 text + 解析后的引用(带 data URL)
        let out = serde_json::json!({
            "text": "已加载图像:1×1",
            "images": [{ "id": saved.id, "name": "a.png", "mime": "image/png" }]
        });
        let (text, images) = svc.parse_tool_output(&out);
        assert_eq!(text, "已加载图像:1×1");
        assert_eq!(images.len(), 1);
        assert!(images[0].data_url.starts_with("data:image/png;base64,"));
        // 非约定形状:原文 JSON 与空图像(既有工具零改动)
        let out = serde_json::json!({ "results": [1, 2] });
        let (text, images) = svc.parse_tool_output(&out);
        assert!(text.contains("results"), "{text}");
        assert!(images.is_empty());
        // 引用解析失败(文件不存在)→ 跳过,text 仍在
        let out = serde_json::json!({
            "text": "x",
            "images": [{ "id": "missing.png", "name": "m.png", "mime": "image/png" }]
        });
        let (text, images) = svc.parse_tool_output(&out);
        assert_eq!(text, "x");
        assert!(images.is_empty(), "解析失败的引用应被跳过");
        // images 为空数组 → 视为非约定形状(回退原文)
        let out = serde_json::json!({ "text": "x", "images": [] });
        let (text, _) = svc.parse_tool_output(&out);
        assert!(text.contains("images"));
    }
}
