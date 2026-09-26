// 产物提交服务(submit 工具的后端):把最终产物写成设备上的文件交付给用户。
//
// 背景(2026-09-17):任务产物此前只留在应用内的 result 文本里。在 Android
// **非 root / 非 Shizuku**(即沙箱档)下,`bash` 够不到 Download 目录、文件写工具
// 又只作用于角色文件区沙箱,模型没有「把成果交付成一个文件」的通道。本服务提供该
// 通道:仅 Android 且当前执行档位为沙箱时可用,产物直接落到用户可见的下载位置。
//
// 可用性口径(与 product 决策一致):
//   - 非 Android:不可用(桌面有保存对话框与 write 工具,不需要本通道);
//   - Android 且档位为 ROOT / Shizuku:不可用——那两档下 `bash` 本就能写任意路径,
//     再给一个 submit 只会让模型在两条等价通道间摇摆(档位判定见 services::exec);
//   - Android 且档位为沙箱:可用。
//
// 与 tools/ 的分工:本模块只做「可用性判定 + 平台分派 + 文件名清洗」等纯逻辑,
// 工具定义与参数校验在 tools/submit.rs(工具层不得直接依赖 native 桥)。

/// 产物文件名长度上限(字符):极端长名在部分文件系统上会失败,且没必要。
pub const FILENAME_MAX_CHARS: usize = 120;

/// 产物内容长度上限(字节):与工具结果截断同量级,防止一次交付把内存与磁盘打满。
/// 超出返回错误而非截断——半截产物比明确失败更糟(用户可能不知道文件不完整)。
pub const CONTENT_MAX_BYTES: usize = 8 * 1024 * 1024;

/// submit 工具当前是否可用(仅 Android 沙箱档)。
///
/// 每次调用现探档位:用户可能在任务执行期间改了授权开关,或 Shizuku 中途生效;
/// 探测结果在 Android 侧带进程内缓存,重复调用无额外开销。
pub fn is_available() -> bool {
    if !cfg!(target_os = "android") {
        return false;
    }
    #[cfg(target_os = "android")]
    {
        crate::services::exec::detect_tier() == crate::services::exec::ShellTier::Sandbox
    }
    #[cfg(not(target_os = "android"))]
    {
        false
    }
}

/// 清洗文件名:去掉路径分隔符与空白首尾,拒绝空名与穿越尝试。
///
/// 只做**保守清洗**而非让调用失败:模型给 `报告/最终.md` 这类带路径的名字时,
/// 拍平为下划线比报错更符合「把产物交出去」的目的。但纯 `..` 或清洗后为空
/// 会被拒——那不是「名字不规整」,而是没有有效名字。
pub fn sanitize_filename(raw: &str) -> Result<String, String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            // 控制字符(含换行):文件名里出现会让部分文件系统失败
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() {
        return Err("文件名无效:请给出不含路径与特殊字符的文件名,例如 报告.md".into());
    }
    let chars = cleaned.chars().count();
    if chars > FILENAME_MAX_CHARS {
        // 截断保留扩展名,避免生成一个没有类型的文件
        let ext = cleaned
            .rsplit_once('.')
            .map(|(_, e)| e.to_string())
            .filter(|e| e.chars().count() <= 10);
        let stem_len =
            FILENAME_MAX_CHARS - ext.as_ref().map(|e| e.chars().count() + 1).unwrap_or(0);
        let stem: String = cleaned.chars().take(stem_len).collect();
        return Ok(match ext {
            Some(e) => format!("{stem}.{e}"),
            None => stem,
        });
    }
    Ok(cleaned)
}

/// 提交产物:写入设备下载位置,返回**实际落盘位置的可读描述**。
///
/// 返回值必须如实回传(而非统一说「已存到下载目录」):API<29 的设备上产物落在
/// 应用外部私有目录,用户需要真实路径才找得到(见 Kotlin `saveToDownloads` 注释)。
pub fn submit(filename: &str, content: &str) -> Result<String, String> {
    // 文件名与内容的校验与平台无关:先做,保证桌面/Android 的失败语义一致
    //(测试就能在桌面覆盖这两条校验,不必等真机)
    let name = sanitize_filename(filename)?;
    if content.trim().is_empty() {
        return Err("产物内容为空:submit 需要提交非空正文".into());
    }
    let bytes = content.len();
    if bytes > CONTENT_MAX_BYTES {
        return Err(format!(
            "产物过大({bytes} 字节,上限 {CONTENT_MAX_BYTES}):请拆分后再提交,或改用分段交付"
        ));
    }
    #[cfg(target_os = "android")]
    {
        crate::services::native_bridge_android::save_to_downloads(&name, content)
    }
    #[cfg(not(target_os = "android"))]
    {
        // 桌面侧:校验已跑完(故上面的用例能断言文件名/内容规则),平台分派在此终止
        let _ = name;
        Err("submit 仅在 Android 沙箱档可用(桌面请用 write 工具或导出保存对话框)".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_on_desktop() {
        // 单测跑在桌面:submit 必须报告不可用(工具层据此不下发)
        assert!(
            !is_available(),
            "桌面不应启用 submit(有保存对话框与 write 工具)"
        );
    }

    #[test]
    fn submit_rejected_on_desktop() {
        let err = submit("a.md", "内容").unwrap_err();
        assert!(err.contains("仅在 Android"), "{err}");
    }

    #[test]
    fn sanitize_flattens_path_separators() {
        assert_eq!(sanitize_filename("报告/最终.md").unwrap(), "报告_最终.md");
        assert_eq!(sanitize_filename("a\\b.txt").unwrap(), "a_b.txt");
        assert_eq!(sanitize_filename("  空白.md  ").unwrap(), "空白.md");
    }

    #[test]
    fn sanitize_rejects_empty_or_dots_only() {
        // 空 / 全空白 / 只有点:拍平与去点之后没有有效名字,必须拒绝
        for bad in ["", "   ", "..", "...", " . "] {
            assert!(
                sanitize_filename(bad).is_err(),
                "{bad:?} 应被拒(没有有效文件名)"
            );
        }
    }

    #[test]
    fn sanitize_flattens_pure_separators_into_underscore() {
        // 纯分隔符是边界情形:清洗后得到 `_`,技术上是个合法文件名,
        // 故不拒绝——但要断言它不会保留分隔符(否则就成了目录穿越面)
        for raw in ["/", "///", "\\", "a/b"] {
            let out = sanitize_filename(raw).unwrap();
            assert!(
                !out.contains('/') && !out.contains('\\'),
                "{raw:?} 清洗后不得含路径分隔符: {out:?}"
            );
        }
    }

    #[test]
    fn sanitize_strips_control_chars_and_windows_reserved() {
        assert_eq!(sanitize_filename("a\nb.md").unwrap(), "a_b.md");
        assert_eq!(sanitize_filename("a:b*c?.md").unwrap(), "a_b_c_.md");
    }

    #[test]
    fn sanitize_truncates_long_name_keeping_extension() {
        let long = format!("{}.md", "字".repeat(FILENAME_MAX_CHARS + 50));
        let out = sanitize_filename(&long).unwrap();
        assert_eq!(
            out.chars().count(),
            FILENAME_MAX_CHARS,
            "应截到上限长度: {}",
            out.chars().count()
        );
        assert!(out.ends_with(".md"), "应保留扩展名: {out}");
    }

    #[test]
    fn submit_rejects_empty_content() {
        let err = submit("a.md", "   \n  ").unwrap_err();
        assert!(err.contains("内容为空"), "{err}");
    }

    #[test]
    fn submit_rejects_oversized_content() {
        let big = "x".repeat(CONTENT_MAX_BYTES + 1);
        let err = submit("a.md", &big).unwrap_err();
        assert!(err.contains("过大"), "{err}");
    }

    #[test]
    fn submit_rejects_bad_filename_before_platform_dispatch() {
        // 文件名非法时应在校验阶段就失败(不进入平台分派),错误文案指向文件名
        let err = submit("..", "内容").unwrap_err();
        assert!(err.contains("文件名无效"), "{err}");
    }
}
