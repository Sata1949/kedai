//! Android 原生能力桥接(JNI,仅 Android 编译)。
//!
//! 与 [`super::keystore_android`] 同一模式:实际逻辑在 Kotlin `KedaiNative`,
//! Rust 只做 String 进出。五项能力:
//!   - `open_external(url)`  用系统浏览器打开外链(避免 WebView 把 SPA 导航走);
//!   - `share_file(name, content)` 写入缓存目录后弹系统分享选单(导出功能,SAF 友好);
//!   - `save_to_downloads(name, content)` 直接写入设备下载目录,返回实际落盘位置
//!     (submit 工具的落点:非 root/非 Shizuku 的沙箱档下产物交付通道);
//!   - `keepalive_start()` / `keepalive_stop()` 启停前台服务,防止后台长任务被冻结。
//!
//! 说明:入参统一编码为 `name\u{1f}content`(单元分隔符),避免额外 JNI 签名重载。
#![cfg(target_os = "android")]

use super::jni_bridge::{call_string_static, call_string_static_no_arg, NATIVE_CLASS};

/// 字段分隔符(US,单元分隔符):Kotlin 侧按此拆分双字段入参
const SEP: char = '\u{1f}';

/// 用系统浏览器打开外链
pub fn open_external(url: &str) -> Result<(), String> {
    call_string_static(NATIVE_CLASS, "openExternal", url).map(|_| ())
}

/// 分享文本内容为文件(写缓存 → FileProvider → 系统分享选单)
pub fn share_file(name: &str, content: &str) -> Result<(), String> {
    let payload = format!("{name}{SEP}{content}");
    call_string_static(NATIVE_CLASS, "shareFile", &payload).map(|_| ())
}

/// 把产物写入设备「下载」目录,返回**实际落盘位置的可读描述**。
///
/// 与 share_file 的差别:不经分享选单,直接落到用户可见位置(API≥29 走
/// MediaStore.Downloads 免权限写入;API<29 回退应用外部目录并把真实路径回传)。
pub fn save_to_downloads(name: &str, content: &str) -> Result<String, String> {
    let payload = format!("{name}{SEP}{content}");
    call_string_static(NATIVE_CLASS, "saveToDownloads", &payload)
}

/// 启动前台服务保活(Kotlin 侧无入参,走无参入口)
pub fn keepalive_start() -> Result<(), String> {
    call_string_static_no_arg(NATIVE_CLASS, "startKeepAlive").map(|_| ())
}

/// 停止前台服务保活(Kotlin 侧无入参,走无参入口)
pub fn keepalive_stop() -> Result<(), String> {
    call_string_static_no_arg(NATIVE_CLASS, "stopKeepAlive").map(|_| ())
}
