// 视觉工具三件(视觉能力包 D4):view_image / zoom_image / image_diff。
//
// 约定式返回:本族工具输出 JSON `{text, images:[{id,name,mime}]}`——图像先落
// DATA_DIR/images(与聊天附件同一通道,`/api/images/{id}` 可渲染),执行器解析出引用并
// 转成 data URL 随 tool 消息下发给模型(解析点见 `agents/engine/executor.rs`;
// 既有工具返回纯文本/其它 JSON,形状不命中即原样透传,零改动)。
//
// 可见性(两道闸门,均在 `task_engine/tool_policy.rs` 的策略层):
//   ① 工作区族:三件都读工作区文件,未绑定作用域的任务与聊天路径整体剔除
//      (`tool_sets::WORKSPACE_TOOLS`;聊天路径经 `exclude_workspace`);
//   ② 视觉闸门:生效连接未开启「视觉输入」能力位时整族剔除(`vision_gate`)。
// 风险级 Safe(只读工作区、图像输出重定向到 images 目录,不改工作区、不执行命令)。
//
// 尺寸上限 8MB 与 fs 族一致(超过明确报错,不静默截断);解码用 image crate,
// 魔数/格式校验由解码器天然覆盖(非图像字节直接报「不是可解码的图像」)。
use crate::models::types::{ImageRef, ToolContext, ToolDefinition};
use crate::tools::registry::ToolRegistry;
use crate::tools::workspace_guard::safe_workspace_path;
use image::{DynamicImage, GenericImageView, ImageFormat};
use serde_json::json;
use std::io::Cursor;
use std::sync::Arc;

use super::agent_tools::ToolDeps;
use super::agent_tools_fs::scope_of;

/// 工具名(单一出处:与 `tool_sets::VISION_TOOLS` 同清单)
pub const VIEW_TOOL: &str = "view_image";
pub const ZOOM_TOOL: &str = "zoom_image";
pub const DIFF_TOOL: &str = "image_diff";

/// 单张输入图像上限(与 fs 族同档:超过明确报错)
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
/// zoom_image 导出长边上限(放大后钳制,防超大图占爆请求体)
const ZOOM_MAX_SIDE: u32 = 2048;
/// zoom_image 默认放大倍数与取值区间
const ZOOM_DEFAULT_SCALE: f64 = 2.0;
const ZOOM_MAX_SCALE: f64 = 8.0;
/// image_diff 判定「显著差异」的通道差阈值(0..255)
const DIFF_THRESHOLD: u8 = 16;

/// 注册视觉工具三件(始终注册;下发与否由策略层两道闸门决定)
pub fn register_vision_tools(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    register_view(registry, deps.clone());
    register_zoom(registry, deps.clone());
    register_diff(registry, deps);
}

/// 约定式返回:`{text, images:[引用]}`(执行器据此把图像随 tool 消息下发)
fn vision_output(text: String, images: Vec<ImageRef>) -> String {
    json!({ "text": text, "images": images }).to_string()
}

/// 读取工作区图像:牢笼校验 → 尺寸上限 → 解码校验。返回 (解码图, 原始字节数)。
fn load_image(
    deps: &ToolDeps,
    ctx: &ToolContext,
    raw: &str,
) -> Result<(DynamicImage, u64), String> {
    let scope = scope_of(ctx)?;
    let path = safe_workspace_path(scope.workspace(), raw, Some(&deps.data_dir))?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("读取文件失败: {e}"))?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "图像超过 8MB 上限({} 字节);请先缩小尺寸或压缩后再查看",
            meta.len()
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("读取文件失败: {e}"))?;
    // 有上限解码(2026-10-02 修复批次):头部尺寸硬校验 + image::Limits 二次防线,
    // 防「压缩字节达标、展开巨大」的解压炸弹(与附件路径同一条防线,单一实现)
    let img = crate::services::image_service::decode_bounded(&bytes)
        .map_err(|e| format!("不是可解码的图像或超出尺寸上限(支持 PNG/JPEG/GIF/WebP):{e}"))?;
    Ok((img, meta.len()))
}

/// 图像编码为 PNG 并落入 DATA_DIR/images,返回引用(供约定式返回携带)
fn save_png(deps: &ToolDeps, name: &str, img: &DynamicImage) -> Result<ImageRef, String> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .map_err(|e| format!("图像编码失败: {e}"))?;
    deps.images.save(name, "image/png", &buf)
}

fn file_name_of(raw: &str) -> String {
    std::path::Path::new(raw)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_string())
}

// ==================== view_image ====================

fn register_view(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: VIEW_TOOL.into(),
            description: "查看工作区里的一幅图像(截图/设计稿/照片)。图像会直接交给模型查看,返回图像与尺寸信息。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "工作区内的图像路径(如 docs/screenshot.png)" }
                },
                "required": ["path"]
            }),
        },
        Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
            let deps = deps.clone();
            // 解码/裁剪/重编码都是同步重活:让出 async worker(park_worker 与截图工具的
            // spawn_blocking 同纪律;修复批次补齐——此前直接跑在 worker 上)
            Box::pin(async move {
                crate::utils::blocking::park_worker(|| view_impl(&deps, &ctx, &args))
            })
        }),
    );
}

fn view_impl(
    deps: &ToolDeps,
    ctx: &ToolContext,
    args: &serde_json::Value,
) -> Result<String, String> {
    let raw = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if raw.is_empty() {
        return Err("缺少 path(工作区内的图像路径)".to_string());
    }
    let (img, bytes) = load_image(deps, ctx, &raw)?;
    let (w, h) = img.dimensions();
    let name = file_name_of(&raw);
    let reference = save_png(deps, &name, &img)?;
    Ok(vision_output(
        format!(
            "已加载图像「{name}」:{w}×{h},{kb}KB。请基于图像实际内容回答(先描述看到的关键信息,再作结论)。",
            kb = bytes.div_ceil(1024)
        ),
        vec![reference],
    ))
}

// ==================== zoom_image ====================

fn register_zoom(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: ZOOM_TOOL.into(),
            description: "裁剪工作区图像的一块区域并放大,用于看清局部细节(小字/图标/某块区域)。坐标为原图像素坐标(原点左上)。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "工作区内的图像路径" },
                    "x": { "type": "integer", "description": "裁剪区域左上角 x(原图像素坐标)" },
                    "y": { "type": "integer", "description": "裁剪区域左上角 y(原图像素坐标)" },
                    "width": { "type": "integer", "description": "裁剪宽度(像素)" },
                    "height": { "type": "integer", "description": "裁剪高度(像素)" },
                    "scale": { "type": "number", "description": "放大倍数(默认 2,上限 8;导出长边超过 2048 时自动收敛)" }
                },
                "required": ["path", "x", "y", "width", "height"]
            }),
        },
        Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
            let deps = deps.clone();
            // 同上:同步重活让出 async worker(修复批次补齐)
            Box::pin(async move {
                crate::utils::blocking::park_worker(|| zoom_impl(&deps, &ctx, &args))
            })
        }),
    );
}

fn zoom_impl(
    deps: &ToolDeps,
    ctx: &ToolContext,
    args: &serde_json::Value,
) -> Result<String, String> {
    let raw = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if raw.is_empty() {
        return Err("缺少 path(工作区内的图像路径)".to_string());
    }
    let x = args
        .get("x")
        .and_then(|v| v.as_i64())
        .ok_or("缺少 x(原图像素坐标)")?;
    let y = args
        .get("y")
        .and_then(|v| v.as_i64())
        .ok_or("缺少 y(原图像素坐标)")?;
    let width = args
        .get("width")
        .and_then(|v| v.as_u64())
        .ok_or("缺少 width")?;
    let height = args
        .get("height")
        .and_then(|v| v.as_u64())
        .ok_or("缺少 height")?;
    if width == 0 || height == 0 {
        return Err("width/height 必须 ≥ 1".to_string());
    }
    let scale = args
        .get("scale")
        .and_then(|v| v.as_f64())
        .unwrap_or(ZOOM_DEFAULT_SCALE)
        .clamp(1.0, ZOOM_MAX_SCALE);

    let (img, _) = load_image(deps, ctx, &raw)?;
    let (iw, ih) = img.dimensions();
    // 区域钳制(越界不报错,钳到图内并如实注明——模型给坐标常带小误差)
    let x0 = x.clamp(0, iw.saturating_sub(1) as i64) as u32;
    let y0 = y.clamp(0, ih.saturating_sub(1) as i64) as u32;
    let cw = (width as u32).min(iw - x0).max(1);
    let ch = (height as u32).min(ih - y0).max(1);
    let clamped =
        (x0 as i64 != x) || (y0 as i64 != y) || (cw as u64 != width) || (ch as u64 != height);

    let cropped = img.crop_imm(x0, y0, cw, ch);
    // 导出长边上限:放大后长边超 2048 时收敛倍数(不小于 1×,只钳不放)
    let long = cw.max(ch) as f64;
    let mut eff = scale;
    if long * eff > ZOOM_MAX_SIDE as f64 {
        eff = (ZOOM_MAX_SIDE as f64 / long).max(1.0);
    }
    let out = if (eff - 1.0).abs() > 1e-9 {
        let nw = ((cw as f64 * eff).round() as u32).max(1);
        let nh = ((ch as f64 * eff).round() as u32).max(1);
        cropped.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3)
    } else {
        cropped
    };
    let (ow, oh) = out.dimensions();
    let name = format!("zoom-{}", file_name_of(&raw));
    let reference = save_png(deps, &name, &out)?;
    let mut text = format!(
        "已裁剪原图 ({x0},{y0}) 起 {cw}×{ch} 区域并放大 {eff:.2}× → 输出 {ow}×{oh}。坐标契约:输入坐标为原图像素坐标(原点左上),超出原图范围时自动钳制。"
    );
    if clamped {
        text.push_str("(注意:本次区域经钳制,与你请求的坐标/尺寸不完全一致)");
    }
    Ok(vision_output(text, vec![reference]))
}

// ==================== image_diff ====================

fn register_diff(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: DIFF_TOOL.into(),
            description: "对比工作区里两张图像的差异,返回显著差异像素占比、平均通道差与可视化差异图。常用于改版前后截图核对。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path_a": { "type": "string", "description": "基准图像路径(比对以它为尺寸基准)" },
                    "path_b": { "type": "string", "description": "对照图像路径" }
                },
                "required": ["path_a", "path_b"]
            }),
        },
        Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
            let deps = deps.clone();
            // 同上:同步重活让出 async worker(修复批次补齐)
            Box::pin(async move {
                crate::utils::blocking::park_worker(|| diff_impl(&deps, &ctx, &args))
            })
        }),
    );
}

fn diff_impl(
    deps: &ToolDeps,
    ctx: &ToolContext,
    args: &serde_json::Value,
) -> Result<String, String> {
    let path_a = args
        .get("path_a")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let path_b = args
        .get("path_b")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if path_a.is_empty() || path_b.is_empty() {
        return Err("缺少 path_a / path_b".to_string());
    }
    let (a, _) = load_image(deps, ctx, &path_a)?;
    let (b, _) = load_image(deps, ctx, &path_b)?;
    let (aw, ah) = a.dimensions();
    let resized = b.dimensions() != (aw, ah);
    let b = if resized {
        b.resize_exact(aw, ah, image::imageops::FilterType::Lanczos3)
    } else {
        b
    };
    let (ar, br) = (a.to_rgb8(), b.to_rgb8());
    let mut diff = image::RgbImage::new(aw, ah);
    let mut changed: u64 = 0;
    let mut total_abs: u64 = 0;
    let total = (aw as u64) * (ah as u64) * 3;
    for (x, y, pa) in ar.enumerate_pixels() {
        let pb = br.get_pixel(x, y);
        let dr = (pa[0] as i16 - pb[0] as i16).unsigned_abs() as u32;
        let dg = (pa[1] as i16 - pb[1] as i16).unsigned_abs() as u32;
        let db = (pa[2] as i16 - pb[2] as i16).unsigned_abs() as u32;
        total_abs += (dr + dg + db) as u64;
        let d = dr.max(dg).max(db) as u8;
        if d > DIFF_THRESHOLD {
            changed += 1;
            // 差异像素:亮红标出(在 A 的灰度底上))
            diff.put_pixel(x, y, image::Rgb([255, 64, 64]));
        } else {
            let g = ((pa[0] as u32 + pa[1] as u32 + pa[2] as u32) / 3 / 3) as u8;
            diff.put_pixel(x, y, image::Rgb([g, g, g]));
        }
    }
    let pct = if total > 0 {
        changed as f64 * 100.0 / (aw as f64 * ah as f64)
    } else {
        0.0
    };
    let mean = if total > 0 {
        total_abs as f64 / total as f64
    } else {
        0.0
    };
    let name = format!("diff-{}", file_name_of(&path_a));
    let reference = save_png(deps, &name, &DynamicImage::ImageRgb8(diff))?;
    let mut text = format!(
        "差异统计:{changed}/{px} 像素({pct:.2}%)存在显著差异(通道差阈值 {DIFF_THRESHOLD}/255),平均通道差 {mean:.2}/255。差异图已随附:亮红=差异像素,暗灰=相同区域(底图为图 A 灰度)。基准:图 A {aw}×{ah}。",
        px = (aw as u64) * (ah as u64)
    );
    if resized {
        text.push_str("(图 B 尺寸与 A 不同,已缩放对齐后再比)");
    }
    Ok(vision_output(text, vec![reference]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::types::ExecScope;
    use crate::utils::test_support::TempDataDir;
    use std::path::{Path, PathBuf};

    /// 工作区根必须是**独立于** DATA_DIR 的目录:workspace_guard 的二次防线会拒绝
    /// 落在数据目录内的路径(真实任务里工作区与 DATA_DIR 也从不相同)。
    fn workspace_guard() -> (TempDataDir, PathBuf) {
        let dir = TempDataDir::new("vision-ws");
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    fn ctx_with_scope(root: &Path) -> ToolContext {
        ToolContext {
            session_id: "task:vision-test".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: Some(Arc::new(ExecScope::new(root.to_path_buf(), false))),
        }
    }

    fn write_png(dir: &Path, name: &str, w: u32, h: u32, seed: u8) -> std::path::PathBuf {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 251) as u8, seed])
        });
        let path = dir.join(name);
        DynamicImage::ImageRgb8(img).save(&path).unwrap();
        path
    }

    fn parse_output(out: &str) -> (String, Vec<ImageRef>) {
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        let text = v["text"].as_str().unwrap().to_string();
        let images: Vec<ImageRef> = serde_json::from_value(v["images"].clone()).unwrap();
        (text, images)
    }

    /// view_image:成功返回约定形状(text+引用),引用可经 images 服务解析回 data URL;
    /// 牢笼拒绝越界路径;超限与坏图给出可操作错误
    #[test]
    fn view_image_returns_text_and_resolvable_ref() {
        let (_data_guard, deps) = ToolDeps::dummy_for_test();
        let (_ws_guard, root) = workspace_guard();
        write_png(&root, "shot.png", 400, 300, 9);
        let ctx = ctx_with_scope(&root);
        let out = view_impl(&deps, &ctx, &json!({ "path": "shot.png" })).unwrap();
        let (text, images) = parse_output(&out);
        assert!(text.contains("400×300"), "{text}");
        assert_eq!(images.len(), 1);
        assert!(images[0].id.ends_with(".png"), "{}", images[0].id);
        let url = deps.images.resolve_data_url(&images[0].id).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        // 牢笼:越界与绝对路径逃逸一律拒绝
        let e = view_impl(&deps, &ctx, &json!({ "path": "../escape.png" })).unwrap_err();
        assert!(
            e.contains("拒绝") || e.contains("越界") || e.contains("工作区"),
            "{e}"
        );
        // 坏图明文报错
        std::fs::write(root.join("bad.png"), b"not an image").unwrap();
        let e = view_impl(&deps, &ctx, &json!({ "path": "bad.png" })).unwrap_err();
        assert!(e.contains("不是可解码的图像"), "{e}");
        // 解压炸弹防线(修复批次):头部尺寸超限在分配像素缓冲之前即拒
        std::fs::write(
            root.join("huge.png"),
            crate::utils::test_support::png_header_with_dims(100_000, 100_000),
        )
        .unwrap();
        let e = view_impl(&deps, &ctx, &json!({ "path": "huge.png" })).unwrap_err();
        assert!(e.contains("尺寸过大"), "{e}");
    }

    /// 未绑定工作区:三件都报可操作错误(执行侧兜底)
    #[test]
    fn vision_tools_require_workspace_scope() {
        let (_guard, deps) = ToolDeps::dummy_for_test();
        let ctx = ToolContext {
            session_id: "s".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: None,
        };
        let e = view_impl(&deps, &ctx, &json!({ "path": "a.png" })).unwrap_err();
        assert!(e.contains("未绑定工作区"), "{e}");
    }

    /// zoom_image:区域裁剪 + 放大;越界坐标钳制并在文案注明;导出长边 ≤ 2048
    #[test]
    fn zoom_image_crops_scales_and_clamps() {
        let (_data_guard, deps) = ToolDeps::dummy_for_test();
        let (_ws_guard, root) = workspace_guard();
        write_png(&root, "big.png", 600, 400, 5);
        let ctx = ctx_with_scope(&root);
        let out = zoom_impl(
            &deps,
            &ctx,
            &json!({ "path": "big.png", "x": 100, "y": 50, "width": 200, "height": 100, "scale": 2.0 }),
        )
        .unwrap();
        let (text, images) = parse_output(&out);
        assert!(text.contains("(100,50)"), "{text}");
        assert!(text.contains("200×100"), "{text}");
        assert!(
            text.contains("× → 输出 400×200") || text.contains("放大 2.00× → 输出 400×200"),
            "{text}"
        );
        assert!(
            !text.contains("本次区域经钳制"),
            "正常范围内不应提示钳制:{text}"
        );
        assert_eq!(images.len(), 1);
        // 越界:坐标与尺寸被钳制并在文案注明
        let out = zoom_impl(
            &deps,
            &ctx,
            &json!({ "path": "big.png", "x": 500, "y": 380, "width": 500, "height": 500, "scale": 8.0 }),
        )
        .unwrap();
        let (text, _) = parse_output(&out);
        assert!(text.contains("本次区域经钳制"), "{text}");
        // 放大后长边 ≤ 2048(大区域 × 8 倍会被收敛)
        let out = zoom_impl(
            &deps,
            &ctx,
            &json!({ "path": "big.png", "x": 0, "y": 0, "width": 600, "height": 400, "scale": 8.0 }),
        )
        .unwrap();
        let (text, _) = parse_output(&out);
        assert!(text.contains("输出 2048×"), "长边应收到 2048:{text}");
    }

    /// image_diff:相同图 → 0% 差异;改一块 → 差异占比 > 0 且差异图随附;缺参报错
    #[test]
    fn image_diff_reports_stats_and_emits_map() {
        let (_data_guard, deps) = ToolDeps::dummy_for_test();
        let (_ws_guard, root) = workspace_guard();
        write_png(&root, "a.png", 200, 200, 0);
        write_png(&root, "b.png", 200, 200, 0);
        let ctx = ctx_with_scope(&root);
        let out = diff_impl(
            &deps,
            &ctx,
            &json!({ "path_a": "a.png", "path_b": "b.png" }),
        )
        .unwrap();
        let (text, images) = parse_output(&out);
        assert!(text.contains("0/40000 像素(0.00%)"), "{text}");
        assert_eq!(images.len(), 1);
        // 改一块:差异 > 0
        let mut changed = image::RgbImage::from_fn(200, 200, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 251) as u8, 0])
        });
        for x in 0..100 {
            for y in 0..100 {
                changed.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
        DynamicImage::ImageRgb8(changed)
            .save(root.join("c.png"))
            .unwrap();
        let out = diff_impl(
            &deps,
            &ctx,
            &json!({ "path_a": "a.png", "path_b": "c.png" }),
        )
        .unwrap();
        let (text, _) = parse_output(&out);
        assert!(!text.contains("0/40000 像素(0.00%)"), "差异不应为 0:{text}");
        assert!(text.contains("亮红=差异像素"), "{text}");
        // 缺参
        let e = diff_impl(&deps, &ctx, &json!({ "path_a": "a.png" })).unwrap_err();
        assert!(e.contains("path_b"), "{e}");
    }
}
