// 任务附件在**目标文本**里的注入段(ATTACH-1,2026-11-03)。
//
// 为什么必须注入:附件落在任务的 fs 作用域里,但模型并不知道有这些文件——
// 不告诉它,「把附件里的表格整理一下」这类目标就只能靠猜文件名。这一段是附件
// 从「磁盘上的文件」变成「模型看得见的输入」的唯一通道。
//
// 三个设计约束:
//   1. **纯函数**:输入是(已有目标文本, 附件清单, 视觉是否可用),输出是拼好的文本。
//      不读库、不碰磁盘、不拿 settings——这样可以逐例单测,且六模式共用同一份实现。
//   2. **只描述、不内联**:不把文件正文塞进目标(附件可达 25MB,内联会炸上下文)。
//      给出相对路径 + 大小 + 建议的读法,让模型自己按需读取。
//   3. **不谎报能力**:视觉未开启时明说「无法查看画面」,而不是写「用 view_image 查看」
//      再让工具调用失败。
use crate::services::task_core::backend::TaskAttachmentBrief;

/// 附件段的标题。写成 `## 附件` 而不是列表前缀,是为了与目标正文在视觉上分层
/// (模型与用户都能一眼看出「下面这段是清单,不是任务要求」)。
const SECTION_TITLE: &str = "## 附件";

/// 单个附件在清单里占一行时的展示上限:太长的名字会把整段挤成噪音。
const MAX_NAME_CHARS: usize = 80;

/// 把附件清单拼进目标文本。
///
/// - `goal` 为空或全空白时,只返回附件段(不留空行在开头);
/// - `attachments` 为空时**原样返回 goal**(逐字节不变——无附件的任务行为零变化);
/// - `vision_available` 决定图像类附件写「用 view_image 查看」还是「当前无法查看画面」。
pub(crate) fn with_attachments(
    goal: &str,
    attachments: &[TaskAttachmentBrief],
    vision_available: bool,
) -> String {
    if attachments.is_empty() {
        return goal.to_string();
    }
    let body = attachments
        .iter()
        .map(|a| format_line(a, vision_available))
        .collect::<Vec<_>>()
        .join("\n");
    let section = format!(
        "{SECTION_TITLE}\n\
         以下文件已随任务提供,路径相对本次任务的工作目录:\n\
         {body}\n\
         需要附件内容时先读取,不要凭文件名猜测。"
    );
    let trimmed = goal.trim_end();
    if trimmed.is_empty() {
        section
    } else {
        format!("{trimmed}\n\n{section}")
    }
}

/// 一个附件的清单行:`- name → path(sized, 读法)`。
fn format_line(a: &TaskAttachmentBrief, vision_available: bool) -> String {
    let name = truncate_name(&a.name);
    let path = &a.rel_path;
    let size = human_size(a.size);
    let hint = read_hint(a, vision_available);
    format!("- `{name}` → `{path}`({size},{hint})")
}

/// 读法建议。判据只有一条:**能不能被模型看到内容**,而不是文件"像不像"图像。
///
/// - 图像 + 视觉开启 → `view_image`;
/// - 图像 + 视觉关闭 → 如实说无法查看(工具调用会失败,提示词不得预告成功);
/// - 其余一律 `fs_read`(文本、代码、PDF 之类都走这条;PDF 的解析质量取决于模型,
///   不在此处承诺)。
fn read_hint(a: &TaskAttachmentBrief, vision_available: bool) -> &'static str {
    match (is_image_mime(a.mime.as_deref()), vision_available) {
        (true, true) => "图像,用 view_image 查看",
        (true, false) => "图像,当前未启用视觉能力,无法查看画面",
        (false, _) => "用 fs_read 读取",
    }
}

/// 图像判定:只看 MIME 前缀。**刻意在本模块内自带一份**,不在 `services` 里反向
/// 依赖 `api/tasks/attachments.rs` 的同名函数:后者是 HTTP 层的私有 helper,
/// 而 L2(services)不得依赖 api 层(`tools/check-arch.mjs` 规则 J)。
fn is_image_mime(mime: Option<&str>) -> bool {
    mime.is_some_and(|m| m.starts_with("image/"))
}

/// 文件名截断:按 **char** 截(中文安全),超出补省略号。
fn truncate_name(name: &str) -> String {
    if name.chars().count() <= MAX_NAME_CHARS {
        return name.to_string();
    }
    let head: String = name.chars().take(MAX_NAME_CHARS).collect();
    format!("{head}…")
}

/// 人类可读大小。用 1024 进制并保留一位小数,与前端附件清单同口径。
fn human_size(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes.max(0) as f64;
    if b < KB {
        format!("{} B", bytes.max(0))
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, rel: &str, size: i64, mime: Option<&str>) -> TaskAttachmentBrief {
        TaskAttachmentBrief {
            name: name.into(),
            rel_path: rel.into(),
            size,
            mime: mime.map(str::to_string),
        }
    }

    /// 无附件:逐字节返回原目标(零行为变化,这是硬要求)。
    #[test]
    fn no_attachments_returns_goal_verbatim() {
        let goal = "整理接口文档\n第二行";
        assert_eq!(with_attachments(goal, &[], true), goal);
        assert_eq!(with_attachments("", &[], true), "");
    }

    /// 有附件:原目标 + 空行 + 附件段;段内有标题、路径、大小与读法。
    #[test]
    fn attachments_append_section_after_goal() {
        let list = vec![att(
            "spec.txt",
            ".kedai/attachments/1-spec.txt",
            1229,
            Some("text/plain"),
        )];
        let out = with_attachments("整理接口文档", &list, true);
        assert!(out.starts_with("整理接口文档\n\n## 附件\n"));
        assert!(out.contains("`.kedai/attachments/1-spec.txt`"));
        assert!(out.contains("1.2 KB"));
        assert!(out.contains("fs_read"));
        assert!(out.ends_with("需要附件内容时先读取,不要凭文件名猜测。"));
    }

    /// 视觉开关决定图像附件的读法提示——关闭时**不得**承诺能看图。
    #[test]
    fn image_hint_depends_on_vision_flag() {
        let list = vec![att(
            "shot.png",
            ".kedai/attachments/1-shot.png",
            240_000,
            Some("image/png"),
        )];
        let on = with_attachments("看图", &list, true);
        assert!(on.contains("view_image"));
        let off = with_attachments("看图", &list, false);
        assert!(off.contains("无法查看画面"));
        assert!(!off.contains("view_image"), "视觉关闭时不得预告可用");
    }

    /// 空目标只出附件段,不留前导空行。
    #[test]
    fn empty_goal_does_not_leave_leading_blank_lines() {
        let list = vec![att("a.txt", "attachments/1-a.txt", 5, Some("text/plain"))];
        let out = with_attachments("   ", &list, true);
        assert!(out.starts_with("## 附件"), "实际: {out}");
    }

    /// 超长文件名按字符截断(中文不会被切坏)。
    #[test]
    fn long_names_are_truncated_by_chars() {
        let long = "很长的名字".repeat(30);
        let list = vec![att(&long, "attachments/1-x.txt", 10, None)];
        let out = with_attachments("g", &list, true);
        assert!(out.contains('…'));
        // 截断后仍是合法 UTF-8 且不超过上限 + 省略号
        let line = out.lines().find(|l| l.starts_with("- `")).unwrap();
        let inside = line.trim_start_matches("- `").split('`').next().unwrap();
        assert_eq!(inside.chars().count(), MAX_NAME_CHARS + 1);
    }

    /// 大小分档:字节 / KB / MB。
    #[test]
    fn human_size_ranges() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(240_000), "234.4 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
        assert_eq!(human_size(-5), "0 B");
    }

    /// 文本类附件无论视觉开关都写 fs_read。
    #[test]
    fn text_hint_is_stable_regardless_of_vision() {
        let list = vec![att("a.md", "attachments/1-a.md", 10, Some("text/markdown"))];
        assert!(with_attachments("g", &list, true).contains("fs_read"));
        assert!(with_attachments("g", &list, false).contains("fs_read"));
    }
}
