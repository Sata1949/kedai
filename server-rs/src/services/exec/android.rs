// Android 执行器:经 Kotlin 桥执行(阶段 C/D)。
//
// 为什么必须在 Kotlin 侧执行(见 docs/计划.md 阶段 3 第 2 条):
// 进程派生、`su` 弹窗、Shizuku binder 都是 Java 层 API;Rust 虽能 `Command::new`,
// 但普通应用 UID 受 SELinux 约束,拿不到 root/ADB 权限。故 Rust 只做字符串进出
// (与 native_bridge_android.rs 同一模式),执行逻辑在 Kotlin `ShellExecutorBridge`。
//
// 入参编码约定(T3 起为 v2 长度无关格式,详见 `exec::wire` 模块头):
//   exec:       v2 ␟ b64(command) ␟ b64(cwd) ␟ timeoutMs  →  v2 ␟ exitCode ␟ b64(stdout) ␟ b64(stderr)
//   detectTier: ""                                         →  root|shizuku|sandbox|disabled
//   requestShizukuPermission: ""                           →  ""(成功)或错误消息
#![cfg(target_os = "android")]

use super::wire::{decode_exec_result, encode_exec_request};
use super::{strip_ansi, truncate_output, ExecRequest, ExecResult, ShellTier};
use crate::services::jni_bridge::{call_string_static, call_string_static_no_arg, EXEC_CLASS};

/// 探测当前可用执行器等级(Kotlin 侧按 ROOT → Shizuku → Sandbox 顺序探测)。
/// 探测失败一律回退 Disabled:宁可不可用,不给「看似可用实则失败」的假象。
///
/// 注意 `detectTier` 在 Kotlin 侧**不接参数**,必须走无参入口——此前误按「一个
/// String 入参」调用,报 `NoSuchMethodError`,使本函数恒回退 disabled(2026-09-17 实测)。
pub fn detect_tier() -> ShellTier {
    match call_string_static_no_arg(EXEC_CLASS, "detectTier") {
        Ok(s) => ShellTier::from_str_lossy(s.trim()),
        Err(e) => {
            tracing::warn!(error = %e, "探测 Android 执行器等级失败,回退 disabled");
            ShellTier::Disabled
        }
    }
}

/// 强制重新探测执行器等级(清 Kotlin 侧缓存后重跑探测)。
/// 同 `detectTier`:Kotlin 侧无入参,必须走无参入口(否则 NoSuchMethodError)。
pub fn refresh_tier() -> ShellTier {
    match call_string_static_no_arg(EXEC_CLASS, "refreshTier") {
        Ok(s) => ShellTier::from_str_lossy(s.trim()),
        Err(e) => {
            tracing::warn!(error = %e, "强制重探 Android 执行器等级失败,回退 disabled");
            ShellTier::Disabled
        }
    }
}

/// 经 Kotlin 桥执行命令。
/// 注意:授权与风险确认已在 bash 工具层完成,本函数只负责执行与结果解析。
pub async fn execute(req: ExecRequest) -> Result<ExecResult, String> {
    let tier = detect_tier();
    if tier == ShellTier::Disabled {
        return Err("Android 执行层不可用:请在设置中开启执行通道(沙箱 / Shizuku / ROOT)".into());
    }

    let timeout_ms = super::clamp_timeout(req.timeout_ms);
    let payload = encode_exec_request(&req.command, req.cwd.as_deref(), timeout_ms);

    // JNI 调用是同步阻塞的,放到阻塞线程池避免占用 async 运行时
    let raw = tokio::task::spawn_blocking(move || call_string_static(EXEC_CLASS, "exec", &payload))
        .await
        .map_err(|e| format!("执行任务调度失败: {e}"))??;

    parse_result(&raw, tier)
}

/// 解析 Kotlin 回传的 `v2 ␟ exitCode ␟ b64(stdout) ␟ b64(stderr)`。
/// 三字段缺失或版本不匹配时给出可诊断的错误(而非静默当空输出)。
fn parse_result(raw: &str, tier: ShellTier) -> Result<ExecResult, String> {
    let (exit_code, stdout, stderr) = decode_exec_result(raw)?;
    Ok(ExecResult {
        exit_code,
        // ANSI 在截断之前剥掉(2026-11-06):序列不占 32K 字符预算,也不会被
        // 「保头保尾」窗口切成两半。口径与桌面侧 `drain_pipe` 同源。
        stdout: truncate_output(&strip_ansi(&stdout)),
        stderr: truncate_output(&strip_ansi(&stderr)),
        tier,
        timed_out: false,
        // 截断判定在整串截断之后:数一下剥完 ANSI 的字符数是否超预算
        truncated: stdout.chars().count() > super::MAX_OUTPUT_CHARS
            || stderr.chars().count() > super::MAX_OUTPUT_CHARS,
    })
}

/// 请求 Shizuku 权限(由前端按钮触发;首次会弹系统授权框)。
/// 同 `detectTier`:Kotlin 侧无入参,必须走无参入口。
pub fn request_shizuku_permission() -> Result<(), String> {
    call_string_static_no_arg(EXEC_CLASS, "requestShizukuPermission").map(|_| ())
}

/// Shizuku 应用是否已安装("1" = 已安装)。供设置页授权面板给出可行动提示。
pub fn shizuku_installed() -> bool {
    matches!(call_string_static_no_arg(EXEC_CLASS, "shizukuInstalled"), Ok(s) if s.trim() == "1")
}

/// Shizuku 当前是否已获授权。
pub fn shizuku_granted() -> bool {
    matches!(call_string_static_no_arg(EXEC_CLASS, "shizukuGranted"), Ok(s) if s.trim() == "1")
}

#[cfg(test)]
mod tests {
    use super::super::wire::SEP;
    use super::*;

    /// v2 线格式:回执按 `v2 ␟ exitCode ␟ b64(stdout) ␟ b64(stderr)` 解析。
    /// 编码侧用 `wire::encode_exec_result`(与 Kotlin 同一算法),两侧对拍由此可用。
    fn make_result(exit_code: i32, stdout: &str, stderr: &str) -> String {
        super::super::wire::encode_exec_result(exit_code, stdout, stderr)
    }

    #[test]
    fn parses_well_formed_result() {
        let raw = make_result(0, "hello", "");
        let r = parse_result(&raw, ShellTier::Shizuku).expect("应解析成功");
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "hello");
        assert_eq!(r.stderr, "");
        assert_eq!(r.tier, ShellTier::Shizuku);
    }

    #[test]
    fn parses_result_with_stderr() {
        let raw = make_result(1, "out", "err message");
        let r = parse_result(&raw, ShellTier::Root).expect("应解析成功");
        assert_eq!(r.exit_code, 1);
        assert_eq!(r.stdout, "out");
        assert_eq!(r.stderr, "err message");
    }

    /// T3 判别性用例:输出含旧分隔符 US 时不得错位(修复前 splitn(3, US) 会切坏)。
    #[test]
    fn parses_output_containing_separator() {
        let raw = make_result(0, "a\u{1f}b", "c\u{1f}d");
        let r = parse_result(&raw, ShellTier::Sandbox).expect("应解析成功");
        assert_eq!(r.stdout, "a\u{1f}b");
        assert_eq!(r.stderr, "c\u{1f}d");
    }

    #[test]
    fn rejects_legacy_wire_format() {
        // 旧格式(v1,US 分隔)必须显式报错,不静默按新格式解读
        let raw = format!("0{SEP}out{SEP}err");
        let err = parse_result(&raw, ShellTier::Root).unwrap_err();
        assert!(err.contains("协议版本不匹配"), "{err}");
    }

    #[test]
    fn rejects_malformed_exit_code() {
        let raw = format!(
            "{}{SEP}notanumber{SEP}{SEP}",
            super::super::wire::WIRE_VERSION
        );
        let err = parse_result(&raw, ShellTier::Root).unwrap_err();
        assert!(err.contains("非法"), "{err}");
    }

    #[test]
    fn truncates_long_output() {
        let long = "x".repeat(super::super::MAX_OUTPUT_CHARS + 50);
        let raw = make_result(0, &long, "");
        let r = parse_result(&raw, ShellTier::Sandbox).expect("应解析成功");
        assert!(r.stdout.contains("已截断"));
    }
}
