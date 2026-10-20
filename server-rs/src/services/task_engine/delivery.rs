// 文件交付终态门槛(TMR-3,2026-10-19)。
//
// 背景:取消任务模式轮次上限后,模型仍可能「读了 32/80 轮、代码全在正文、工作区一个
// 文件没写」,而执行器照常返回 Done——轮次墙不是产物保证。审计文档 R-3 的原始建议是
// 加「必需文件」字段;本批按用户裁定改为**不加字段**的弱保证:绑定工作区、且本轮启用
// 文件写工具的任务,终态至少要有一次非回滚文件变更。
//
// 判据来源是 `task_file_changes` 台账(执行前记基线、执行后取差值),因此 fs_* 与
// bash 的落盘都算;未记账的写入(MCP/自定义工具)不在保证范围内(遗留 TMR-L2/L3)。
// 纯函数独立成模块,便于把「Done→Partial / 已有 error 追加」的六种组合逐条钉住。
use crate::models::types::TaskStatus;
use crate::services::task_core::TaskTerminal;

/// 未检测到文件变更时写入 `tasks.error` 的用户可见说明。
pub(crate) const FILE_DELIVERY_MISSING_NOTE: &str = "未检测到文件变更:该任务绑定了工作区并启用了文件工具,但本次执行没有产生任何文件产物(可能只把结果留在正文);请检查工作区或追加指令重试。";

/// 对终态应用文件交付门槛。
///
/// 返回 `(terminal, downgraded)`:
/// - `downgraded = true` 仅表示「本次把 Done/Partial 改写成带缺失说明的 Partial」,
///   调用方据此补一条任务 Status 事件(可解释性);
/// - `required = false`(未绑定工作区/策略不给写工具/模式不适用)或 `changed = true`
///   时原样返回,调用方零行为变化;
/// - 只有 `TaskTerminal::Complete` 会被改写:Followup(追加指令可以只回答文字)、
///   AwaitApproval(尚未执行)、Failed/Noop(本来就不是 Done)一律不碰。
pub(crate) fn apply_file_delivery_gate(
    terminal: TaskTerminal,
    required: bool,
    changed: bool,
) -> (TaskTerminal, bool) {
    if !required || changed {
        return (terminal, false);
    }
    match terminal {
        TaskTerminal::Complete {
            result,
            status,
            error,
        } => match status {
            TaskStatus::Done | TaskStatus::Partial => {
                let note = merge_note(error.as_deref());
                (
                    TaskTerminal::Complete {
                        result,
                        status: TaskStatus::Partial,
                        error: Some(note),
                    },
                    true,
                )
            }
            // Error/Ended 等非成功终态不因「没产物」再改一次;它们本就有自己的原因。
            _ => (
                TaskTerminal::Complete {
                    result,
                    status,
                    error,
                },
                false,
            ),
        },
        other => (other, false),
    }
}

/// 保留既有原因,再追加「未检测到文件变更」——不覆盖队首的具体失败/审计原因。
fn merge_note(existing: Option<&str>) -> String {
    match existing.map(str::trim).filter(|s| !s.is_empty()) {
        Some(e) => format!("{e}\n{FILE_DELIVERY_MISSING_NOTE}"),
        None => FILE_DELIVERY_MISSING_NOTE.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(status: TaskStatus, error: Option<&str>) -> TaskTerminal {
        TaskTerminal::Complete {
            result: "产出正文".into(),
            status,
            error: error.map(str::to_string),
        }
    }

    fn status_of(terminal: &TaskTerminal) -> TaskStatus {
        match terminal {
            TaskTerminal::Complete { status, .. } => *status,
            _ => panic!("本用例只构造 Complete"),
        }
    }

    fn error_of(terminal: &TaskTerminal) -> Option<&str> {
        match terminal {
            TaskTerminal::Complete { error, .. } => error.as_deref(),
            _ => panic!("本用例只构造 Complete"),
        }
    }

    /// Done + 无变更 → Partial + 缺失说明;这是本门槛的唯一「降级」形态。
    #[test]
    fn done_without_changes_becomes_partial() {
        let (out, downgraded) =
            apply_file_delivery_gate(complete(TaskStatus::Done, None), true, false);
        assert!(downgraded);
        assert_eq!(status_of(&out), TaskStatus::Partial);
        assert!(error_of(&out).unwrap().contains("未检测到文件变更"));
    }

    /// 有变更 / 不要求校验 → 逐字节原样(执行器返回值不被改写)。
    #[test]
    fn changed_or_not_required_is_untouched() {
        let with_change = apply_file_delivery_gate(complete(TaskStatus::Done, None), true, true);
        assert!(!with_change.1);
        assert_eq!(status_of(&with_change.0), TaskStatus::Done);

        let not_required = apply_file_delivery_gate(complete(TaskStatus::Done, None), false, false);
        assert!(!not_required.1);
        assert_eq!(status_of(&not_required.0), TaskStatus::Done);
    }

    /// 已是 Partial(如审计不过)时保留原原因,把缺失说明追加在后面,不覆盖。
    #[test]
    fn partial_keeps_existing_reason_and_appends_note() {
        let (out, downgraded) = apply_file_delivery_gate(
            complete(TaskStatus::Partial, Some("审计/终审未通过")),
            true,
            false,
        );
        assert!(downgraded);
        let error = error_of(&out).unwrap();
        assert!(error.starts_with("审计/终审未通过"), "{error}");
        assert!(error.contains("未检测到文件变更"), "{error}");
    }

    /// Done 但带原因(理论上的矛盾形态)也要保留原因并降 Partial。
    #[test]
    fn done_with_reason_keeps_reason() {
        let (out, downgraded) =
            apply_file_delivery_gate(complete(TaskStatus::Done, Some("旧原因")), true, false);
        assert!(downgraded);
        assert_eq!(status_of(&out), TaskStatus::Partial);
        let error = error_of(&out).unwrap();
        assert!(error.contains("旧原因") && error.contains("未检测到文件变更"));
    }

    /// 非 Complete 终态一律不碰:Followup 可以只回答文字,Failed/Noop 本就不是成功。
    #[test]
    fn non_complete_terminals_are_untouched() {
        let followup = TaskTerminal::Followup {
            msg_text: "只回答文字".into(),
            new_result: "结果".into(),
            status: TaskStatus::Done,
        };
        let (out, downgraded) = apply_file_delivery_gate(followup, true, false);
        assert!(!downgraded);
        assert!(matches!(out, TaskTerminal::Followup { .. }));

        let failed = TaskTerminal::Failed {
            error: Some("连接失败".into()),
        };
        let (out, downgraded) = apply_file_delivery_gate(failed, true, false);
        assert!(!downgraded);
        assert!(matches!(out, TaskTerminal::Failed { .. }));

        let noop = TaskTerminal::Noop;
        let (out, downgraded) = apply_file_delivery_gate(noop, true, false);
        assert!(!downgraded);
        assert!(matches!(out, TaskTerminal::Noop));
    }
}
