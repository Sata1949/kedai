// 命令执行抽象层(阶段 C):bash 工具与 Android 执行层共用的单一执行入口。
//
// 分层动机(见 docs/计划.md 决策 4):
// - 桌面/服务端:`std::process` 直接派生(desktop.rs);
// - Android:进程派生、su 弹窗、Shizuku binder 均为 Java API,必须经 Kotlin 桥
//   (android.rs),Rust 侧只做字符串进出。
// 执行器按 ROOT → Shizuku → Sandbox 逐级探测;探测结果暴露给前端展示(等级可见)。
//
// 与权限的关系:本层**不做授权判定**。授权/确认在调用方(bash 工具)经
// `tools::permissions` 与「命令级风险强制确认」完成后才落到这里执行。
// 本层只负责:按等级执行、限时、强杀、输出截断。

use serde::Serialize;

/// 执行器等级(危险度递减)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellTier {
    /// root 提权(su / Magisk / KernelSU)
    Root,
    /// Shizuku:以 ADB shell(UID 2000)权限执行,无需 root
    Shizuku,
    /// 沙箱:应用自身 UID,能力等于本进程
    Sandbox,
    /// 不可用(Android 未开任何通道 / 探测失败)
    Disabled,
}

impl ShellTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Shizuku => "shizuku",
            Self::Sandbox => "sandbox",
            Self::Disabled => "disabled",
        }
    }

    /// 面向用户的中文等级名(前端展示与确认卡)
    pub fn label(&self) -> &'static str {
        match self {
            Self::Root => "ROOT 提权",
            Self::Shizuku => "Shizuku(ADB 权限)",
            Self::Sandbox => "沙箱(应用自身权限)",
            Self::Disabled => "已禁用",
        }
    }

    /// 从线格式字符串解析(前端/设置侧传入的等级开关校验用)。
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "root" => Self::Root,
            "shizuku" => Self::Shizuku,
            "sandbox" => Self::Sandbox,
            _ => Self::Disabled,
        }
    }
}

/// 一次执行请求。
#[derive(Debug, Clone)]
pub struct ExecRequest {
    /// 完整命令原文(交给 shell 解释;调用方已完成风险分级与授权)
    pub command: String,
    /// 工作目录(None = 进程默认 cwd)
    pub cwd: Option<String>,
    /// 超时毫秒(调用方已夹取上限;None = 默认)
    pub timeout_ms: Option<u64>,
}

/// 一次执行结果。
#[derive(Debug, Clone, Serialize)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// 实际使用的执行器等级(供审计与前端展示)
    pub tier: ShellTier,
    /// 是否因超时被强杀
    pub timed_out: bool,
}

/// 输出保留上限(字符):超出部分截断并附省略说明。
/// 与 tools/registry 的 64KB 结果截断同量级,避免一条命令把上下文灌满。
pub const MAX_OUTPUT_CHARS: usize = 32_768;

/// 尾部保留份额(批次 2 HARNESS3-4):构建/测试类命令的**失败摘要在尾部**
/// (`error[E...]`、`test result:` 那几行),只保头会把「哪一步失败」整段切掉。
/// 头:尾 = 3:1——头部留上下文(命令回显与前段进度),尾部留结论。
const TAIL_KEEP_CHARS: usize = MAX_OUTPUT_CHARS / 4;
/// 头部保留份额(= 总额 - 尾部,保证两者之和恰为 `MAX_OUTPUT_CHARS`)。
const HEAD_KEEP_CHARS: usize = MAX_OUTPUT_CHARS - TAIL_KEEP_CHARS;

/// 默认与上限超时(毫秒)。上限防止模型给出超大值导致挂起。
/// **1800s(批次 2 HARNESS3-4)**:编码类任务的自测闭环要能跑完 `cargo test` 级校验
/// (本机实测 1~16 分钟),旧值 300s 恰好卡在门槛下——自测跑不完,「改完先验证」的纪律就落空。
/// 抬这个值**不是单点改动**:注册表侧 `tools/bash.rs` 的整工具超时、任务空闲看守下限、
/// 任务步骤墙钟预算三处都按它推导(见 `settings_service::params::task_idle_floor_secs`),
/// 漏改任何一处都会让合法长命令被上层闸门掐掉或误判成空闲。
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;
pub const MAX_TIMEOUT_MS: u64 = 1_800_000;

/// 夹取超时到 [1s, MAX_TIMEOUT_MS]。
pub fn clamp_timeout(ms: Option<u64>) -> u64 {
    ms.unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1_000, MAX_TIMEOUT_MS)
}

/// 截断输出并附省略说明(按字符,避免切坏多字节);**保头 + 保尾**。
pub fn truncate_output(s: &str) -> String {
    let count = s.chars().count();
    if count <= MAX_OUTPUT_CHARS {
        return s.to_string();
    }
    let head: String = s.chars().take(HEAD_KEEP_CHARS).collect();
    let tail: String = s.chars().skip(count - TAIL_KEEP_CHARS).collect();
    let omitted = count - HEAD_KEEP_CHARS - TAIL_KEEP_CHARS;
    format!(
        "{head}\n…(输出超 {MAX_OUTPUT_CHARS} 字符已截断:保留头 {HEAD_KEEP_CHARS} + 尾 {TAIL_KEEP_CHARS} 字符,中间省略 {omitted} 字符,完整内容见审计日志)\n{tail}"
    )
}

/// 命令执行的单一入口:按当前可用等级执行。
/// 平台实现见 desktop.rs / android.rs;本函数只做等级分派与结果归一。
pub async fn execute(req: ExecRequest, allowed_tiers: &[ShellTier]) -> Result<ExecResult, String> {
    // 「等级可见 + 可关闭」的落点:探测到的等级必须被用户显式放行。
    // 桌面端恒为 Sandbox,由 bash 工具的 exec_enabled 总开关把关,此处一并生效。
    let tier = detect_tier();
    if !allowed_tiers.contains(&tier) {
        return Err(format!(
            "当前执行器等级「{}」未在设置中开启;请在「设置 → 授权管理 → 命令执行」放行该等级。",
            tier.label()
        ));
    }
    #[cfg(target_os = "android")]
    {
        return android::execute(req).await;
    }
    #[cfg(not(target_os = "android"))]
    {
        desktop::execute(req).await
    }
}

/// 探测当前可用等级(供前端展示与设置校验)。
pub fn detect_tier() -> ShellTier {
    #[cfg(target_os = "android")]
    {
        android::detect_tier()
    }
    #[cfg(not(target_os = "android"))]
    {
        // 桌面/服务端:可直接派生子进程,恒为 Sandbox(无提权语义)
        ShellTier::Sandbox
    }
}

/// 强制重新探测当前可用等级(前端「刷新」按钮用)。
///
/// 为什么需要独立入口:Android 侧 `detectTier` 带进程内缓存(避免每次 exec 都跑一遍
/// `su` 探测),而用户「新装了 Shizuku / 刚授权 / 刚装 Magisk」之后缓存就是过期值——
/// 只调 `detect_tier()` 永远拿到旧结果。Kotlin 侧早有 `refreshTier` 清缓存重探,
/// 但此前**没有任何 Rust 调用者**,导致设置页的「刷新」按钮点了等于没点
/// (2026-09-17 实测)。本函数把该能力接通。
///
/// 桌面无缓存语义,等价于 `detect_tier()`。
pub fn refresh_tier() -> ShellTier {
    #[cfg(target_os = "android")]
    {
        android::refresh_tier()
    }
    #[cfg(not(target_os = "android"))]
    {
        detect_tier()
    }
}

/// Shizuku 应用是否已安装。桌面恒 false(无 Shizuku 概念)。
///
/// 用途:设置页授权面板在用户点「请求授权」**之前**给出可行动提示——
/// 未安装时引导去安装,而不是让用户点一个必然失败的按钮。
pub fn shizuku_installed() -> bool {
    #[cfg(target_os = "android")]
    {
        android::shizuku_installed()
    }
    #[cfg(not(target_os = "android"))]
    {
        false
    }
}

/// Shizuku 是否已获授权。桌面恒 false。
pub fn shizuku_granted() -> bool {
    #[cfg(target_os = "android")]
    {
        android::shizuku_granted()
    }
    #[cfg(not(target_os = "android"))]
    {
        false
    }
}

pub mod audit;

#[cfg(not(target_os = "android"))]
pub mod desktop;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_clamped_to_bounds() {
        assert_eq!(clamp_timeout(None), DEFAULT_TIMEOUT_MS);
        assert_eq!(clamp_timeout(Some(1)), 1_000, "低于下限抬到 1s");
        assert_eq!(
            clamp_timeout(Some(10_000_000)),
            MAX_TIMEOUT_MS,
            "超上限夹到 1800s"
        );
        assert_eq!(clamp_timeout(Some(5_000)), 5_000);
        // 批次 2 的正面断言:`cargo test` 级时长必须落在合法区间内(旧上限 300s 卡在门槛下)
        assert_eq!(
            clamp_timeout(Some(900_000)),
            900_000,
            "15 分钟的自测命令应被原样接受,而不是被夹到 300s"
        );
    }

    #[test]
    fn truncate_keeps_short_output_intact() {
        assert_eq!(truncate_output("hello"), "hello");
    }

    #[test]
    fn truncate_marks_long_output() {
        let long = "字".repeat(MAX_OUTPUT_CHARS + 100);
        let out = truncate_output(&long);
        assert!(out.starts_with('字'));
        assert!(out.contains("已截断"));
        // 截断后总长受控
        assert!(out.chars().count() < long.chars().count());
    }

    /// **保尾是本批的目的**(HARNESS3-4):构建/测试输出的失败摘要在尾部,
    /// 只保头就等于「跑完了但看不见为什么失败」。
    #[test]
    fn truncate_keeps_both_head_and_tail() {
        // 头尾各放可识别标记,中间是不可区分的大量填充
        let mut s = String::from("HEAD-MARKER\n");
        s.push_str(&"x".repeat(MAX_OUTPUT_CHARS * 2));
        s.push_str("\nTAIL-MARKER: 3 tests failed");
        let out = truncate_output(&s);
        assert!(out.contains("HEAD-MARKER"), "头部被丢: {out}");
        assert!(
            out.contains("TAIL-MARKER: 3 tests failed"),
            "尾部被丢(旧实现只保头,失败摘要整段消失): {out}"
        );
        assert!(out.contains("已截断"), "必须留可见的省略说明");
        // 总量受控:头 + 尾 + 一行说明,不得接近原文的两倍预算
        assert!(
            out.chars().count() <= MAX_OUTPUT_CHARS + 200,
            "截断后仍应受预算约束: {}",
            out.chars().count()
        );
    }

    /// **两层预算的关系**(实测才定下来):内层按字符、外层 `tools/registry` 按 64KB 字节。
    /// CJK 下 32768 字符可达 ~98KB,外层会再切一次——所以外层也必须保尾,
    /// 否则内层辛苦留下的尾部又被外层削掉。这里锁住内层的最坏字节量,供外层预算对照。
    #[test]
    fn truncate_budget_under_multibyte_worst_case() {
        let long = "字".repeat(MAX_OUTPUT_CHARS * 2);
        let out = truncate_output(&long);
        assert!(out.chars().count() <= MAX_OUTPUT_CHARS + 200);
        // 每个汉字 3 字节:内层结果的最坏字节量 > 外层 64KB,故外层保尾是**必需**的
        assert!(
            out.len() > 64 * 1024,
            "本用例的前提(内层最坏字节量超外层预算)不成立: {}",
            out.len()
        );
    }

    #[test]
    fn tier_roundtrip() {
        for (s, t) in [
            ("root", ShellTier::Root),
            ("shizuku", ShellTier::Shizuku),
            ("sandbox", ShellTier::Sandbox),
            ("disabled", ShellTier::Disabled),
            ("garbage", ShellTier::Disabled),
        ] {
            assert_eq!(ShellTier::from_str_lossy(s), t, "解析 {s}");
            // 未知值回退 disabled,故仅对已知值断言原文往返
            if s != "garbage" {
                assert_eq!(t.as_str(), s, "往返 {s}");
            }
        }
    }
}
