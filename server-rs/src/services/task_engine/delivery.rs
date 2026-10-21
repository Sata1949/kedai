// 文件交付终态门槛(TMR-3,2026-10-19)与交付摘要(TMR-6,2026-10-20)。
//
// 背景:取消任务模式轮次上限后,模型仍可能「读了 32/80 轮、代码全在正文、工作区一个
// 文件没写」,而执行器照常返回 Done——轮次墙不是产物保证。TMR-3 落地的弱保证是:
// 绑定工作区、且本轮启用文件写工具的任务,终态至少要有一次非回滚文件变更。
//
// TMR-6 在弱保证之上补两件事:
//   1. **必需文件**:team 规划器为每个子目标声明 `files`(TMR-4 写入 TaskStep),
//      终态检查这些文件是否真实存在;缺失即 Partial 并把缺失清单写进 tasks.error。
//   2. **交付摘要**:任务详情返回「必需/变更/缺失」,前端成果卡据此展示。摘要是
//      读时合成(不新增表/列),变更路径来自既有 task_file_changes 台账。
//
// 判据来源是 `task_file_changes` 台账(执行前记基线、执行后取差值),因此 fs_* 与
// bash 的落盘都算;未记账的写入(MCP/自定义工具)不在保证范围内(遗留 TMR-L2/L3)。
// 纯函数独立成模块,便于把门槛组合与摘要形状逐条钉住。
use crate::models::ownership::normalize_workspace_rel;
use crate::models::types::{TaskStatus, TaskStep};
use crate::services::task_core::TaskTerminal;
use serde::Serialize;
use std::path::Path;

/// 未检测到文件变更时写入 `tasks.error` 的用户可见说明(TMR-3 口径)。
pub(crate) const FILE_DELIVERY_MISSING_NOTE: &str = "未检测到文件变更:该任务绑定了工作区并启用了文件工具,但本次执行没有产生任何文件产物(可能只把结果留在正文);请检查工作区或追加指令重试。";

/// 必需文件缺失说明的前缀(TMR-6)。
pub(crate) const FILE_DELIVERY_MISSING_FILES_PREFIX: &str = "缺少必需文件:";

/// 交付摘要里「变更路径」的返回上限;总数仍由 `changed_count` 给出。
/// 详情端点在运行中会被 SSE 频繁触发,不能把整张台账塞进响应。
pub(crate) const DELIVERY_CHANGED_LIMIT: usize = 200;

/// 单个必需文件的展示形态。
#[derive(Debug, Clone, Serialize)]
pub struct TaskDeliveryFile {
    /// 工作区相对路径(已归一化)
    pub path: String,
    /// 规划器里的步骤名(含「【主Agent-N】」前缀)
    pub owner: String,
    /// 磁盘上当前是否存在(is_file);工作区不可访问时为 false
    pub exists: bool,
}

/// 任务详情顶层 `delivery`(TMR-6)。
#[derive(Debug, Clone, Serialize)]
pub struct TaskDeliverySummary {
    /// 是否声明了必需文件(false = 只按「本次有没有文件变更」弱校验)
    pub checked: bool,
    /// 工作区当前是否可访问(false 时缺失判定不可信,前端必须显式提示)
    pub workspace_available: bool,
    /// 声明的必需文件(按 plan 顺序去重)
    pub required: Vec<TaskDeliveryFile>,
    /// 声明了但磁盘上不存在(或不是文件)的路径
    pub missing: Vec<String>,
    /// 台账里出现过的非回滚变更路径(去重、最近发生优先、上限 [DELIVERY_CHANGED_LIMIT])
    pub changed: Vec<String>,
    /// 去重后的变更文件总数(不受 changed 截断影响)
    pub changed_count: usize,
}

/// 从 plan 提取必需文件:(规范化相对路径, 步骤名) 去重,首声明胜出。
///
/// 非法路径(存量 plan 被手工改出 `..`/绝对路径)跳过并 warn——摘要与门槛都不该
/// 把越界路径交给文件系统。
pub(crate) fn required_from_plan(plan: &[TaskStep]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for step in plan {
        for raw in &step.files {
            let Some(path) = normalize_workspace_rel(raw) else {
                tracing::warn!(path = raw.as_str(), "交付摘要:非法声明路径已跳过");
                continue;
            };
            if out.iter().any(|(p, _)| p == &path) {
                continue;
            }
            let owner = if step.name.trim().is_empty() {
                "未命名步骤".to_string()
            } else {
                step.name.trim().to_string()
            };
            out.push((path, owner));
        }
    }
    out
}

/// 必需文件中缺失的路径。
///
/// 工作区不可访问时**不产生缺失结论**(返回空):此时无法区分「文件缺失」与
/// 「盘不在/目录被移走」,制造假缺失会让任务被误判 partial;摘要里的
/// `workspace_available=false` 负责如实告知前端。
pub(crate) fn missing_files(workspace: &Path, required: &[(String, String)]) -> Vec<String> {
    if !workspace.is_dir() {
        return Vec::new();
    }
    required
        .iter()
        .filter(|(path, _)| !workspace.join(path).is_file())
        .map(|(path, _)| path.clone())
        .collect()
}

/// 读时合成交付摘要(详情接口调用;纯函数,不碰 DB)。
pub(crate) fn summarize(
    plan: &[TaskStep],
    workspace: &Path,
    changed: Vec<String>,
    changed_count: usize,
) -> TaskDeliverySummary {
    let required = required_from_plan(plan);
    let workspace_available = workspace.is_dir();
    let missing = missing_files(workspace, &required);
    let files = required
        .iter()
        .map(|(path, owner)| TaskDeliveryFile {
            path: path.clone(),
            owner: owner.clone(),
            exists: workspace_available && workspace.join(path).is_file(),
        })
        .collect();
    TaskDeliverySummary {
        checked: !required.is_empty(),
        workspace_available,
        required: files,
        missing,
        changed,
        changed_count,
    }
}

/// 对终态应用文件交付门槛。
///
/// 返回 `(terminal, downgraded)`:
/// - `downgraded = true` 仅表示「本次把 Done/Partial 改写成带缺失说明的 Partial」,
///   调用方据此补一条任务 Status 事件(可解释性);
/// - `enabled = false`(未绑定工作区/策略不给写工具/模式不适用)时原样返回;
/// - `enabled = true` 时:
///   - 声明了必需文件且任一缺失 → Partial +「缺少必需文件:…」;
///   - 声明了必需文件且齐全,但本次零变更 → Partial +「未检测到文件变更」;
///   - 未声明必需文件,但本次零变更 → Partial +「未检测到文件变更」(TMR-3 弱门槛);
///   - 其余原样(有变更 / 声明齐全且有变更)。
/// - 只有 `TaskTerminal::Complete` 会被改写:Followup(追加指令可以只回答文字)、
///   AwaitApproval(尚未执行)、Failed/Noop(本来就不是 Done)一律不碰。
pub(crate) fn apply_file_delivery_gate(
    terminal: TaskTerminal,
    enabled: bool,
    required: &[(String, String)],
    missing: &[String],
    changed: bool,
) -> (TaskTerminal, bool) {
    if !enabled {
        return (terminal, false);
    }
    let unmet = (!required.is_empty() && !missing.is_empty()) || !changed;
    if !unmet {
        return (terminal, false);
    }
    match terminal {
        TaskTerminal::Complete {
            result,
            status,
            error,
        } => match status {
            TaskStatus::Done | TaskStatus::Partial => {
                let note = if missing.is_empty() {
                    FILE_DELIVERY_MISSING_NOTE.to_string()
                } else {
                    missing_files_note(missing)
                };
                (
                    TaskTerminal::Complete {
                        result,
                        status: TaskStatus::Partial,
                        error: Some(merge_note(error.as_deref(), &note)),
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

/// 门槛降级时发给前端的 Status 事件文案(与写进 tasks.error 的说明同源)。
pub(crate) fn delivery_event_detail(missing: &[String]) -> String {
    if missing.is_empty() {
        "文件产物校验未通过:本次执行未检测到工作区文件变更,任务记为 partial".to_string()
    } else {
        format!("文件产物校验未通过:{}", missing_files_note(missing))
    }
}

/// 缺失清单的中文说明(最多点名 10 个,超出写总数)。
fn missing_files_note(missing: &[String]) -> String {
    let head: Vec<&str> = missing.iter().take(10).map(|s| s.as_str()).collect();
    let more = if missing.len() > head.len() {
        format!(" 等 {} 个", missing.len())
    } else {
        String::new()
    };
    format!(
        "{FILE_DELIVERY_MISSING_FILES_PREFIX}{}{more};请检查工作区或追加指令重试。",
        head.join("、")
    )
}

/// 保留既有原因,再追加交付说明——不覆盖队首的具体失败/审计原因。
fn merge_note(existing: Option<&str>, note: &str) -> String {
    match existing.map(str::trim).filter(|s| !s.is_empty()) {
        Some(e) => format!("{e}\n{note}"),
        None => note.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

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

    fn required(items: &[&str]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|p| (p.to_string(), "【主Agent-1】写分片".to_string()))
            .collect()
    }

    /// 门槛关闭(未绑定工作区/策略不给写工具)→ 逐字节原样。
    #[test]
    fn gate_disabled_is_untouched() {
        let (out, downgraded) =
            apply_file_delivery_gate(complete(TaskStatus::Done, None), false, &[], &[], false);
        assert!(!downgraded);
        assert_eq!(status_of(&out), TaskStatus::Done);
    }

    /// TMR-3 弱门槛:未声明必需文件 + 本次零变更 → Partial +「未检测到文件变更」。
    #[test]
    fn weak_gate_without_declarations_downgrades_zero_change() {
        let (out, downgraded) =
            apply_file_delivery_gate(complete(TaskStatus::Done, None), true, &[], &[], false);
        assert!(downgraded);
        assert_eq!(status_of(&out), TaskStatus::Partial);
        assert!(error_of(&out).unwrap().contains("未检测到文件变更"));
    }

    /// 声明齐全 + 有变更 → 原样(不因门槛误伤)。
    #[test]
    fn declared_files_present_with_change_passes() {
        let req = required(&["index.html", "parts/ui.js"]);
        let (out, downgraded) =
            apply_file_delivery_gate(complete(TaskStatus::Done, None), true, &req, &[], true);
        assert!(!downgraded);
        assert_eq!(status_of(&out), TaskStatus::Done);
    }

    /// 声明齐全但本次零变更 → Partial(TMR-3 口径:本次没有产出)。
    #[test]
    fn declared_files_present_without_change_downgrades() {
        let req = required(&["index.html"]);
        let (out, downgraded) =
            apply_file_delivery_gate(complete(TaskStatus::Done, None), true, &req, &[], false);
        assert!(downgraded);
        assert_eq!(status_of(&out), TaskStatus::Partial);
        assert!(error_of(&out).unwrap().contains("未检测到文件变更"));
    }

    /// 必需文件缺失 → Partial,原因点名缺失文件(即使本次改过别的文件)。
    #[test]
    fn missing_required_files_downgrades_even_with_changes() {
        let req = required(&["index.html", "parts/ui.js"]);
        let missing = vec!["parts/ui.js".to_string()];
        let (out, downgraded) =
            apply_file_delivery_gate(complete(TaskStatus::Done, None), true, &req, &missing, true);
        assert!(downgraded);
        assert_eq!(status_of(&out), TaskStatus::Partial);
        let error = error_of(&out).unwrap();
        assert!(error.contains("缺少必需文件"), "{error}");
        assert!(error.contains("parts/ui.js"), "{error}");
    }

    /// 已是 Partial(如审计不过)时保留原原因,把缺失说明追加在后面,不覆盖。
    #[test]
    fn missing_note_appends_existing_reason() {
        let req = required(&["index.html"]);
        let missing = vec!["index.html".to_string()];
        let (out, downgraded) = apply_file_delivery_gate(
            complete(TaskStatus::Partial, Some("审计/终审未通过")),
            true,
            &req,
            &missing,
            false,
        );
        assert!(downgraded);
        let error = error_of(&out).unwrap();
        assert!(error.starts_with("审计/终审未通过"), "{error}");
        assert!(error.contains("缺少必需文件"), "{error}");
    }

    /// 非 Complete 终态一律不碰:Followup 可以只回答文字,Failed/Noop 本就不是成功。
    #[test]
    fn non_complete_terminals_are_untouched() {
        let req = required(&["index.html"]);
        let missing = vec!["index.html".to_string()];
        let followup = TaskTerminal::Followup {
            msg_text: "只回答文字".into(),
            new_result: "结果".into(),
            status: TaskStatus::Done,
        };
        let (out, downgraded) = apply_file_delivery_gate(followup, true, &req, &missing, false);
        assert!(!downgraded);
        assert!(matches!(out, TaskTerminal::Followup { .. }));

        let failed = TaskTerminal::Failed {
            error: Some("连接失败".into()),
        };
        let (out, downgraded) = apply_file_delivery_gate(failed, true, &req, &missing, false);
        assert!(!downgraded);
        assert!(matches!(out, TaskTerminal::Failed { .. }));

        let noop = TaskTerminal::Noop;
        let (out, downgraded) = apply_file_delivery_gate(noop, true, &req, &missing, false);
        assert!(!downgraded);
        assert!(matches!(out, TaskTerminal::Noop));
    }

    /// 摘要:必需文件存在性、缺失清单、变更数与非法声明跳过。
    #[test]
    fn summarize_reports_required_changed_and_missing() {
        let dir = TempDataDir::new("delivery-summary");
        let ws = dir.join("ws");
        std::fs::create_dir_all(ws.join("parts")).expect("创建摘要测试工作区失败");
        std::fs::write(ws.join("index.html"), "<html></html>").expect("写摘要测试文件失败");

        let plan = vec![
            TaskStep {
                name: "【主Agent-1】写入口".into(),
                files: vec!["index.html".into(), "../escape.js".into()],
                ..TaskStep::default()
            },
            TaskStep {
                name: "【主Agent-2】写分片".into(),
                files: vec!["index.html".into(), "parts/ui.js".into()],
                ..TaskStep::default()
            },
        ];
        let summary = summarize(&plan, &ws, vec!["index.html".into()], 2);
        assert!(summary.checked, "有声明时 checked 应为 true");
        assert!(summary.workspace_available);
        assert_eq!(
            summary.required.len(),
            2,
            "必需文件应去重: {:?}",
            summary.required
        );
        assert_eq!(summary.required[0].path, "index.html");
        assert!(summary.required[0].exists);
        assert_eq!(summary.required[1].path, "parts/ui.js");
        assert!(!summary.required[1].exists);
        assert_eq!(summary.missing, vec!["parts/ui.js"]);
        assert_eq!(summary.changed, vec!["index.html"]);
        assert_eq!(summary.changed_count, 2);
    }

    /// 工作区不可访问 → 不制造假缺失(摘要只标记 workspace_available=false)。
    #[test]
    fn summarize_does_not_fake_missing_when_workspace_unavailable() {
        let dir = TempDataDir::new("delivery-unavailable");
        let ws = dir.join("missing-ws");
        let plan = vec![TaskStep {
            name: "【主Agent-1】写入口".into(),
            files: vec!["index.html".into()],
            ..TaskStep::default()
        }];
        let summary = summarize(&plan, &ws, Vec::new(), 0);
        assert!(!summary.workspace_available);
        assert!(summary.missing.is_empty(), "不可访问时不得判缺失");
        assert!(!summary.required[0].exists);
    }
}
