// Windows 截图工具(视觉能力包 D5,2026-10-02):全屏 / 指定显示器 / 区域 / 窗口。
//
// 与 D4 视觉工具同通道:返回约定式 `{text, images:[引用]}`——PNG 落 DATA_DIR/images,
// 执行器把引用转为 data URL 随 tool 消息下发(模型真正「看」到屏幕)。
//
// 可见性:由「视觉与截图」总开关 `vision_screenshot_enabled`(默认关,隐私敏感)约束——
// 下发侧:任务路径经 `tool_policy::screenshot_gate`、聊天路径经
// `tool_sets::filter_screenshot`,两处同一判据;执行侧再兜底校验一次(防凭历史臆造调用)。
// 风险级 **Sensitive**(读屏即读敏感内容;但不写文件系统、不可回退,归危险级会被任务
// 默认策略剔除)。**审计只记元数据**(范围/尺寸/DPI/窗口名,不含像素——exec_audit 仍是
// bash 专属,见 docs/遗留.md 的工具级审计登记)。
//
// 平台:`#[cfg(windows)]` 走原生 GDI(PrintWindow(PW_RENDERFULLCONTENT) 兜遮挡窗口,
// 线程级 PER_MONITOR_AWARE_V2——server 与 Tauri 同进程,不做进程级);非 Windows
// 注册一个**明确报错**的实现(文案指向移动端批次),编译与工具面在安卓/其它平台上不变。

use crate::models::types::{ToolContext, ToolDefinition};
use crate::tools::registry::ToolRegistry;
use serde_json::json;
use std::sync::Arc;

use super::agent_tools::ToolDeps;

/// 工具名(单一出处:策略闸门与执行侧兜底都以它为准)
pub const TOOL_NAME: &str = "screenshot";

/// 截图目标(纯函数 `plan_target` 产出,便于参数校验单测)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// 全屏:整个虚拟屏(多显示器拼合)的物理像素
    Fullscreen,
    /// 指定显示器(0 起,按系统枚举顺序)
    Display(usize),
    /// 区域内截图(虚拟屏物理像素坐标;原点在主显示器左上,向左/上有负坐标)
    Region { x: i64, y: i64, w: u32, h: u32 },
    /// 指定标题的可见窗口(精确优先、子串兜底、多候选列出)
    Window(String),
}

/// 参数校验(纯函数):display / region / window_title 三种范围互斥;region 宽高 ≥1。
pub(crate) fn plan_target(
    display: Option<i64>,
    region: Option<(i64, i64, u64, u64)>,
    window_title: Option<&str>,
) -> Result<Target, String> {
    let window_title = window_title.map(str::trim).filter(|s| !s.is_empty());
    if window_title.is_some() && region.is_some() {
        return Err("window_title 与 region 互斥:一次只截一种范围".to_string());
    }
    if window_title.is_some() && display.is_some() {
        return Err("window_title 与 display 互斥:一次只截一种范围".to_string());
    }
    if let Some((x, y, w, h)) = region {
        if w == 0 || h == 0 {
            return Err("region 的 width/height 必须 ≥ 1".to_string());
        }
        let (w, h) = (u32::try_from(w), u32::try_from(h));
        let (Ok(w), Ok(h)) = (w, h) else {
            return Err("region 尺寸超出上限".to_string());
        };
        return Ok(Target::Region { x, y, w, h });
    }
    if let Some(title) = window_title {
        return Ok(Target::Window(title.to_string()));
    }
    match display {
        Some(i) if i < 0 => Err("display 索引不能为负(0 = 第一个显示器)".to_string()),
        Some(i) => Ok(Target::Display(i as usize)),
        None => Ok(Target::Fullscreen),
    }
}

/// 按标题从候选里选窗口(纯函数):精确匹配(忽略大小写/首尾空白)优先,唯一即中;
/// 多个精确或仅子串命中多个 → 报错并列出候选(要求更精确的标题);零命中 → 报错。
pub(crate) fn pick_window(titles: &[String], wanted: &str) -> Result<usize, String> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return Err("window_title 不能为空".to_string());
    }
    let wl = wanted.to_lowercase();
    let exact: Vec<usize> = titles
        .iter()
        .enumerate()
        .filter(|(_, t)| t.trim().to_lowercase() == wl)
        .map(|(i, _)| i)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    if exact.len() > 1 {
        return Err(format!(
            "有 {} 个窗口标题完全一致「{}」,无法区分:{}",
            exact.len(),
            wanted,
            list_titles(titles, &exact)
        ));
    }
    let partial: Vec<usize> = titles
        .iter()
        .enumerate()
        .filter(|(_, t)| t.trim().to_lowercase().contains(&wl))
        .map(|(i, _)| i)
        .collect();
    match partial.len() {
        1 => Ok(partial[0]),
        0 => Err(format!(
            "未找到标题匹配「{wanted}」的可见窗口(标题需为窗口标题的子串)"
        )),
        _ => Err(format!(
            "有 {} 个窗口匹配「{}」,请用更精确的标题:{}",
            partial.len(),
            wanted,
            list_titles(titles, &partial)
        )),
    }
}

fn list_titles(titles: &[String], idx: &[usize]) -> String {
    idx.iter()
        .take(5)
        .map(|&i| format!("「{}」", titles[i]))
        .collect::<Vec<_>>()
        .join("、")
}

/// 区域与虚拟屏求交(纯函数,跨平台单测;2026-10-02 修复批次):
/// - 完全在屏外 → `None`(调用方给可操作报错并列出虚拟屏范围);
/// - 部分越界 → 返回钳制后的 `(x, y, w, h)` 与 `clamped=true`(调用方在描述里注明);
/// - 坐标极端值由 `saturating_add` 兜底,不 panic。
///
/// 入参均为虚拟屏物理像素(原点在主显示器左上,可为负)。
pub(crate) fn clamp_region(
    region: (i64, i64, u32, u32),
    screen: (i32, i32, u32, u32),
) -> Option<(i64, i64, u32, u32, bool)> {
    let (x, y, w, h) = region;
    let (vs_x, vs_y, vs_w, vs_h) = screen;
    let left = x.max(vs_x as i64);
    let top = y.max(vs_y as i64);
    let right = x.saturating_add(w as i64).min(vs_x as i64 + vs_w as i64);
    let bottom = y.saturating_add(h as i64).min(vs_y as i64 + vs_h as i64);
    if right <= left || bottom <= top {
        return None;
    }
    let (nw, nh) = ((right - left) as u32, (bottom - top) as u32);
    let clamped = left != x || top != y || nw != w || nh != h;
    Some((left, top, nw, nh, clamped))
}

/// 注册截图工具(始终注册;可见性由「视觉与截图」开关在策略层过滤)
pub fn register_screenshot_tool(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    let definition = ToolDefinition {
        name: TOOL_NAME.into(),
        description: "截取屏幕画面(全屏 / 指定显示器 / 区域 / 窗口标题),把图像交给模型查看。用于读屏视觉验证(UI 检查、改版前后对比)。".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "display": { "type": "integer", "description": "指定显示器索引(0 起,0 = 第一个);缺省且无其它范围时截整个虚拟屏(全屏)" },
                "region": {
                    "type": "object",
                    "description": "区域内截图(虚拟屏物理像素坐标;与 window_title 互斥)",
                    "properties": {
                        "x": { "type": "integer" }, "y": { "type": "integer" },
                        "width": { "type": "integer" }, "height": { "type": "integer" }
                    },
                    "required": ["x", "y", "width", "height"]
                },
                "window_title": { "type": "string", "description": "按标题截取窗口(精确优先、子串兜底;多个匹配会列出候选要求更精确);与 region/display 互斥" }
            }
        }),
    };
    #[cfg(windows)]
    {
        registry.register(
            definition,
            Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
                let deps = deps.clone();
                Box::pin(async move { screenshot_impl(&deps, &ctx, &args).await })
            }),
        );
    }
    #[cfg(not(windows))]
    {
        let _ = deps;
        registry.register(
            definition,
            Arc::new(|_args: serde_json::Value, _ctx: ToolContext| {
                Box::pin(async move {
                    Err("截图工具当前仅支持 Windows 桌面(安卓截图见移动端批次)".to_string())
                })
            }),
        );
    }
}

#[cfg(windows)]
async fn screenshot_impl(
    deps: &ToolDeps,
    ctx: &ToolContext,
    args: &serde_json::Value,
) -> Result<String, String> {
    // 执行侧兜底:开关关闭时明确拒绝(下发侧已剔除;防模型凭历史上下文臆造调用)
    if !deps.settings_snapshot().vision_screenshot_enabled {
        return Err("截图未启用:请在 设置 → 视觉与截图 中开启「允许截图工具取屏」".to_string());
    }
    let region = args.get("region").and_then(|v| v.as_object()).map(|o| {
        (
            o.get("x").and_then(|v| v.as_i64()).unwrap_or(0),
            o.get("y").and_then(|v| v.as_i64()).unwrap_or(0),
            o.get("width").and_then(|v| v.as_u64()).unwrap_or(0),
            o.get("height").and_then(|v| v.as_u64()).unwrap_or(0),
        )
    });
    let plan = plan_target(
        args.get("display").and_then(|v| v.as_i64()),
        region,
        args.get("window_title").and_then(|v| v.as_str()),
    )?;
    let target_desc = format!("{plan:?}");
    let shot = tokio::task::spawn_blocking(move || win::capture(&plan))
        .await
        .map_err(|e| format!("截图任务失败: {e}"))??;
    // 审计只记元数据(不含像素):范围 / 尺寸 / DPI / 窗口名
    tracing::info!(
        session_id = ctx.session_id.as_str(),
        target = target_desc.as_str(),
        width = shot.width,
        height = shot.height,
        dpi = shot.dpi,
        png_bytes = shot.png.len(),
        "截图完成(仅元数据)"
    );
    let mut text = format!(
        "已截图({}):输出 {}×{},DPI {}。请基于图像实际内容回答。",
        shot.desc, shot.width, shot.height, shot.dpi
    );
    if shot.uniform {
        text.push_str(
            "(本次截图内容为单一颜色,可能窗口受保护/最小化或画面全黑,如非预期请改用其它范围)",
        );
    }
    let reference = deps.images.save(
        &format!("shot-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S")),
        "image/png",
        &shot.png,
    )?;
    Ok(json!({ "text": text, "images": [reference] }).to_string())
}

/// 一次截图的产物(png 字节 + 元数据;像素只落盘,不进日志)
pub(crate) struct Shot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    /// 整个画面为单一颜色(全黑/全白):如实注记,不当作失败
    pub uniform: bool,
    /// 范围描述(显示器 i/N(1920×1080)/窗口「标题」(PrintWindow|屏幕回退)等)
    pub desc: String,
}

/// BGRA(自顶向下 32bpp)→ RGBA(编码 PNG 与单色检测的公共前置;
/// 尺寸由调用方保证与 bgra 长度一致)
pub(crate) fn rgba_from_bgra(bgra: &[u8], _width: u32, _height: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; bgra.len()];
    for (src, dst) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = 255;
    }
    rgba
}

/// 单色检测(全黑/全白等):与首个像素逐通道比对(纯函数,便于单测)
pub(crate) fn is_uniform(rgba: &[u8]) -> bool {
    let Some(first) = rgba.get(..4) else {
        return false;
    };
    rgba.chunks_exact(4).all(|px| px == first)
}

#[cfg(windows)]
mod win {
    use super::{clamp_region, is_uniform, rgba_from_bgra, Shot, Target};
    use std::io::Cursor;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi;
    use windows_sys::Win32::UI::HiDpi;
    use windows_sys::Win32::UI::WindowsAndMessaging as wm;

    /// 线程级 DPI 感知守卫:进入时切 PER_MONITOR_AWARE_V2(拿物理像素),离开时还原。
    /// **不做进程级**(server 与 Tauri 同进程,进程级会改变宿主渲染行为)。
    struct DpiGuard(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);

    impl DpiGuard {
        fn enter() -> Self {
            let prev = unsafe {
                HiDpi::SetThreadDpiAwarenessContext(
                    HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                )
            };
            Self(prev)
        }
    }

    impl Drop for DpiGuard {
        fn drop(&mut self) {
            unsafe {
                HiDpi::SetThreadDpiAwarenessContext(self.0);
            }
        }
    }

    /// 执行截图(阻塞;调用方经 spawn_blocking)。
    pub(super) fn capture(target: &Target) -> Result<Shot, String> {
        let _dpi = DpiGuard::enter();
        match target {
            Target::Fullscreen => {
                let (x, y, w, h) = virtual_screen()?;
                capture_rect(x as i64, y as i64, w, h, format!("全屏虚拟屏 {w}×{h}"))
            }
            Target::Display(i) => {
                let monitors = monitors();
                if monitors.is_empty() {
                    return Err("未能枚举到任何显示器".to_string());
                }
                let Some(m) = monitors.get(*i) else {
                    let list = monitors
                        .iter()
                        .enumerate()
                        .map(|(idx, m)| {
                            format!(
                                "[{idx}] {}×{}",
                                m.rect.right - m.rect.left,
                                m.rect.bottom - m.rect.top
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    return Err(format!(
                        "display 索引 {i} 超出范围(本机 {} 个显示器:{list})",
                        monitors.len()
                    ));
                };
                let w = (m.rect.right - m.rect.left).max(0) as u32;
                let h = (m.rect.bottom - m.rect.top).max(0) as u32;
                if w == 0 || h == 0 {
                    return Err("该显示器尺寸无效".to_string());
                }
                let primary = if m.primary { ",主显示器" } else { "" };
                capture_rect(
                    m.rect.left as i64,
                    m.rect.top as i64,
                    w,
                    h,
                    format!("显示器 {}/{} {w}×{h}{primary}", i + 1, monitors.len()),
                )
            }
            Target::Region { x, y, w, h } => {
                // 钳制到虚拟屏(修复批次):越界部分此前会截出未定义/黑块;完全在屏外
                // 给可操作报错(附虚拟屏范围),部分越界钳制并在描述中注明
                let (vs_x, vs_y, vs_w, vs_h) = virtual_screen()?;
                match clamp_region((*x, *y, *w, *h), (vs_x, vs_y, vs_w, vs_h)) {
                    Some((cx, cy, cw, ch, clamped)) => {
                        let desc = if clamped {
                            format!("区域 ({x},{y}) {w}×{h} → 钳制为 ({cx},{cy}) {cw}×{ch}")
                        } else {
                            format!("区域 ({cx},{cy}) {cw}×{ch}")
                        };
                        capture_rect(cx, cy, cw, ch, desc)
                    }
                    None => Err(format!(
                        "区域 ({x},{y}) {w}×{h} 完全在屏幕范围外(虚拟屏原点 ({vs_x},{vs_y}),尺寸 {vs_w}×{vs_h})"
                    )),
                }
            }
            Target::Window(title) => capture_window(title),
        }
    }

    fn virtual_screen() -> Result<(i32, i32, u32, u32), String> {
        let (x, y) = (
            unsafe { wm::GetSystemMetrics(wm::SM_XVIRTUALSCREEN) },
            unsafe { wm::GetSystemMetrics(wm::SM_YVIRTUALSCREEN) },
        );
        let (w, h) = (
            unsafe { wm::GetSystemMetrics(wm::SM_CXVIRTUALSCREEN) },
            unsafe { wm::GetSystemMetrics(wm::SM_CYVIRTUALSCREEN) },
        );
        if w <= 0 || h <= 0 {
            return Err("获取虚拟屏尺寸失败(无可用桌面会话?)".to_string());
        }
        Ok((x, y, w as u32, h as u32))
    }

    struct Mon {
        rect: RECT,
        primary: bool,
    }

    /// 枚举显示器(顺序即系统枚举顺序;display 索引以此为准)
    fn monitors() -> Vec<Mon> {
        unsafe extern "system" fn cb(
            hmon: Gdi::HMONITOR,
            _hdc: Gdi::HDC,
            _rect: *mut RECT,
            lparam: LPARAM,
        ) -> windows_sys::core::BOOL {
            let out = unsafe { &mut *(lparam as *mut Vec<Mon>) };
            let mut mi: Gdi::MONITORINFO = unsafe { std::mem::zeroed() };
            mi.cbSize = std::mem::size_of::<Gdi::MONITORINFO>() as u32;
            if unsafe { Gdi::GetMonitorInfoW(hmon, &mut mi) } != 0 {
                out.push(Mon {
                    rect: mi.rcMonitor,
                    primary: mi.dwFlags & wm::MONITORINFOF_PRIMARY != 0,
                });
            }
            1
        }
        let mut v: Vec<Mon> = Vec::new();
        unsafe {
            Gdi::EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(cb),
                &mut v as *mut _ as LPARAM,
            );
        }
        v
    }

    /// 以给定虚拟屏物理像素矩形做屏幕 BitBlt 截图(i64 入参:虚拟屏坐标可含负值,
    /// 超出 i32 的异常输入直接钳制)
    fn capture_rect(x: i64, y: i64, w: u32, h: u32, desc: String) -> Result<Shot, String> {
        let clamp32 = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let (x, y) = (clamp32(x), clamp32(y));
        // 修复批次:w/h 收敛为 i32 前先做上限校验(此前 `as i32` 对超大值会静默回绕)
        let (w, h) = match (i32::try_from(w), i32::try_from(h)) {
            (Ok(w), Ok(h)) => (w, h),
            _ => return Err("截图区域尺寸超出上限".to_string()),
        };
        unsafe {
            let screen_dc = Gdi::GetDC(std::ptr::null_mut());
            if screen_dc.is_null() {
                return Err("获取屏幕 DC 失败".to_string());
            }
            let dpi = Gdi::GetDeviceCaps(screen_dc, Gdi::LOGPIXELSX as i32) as u32;
            let mem_dc = Gdi::CreateCompatibleDC(screen_dc);
            let bmp = Gdi::CreateCompatibleBitmap(screen_dc, w, h);
            let old = Gdi::SelectObject(mem_dc, bmp as _);
            let ok = Gdi::BitBlt(mem_dc, 0, 0, w, h, screen_dc, x, y, Gdi::SRCCOPY);
            let out = if ok == 0 {
                Err("BitBlt 截图失败".to_string())
            } else {
                read_pixels(mem_dc, bmp, w as u32, h as u32, dpi).map(|(png, uniform)| {
                    let desc = format!("{desc},DPI {dpi}");
                    Shot {
                        png,
                        width: w as u32,
                        height: h as u32,
                        dpi,
                        uniform,
                        desc,
                    }
                })
            };
            Gdi::SelectObject(mem_dc, old);
            Gdi::DeleteObject(bmp as _);
            Gdi::DeleteDC(mem_dc);
            Gdi::ReleaseDC(std::ptr::null_mut(), screen_dc);
            out
        }
    }

    /// 窗口截图:PrintWindow(PW_RENDERFULLCONTENT) 兜遮挡;失败回退窗口 DC BitBlt(仅可见内容)
    fn capture_window(title: &str) -> Result<Shot, String> {
        let hwnd = find_window(title)?;
        unsafe {
            let mut rect: RECT = std::mem::zeroed();
            if wm::GetWindowRect(hwnd, &mut rect) == 0 {
                return Err("获取窗口位置失败".to_string());
            }
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;
            if w <= 0 || h <= 0 {
                return Err("窗口尺寸无效(可能已最小化)".to_string());
            }
            let dpi = HiDpi::GetDpiForWindow(hwnd);
            let screen_dc = Gdi::GetDC(std::ptr::null_mut());
            let mem_dc = Gdi::CreateCompatibleDC(screen_dc);
            let bmp = Gdi::CreateCompatibleBitmap(screen_dc, w, h);
            let old = Gdi::SelectObject(mem_dc, bmp as _);
            let printed = windows_sys::Win32::Storage::Xps::PrintWindow(
                hwnd,
                mem_dc,
                wm::PW_RENDERFULLCONTENT,
            ) != 0;
            let mut via = "PrintWindow";
            let mut ok = printed;
            let mut note = String::new();
            if !printed {
                // 回退:窗口 DC 直接 BitBlt(只能拿到可见内容;遮挡部分会是叠在上面的窗口)
                let wdc = Gdi::GetWindowDC(hwnd);
                ok = Gdi::BitBlt(mem_dc, 0, 0, w, h, wdc, 0, 0, Gdi::SRCCOPY) != 0;
                Gdi::ReleaseDC(hwnd, wdc);
                via = "屏幕回退";
                if ok {
                    note = "(窗口未能完整渲染,已回退为可见区域截图)".to_string();
                }
            }
            let out = if !ok {
                Err("窗口截图失败(PrintWindow 与屏幕回退均不可用)".to_string())
            } else {
                read_pixels(mem_dc, bmp, w as u32, h as u32, dpi).map(|(png, uniform)| {
                    let desc = format!("窗口「{title}」(via {via}){note}");
                    Shot {
                        png,
                        width: w as u32,
                        height: h as u32,
                        dpi,
                        uniform,
                        desc,
                    }
                })
            };
            Gdi::SelectObject(mem_dc, old);
            Gdi::DeleteObject(bmp as _);
            Gdi::DeleteDC(mem_dc);
            Gdi::ReleaseDC(std::ptr::null_mut(), screen_dc);
            out
        }
    }

    fn find_window(title: &str) -> Result<HWND, String> {
        unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
            let out = unsafe { &mut *(lparam as *mut Vec<(HWND, String)>) };
            if unsafe { wm::IsWindowVisible(hwnd) } == 0 {
                return 1;
            }
            let len = unsafe { wm::GetWindowTextLengthW(hwnd) };
            if len <= 0 {
                return 1;
            }
            let mut buf = vec![0u16; (len + 1) as usize];
            let n = unsafe { wm::GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
            if n > 0 {
                out.push((hwnd, String::from_utf16_lossy(&buf[..n as usize])));
            }
            1
        }
        let mut windows: Vec<(HWND, String)> = Vec::new();
        unsafe {
            wm::EnumWindows(Some(cb), &mut windows as *mut _ as LPARAM);
        }
        let titles: Vec<String> = windows.iter().map(|(_, t)| t.clone()).collect();
        let idx = super::pick_window(&titles, title)?;
        Ok(windows[idx].0)
    }

    /// GetDIBits 读回 32bpp 自顶向下 BGRA → RGBA → PNG;并做单色检测
    fn read_pixels(
        mem_dc: Gdi::HDC,
        bmp: Gdi::HBITMAP,
        w: u32,
        h: u32,
        _dpi: u32,
    ) -> Result<(Vec<u8>, bool), String> {
        unsafe {
            let mut bmi: Gdi::BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = std::mem::size_of::<Gdi::BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w as i32;
            // 负高度 = 自顶向下(行序与图像一致,免去翻转)
            bmi.bmiHeader.biHeight = -(h as i32);
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = Gdi::BI_RGB;
            let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
            let lines = Gdi::GetDIBits(
                mem_dc,
                bmp,
                0,
                h,
                buf.as_mut_ptr() as *mut _,
                &mut bmi,
                Gdi::DIB_RGB_COLORS,
            );
            if lines == 0 {
                return Err("读取截图像素失败(GetDIBits)".to_string());
            }
            let rgba = rgba_from_bgra(&buf, w, h);
            let uniform = is_uniform(&rgba);
            let img = image::RgbaImage::from_raw(w, h, rgba)
                .ok_or_else(|| "构建图像缓冲区失败".to_string())?;
            let mut png = Vec::new();
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .map_err(|e| format!("图像编码失败: {e}"))?;
            Ok((png, uniform))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_target_defaults_and_conflicts() {
        assert_eq!(plan_target(None, None, None).unwrap(), Target::Fullscreen);
        assert_eq!(
            plan_target(Some(1), None, None).unwrap(),
            Target::Display(1)
        );
        assert_eq!(
            plan_target(None, Some((-100, 0, 800, 600)), None).unwrap(),
            Target::Region {
                x: -100,
                y: 0,
                w: 800,
                h: 600
            }
        );
        assert_eq!(
            plan_target(None, None, Some("记事本")).unwrap(),
            Target::Window("记事本".into())
        );
        // 互斥与非法值
        assert!(plan_target(None, Some((0, 0, 10, 10)), Some("x")).is_err());
        assert!(plan_target(Some(0), None, Some("x")).is_err());
        assert!(plan_target(None, Some((0, 0, 0, 10)), None).is_err());
        assert!(plan_target(Some(-1), None, None).is_err());
        // 空白标题视为缺省
        assert_eq!(
            plan_target(None, None, Some("   ")).unwrap(),
            Target::Fullscreen
        );
    }

    #[test]
    fn pick_window_prefers_exact_then_substring() {
        let titles = vec![
            "Kedai — 设置".to_string(),
            "记事本".to_string(),
            "项目 - 记事本".to_string(),
        ];
        // 精确优先:子串命中两个时,精确那个胜出
        assert_eq!(pick_window(&titles, "记事本").unwrap(), 1);
        // 子串唯一:命中「项目 - 记事本」
        assert_eq!(pick_window(&titles, "项目").unwrap(), 2);
        // 忽略大小写与首尾空白
        assert_eq!(pick_window(&titles, "  kedai — 设置 ").unwrap(), 0);
        assert_eq!(pick_window(&titles, "KEDAI").unwrap(), 0);
        // 子串命中多个 → 列出候选要求更精确
        let e = pick_window(&titles, "记").unwrap_err();
        assert!(e.contains("更精确"), "{e}");
        assert!(e.contains("记事本"), "{e}");
        // 零命中
        let e = pick_window(&titles, "不存在窗口").unwrap_err();
        assert!(e.contains("未找到"), "{e}");
    }

    /// 区域钳制(修复批次):屏内不变、部分越界钳制并标注、完全屏外 None、负原点屏兼容
    #[test]
    fn clamp_region_intersects_virtual_screen() {
        // 单屏 1920×1080,原点 (0,0):屏内区域原样返回
        assert_eq!(
            clamp_region((100, 100, 800, 600), (0, 0, 1920, 1080)),
            Some((100, 100, 800, 600, false))
        );
        // 右下越界:钳到屏边并标注 clamped
        assert_eq!(
            clamp_region((1800, 1000, 500, 500), (0, 0, 1920, 1080)),
            Some((1800, 1000, 120, 80, true))
        );
        // 完全在屏外(右侧)→ None(调用方给可操作报错)
        assert_eq!(clamp_region((2000, 0, 100, 100), (0, 0, 1920, 1080)), None);
        // 负原点虚拟屏(副屏在主屏左/上):负坐标是合法区域,不误判
        assert_eq!(
            clamp_region((-1000, -500, 400, 300), (-1000, -500, 2940, 1580)),
            Some((-1000, -500, 400, 300, false))
        );
        // 极端坐标不 panic 且判为屏外
        assert_eq!(
            clamp_region((i64::MAX, 0, 10, 10), (0, 0, 1920, 1080)),
            None
        );
    }

    #[test]
    fn uniform_detection_and_bgra_conversion() {
        // 2x2 全同色 → uniform
        let bgra = [10u8, 20, 30, 255].repeat(4);
        let rgba = rgba_from_bgra(&bgra, 2, 2);
        assert_eq!(&rgba[..4], &[30, 20, 10, 255]);
        assert!(is_uniform(&rgba));
        // 改一个像素 → 非 uniform
        let mut bgra2 = bgra;
        bgra2[0] = 99;
        assert!(!is_uniform(&rgba_from_bgra(&bgra2, 2, 2)));
    }

    /// 开关默认关时,执行侧兜底明确拒绝(不触任何 GDI 调用)
    #[tokio::test]
    async fn screenshot_refused_when_switch_off() {
        let (_guard, deps) = ToolDeps::dummy_for_test();
        let ctx = crate::models::types::ToolContext {
            session_id: "s".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: None,
        };
        #[cfg(windows)]
        {
            let e = screenshot_impl(&deps, &ctx, &json!({})).await.unwrap_err();
            assert!(e.contains("视觉与截图"), "{e}");
            assert!(e.contains("未启用"), "{e}");
        }
        #[cfg(not(windows))]
        {
            // 非 Windows:注册的实现直接给平台文案(本测试在桌面跑,此分支不执行)
            let _ = (&deps, &ctx);
        }
    }

    /// 真机手动验证(默认 `#[ignore]`,按 docs/经验.md 纪律以 `--ignored` 显式跑):
    /// 真截一次主显示器,验证 GDI 全链(取 DC / BitBlt / GetDIBits / PNG 编码)在本机可用。
    /// **只断言尺寸与编码有效性,不打印像素**(留证只记元数据)。
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "真机手动验证:需要桌面会话,CI/无头环境不跑;以 --ignored 显式执行"]
    async fn real_capture_primary_display_manual() {
        let shot = tokio::task::spawn_blocking(|| win::capture(&Target::Display(0)))
            .await
            .unwrap()
            .expect("主显示器截图应成功");
        assert!(shot.width > 0 && shot.height > 0, "应得到非零尺寸");
        assert!(shot.png.len() > 100, "PNG 编码应有实质内容");
        assert!(shot.desc.contains("显示器 1/"), "{}", shot.desc);
        eprintln!(
            "[manual] 截图留证(仅元数据):{}×{},DPI {},PNG {} 字节,单色={}",
            shot.width,
            shot.height,
            shot.dpi,
            shot.png.len(),
            shot.uniform
        );
    }
}
