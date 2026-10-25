// team 组装:工作区文件实况、审计/汇总/终审输入,以及终态原因与步骤标签。
// 由原 `team.rs` 整段搬来(C-2D,纯搬运)。
use super::topology::declared_files_of;
use super::{AuditVerdict, TeamMain};
use crate::models::types::{TaskStep, TaskStepStatus};
use crate::services::task_engine::delivery::hygiene_issues;
use crate::services::task_engine::wiring::wiring_issues;

/// 审计/终审/汇总输入里的「工作区声明文件实况」(SKY-3,2026-10-23)。
///
/// 为什么需要:空岛 team 实跑中,审计员用 `fs_glob("*.js")` 核对文件,只看到 9 个
/// 分片便断言「没有 index.html」——而它其实已存在(27,882B 骨架)。文件核对不能
/// 依赖模型自选通配模式。这里按规划器声明的**全类型**路径逐项 stat,把「存在/缺失 +
/// 字节数」直接写进输入,审计与汇总都以同一份事实为起点;审计若与清单冲突,以清单为准。
///
/// 文本文件另标「含 N 行 @@ 未替换槽位」(SKY-4/SKY-5):占位符是否清零不再靠审计
/// 自己 grep,引擎先给出可直接引用的计数。
///
/// SKY-1 确定性部分(2026-10-24):同一份清单后附**静态接线检查**——导入名是否真被
/// 导出、本地模块/资源是否存在、单文件目标是否仍外链本地文件。审计不必靠自由文本
/// 发现 `buildSubIslands` 与 `generateSubIslands` 这类硬错配。
///
/// 工作区不可访问时返回空串(与交付摘要同口径:不制造假缺失)。
pub(super) fn workspace_manifest(
    workspace: &std::path::Path,
    mains: &[TeamMain],
    goal: &str,
) -> String {
    if !workspace.is_dir() {
        return String::new();
    }
    let mut seen: Vec<String> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for (i, main) in mains.iter().enumerate() {
        for path in declared_files_of(main) {
            if seen.contains(&path) {
                continue;
            }
            seen.push(path.clone());
            let abs = workspace.join(&path);
            let state = match std::fs::metadata(&abs) {
                Ok(m) if m.is_file() => {
                    let mut state = format!("存在({}B)", m.len());
                    if let Some(n) =
                        crate::services::task_engine::delivery::placeholder_line_count(&abs)
                            .filter(|n| *n > 0)
                    {
                        state.push_str(&format!(",含 {n} 行 @@ 未替换槽位"));
                    }
                    state
                }
                Ok(_) => "存在但非普通文件".to_string(),
                Err(_) => "缺失".to_string(),
            };
            lines.push(format!("- 主 Agent-{} `{path}`:{state}", i + 1));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "\n工作区声明文件实况(引擎在审计前按 TaskStep.files 全类型枚举;与主 agent 自述冲突时以本清单为准):\n{}\n",
        lines.join("\n")
    );
    let wiring = wiring_issues(workspace, &seen, goal);
    if !wiring.is_empty() {
        out.push_str(
            "\n引擎静态接线检查(导入名未导出/本地模块或资源不存在/单文件交付外链;确定性判定,与自述冲突时以本清单为准):\n",
        );
        for issue in &wiring {
            out.push_str(&format!("- {issue}\n"));
        }
    }
    // SKY-1 交付卫生:根目录未声明的临时脚本/草稿(如 `_verify_*.js`)会让交付目录
    // 混入非交付物。只作提示,不降终态;审计与汇总都看到同一份清单。
    let declared_pairs: Vec<(String, String)> = seen
        .iter()
        .map(|path| (path.clone(), String::new()))
        .collect();
    let hygiene = hygiene_issues(workspace, &declared_pairs);
    if !hygiene.is_empty() {
        out.push_str("\n引擎交付卫生检查(根目录未声明的临时/草稿文件;只作提示,不据此判失败):\n");
        for issue in &hygiene {
            out.push_str(&format!("- {issue}\n"));
        }
    }
    out
}

/// 去掉 plan 步骤名的「【主Agent-N】」前缀(前端分工卡分组契约的前缀由执行器添加)
pub(super) fn strip_main_prefix(name: &str) -> &str {
    match name.find('】') {
        Some(i) => name[i + '】'.len_utf8()..].trim_start(),
        None => name,
    }
}

/// 收尾判定:plan 中是否残留终态 Error 的子目标步骤(补做成功的步骤已被覆盖为 Done,
/// 不再拖累终态)→ 决定任务落 done 还是 partial。
pub(super) fn has_error_steps(plan: &[TaskStep]) -> bool {
    plan.iter().any(|s| s.status == TaskStepStatus::Error)
}

/// 收集 plan 中终态 Error 的子目标名(带主 agent 序号,便于用户定位是哪个分工),
/// 用于 partial 终态的原因文本。空 = 无失败步骤。
pub(super) fn error_step_labels(plan: &[TaskStep]) -> Vec<String> {
    plan.iter()
        .filter(|s| s.status == TaskStepStatus::Error)
        .map(step_label)
        .collect()
}

/// 收集被守卫截断(partial)的子目标名,用于 partial 终态的原因文本。
/// 空 = 没有步骤被截断。
pub(super) fn partial_step_labels(plan: &[TaskStep]) -> Vec<String> {
    plan.iter()
        .filter(|s| s.status == TaskStepStatus::Partial)
        .map(step_label)
        .collect()
}

/// 步骤名的用户可见定位(带「主 Agent-N」前缀时拆出来重排)。
fn step_label(s: &TaskStep) -> String {
    // plan 步骤名前缀「【主Agent-N】」由执行器构造(前端分工卡分组契约),可靠
    match s
        .name
        .strip_prefix("【主Agent-")
        .and_then(|r| r.split('】').next())
        .and_then(|n| n.parse::<usize>().ok())
    {
        Some(n) => format!("主 Agent-{n}「{}」", strip_main_prefix(&s.name)),
        None => strip_main_prefix(&s.name).to_string(),
    }
}

/// 审计/汇总的输入文本:总体目标 + 工作区声明文件实况 + 各主产出
/// (失败主保留错误文本,供覆盖度判定)。
pub(super) fn build_audit_input(
    goal: &str,
    mains: &[TeamMain],
    outputs: &[Option<String>],
    manifest: &str,
) -> String {
    let mut user = format!("用户目标:\n{goal}\n");
    user.push_str(manifest);
    user.push_str("\n各主 agent 产出:\n");
    for (i, m) in mains.iter().enumerate() {
        let body = match &outputs[i] {
            Some(text) => text.clone(),
            None => "(该主 agent 执行失败,无产出)".to_string(),
        };
        user.push_str(&format!(
            "### 主 Agent-{}「{}」:\n{}\n\n",
            i + 1,
            m.name,
            body
        ));
    }
    user
}

/// 汇总的输入文本:总体目标 + 各主产出 + 审计结论,并要求「只整合已选定版本、
/// 不得另起一版」。旧实现汇总输入不含审计结论,汇总者对审计认定的不一致毫不知情,
/// 自由再生产一版,最终 result 正文与「## 审计结论」段不同源(实跑问题 2)。
pub(super) fn build_summary_input(
    goal: &str,
    mains: &[TeamMain],
    outputs: &[Option<String>],
    audit_conclusion: &str,
    manifest: &str,
) -> String {
    let mut user = build_audit_input(goal, mains, outputs, manifest);
    user.push_str("审计结论(必须遵守;若指出某主产出为旧版/有缺漏,以补做后的最终版本为准):\n");
    user.push_str(audit_conclusion);
    user.push_str(
        "\n\n请只整合上述各主 agent 的最终产出,输出一份统一的最终成果:\
         不要另起一版、不要改写或替换已通过审计的产出内容、不要新增未被要求的内容。\
         若各主产出存在重复或前后不一致,以审计结论所指的最终版本为准。",
    );
    user
}

/// 终审的输入文本:补做后的各主产出 + 首轮审计意见(结论与打回清单),
/// 供终审员判断首轮指出的问题是否已解决(终审只产出结论文本,不再打回)。
pub(super) fn build_final_review_input(
    goal: &str,
    mains: &[TeamMain],
    outputs: &[Option<String>],
    verdict: &AuditVerdict,
    manifest: &str,
) -> String {
    let mut user = build_audit_input(goal, mains, outputs, manifest);
    user.push_str("首轮审计结论:\n");
    user.push_str(&verdict.conclusion);
    user.push_str("\n\n首轮打回(均已完成补做,补做产出已替换旧版本):\n");
    for k in &verdict.kickbacks {
        let target = match k.step {
            Some(s) => format!("主 Agent-{} 第 {} 个子目标", k.main + 1, s + 1),
            None => format!("主 Agent-{}", k.main + 1),
        };
        user.push_str(&format!("- {target}:{}\n", k.instruction));
    }
    user
}
