// 安卓截图服务桥(移动端视觉能力包 A3,2026-10-02):无障碍 takeScreenshot 的 Rust 侧。
//
// 与 [`super::native_bridge_android`] 同一模式:实际逻辑在 Kotlin
// (`KedaiAccessibilityService` / `ScreenCaptureBridge`),Rust 只做字符串进出。
// 三项能力:
//   - [`capture_to_file`]  整屏截图 PNG 原子写入给定路径(JNI 阻塞,调用方 spawn_blocking);
//   - [`service_enabled`]  无障碍服务启用状态(进程内缓存;`refresh` 清缓存重探);
//   - [`open_settings`]    跳系统「无障碍」设置页(设置面板按钮触发)。
//
// 本文件**不是整文件 `#![cfg(target_os = "android")]`**(与 exec/android.rs 的差别):
// status 解析、PNG 魔数/尺寸校验等纯函数要在宿主机单测能跑(Android cfg 代码在
// Windows 上不参与编译,规则 L 只能源文本守;能宿主测的逻辑都放在宿主可测位置)。
// 仅 JNI 调用点与状态缓存按 android 目标编译。
//
// 入参/出参编码(与 Kotlin 侧 SEP 一致,US 单元分隔符):
//   capture:  path ␟ timeoutMs  →  ""(成功;PNG 已原子落盘)或抛异常
//   status:   ""                →  enabled␟描述 | disabled␟原因

/// 单元分隔符(US):与 Kotlin 侧 SEP 一致
pub const SEP: char = '\u{1f}';

/// 截图超时(毫秒):系统 takeScreenshot 正常在数十毫秒内回调,8s 已是宽裕上限;
/// 超时即报错(不挂死调用线程)。非 android 目标仅供测试引用,故放行 dead_code。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub const CAPTURE_TIMEOUT_MS: u64 = 8000;

/// 解析 Kotlin `status()` 回传:`enabled␟描述` / `disabled␟原因` → (是否启用, 原因/描述)。
///
/// 形状不符(空串 / **缺分隔符** / 前缀不认识)一律按**未启用**处理并如实回显原文——
/// 探测路径宁严勿宽:拿不准就不给「看似可用」的假象(与 exec 探测失败回退 disabled 同口径)。
/// 「缺分隔符」特指裸 `enabled`/`disabled`:Kotlin 侧任何一次合法回传都带 SEP,不带即视为
/// 形状异常;「合法前缀 + 空文本」(如 `enabled␟`)才给默认文案。
pub fn parse_status(raw: &str) -> (bool, String) {
    let raw = raw.trim();
    let Some((flag, text)) = raw.split_once(SEP) else {
        return (
            false,
            if raw.is_empty() {
                "无障碍服务状态未知(空响应)".to_string()
            } else {
                format!("无障碍服务状态未知:{raw}")
            },
        );
    };
    let text = text.trim();
    match flag.trim() {
        "enabled" => (
            true,
            if text.is_empty() {
                "无障碍截图服务已启用".to_string()
            } else {
                text.to_string()
            },
        ),
        "disabled" => (
            false,
            if text.is_empty() {
                "无障碍截图服务未启用".to_string()
            } else {
                text.to_string()
            },
        ),
        other => (false, format!("无障碍服务状态未知:{other}")),
    }
}

/// PNG 魔数校验(8 字节签名):截图回读的第一道校验,与图像通道的魔数嗅探同口径。
/// 不校验语义完整性(尺寸另经 [`png_dimensions`] 只读头部核对)。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

/// 从 PNG 头部读尺寸(IHDR 宽高,**只读头部不解码**)。非 PNG / 头部截断 / 零尺寸 → None。
/// 为什么不用解码器:截图回读只需一个健全性核对,不必为读宽高解一遍全图。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let head = bytes.get(..24)?;
    if !head.starts_with(b"\x89PNG\r\n\x1a\n") || &head[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// 无障碍截图服务启用状态(带进程内缓存;`refresh=true` 清缓存重探)。
///
/// 非 Android 目标恒 `(false, …)`——桌面截图走 Windows 原生 GDI,不经本通道。
/// 缓存语义与 exec 档位探测同型:用户刚在系统设置里开了服务,需经设置页「刷新」
/// 或本函数 `refresh=true` 才作数(避免每次工具编译/每次请求都扫一遍系统服务列表)。
pub fn service_enabled(refresh: bool) -> (bool, String) {
    #[cfg(target_os = "android")]
    {
        android::service_enabled(refresh)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = refresh;
        (
            false,
            "非 Android 平台(桌面截图走 Windows 原生 GDI,不经无障碍通道)".to_string(),
        )
    }
}

/// 截整屏 PNG **原子落盘**到给定路径(仅 Android;其它目标恒 Err)。
/// JNI 阻塞:调用方必须放进 `spawn_blocking`。
pub fn capture_to_file(path: &str, timeout_ms: u64) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        android::capture_to_file(path, timeout_ms)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (path, timeout_ms);
        Err("截图仅支持 Android 与 Windows 桌面".to_string())
    }
}

/// 跳系统「无障碍」设置页(仅 Android;其它目标恒 Err)。
pub fn open_settings() -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        android::open_settings()
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("仅 Android 平台有无障碍设置页".to_string())
    }
}

#[cfg(target_os = "android")]
mod android {
    use super::{parse_status, SEP};
    use crate::services::jni_bridge::{
        call_string_static, call_string_static_no_arg, SCREEN_CAPTURE_CLASS,
    };
    use std::sync::{Mutex, OnceLock};

    /// 状态探测缓存(是否启用, 原因/描述)
    static STATUS_CACHE: OnceLock<Mutex<Option<(bool, String)>>> = OnceLock::new();

    fn cache() -> &'static Mutex<Option<(bool, String)>> {
        STATUS_CACHE.get_or_init(|| Mutex::new(None))
    }

    pub(super) fn capture_to_file(path: &str, timeout_ms: u64) -> Result<(), String> {
        let payload = format!("{path}{SEP}{timeout_ms}");
        call_string_static(SCREEN_CAPTURE_CLASS, "capture", &payload).map(|_| ())
    }

    pub(super) fn open_settings() -> Result<(), String> {
        call_string_static_no_arg(SCREEN_CAPTURE_CLASS, "openSettings").map(|_| ())
    }

    pub(super) fn service_enabled(refresh: bool) -> (bool, String) {
        if !refresh {
            if let Some(hit) = cache().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                return hit.clone();
            }
        }
        let probed = match call_string_static_no_arg(SCREEN_CAPTURE_CLASS, "status") {
            Ok(raw) => parse_status(&raw),
            Err(e) => {
                // 探测失败按未启用处理(宁严勿宽);错误原文进日志,给用户的是可操作指引
                tracing::warn!(error = %e, "无障碍截图状态探测失败,按未启用处理");
                (
                    false,
                    "无障碍服务状态探测失败(应用可能尚未完全启动,请稍后重试)".to_string(),
                )
            }
        };
        *cache().lock().unwrap_or_else(|e| e.into_inner()) = Some(probed.clone());
        probed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_enabled_and_disabled_status() {
        let (ok, why) = parse_status(&format!("enabled{SEP}无障碍截图服务已启用"));
        assert!(ok);
        assert!(why.contains("已启用"), "{why}");

        let (ok, why) = parse_status(&format!(
            "disabled{SEP}无障碍截图服务未启用(系统设置 → 无障碍)"
        ));
        assert!(!ok);
        assert!(why.contains("未启用"), "{why}");
    }

    /// 形状异常一律按未启用(宁严勿宽):空串 / 裸 `enabled`(缺分隔符)/ 未知前缀;
    /// 合法前缀 + 空文本才给默认文案(Kotlin 合法回传必带 SEP)
    #[test]
    fn unknown_or_malformed_status_is_not_enabled() {
        for raw in [
            "",
            "enabled",
            "disabled",
            "yes",
            "enabled-without-sep",
            "莫名前缀",
        ] {
            let (ok, _) = parse_status(raw);
            assert!(!ok, "raw={raw:?} 不应判为启用");
        }
        let (ok, why) = parse_status(&format!("enabled{SEP}"));
        assert!(ok, "带分隔符的空描述应按「启用 + 默认文案」");
        assert!(why.contains("已启用"), "{why}");
        let (ok, why) = parse_status(&format!("disabled{SEP}"));
        assert!(!ok);
        assert!(why.contains("未启用"), "{why}");
    }

    #[test]
    fn png_math_and_dimensions() {
        // 构造一个最小 PNG 头部(签名 + IHDR 宽高);不构造完整文件(只测头部读取)
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes.extend_from_slice(&13u32.to_be_bytes()); // IHDR 长度
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&1080u32.to_be_bytes());
        bytes.extend_from_slice(&2400u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]); // 位深/颜色类型等占位
        assert!(is_png(&bytes));
        assert_eq!(png_dimensions(&bytes), Some((1080, 2400)));

        // 非 PNG
        assert!(!is_png(b"not a png at all"));
        assert_eq!(png_dimensions(b"not a png at all"), None);
        // 截断头(只有签名)
        assert_eq!(png_dimensions(&bytes[..8]), None);
        // 零尺寸 → None
        let mut zero = bytes.clone();
        zero[16..20].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(png_dimensions(&zero), None);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn non_android_stub_reports_unavailable() {
        let (ok, why) = service_enabled(false);
        assert!(!ok);
        assert!(why.contains("非 Android"), "{why}");
        assert!(capture_to_file("x.png", 1000).is_err());
        assert!(open_settings().is_err());
    }
}
