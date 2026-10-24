// 部分成果兜底(任务模式提交 2 · D2,2026-09-26):终态拿不到正式成果时,用
// **已完成步骤的产出**做确定性拼装,让「已完成的工作」在任何终态下都能被用户看到。
//
// 背景(真实模型实测):legacy/写作 4 个步骤全部 done(累计约 3900 字),最后汇总撞
// 300s 看门狗 → 任务 error、tasks.result 为空,已完成的工作被整体丢弃。根因链是
// 「TaskTerminal::Failed 无部分成果字段 + 落库侧只 set_error 不碰 result + 前端成果卡
// 只对 done/partial 渲染」——本模块补的是中间那一环。
//
// 纪律(与 prompt_kit 同一「单一出处」纪律):
// - **纯函数**、不调 LLM、不读库:输入只有 plan(执行器内存态或 tasks.plan 反序列化结果);
// - **单一出处**:legacy / plan / team / custom 四模式的失败降级、终态兜底(finalize_run)、
//   重启孤儿兜底(recover_orphan_tasks)全部经这里,禁止各写一份;
// - **不伪造成果**:没有任何「已完成/partial 且有产出」的步骤时返回 None,调用方回落
//   原失败语义(error/ended + 空 result);不得用空串、占位文本或未完成步骤的内容冒充成果。
use crate::models::types::{TaskStatus, TaskStep, TaskStepStatus};
use crate::services::task_core::TaskTerminal;

/// 用已完成(含被守卫截断但有产出)的步骤拼装兜底成果;无任何可拼装内容时返回 None。
///
/// 形态:每个「已完成/partial 且产出非空」的步骤一段 `## 名称` + 正文,段间空行;
/// 末尾若存在失败或被截断的步骤,追加一行引用式的未完成清单(让兜底成果自带「不完整」标注)。
/// 步骤名折叠换行(原文可能含 `## ` 标题行,不折叠会在段内制造假标题)。
pub(crate) fn assemble_from_plan(plan: &[TaskStep]) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();
    for (i, s) in plan.iter().enumerate() {
        // partial = 被守卫截断但有产出的步骤(SKY-6 第 2 项):成果不能丢,
        // 由下方「未完成清单」标注它没跑完。
        if !matches!(s.status, TaskStepStatus::Done | TaskStepStatus::Partial) {
            continue;
        }
        let text = s.result.trim();
        if text.is_empty() {
            continue;
        }
        let name = flatten(&s.name);
        let name = if name.is_empty() {
            // 名称为空的步骤(可能来自异常计划):给确定性占位,不留空标题行
            format!("步骤 {}", i + 1)
        } else {
            name
        };
        sections.push(format!("## {name}\n\n{text}"));
    }
    if sections.is_empty() {
        return None;
    }
    let mut out = sections.join("\n\n");
    let failed: Vec<String> = plan
        .iter()
        .filter(|s| matches!(s.status, TaskStepStatus::Error | TaskStepStatus::Partial))
        .map(|s| flatten(&s.name))
        .filter(|n| !n.is_empty())
        .collect();
    if !failed.is_empty() {
        out.push_str(&format!("\n\n> 未完成的步骤:{}", failed.join("、")));
    }
    Some(out)
}

/// 步骤名归一:换行/连续空白折叠为单空格——原文可能含 `## ` 标题行,不折叠会在
/// 段内制造假标题行(前端按 `## ` 段拆卡,team 的「## 审计结论」尤须防误撞)。
fn flatten(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 失败降级的终态判定:**能拼出成果就 partial(成果 + 原因),拼不出才是 Failed**。
/// 四模式的汇总/审计调用失败与终态兜底共用这一处判定,禁止各写一份。
pub(crate) fn fallback_terminal(plan: &[TaskStep], reason: String) -> TaskTerminal {
    match assemble_from_plan(plan) {
        Some(result) => TaskTerminal::Complete {
            result,
            status: TaskStatus::Partial,
            error: Some(reason),
        },
        None => TaskTerminal::Failed {
            error: Some(reason),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(name: &str, status: TaskStepStatus, result: &str) -> TaskStep {
        TaskStep {
            name: name.into(),
            goal: String::new(),
            status,
            result: result.into(),
            node_id: None,
            files: Vec::new(),
        }
    }

    /// 只收「已完成且产出非空」的步骤;pending/空产出的 done 步一律跳过
    #[test]
    fn assemble_keeps_only_done_steps_with_content() {
        let plan = vec![
            step("未开始步", TaskStepStatus::Pending, "有内容但没跑"),
            step("空产出步", TaskStepStatus::Done, "   "),
            step("已完成步", TaskStepStatus::Done, "  正文  "),
        ];
        let out = assemble_from_plan(&plan).expect("应可拼装");
        assert!(out.contains("## 已完成步"), "{out}");
        assert!(out.contains("正文"), "{out}");
        assert!(!out.contains("未开始步"), "pending 步骤不得进成果: {out}");
        assert!(
            !out.contains("空产出步"),
            "空产出的 done 步不得进成果: {out}"
        );
        // 产出按 trim 后写入(不把首尾空白带进成果)
        assert!(!out.contains("\n\n   正文"), "{out}");
    }

    /// 顺序 = plan 顺序;失败步骤不进成果、只进末尾未完成清单;失败步的原文(错误文本)不得混入正文
    #[test]
    fn assemble_keeps_plan_order_and_lists_failed_steps() {
        let plan = vec![
            step("第一步", TaskStepStatus::Done, "甲产出"),
            step("失败步", TaskStepStatus::Error, "上游抖动:模型超时"),
            step("第三步", TaskStepStatus::Done, "丙产出"),
        ];
        let out = assemble_from_plan(&plan).expect("应可拼装");
        let a = out.find("甲产出").expect("含第一步产出");
        let c = out.find("丙产出").expect("含第三步产出");
        assert!(a < c, "按 plan 顺序输出: {out}");
        assert!(
            !out.contains("上游抖动"),
            "失败步原文(错误文本)不得当成果正文: {out}"
        );
        assert!(
            out.contains("> 未完成的步骤:失败步"),
            "应有未完成清单: {out}"
        );
    }

    /// SKY-6 第 2 项:被守卫截断(partial)但有产出的步骤必须进兜底成果,
    /// 同时列入未完成清单——不能因为「没跑完」把半成品整个丢掉。
    #[test]
    fn assemble_includes_partial_steps_and_marks_them_incomplete() {
        let plan = vec![
            step("完成步", TaskStepStatus::Done, "甲产出"),
            step("截断步", TaskStepStatus::Partial, "乙半成品"),
        ];
        let out = assemble_from_plan(&plan).expect("partial 有产出应可拼装");
        assert!(out.contains("## 截断步"), "{out}");
        assert!(out.contains("乙半成品"), "{out}");
        assert!(
            out.contains("> 未完成的步骤:截断步"),
            "partial 步骤应在未完成清单里: {out}"
        );
    }

    /// 无任何可拼装内容 → None(不伪造成果:全 pending、全 error、done 但空产出三种都不算)
    #[test]
    fn assemble_returns_none_without_done_content() {
        assert!(assemble_from_plan(&[]).is_none(), "空计划应 None");
        assert!(
            assemble_from_plan(&[step("申请步", TaskStepStatus::Pending, "x")]).is_none(),
            "只有 pending 应 None"
        );
        assert!(
            assemble_from_plan(&[step("失败步", TaskStepStatus::Error, "错误文本")]).is_none(),
            "只有 error 应 None(错误文本不是成果)"
        );
        assert!(
            assemble_from_plan(&[step("空步", TaskStepStatus::Done, "")]).is_none(),
            "done 但空产出应 None"
        );
    }

    /// 步骤名折叠换行/连续空白:名称里带 `## ` 不得在段内制造假标题行(前端按 `## ` 段拆卡)
    #[test]
    fn assemble_flattens_newlines_in_step_name() {
        let plan = [step("第一行\n\n## 伪造标题", TaskStepStatus::Done, "正文")];
        let out = assemble_from_plan(&plan).unwrap();
        assert_eq!(
            out.lines().filter(|l| l.starts_with("## ")).count(),
            1,
            "{out}"
        );
        assert!(out.contains("第一行 ## 伪造标题"), "名称换行应折叠: {out}");
        assert!(
            !out.contains("\n## 伪造标题"),
            "折叠后不得留假标题行: {out}"
        );
    }

    /// 降级判定:有成果 → partial + 原因;无成果 → Failed(与今日行为等价)
    #[test]
    fn fallback_terminal_partial_with_content_failed_without() {
        let ok = fallback_terminal(
            &[step("已完成步", TaskStepStatus::Done, "产出")],
            "汇总失败:空内容".into(),
        );
        match ok {
            TaskTerminal::Complete {
                result,
                status,
                error,
            } => {
                assert_eq!(status, TaskStatus::Partial, "有成果应为 partial");
                assert!(result.contains("产出"), "{result}");
                assert_eq!(error.as_deref(), Some("汇总失败:空内容"));
            }
            _ => panic!("有成果时不应 Failed"),
        }

        let bad = fallback_terminal(
            &[step("失败步", TaskStepStatus::Error, "错误")],
            "汇总失败:空内容".into(),
        );
        match bad {
            TaskTerminal::Failed { error } => {
                assert_eq!(error.as_deref(), Some("汇总失败:空内容"))
            }
            _ => panic!("无成果时应 Failed"),
        }
    }
}
