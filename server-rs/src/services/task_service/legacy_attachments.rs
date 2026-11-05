// legacy 三段纯文本生成的附件注入(ATTACH-1-L1，2026-11-03)。
//
// **为什么单独一个文件**:`executor.rs` 有 1472 行的只降不升基线，而这次要加的逻辑
// （读清单 → 转 brief → 拼段）有 40 余行。把注入口搬到独立模块，`executor.rs` 只留
// 一行调用，基线不动——比放宽基线干净（放宽等于关掉那道护栏）。
//
// **这个缺口是怎么发现的**:2026-11-03 真实模型实测。规划器能看到附件段（它拿的是
// `ctx.goal`），但模型在执行阶段回答「我没有收到任何附件」。根因是 legacy 的两个
// 原语各读各的字段：
//   - `generate_step_with` 的 user 消息只用 `step.goal`（规划器写的，不含清单）；
//   - `summarize_task_with` 的 user 消息用 `task.title`（原始标题）。
// 两者都拿不到 `task_engine::run_inner` 合成的那份「目标 + 附件段」。
use super::TaskService;
use crate::models::types::TaskRecord;
use crate::services::task_core::backend::TaskAttachmentBrief;

impl TaskService {
    /// legacy 消息尾部的附件段。
    ///
    /// 返回带前导两个换行的段落（调用方直接拼在目标/步骤文本后面）；**无附件时返回
    /// 空串**——存量任务逐字节不变。
    ///
    /// `tools_available` 固定为 `false`：本模块只服务 legacy 的三段纯文本生成，
    /// 该模式 `StepCapability::NoTools`，模型确实拿不到 `fs_read`。注入文案必须如实
    /// 说「本模式无读取工具」，否则模型会照做不了、只能回报「读取工具返回空白」，
    /// 等于提示词在说谎（同一轮实测抓到的 ATTACH-1-L2）。
    pub(super) fn attachment_suffix(&self, task: &TaskRecord) -> String {
        let list = match self.list_attachments(&task.id) {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => return String::new(),
            Err(e) => {
                tracing::warn!(
                    task_id = task.id.as_str(),
                    "读取附件清单失败,消息不含附件段: {e}"
                );
                return String::new();
            }
        };
        // 注入函数吃的是 TaskAttachmentBrief（窄接口层不认识 DB 行），这里转一次
        let briefs: Vec<TaskAttachmentBrief> = list
            .into_iter()
            .map(|a| TaskAttachmentBrief {
                name: a.name,
                rel_path: a.rel_path,
                size: a.size,
                mime: a.mime,
            })
            .collect();
        // 用空目标拼接，只取附件段本身（标题/目标位置由各调用方自己给）
        let section = crate::services::task_engine::attachments::with_attachments(
            "",
            &briefs,
            crate::services::settings_service::vision_enabled(&self.task_settings()),
            false,
        );
        if section.is_empty() {
            return String::new();
        }
        format!("\n\n{section}")
    }

    /// 步骤生成的 user 消息：**附件段在前、步骤目标在后**。
    ///
    /// 附件清单是「有什么材料」的背景，先给材料再给要做的事更自然；而且 `step.goal`
    /// 可能很长，把清单放末尾会被 `task_llm_calls` 的 800 字/条摘要截掉。
    ///
    /// 两段之间必须留空行（ATTFIX-8）：`suffix.trim_start()` 只剥前导空白，若直接
    /// 拼 `step_goal`，附件段的末行「…请改用 Solo / 多 Agent / 团队等带工具的模式。」
    /// 会和步骤目标粘成同一行——Markdown 上是一句，模型读起来也是一句。
    pub(super) fn legacy_step_user_message(&self, task: &TaskRecord, step_goal: &str) -> String {
        join_step_user_message(&self.attachment_suffix(task), step_goal)
    }

    /// 汇总的 user 消息：目标 + 附件段 + 各步骤结果。
    ///
    /// 附件段放在**目标之后、结果之前**——各步骤结果可以很长（实测单步产出动辄数千字），
    /// 放末尾既会被摘要截掉，也让模型在长上下文里更难注意到。
    pub(super) fn legacy_summary_user_message(
        &self,
        task: &TaskRecord,
        plan: &[crate::models::types::TaskStep],
    ) -> String {
        let mut user = format!(
            "用户目标:\n{}{}\n\n各步骤结果:\n",
            task.title,
            self.attachment_suffix(task)
        );
        for (i, s) in plan.iter().enumerate() {
            user.push_str(&format!(
                "{}. {}({}):\n{}\n",
                i + 1,
                s.name,
                s.status.as_str(),
                s.result
            ));
        }
        user
    }
}

/// 附件段与步骤目标的拼接(纯函数,便于逐例单测而无须装配整个 TaskService)。
///
/// - 无附件(`suffix` 为空)→ 逐字节返回 `step_goal`,存量任务行为零变化;
/// - 有附件 → 附件段在前、空行分隔、步骤目标在后。
fn join_step_user_message(suffix: &str, step_goal: &str) -> String {
    if suffix.is_empty() {
        return step_goal.to_string();
    }
    format!("{}\n\n{step_goal}", suffix.trim_start())
}

#[cfg(test)]
mod tests {
    use super::join_step_user_message;

    /// 无附件时逐字节不变(存量任务零行为变化)。
    #[test]
    fn no_attachments_returns_step_goal_verbatim() {
        assert_eq!(join_step_user_message("", "写一份说明"), "写一份说明");
    }

    /// **有附件时附件段与步骤目标之间必须有两个换行**(ATTFIX-8)。
    ///
    /// 缺陷形态:`format!("{}{step_goal}", suffix.trim_start())` 把附件段末行与
    /// 步骤目标粘成一句,模型读到的是一行混排文本。
    #[test]
    fn attachment_section_is_separated_by_blank_line() {
        let suffix = "\n\n## 附件\n以下文件已随任务提供\n- `a.txt` → `attachments/1-a.txt`";
        let out = join_step_user_message(suffix, "整理成表格");
        assert!(
            out.ends_with("`attachments/1-a.txt`\n\n整理成表格"),
            "附件段与步骤目标之间必须留空行,实际: {out:?}"
        );
        // 前导空白被剥掉(段首不留空行)
        assert!(out.starts_with("## 附件"), "实际: {out:?}");
        // 目标原样保留在后
        assert!(out.ends_with("整理成表格"));
    }

    /// 多行步骤目标整体保留,不被拼接破坏。
    #[test]
    fn multi_line_step_goal_is_preserved() {
        let goal = "第一步\n第二步\n第三步";
        let out = join_step_user_message("\n\n## 附件\n- `a.txt`", goal);
        assert!(out.ends_with(goal), "步骤目标必须逐字保留: {out:?}");
    }
}
