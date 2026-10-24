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
use crate::models::types::{TaskStatus, TaskStep, TaskStepStatus};
use crate::services::task_core::TaskTerminal;
use crate::services::task_engine::wiring;
use serde::Serialize;
use std::path::Path;

/// 未检测到文件变更时写入 `tasks.error` 的用户可见说明(TMR-3 口径)。
pub(crate) const FILE_DELIVERY_MISSING_NOTE: &str = "未检测到文件变更:该任务绑定了工作区并启用了文件工具,但本次执行没有产生任何文件产物(可能只把结果留在正文);请检查工作区或追加指令重试。";

/// 必需文件缺失说明的前缀(TMR-6)。
pub(crate) const FILE_DELIVERY_MISSING_FILES_PREFIX: &str = "缺少必需文件:";

/// 交付内容校验失据的前缀(SKY-6 第 3 项,2026-10-24)。
pub(crate) const FILE_DELIVERY_CONTENT_PREFIX: &str = "交付内容校验未通过:";

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
    /// 声明文件的内容体检不通过项(SKY-6 第 3 项:空文件/仅空白/未替换 `@@` 槽位;
    /// SKY-1 确定性部分:导入名未导出/本地模块或资源不存在/单文件交付仍外链)。
    /// 与终态门槛同源,前端「交付校验」卡直接展示。
    pub content_issues: Vec<String>,
    /// 被空转守卫截断(步骤 partial)的子目标名(SKY-6 第 2 项)。空 = 无截断。
    pub truncated: Vec<String>,
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

/// 单步终态的磁盘事实检查(SKY-8,2026-10-24)。
///
/// 用途:预算收尾(步骤墙钟/token/团队份额)或空转守卫截断时,步骤终态不能只看
/// 「循环以什么方式结束」,还要看该步 `TaskStep.files` 是否真的落盘。判定口径:
/// - `None` = 工作区不可访问,无法判定(调用方保持旧语义,不制造假 partial);
/// - `Some([])` = 全部声明文件存在且内容体检通过;
/// - `Some(issues)` = 非法路径 / 文件缺失 / 不是文件 / 0B / 仅空白 / 未替换 `@@`。
///
/// **只查本步声明文件,不做递归导入接线检查**:接线检查要等任务终态、上游文件全部
/// 落盘后才有意义;在步骤收尾时扫描会把「本步先落盘、下游文件后落盘」误判成失败。
/// 递归接线仍由 [`content_issues`] 在终态门槛统一负责。
pub(crate) fn step_files_unmet(workspace: &Path, files: &[String]) -> Option<Vec<String>> {
    if !workspace.is_dir() {
        return None;
    }
    let mut issues: Vec<String> = Vec::new();
    for raw in files {
        let Some(path) = normalize_workspace_rel(raw) else {
            issues.push(format!("{raw}:非法声明路径"));
            continue;
        };
        let abs = workspace.join(&path);
        let Ok(meta) = std::fs::metadata(&abs) else {
            issues.push(format!("{path}:文件缺失"));
            continue;
        };
        if !meta.is_file() {
            issues.push(format!("{path}:不是文件"));
            continue;
        }
        if meta.len() == 0 {
            issues.push(format!("{path}:文件为空(0B)"));
            continue;
        }
        let Some(lines) = placeholder_line_count(&abs) else {
            continue;
        };
        if lines > 0 {
            issues.push(format!("{path}:含 {lines} 行 @@ 未替换槽位"));
        } else if std::fs::read_to_string(&abs)
            .map(|text| text.trim().is_empty())
            .unwrap_or(false)
        {
            issues.push(format!("{path}:文件仅含空白"));
        }
    }
    Some(issues)
}

/// 文本类交付文件的扩展名白名单(内容扫描只对文本;空文件判定对所有声明文件生效)。
const TEXT_EXTENSIONS: &[&str] = &[
    "html", "htm", "js", "mjs", "cjs", "css", "json", "svg", "md", "txt", "xml",
];

/// 文本类交付文件里含 `@@` 的行数:未替换模板槽位(如 `/*@@MAIN@@*/`)的直接证据。
///
/// SKY-4/SKY-5 先用于审计清单(可见性);SKY-6 第 3 项起交付门槛也消费同一函数
/// ——「占位符没清零」是半成品的确定性证据。仅扫常见文本扩展名且 ≤512KB;
/// 读失败/非文本返回 None。
pub(crate) fn placeholder_line_count(path: &Path) -> Option<usize> {
    const MAX_BYTES: u64 = 512 * 1024;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !TEXT_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    if std::fs::metadata(path).ok()?.len() > MAX_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.lines().filter(|line| line.contains("@@")).count())
}

/// 声明文件的内容体检(SKY-6 第 3 项 + SKY-1 确定性部分):空文件 / 仅空白 /
/// 未替换 `@@` 槽位 / 静态接线问题(导入名未导出、本地模块或资源不存在、
/// 单文件交付仍外链本地文件)。
///
/// 只检查 `TaskStep.files` 声明的交付物及其相对导入(团队规划器声明),不做全工作区
/// 扫描——未声明文件的任务(研究/问答/模板类)不得因正文或模板里的 `@@` 被误伤
/// (判据边界见 `docs/计划.md` SKY-6 与 `docs/遗留.md` §三十一)。
///
/// 工作区不可访问时不产生结论(与缺失判定同口径);不存在的文件交给缺失清单,
/// 不在本函数重复报错。返回项形如 `index.html:含 3 行 @@ 未替换槽位`。
///
/// `goal` 用于判定「单文件交付」要求(见 `wiring::single_file_requested`);任务目标
/// 就是 title(创建接口无 goal 字段),详情摘要与终态门槛传同一份文本。
pub(crate) fn content_issues(
    workspace: &Path,
    required: &[(String, String)],
    goal: &str,
) -> Vec<String> {
    if !workspace.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (path, _) in required {
        let abs = workspace.join(path);
        let Ok(meta) = std::fs::metadata(&abs) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if meta.len() == 0 {
            out.push(format!("{path}:文件为空(0B)"));
            continue;
        }
        let Some(lines) = placeholder_line_count(&abs) else {
            continue;
        };
        if lines > 0 {
            out.push(format!("{path}:含 {lines} 行 @@ 未替换槽位"));
        } else if std::fs::read_to_string(&abs)
            .map(|text| text.trim().is_empty())
            .unwrap_or(false)
        {
            out.push(format!("{path}:文件仅含空白"));
        }
    }
    let declared: Vec<String> = required.iter().map(|(p, _)| p.clone()).collect();
    out.extend(wiring::wiring_issues(workspace, &declared, goal));
    out
}

/// 从 plan 提取未完成(partial)的步骤名。
/// 空 = 没有未完成步骤。名字沿用 plan 里的「【主Agent-N】…」前缀,前端可直接定位。
/// SKY-8 起 partial 可能来自空转守卫截断,也可能来自预算收尾且声明文件缺失。
pub(crate) fn truncated_from_plan(plan: &[TaskStep]) -> Vec<String> {
    plan.iter()
        .filter(|s| s.status == TaskStepStatus::Partial)
        .map(|s| {
            let name = s.name.trim();
            if name.is_empty() {
                "未命名步骤".to_string()
            } else {
                name.to_string()
            }
        })
        .collect()
}

/// 读时合成交付摘要(详情接口调用;纯函数,不碰 DB)。
pub(crate) fn summarize(
    plan: &[TaskStep],
    workspace: &Path,
    changed: Vec<String>,
    changed_count: usize,
    goal: &str,
) -> TaskDeliverySummary {
    let required = required_from_plan(plan);
    let workspace_available = workspace.is_dir();
    let missing = missing_files(workspace, &required);
    let content_issues = content_issues(workspace, &required, goal);
    let truncated = truncated_from_plan(plan);
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
        content_issues,
        truncated,
    }
}

/// 文件交付门槛的输入(单一结构体,避免参数继续膨胀)。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DeliveryGateInput<'a> {
    /// 规划器声明的必需文件(规范化路径 + 步骤名)
    pub required: &'a [(String, String)],
    /// 声明了但磁盘上不存在的路径
    pub missing: &'a [String],
    /// 声明文件的内容体检不通过项(空文件/仅空白/`@@` 槽位)
    pub content_issues: &'a [String],
    /// 被守卫截断(partial)的子目标名
    pub truncated: &'a [String],
    /// 本次执行是否产生了非回滚文件变更
    pub changed: bool,
}

impl DeliveryGateInput<'_> {
    /// 文件产物侧是否有未满足项(缺失 / 内容体检 / 零变更)。
    fn file_unmet(&self) -> bool {
        (!self.required.is_empty() && !self.missing.is_empty())
            || !self.content_issues.is_empty()
            || !self.changed
    }

    /// 门槛原因的完整清单(缺什么/内容问题/截断),按固定顺序拼接。
    fn notes(&self) -> Vec<String> {
        let mut notes: Vec<String> = Vec::new();
        if !self.missing.is_empty() {
            notes.push(missing_files_note(self.missing));
        }
        if !self.content_issues.is_empty() {
            notes.push(content_issues_note(self.content_issues));
        }
        if !self.changed {
            notes.push(FILE_DELIVERY_MISSING_NOTE.to_string());
        }
        notes
    }
}

/// 对终态应用文件交付门槛。
///
/// 返回 `(terminal, downgraded)`:
/// - `downgraded = true` 仅表示「本次把 Done/Partial 改写成带原因的 Partial」,
///   调用方据此补一条任务 Status 事件(可解释性);
/// - `enabled = false`(未绑定工作区/策略不给写工具/模式不适用)时原样返回;
/// - `enabled = true` 时任一条件未满足即 Partial:
///   - 声明了必需文件且任一缺失 → 「缺少必需文件:…」;
///   - 声明文件内容体检不过(空文件/仅空白/未替换 `@@` 槽位)→ 「交付内容校验未通过:…」
///     (SKY-6 第 3 项,2026-10-24);
///   - 本次零变更 → 「未检测到文件变更」(TMR-3 弱门槛);
///   - 存在被守卫截断的 partial 步骤 → 先确保终态不是 Done(SKY-6 第 2 项;
///     执行器通常已写原因,此处不重复追加)。
/// - 只有 `TaskTerminal::Complete` 会被改写:Followup(追加指令可以只回答文字)、
///   AwaitApproval(尚未执行)、Failed/Noop(本来就不是 Done)一律不碰。
pub(crate) fn apply_file_delivery_gate(
    terminal: TaskTerminal,
    enabled: bool,
    input: &DeliveryGateInput<'_>,
) -> (TaskTerminal, bool) {
    if !enabled {
        return (terminal, false);
    }
    let file_unmet = input.file_unmet();
    let truncated = !input.truncated.is_empty();
    if !file_unmet && !truncated {
        return (terminal, false);
    }
    match terminal {
        TaskTerminal::Complete {
            result,
            status,
            error,
        } => match status {
            TaskStatus::Done | TaskStatus::Partial => {
                let mut notes = input.notes();
                // 截断原因:执行器/team 通常已写进 error;不重复追加
                //(未来某个模式漏写时,门槛仍要留下可解释原因)。
                if truncated && !error.as_deref().unwrap_or("").contains("守卫截断") {
                    notes.push(truncated_note(input.truncated));
                }
                if notes.is_empty() {
                    // 原因已齐备(通常是执行器已把「守卫截断」写进 error):
                    // Done 仍须降 Partial;已是 Partial 则原样返回,不追加空行。
                    if status == TaskStatus::Done {
                        return (
                            TaskTerminal::Complete {
                                result,
                                status: TaskStatus::Partial,
                                error,
                            },
                            true,
                        );
                    }
                    return (
                        TaskTerminal::Complete {
                            result,
                            status,
                            error,
                        },
                        false,
                    );
                }
                (
                    TaskTerminal::Complete {
                        result,
                        status: TaskStatus::Partial,
                        error: Some(merge_note(error.as_deref(), &notes.join("\n"))),
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
pub(crate) fn delivery_event_detail(input: &DeliveryGateInput<'_>) -> String {
    let mut notes = input.notes();
    if !input.truncated.is_empty() {
        notes.push(truncated_note(input.truncated));
    }
    // 只有截断原因时不冒充「文件产物」问题(截断是执行形态,不是文件校验)。
    let prefix = if input.missing.is_empty() && input.content_issues.is_empty() && input.changed {
        "交付校验未通过"
    } else {
        "文件产物校验未通过"
    };
    format!("{prefix}:{}", notes.join(" "))
}

/// 内容体检不通过项的中文说明(最多点名 10 项,超出写总数)。
fn content_issues_note(issues: &[String]) -> String {
    let head: Vec<&str> = issues.iter().take(10).map(|s| s.as_str()).collect();
    let more = if issues.len() > head.len() {
        format!(" 等 {} 项", issues.len())
    } else {
        String::new()
    };
    format!(
        "{FILE_DELIVERY_CONTENT_PREFIX}{}{more};请补齐内容后重试。",
        head.join(";")
    )
}

/// 未完成子目标的说明(最多点名 10 个,超出写总数)。
///
/// SKY-8 起 `Partial` 不再只来自空转守卫:预算收尾且声明文件缺失也会记 partial,
/// 因此文案用「未完成」而不是「被守卫截断」——字段名 `truncated` 保持不变(线格式兼容)。
fn truncated_note(steps: &[String]) -> String {
    let head: Vec<&str> = steps.iter().take(10).map(|s| s.as_str()).collect();
    let more = if steps.len() > head.len() {
        format!(" 等 {} 个", steps.len())
    } else {
        String::new()
    };
    format!(
        "以下子目标未完成(守卫截断或预算收尾),成果可能不完整:{}{more};可追加指令继续完成。",
        head.join("、")
    )
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

    /// 门槛输入的最小构造:不含内容问题与截断(其余用例按需覆盖字段)。
    fn gate_input<'a>(
        required: &'a [(String, String)],
        missing: &'a [String],
        changed: bool,
    ) -> DeliveryGateInput<'a> {
        DeliveryGateInput {
            required,
            missing,
            content_issues: &[],
            truncated: &[],
            changed,
        }
    }

    /// 门槛关闭(未绑定工作区/策略不给写工具)→ 逐字节原样。
    #[test]
    fn gate_disabled_is_untouched() {
        let (out, downgraded) = apply_file_delivery_gate(
            complete(TaskStatus::Done, None),
            false,
            &gate_input(&[], &[], false),
        );
        assert!(!downgraded);
        assert_eq!(status_of(&out), TaskStatus::Done);
    }

    /// TMR-3 弱门槛:未声明必需文件 + 本次零变更 → Partial +「未检测到文件变更」。
    #[test]
    fn weak_gate_without_declarations_downgrades_zero_change() {
        let (out, downgraded) = apply_file_delivery_gate(
            complete(TaskStatus::Done, None),
            true,
            &gate_input(&[], &[], false),
        );
        assert!(downgraded);
        assert_eq!(status_of(&out), TaskStatus::Partial);
        assert!(error_of(&out).unwrap().contains("未检测到文件变更"));
    }

    /// 声明齐全 + 有变更 → 原样(不因门槛误伤)。
    #[test]
    fn declared_files_present_with_change_passes() {
        let req = required(&["index.html", "parts/ui.js"]);
        let (out, downgraded) = apply_file_delivery_gate(
            complete(TaskStatus::Done, None),
            true,
            &gate_input(&req, &[], true),
        );
        assert!(!downgraded);
        assert_eq!(status_of(&out), TaskStatus::Done);
    }

    /// 声明齐全但本次零变更 → Partial(TMR-3 口径:本次没有产出)。
    #[test]
    fn declared_files_present_without_change_downgrades() {
        let req = required(&["index.html"]);
        let (out, downgraded) = apply_file_delivery_gate(
            complete(TaskStatus::Done, None),
            true,
            &gate_input(&req, &[], false),
        );
        assert!(downgraded);
        assert_eq!(status_of(&out), TaskStatus::Partial);
        assert!(error_of(&out).unwrap().contains("未检测到文件变更"));
    }

    /// 必需文件缺失 → Partial,原因点名缺失文件(即使本次改过别的文件)。
    #[test]
    fn missing_required_files_downgrades_even_with_changes() {
        let req = required(&["index.html", "parts/ui.js"]);
        let missing = vec!["parts/ui.js".to_string()];
        let (out, downgraded) = apply_file_delivery_gate(
            complete(TaskStatus::Done, None),
            true,
            &gate_input(&req, &missing, true),
        );
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
            &gate_input(&req, &missing, false),
        );
        assert!(downgraded);
        let error = error_of(&out).unwrap();
        assert!(error.starts_with("审计/终审未通过"), "{error}");
        assert!(error.contains("缺少必需文件"), "{error}");
    }

    /// SKY-6 第 3 项:声明文件有未替换 `@@` 槽位 / 空文件 → 即使文件齐全且有变更
    /// 也降 Partial,并在 error 里点名具体文件与问题。
    #[test]
    fn content_issues_downgrade_even_when_files_present() {
        let req = required(&["index.html", "parts/main.js"]);
        let missing: Vec<String> = Vec::new();
        let issues = vec![
            "index.html:含 3 行 @@ 未替换槽位".to_string(),
            "parts/main.js:文件为空(0B)".to_string(),
        ];
        let input = DeliveryGateInput {
            required: &req,
            missing: &missing,
            content_issues: &issues,
            truncated: &[],
            changed: true,
        };
        let (out, downgraded) =
            apply_file_delivery_gate(complete(TaskStatus::Done, None), true, &input);
        assert!(downgraded);
        assert_eq!(status_of(&out), TaskStatus::Partial);
        let error = error_of(&out).unwrap();
        assert!(error.contains("交付内容校验未通过"), "{error}");
        assert!(error.contains("index.html"), "{error}");
        assert!(error.contains("@@"), "{error}");
        assert!(error.contains("parts/main.js"), "{error}");
    }

    /// SKY-6 第 2 项:存在被守卫截断的步骤时,即便文件齐全且有变更也要降 Partial;
    /// 已有「守卫截断」原因时不重复追加。
    #[test]
    fn truncated_steps_force_partial_without_duplicating_reason() {
        let req = required(&["index.html"]);
        let truncated = vec!["【主Agent-1】读分片".to_string()];
        let missing: Vec<String> = Vec::new();
        let input = DeliveryGateInput {
            required: &req,
            missing: &missing,
            content_issues: &[],
            truncated: &truncated,
            changed: true,
        };
        // 执行器已写原因:只降状态,不重复追加
        let (out, downgraded) = apply_file_delivery_gate(
            complete(
                TaskStatus::Done,
                Some("以下子目标被守卫截断,成果可能不完整:主 Agent-1「读分片」"),
            ),
            true,
            &input,
        );
        assert!(downgraded);
        assert_eq!(status_of(&out), TaskStatus::Partial);
        assert_eq!(
            error_of(&out).unwrap().matches("守卫截断").count(),
            1,
            "已有原因不得重复追加: {}",
            error_of(&out).unwrap()
        );

        // 未来模式漏写原因:门槛补一条可解释说明
        let (out2, downgraded2) =
            apply_file_delivery_gate(complete(TaskStatus::Done, None), true, &input);
        assert!(downgraded2);
        assert_eq!(status_of(&out2), TaskStatus::Partial);
        assert!(error_of(&out2).unwrap().contains("以下子目标被守卫截断"));
    }

    /// 门槛输入的两类原因会同时出现:文件缺失 + 内容问题 + 截断,用户能看到全貌。
    #[test]
    fn gate_input_notes_combine_all_reasons() {
        let req = required(&["index.html", "parts/ui.js"]);
        let missing = vec!["parts/ui.js".to_string()];
        let issues = vec!["index.html:含 1 行 @@ 未替换槽位".to_string()];
        let truncated = vec!["【主Agent-2】总装".to_string()];
        let input = DeliveryGateInput {
            required: &req,
            missing: &missing,
            content_issues: &issues,
            truncated: &truncated,
            changed: true,
        };
        let (out, downgraded) =
            apply_file_delivery_gate(complete(TaskStatus::Done, None), true, &input);
        assert!(downgraded);
        let error = error_of(&out).unwrap();
        assert!(error.contains("缺少必需文件"), "{error}");
        assert!(error.contains("交付内容校验未通过"), "{error}");
        assert!(error.contains("被守卫截断"), "{error}");
        let event = delivery_event_detail(&input);
        assert!(event.contains("文件产物校验未通过"), "{event}");
        assert!(event.contains("parts/ui.js"), "{event}");
    }

    /// 非 Complete 终态一律不碰:Followup 可以只回答文字,Failed/Noop 本就不是成功。
    #[test]
    fn non_complete_terminals_are_untouched() {
        let req = required(&["index.html"]);
        let missing = vec!["index.html".to_string()];
        let input = gate_input(&req, &missing, false);
        let followup = TaskTerminal::Followup {
            msg_text: "只回答文字".into(),
            new_result: "结果".into(),
            status: TaskStatus::Done,
        };
        let (out, downgraded) = apply_file_delivery_gate(followup, true, &input);
        assert!(!downgraded);
        assert!(matches!(out, TaskTerminal::Followup { .. }));

        let failed = TaskTerminal::Failed {
            error: Some("连接失败".into()),
        };
        let (out, downgraded) = apply_file_delivery_gate(failed, true, &input);
        assert!(!downgraded);
        assert!(matches!(out, TaskTerminal::Failed { .. }));

        let noop = TaskTerminal::Noop;
        let (out, downgraded) = apply_file_delivery_gate(noop, true, &input);
        assert!(!downgraded);
        assert!(matches!(out, TaskTerminal::Noop));
    }

    /// 摘要:必需文件存在性、缺失清单、变更数、内容体检与截断清单。
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
        let summary = summarize(&plan, &ws, vec!["index.html".into()], 2, "");
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
        assert!(
            summary.content_issues.is_empty(),
            "缺文件交给 missing,不重复报"
        );
        assert!(summary.truncated.is_empty());
    }

    /// SKY-6 第 3 项:内容体检抓到未替换 `@@` 槽位 / 空文件 / 仅空白;
    /// 第 2 项:截断清单来自 plan 的 partial 步骤。
    #[test]
    fn summarize_reports_content_issues_and_truncated_steps() {
        let dir = TempDataDir::new("delivery-content");
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).expect("创建内容体检工作区失败");
        std::fs::write(
            ws.join("index.html"),
            "<html>\n/*@@BODY@@*/\n/*@@UI@@*/\n</html>",
        )
        .expect("写占位模板失败");
        std::fs::write(ws.join("empty.js"), "").expect("写空文件失败");
        std::fs::write(ws.join("blank.css"), "  \n\t\n").expect("写空白文件失败");
        std::fs::write(ws.join("ok.js"), "export const ok = 1;\n").expect("写健康文件失败");

        let plan = vec![TaskStep {
            name: "【主Agent-1】总装".into(),
            status: TaskStepStatus::Partial,
            files: vec![
                "index.html".into(),
                "empty.js".into(),
                "blank.css".into(),
                "ok.js".into(),
            ],
            ..TaskStep::default()
        }];
        let summary = summarize(&plan, &ws, vec!["index.html".into()], 1, "");
        assert_eq!(
            summary.content_issues,
            vec![
                "index.html:含 2 行 @@ 未替换槽位".to_string(),
                "empty.js:文件为空(0B)".to_string(),
                "blank.css:文件仅含空白".to_string(),
            ],
            "内容体检顺序与声明顺序一致,健康文件不误报"
        );
        assert_eq!(
            summary.truncated,
            vec!["【主Agent-1】总装".to_string()],
            "partial 步骤应进截断清单"
        );
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
        let summary = summarize(&plan, &ws, Vec::new(), 0, "");
        assert!(!summary.workspace_available);
        assert!(summary.missing.is_empty(), "不可访问时不得判缺失");
        assert!(
            summary.content_issues.is_empty(),
            "不可访问时不得做内容体检"
        );
        assert!(!summary.required[0].exists);
    }

    /// SKY-1 确定性部分:命名导入未被目标文件导出必须进入内容体检(终态门槛同源)。
    #[test]
    fn content_issues_include_static_wiring_mismatch() {
        let dir = TempDataDir::new("delivery-wiring");
        let ws = dir.join("ws");
        std::fs::create_dir_all(ws.join("parts")).expect("创建接线测试工作区失败");
        std::fs::write(ws.join("parts/a.js"), "export const A = 1;\n").expect("写分片失败");
        std::fs::write(
            ws.join("index.html"),
            "<html><script type=\"module\">import { B } from './parts/a.js';</script></html>",
        )
        .expect("写入口失败");

        let issues = content_issues(
            &ws,
            &required(&["index.html", "parts/a.js"]),
            "接线一致性用例",
        );
        assert!(
            issues.iter().any(|i| i.contains("index.html")
                && i.contains("B")
                && i.contains("未被目标文件导出")),
            "应报告未导出的命名导入: {issues:?}"
        );
    }

    /// SKY-4:单文件目标仍外链本地分片 → 内容体检失败;健康内联不误伤。
    #[test]
    fn content_issues_single_file_policy_flags_local_refs_only_for_single_file_goals() {
        let dir = TempDataDir::new("delivery-single");
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).expect("创建单文件测试工作区失败");
        std::fs::write(ws.join("a.js"), "export const A = 1;\n").expect("写分片失败");
        std::fs::write(
            ws.join("external.html"),
            "<html><script src=\"./a.js\"></script></html>",
        )
        .expect("写外链入口失败");
        std::fs::write(
            ws.join("inline.html"),
            "<html><script type=\"module\">const A = 1;</script></html>",
        )
        .expect("写内联入口失败");

        let single = content_issues(
            &ws,
            &required(&["external.html", "inline.html", "a.js"]),
            "最终只交付一个 HTML 文件",
        );
        assert!(
            single
                .iter()
                .any(|i| i.contains("单文件交付不得引用本地外部资源")),
            "单文件目标必须拦截本地外链: {single:?}"
        );
        assert!(
            !single.iter().any(|i| i.contains("inline.html")),
            "健康内联入口不应被误伤: {single:?}"
        );

        let multi = content_issues(
            &ws,
            &required(&["external.html", "inline.html", "a.js"]),
            "普通多文件项目",
        );
        assert!(multi.is_empty(), "非单文件目标不应拦截本地外链: {multi:?}");
    }

    /// SKY-8:单步磁盘事实检查——缺失/0B/空白/`@@`/非法路径逐项报告;
    /// 齐全文件为空问题;工作区不可访问返回 None(不制造假 partial)。
    #[test]
    fn step_files_unmet_reports_missing_empty_placeholder_and_unavailable_workspace() {
        let dir = TempDataDir::new("step-files");
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).expect("创建工作区失败");
        std::fs::write(ws.join("ok.txt"), "hello").expect("写健康文件失败");
        std::fs::write(ws.join("empty.txt"), "").expect("写空文件失败");
        std::fs::write(ws.join("blank.txt"), "  \n\t\n").expect("写空白文件失败");
        std::fs::write(ws.join("slot.txt"), "a\n/*@@MAIN@@*/\n").expect("写槽位文件失败");

        assert_eq!(
            step_files_unmet(&ws, &["ok.txt".to_string()]),
            Some(Vec::new()),
            "齐全且内容健康的声明文件不应有问题"
        );
        let files: Vec<String> = [
            "ok.txt",
            "missing.txt",
            "empty.txt",
            "blank.txt",
            "slot.txt",
            "../escape.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let issues = step_files_unmet(&ws, &files).expect("工作区存在应可判定");
        for needle in [
            "missing.txt:文件缺失",
            "empty.txt:文件为空(0B)",
            "blank.txt:文件仅含空白",
            "slot.txt:含 1 行 @@ 未替换槽位",
            "../escape.txt:非法声明路径",
        ] {
            assert!(
                issues.iter().any(|i| i.contains(needle)),
                "应报告「{needle}」: {issues:?}"
            );
        }
        assert!(
            !issues.iter().any(|i| i.contains("ok.txt")),
            "健康文件不应被误伤: {issues:?}"
        );

        let unavailable = ws.join("not-a-dir");
        assert_eq!(
            step_files_unmet(&unavailable, &files),
            None,
            "工作区不可访问时必须返回 None(无法判定),不得制造缺失"
        );
    }
}
