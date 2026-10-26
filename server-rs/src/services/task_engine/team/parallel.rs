// team 并行执行:主 agent 的逐个/单波执行、补做轮、产出重建与终态回写。
// 由原 `team.rs` 整段搬来(C-2D,纯搬运)。
use super::assemble::strip_main_prefix;
use super::budget::fair_share;
use super::topology::{declared_files_of, declares_html, is_integration_main, plan_waves};
use super::{Kickback, MainJoinOutput, TeamExecutor, TeamMain};
use crate::models::ownership::{normalize_workspace_rel, WriteOwnership};
use crate::models::types::{TaskEventKind, TaskStep, TaskStepStatus, TokenUsage};
use crate::services::task_core::prompt_consts::{
    TEAM_ASSEMBLY_CONTRACT_NOTE, TEAM_SINGLE_FILE_ASSEMBLY_NOTE,
};
use crate::services::task_engine::context::TaskRunContext;
use crate::services::task_engine::executor::usage_as_output;
use crate::services::task_engine::solo::{run_agent_loop, AgentLoopCall, AgentLoopOutput};
use crate::services::task_engine::wiring::{file_exports, single_file_requested};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;

/// 步骤终态 = 收尾方式 + 本步声明文件的磁盘事实(SKY-8,2026-10-24)。
///
/// 规则(与 `docs/契约-协议与配置.md` 的步骤状态语义表同源):
/// - 正常返回 → `done`;
/// - **守卫截断**:声明文件全部存在且内容体检通过 → `done`(结果留一条收尾说明);
///   否则 `partial`;
/// - **预算收尾**:声明文件有缺失/空/未替换槽位 → `partial`;否则 `done`
///   (TMR-3 既有语义不变);
/// - 未声明文件或工作区不可访问:保持旧语义(守卫→partial,预算→done),不制造假判定。
/// - **单文件目标**:声明了 HTML 但不是最终入口的**中间模板**步骤,以最终入口的
///   磁盘事实为准——最终入口干净 → done;入口缺失/不干净 → partial。模板里的 `@@`
///   不再单独把中间步骤判死(模板是中间产物,最终入口才是交付物)。
///
/// 只查本步 `TaskStep.files` 的存在/空/`@@`,不递归查导入——上游文件可能尚未落盘,
/// 递归接线留给任务终态的 `delivery.content_issues`。
pub(super) fn step_outcome(
    workspace: Option<&Path>,
    step: &TaskStep,
    text: &str,
    guard_stopped: Option<&str>,
    budget_stopped: bool,
    final_entry: Option<&str>,
) -> (TaskStepStatus, String) {
    let text = text.trim_end();
    if let Some(final_rel) = final_entry {
        let step_has_html = step.files.iter().any(|file| {
            let lower = file.to_ascii_lowercase();
            lower.ends_with(".html") || lower.ends_with(".htm")
        });
        let step_has_final = step
            .files
            .iter()
            .any(|file| normalize_workspace_rel(file).as_deref() == Some(final_rel));
        if step_has_html && !step_has_final {
            let final_ok = workspace
                .and_then(|ws| {
                    crate::services::task_engine::delivery::step_files_unmet(
                        ws,
                        &[final_rel.to_string()],
                    )
                })
                .map(|issues| issues.is_empty())
                .unwrap_or(false);
            let note = if final_ok {
                "[单文件交付] 中间模板不作为交付必需项；最终入口已就绪，按完成计。"
            } else {
                "[单文件交付] 最终入口缺失或内容未通过；该中间步骤仍未完成。"
            };
            return (
                if final_ok {
                    TaskStepStatus::Done
                } else {
                    TaskStepStatus::Partial
                },
                step_result_with_note(text, note),
            );
        }
    }
    let unmet = workspace
        .and_then(|ws| crate::services::task_engine::delivery::step_files_unmet(ws, &step.files));
    if let Some(reason) = guard_stopped {
        match unmet {
            Some(issues) if !step.files.is_empty() && issues.is_empty() => (
                TaskStepStatus::Done,
                step_result_with_note(
                    text,
                    &format!(
                        "[收尾说明] 工具循环被守卫提前截断（{reason}），但本步声明文件已全部落盘且内容体检通过，按完成计。"
                    ),
                ),
            ),
            Some(issues) => {
                let detail = if issues.is_empty() {
                    format!("[守卫截断] {reason}；该子目标可能未完成。")
                } else {
                    format!(
                        "[守卫截断] {reason}；该子目标可能未完成。未满足:{}",
                        issues.join(";")
                    )
                };
                (
                    TaskStepStatus::Partial,
                    step_result_with_note(text, &detail),
                )
            }
            None => (
                TaskStepStatus::Partial,
                step_result_with_note(
                    text,
                    &format!("[守卫截断] {reason}；该子目标可能未完成。"),
                ),
            ),
        }
    } else if budget_stopped {
        match unmet {
            Some(issues) if !step.files.is_empty() && !issues.is_empty() => (
                TaskStepStatus::Partial,
                step_result_with_note(
                    text,
                    &format!(
                        "[预算收尾] 工具循环因预算到点停止；本步声明文件未满足:{}",
                        issues.join(";")
                    ),
                ),
            ),
            _ => (TaskStepStatus::Done, text.to_string()),
        }
    } else {
        (TaskStepStatus::Done, text.to_string())
    }
}

/// 在步骤正文后追加收尾说明;正文为空时不留下前导空行。
fn step_result_with_note(text: &str, note: &str) -> String {
    if text.is_empty() {
        note.to_string()
    } else {
        format!("{text}\n\n{note}")
    }
}

/// 依赖主已完成子目标的产出摘要(TMR3-3):总装主启动时注入目标文本。
/// 只取有 `result` 的步骤;步骤名附带声明文件与**引擎实测的导出名**
/// (SKY-1,2026-10-23:总装主不必再猜上游模块有哪些导出)。
pub(super) fn upstream_seed_for(
    mains: &[TeamMain],
    step_ranges: &[Vec<usize>],
    plan: &[TaskStep],
    main_index: usize,
    workspace: Option<&std::path::Path>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(main) = mains.get(main_index) else {
        return out;
    };
    for &dep in &main.after {
        let Some(range) = step_ranges.get(dep) else {
            continue;
        };
        for &si in range {
            let Some(step) = plan.get(si) else {
                continue;
            };
            if step.result.trim().is_empty() {
                continue;
            }
            let ifaces: Vec<String> = workspace
                .map(|ws| {
                    step.files
                        .iter()
                        .filter_map(|file| {
                            let exports = file_exports(&ws.join(file));
                            (!exports.is_empty())
                                .then(|| format!("{file} → {}", exports.join(" / ")))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let iface_note = if ifaces.is_empty() {
                String::new()
            } else {
                format!(" [引擎实测导出:{}]", ifaces.join(";"))
            };
            let files = if step.files.is_empty() {
                String::new()
            } else {
                format!(" [{}]", step.files.join(", "))
            };
            out.push((
                format!("{}{}{}", step.name, files, iface_note),
                step.result.clone(),
            ));
        }
    }
    out
}

/// 依赖主声明的上游文件路径（去重、保持 plan 顺序）。供集成主预置骨架使用。
fn upstream_files_for(
    mains: &[TeamMain],
    step_ranges: &[Vec<usize>],
    plan: &[TaskStep],
    main_index: usize,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(main) = mains.get(main_index) else {
        return out;
    };
    for &dep in &main.after {
        let Some(range) = step_ranges.get(dep) else {
            continue;
        };
        for &si in range {
            let Some(step) = plan.get(si) else {
                continue;
            };
            for file in &step.files {
                if !out.iter().any(|existing| existing == file) {
                    out.push(file.clone());
                }
            }
        }
    }
    out
}

/// 集成主启动前预置单文件最终入口（SKY-8 全题复跑兜底；见 `assemble_pre_seed`）。
///
/// 只在「单文件目标 + 有上游分片 + 本主声明了 HTML 入口」时触发；已有入口不覆盖。
/// 返回给目标文本的说明（也作为 AgentStatus 事件透出）；预置失败返回 None，模型
/// 仍可用 `fs_inline_html` 自行装配。
pub(super) fn pre_seed_single_file(
    ctx: &TaskRunContext,
    scope: Option<&Arc<crate::models::types::ExecScope>>,
    declared_files: &[String],
    upstream_files: &[String],
) -> Option<String> {
    if !single_file_requested(&ctx.goal) || upstream_files.is_empty() {
        return None;
    }
    // 取**最后一个** HTML：单文件目标里 `src/shell.html` 之类模板可能先声明，
    // 最终入口按 plan 顺序在后（与 `required_from_plan_for_goal` 同口径）。
    let html = declared_files
        .iter()
        .rfind(|file| {
            let lower = file.to_ascii_lowercase();
            lower.ends_with(".html") || lower.ends_with(".htm")
        })
        .cloned()?;
    // 必须用本主**派生后的 scope**（带 owner/声明文件），不能用 ctx 的 base scope：
    // 否则 reserve_write 会因「声明归属 ≠ 空 owner」拒绝预置（全题复跑前的实测）。
    let scope = scope?;
    let workspace = scope.workspace();
    let out_path = workspace.join(&html);
    if out_path.is_file() {
        return Some(format!(
            "引擎检测到最终入口 {html} 已存在:请先 fs_read 核对,再用 fs_edit 或 fs_inline_html 补全入口调用;不要重写整份内联。"
        ));
    }
    let include_three = ctx.goal.to_ascii_lowercase().contains("three");
    let outcome = crate::tools::agent_tools_fs_inline::assemble_pre_seed(
        workspace,
        &html,
        upstream_files,
        "Kedai 单文件交付（引擎预置骨架）",
        include_three,
    );
    // 跳过的 ESM 分片必须点名:全题复跑里预置静默跳过 ESM,模型既没有入口也不知道
    // 为什么没有,只能重新规划或空转。这里把「引擎不打包 ESM」变成可执行指令。
    let skipped = if outcome.skipped_esm.is_empty() {
        String::new()
    } else {
        format!(
            ";跳过 ESM 分片(引擎不打包 ESM):{}。请改写为经典脚本(window.*)后重新装配,或改用只引用远程 URL 的模板",
            outcome.skipped_esm.join("、")
        )
    };
    let Some(text) = outcome.html else {
        return Some(format!(
            "引擎未能预置最终入口 {html}:{}{skipped}。请把上游分片改写为经典脚本后调用 fs_inline_html,或提供只引用远程 URL 的模板。",
            outcome.reason.unwrap_or_default()
        ));
    };
    if let Err(reason) = scope.reserve_write(&out_path) {
        tracing::warn!(
            path = html.as_str(),
            reason = reason.as_str(),
            "预置骨架被所有权拒绝"
        );
        return None;
    }
    if let Err(error) = crate::utils::fs_atomic::write_atomic(&out_path, text.as_bytes()) {
        tracing::warn!(path = html.as_str(), error = %error, "预置骨架写入失败");
        return None;
    }
    if let Ok(stamp) = crate::models::types::ExecScope::stamp_of(&out_path) {
        scope.note_read(&out_path, stamp);
    }
    Some(format!(
        "引擎已在工作区预置最终入口 {html}({} 字节,内联 {} 个上游分片{}{skipped}):请先 fs_read 核对,再用 fs_edit 或 fs_inline_html 补全入口调用;不要重写整份内联。",
        text.len(),
        outcome.inlined,
        if include_three {
            ",已带 Three.js CDN"
        } else {
            ""
        }
    ))
}

/// 补做轮的 prior 种子:从 plan 取该主既有子目标产出,供补做子目标衔接并保持版本一致。
/// 首轮 prior 为空、由 run_main 循环内自然累积;补做轮会跳过未点名子目标(goal_filter),
/// 循环内累积不到任何东西 —— 必须在此显式种入,否则补做子目标既看不到同主其他成果,
/// 也看不到自己被替换的旧版本,「多版本并存」会在打回场景原样复现(实跑问题 2)。
/// 被打回子目标的旧版本特别标注「旧版本(本次产出将替换它)」,让模型明确是替换而非新增。
/// 仅当本轮确为子目标粒度补做(goal_filter 有值)时种入;整主补做走首轮同构路径。
pub(super) fn before_kickback_prior(
    step_ranges: &[Vec<usize>],
    plan: &[TaskStep],
    main_index: usize,
    goal_filter: Option<&[usize]>,
) -> Vec<(String, String)> {
    let Some(only) = goal_filter else {
        return Vec::new();
    };
    let Some(range) = step_ranges.get(main_index) else {
        return Vec::new();
    };
    let mut seed = Vec::new();
    for (k, &si) in range.iter().enumerate() {
        let step = &plan[si];
        // 只种入有产出的终态子目标(失败/未跑的没有可衔接内容);守卫截断的步骤
        // 也有产出,补做时要让模型看到旧版本内容。
        if !matches!(step.status, TaskStepStatus::Done | TaskStepStatus::Partial) {
            continue;
        }
        let name = strip_main_prefix(&step.name).to_string();
        if only.contains(&k) {
            seed.push((
                format!("{name}(旧版本,本次产出将替换它)"),
                step.result.clone(),
            ));
        } else {
            seed.push((name, step.result.clone()));
        }
    }
    seed
}

/// 从 plan 重建各主 agent 产出:plan 是子目标结果唯一真相源。每主按子目标顺序拼接
/// 「子目标 N「名」:产出」(N 为主内 1-based 序号)——编号是审计/终审输出 `step`
/// 字段的定位契约:审计提示词要求回填「该主第几个子目标」,输入里显式编号模型才能
/// 数对,避免靠猜序号导致打回重跑错子目标或误退整主(实跑问题 2)。
/// 失败子目标附错误说明供审计判定覆盖度。**该主无任何 Done 子目标时返回 None**
///(全败主不产生产出,维持「全部主失败 → 团队无产出」语义)。
pub(super) fn rebuild_outputs(
    mains: &[TeamMain],
    step_ranges: &[Vec<usize>],
    plan: &[TaskStep],
) -> Vec<Option<String>> {
    mains
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let mut sections: Vec<String> = Vec::new();
            let mut any_done = false;
            for (k, &si) in step_ranges[i].iter().enumerate() {
                let step = &plan[si];
                let name = strip_main_prefix(&step.name);
                match step.status {
                    // 守卫截断的步骤带已有产出(和 done 一样进审计/汇总),截断标记
                    // 已由执行器写进 result(SKY-6 第 2 项)。
                    TaskStepStatus::Done | TaskStepStatus::Partial => {
                        any_done = true;
                        sections.push(format!("子目标 {}「{name}」:\n{}", k + 1, step.result));
                    }
                    TaskStepStatus::Error => {
                        sections.push(format!(
                            "子目标 {}「{name}」:(执行失败:{})",
                            k + 1,
                            step.result
                        ));
                    }
                    _ => {}
                }
            }
            if any_done {
                Some(sections.join("\n\n"))
            } else {
                None
            }
        })
        .collect()
}

/// 全局步骤下标 → 该主内子目标下标(反查;不在该主范围内返回 None)
fn main_goal_index(step_ranges: &[Vec<usize>], main: usize, step: usize) -> Option<usize> {
    step_ranges.get(main)?.iter().position(|s| *s == step)
}

/// JoinError(panic 等)兜底:无法从 JoinError 反查是哪一主 panic(不携带业务下标),
/// 保守地把本轮 batch 中仍 running 的步骤统一置 error(终态至少 partial,不留永远
/// running 的步骤)。其余主若仍在跑,其步骤会在完成回写时覆盖为最终态,此处写的只是
/// 瞬时兜底;已终态步骤与 batch 外步骤不动。返回置 error 的步骤数。
/// 终态 partial 判定由收尾处扫描 plan 中残留的 Error 步骤承担(不再单独记 had_error,
/// 避免「首轮失败 → 打回补做成功 → had_error 未复位 → 仍判 partial」的假阴性)。
pub(super) fn fail_batch_running_steps(
    plan: &mut [TaskStep],
    batch_idx: &[usize],
    step_ranges: &[Vec<usize>],
) -> usize {
    let mut failed = 0usize;
    for &i in batch_idx {
        for &si in &step_ranges[i] {
            if plan[si].status == TaskStepStatus::Running {
                plan[si].status = TaskStepStatus::Error;
                plan[si].result = "主 agent 任务异常终止".into();
                failed += 1;
            }
        }
    }
    failed
}

impl TeamExecutor {
    /// 一个主 agent 的完整执行:按子目标逐个独立调用 run_agent_loop(每子目标一次
    /// agent 调用,同一主的多个子目标复用同一虚拟 session task:{id}:main:{n} 保持
    /// 人设一致)。返回 (主序号 0-based, 各子目标结果[全局步骤下标, 子目标名, 结果]),
    /// 由调用方逐个回写 plan 步骤并聚合进审计/汇总输入。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_main(
        &self,
        ctx: &TaskRunContext,
        main_index: usize,
        main: &TeamMain,
        step_idxs: &[usize],
        extra_instruction: Option<&str>,
        goal_filter: Option<&[usize]>,
        prior_seed: Vec<(String, String)>,
        upstream_seed: Vec<(String, String)>,
        // SKY-8:依赖主声明的上游文件路径(供集成主启动前预置单文件骨架)。
        upstream_files: Vec<String>,
        deadline_reason: &'static str,
        // SKY-7:是否豁免两个计数守卫。判定由调用方按「依赖 DAG 汇点」算出
        // (见 `is_integration_main`);不要在本函数里按 after 非空自行判定。
        integration_main: bool,
        ownership: Arc<WriteOwnership>,
    ) -> (usize, Vec<(usize, String, Result<AgentLoopOutput, String>)>) {
        let n = main_index + 1;
        let session_id = format!("task:{}:main:{}", ctx.task_id, n);
        // TMR-4:该主自己的负责文件(并集)与带 owner 的 scope。声明只用于所有权
        // 拦截与提示词;同主多个子目标共用一份声明,避免「子目标 A 写不了自己
        // 在子目标 B 里声明的总装文件」这种主内摩擦。
        let declared_files = declared_files_of(main);
        let scope = ctx.scope.as_ref().map(|base| {
            base.with_write_ownership(ownership, format!("主 Agent-{n}"), declared_files.clone())
        });
        // SKY-6/SKY-7:集成/总装主(依赖汇点)先读完全部分片再写大文件是合法前置,
        // 不能按「探索空转」截断;只关只读计数与无进展计数,语义熔断/步骤墙钟/
        // 团队总预算仍是上限。判定收窄为依赖 DAG 汇点:中段生产者(还有下游主)
        // 不再豁免(2026-10-24 全题复跑:主 2 有 after 无下游豁免后 342 轮零写盘)。
        let mut results = Vec::with_capacity(main.goals.len());
        // 本主已完成子目标的产出:(子目标名, 产出) —— 注入后续子目标,让同一主内各
        // 子目标在彼此成果上衔接,而非各自重造一整份交付物(实跑问题 2 结构性根因)。
        // 首轮为空、循环内自然累积;补做轮因跳过未点名子目标(见下方 continue),必须由
        // 调用方经 prior_seed 显式种入该主既有产出,否则补做子目标既看不到同主其他成果,
        // 也看不到自己被替换的旧版本,版本冲突会在打回场景原样复现。
        let mut prior: Vec<(String, String)> = prior_seed;
        // SKY-8:集成主启动前用上游分片预置最终入口。全题复跑里总装主把 24.5 分钟
        // 花在读分片/写扫描脚本上，最后只交 `src/shell.html`（12 处 @@），
        // `index.html` 从未落盘；预置让「入口存在」不再依赖模型的长尾行为。
        // 已有入口时不覆盖（补做轮保留模型自己的版本），只把「核对/补全」写进目标。
        let pre_seed_note = if integration_main {
            pre_seed_single_file(ctx, scope.as_ref(), &declared_files, &upstream_files)
        } else {
            None
        };
        if let Some(note) = &pre_seed_note {
            self.svc.emit_event(
                TaskEventKind::AgentStatus,
                &ctx.task_id,
                None,
                None,
                Some(note.clone()),
            );
        }
        for (k, g) in main.goals.iter().enumerate() {
            // 子目标粒度补做:仅重跑被点名的子目标(其余保留首轮产出);
            // 被跳过的子目标仍要入 results(带既有产出)以便调用方统一回写/重建
            if let Some(only) = goal_filter {
                if !only.contains(&k) {
                    continue;
                }
            }
            // 取消(含上一子目标被中断后):剩余子目标统一标记中断,保证每步都有终态
            if *ctx.cancel.borrow() {
                for (k2, g2) in main.goals.iter().enumerate().skip(k) {
                    if goal_filter.is_some_and(|only| !only.contains(&k2)) {
                        continue;
                    }
                    results.push((step_idxs[k2], g2.name.clone(), Err("任务已停止".into())));
                }
                break;
            }
            // 任务总预算用尽(PRODCAP-2):剩余子目标不再启动,统一标错(与取消同款
            // 「每步都有终态」纪律);已产出的子目标结果照常聚合。
            if crate::services::task_engine::deadline_exhausted(ctx.deadline) {
                for (k2, g2) in main.goals.iter().enumerate().skip(k) {
                    if goal_filter.is_some_and(|only| !only.contains(&k2)) {
                        continue;
                    }
                    results.push((step_idxs[k2], g2.name.clone(), Err(deadline_reason.into())));
                }
                break;
            }
            let mut goal = format!(
                "总体目标:\n{}\n\n你是主 Agent-{n},负责分工「{}」。当前子目标(第 {}/{} 个):{}\n{}\n",
                ctx.goal,
                main.name,
                k + 1,
                main.goals.len(),
                g.name,
                g.goal
            );
            // TMR-4:把「你负责哪些文件」写进目标文本,让模型在动手前就知道边界;
            // 运行期还有一道硬拦截(fs_* 调用被拒绝并回灌同一份清单)。
            if !declared_files.is_empty() {
                goal.push_str(
                    "\n你负责的文件(工作区相对路径;只能写这些,其他文件由别的主 agent 负责):\n",
                );
                for path in &declared_files {
                    goal.push_str(&format!("- {path}\n"));
                }
                goal.push_str("若必须改动未列出的文件,请在结果正文说明原因与建议,不要直接覆盖。\n");
            }
            // 同主前序产出:仅供衔接与保持一致,明确禁止整篇复述或另造版本
            if !prior.is_empty() {
                goal.push_str(
                    "\n本主此前已完成子目标的产出(仅供衔接与保持一致;不要在本次产出中整篇复述或另起一版):\n",
                );
                for (name, text) in &prior {
                    let brief: String = text.trim().chars().take(1200).collect();
                    goal.push_str(&format!("- 「{name}」:{brief}\n"));
                }
            }
            // TMR3-3:依赖主已完成——把其产出摘要带给本主,并提示文件已落盘。
            // 这是「总装主等 parts 主」的可见依据;正文摘要只作衔接,交付以文件为准。
            if !upstream_seed.is_empty() {
                goal.push_str(
                    "\n依赖的上游主 agent 产出(相关文件已落盘;请用 fs_read 核对后再总装/衔接,不要整篇复述):\n",
                );
                for (name, text) in upstream_seed.iter().take(8) {
                    let brief: String = text.trim().chars().take(1200).collect();
                    goal.push_str(&format!("- 「{name}」:{brief}\n"));
                }
                // SKY-1:总装/衔接形态——签名以文件为准,写后必须实际运行自检
                goal.push('\n');
                goal.push_str(TEAM_ASSEMBLY_CONTRACT_NOTE);
                goal.push('\n');
            }
            // SKY-4/SKY-1 单文件装配纪律:目标要求唯一单文件、且本主声明了 HTML 入口时,
            // 把「必须自包含、不得外链本地文件、不得残留槽位」写成装配边界;引擎在交付
            // 门槛对本地外链与未导出导入做确定性判定(见 wiring 模块)。
            if single_file_requested(&ctx.goal) && declares_html(&declared_files) {
                goal.push('\n');
                goal.push_str(TEAM_SINGLE_FILE_ASSEMBLY_NOTE);
                goal.push('\n');
            }
            if let Some(note) = &pre_seed_note {
                goal.push('\n');
                goal.push_str(note);
                goal.push('\n');
            }
            if let Some(extra) = extra_instruction {
                goal.push_str(&format!("\n审计补做指令:\n{extra}\n"));
            }
            goal.push_str("请完成该子目标并直接产出最终结果。");
            let call = AgentLoopCall {
                task_id: ctx.task_id.clone(),
                session_id: session_id.clone(),
                goal,
                settings: ctx.settings.clone(),
                executor_id: ctx.executor_id.clone(),
                character_id: ctx.character_id.clone(),
                phase: "agent",
                step_index: Some(step_idxs[k]),
                label: format!("主 agent {n} 子目标 {}", k + 1),
                connection_id: ctx.connection_id.clone(),
                // 工作区作用域(编码通道批次 1):各主共享同一工作区;
                // TMR-4 起再带本主的写入者身份与负责文件(所有权拦截用)。
                scope: scope.clone(),
                // 任务级总预算(PRODCAP-2):各主 agent 的循环同受截止期约束
                deadline: ctx.deadline,
                // SKY-6:集成主(有上游依赖)读全部分片是合法前置,两个计数闸门都关;
                // 非集成主维持默认,继续保护「只读探索 / 只说不写」。
                readonly_guard: !integration_main,
                no_progress_guard: !integration_main,
                // SKY-8:集成主读满 8 次 fs_read 后提醒一次「先写入口骨架」;
                // 中段生产者/普通主走既有的只读与无进展守卫,不需要这条软提醒。
                shard_read_nudge_after: integration_main
                    .then_some(crate::services::task_engine::TASK_SHARD_READ_NUDGE),
            };
            let result = run_agent_loop(
                self.svc.clone(),
                self.engine.clone(),
                call,
                ctx.cancel.clone(),
            )
            .await;
            // TMR3-L7:成功子目标的 usage 在**完成即入账**,不等 run_main 返回后由
            // 并行收集方统一记。这样主 agent 若在后续子目标 panic(JoinError),
            // 已完成部分的 token 不会随任务一起消失;汇入 `total` 仍由收集方按返回值做。
            if let Ok(out) = &result {
                self.svc.record_usage(
                    &ctx.task_id,
                    "agent",
                    Some(step_idxs[k]),
                    &usage_as_output(&out.usage),
                );
            }
            // 成功产出进 prior 供后续子目标衔接(失败不注入,避免把错误文本当参考)
            if let Ok(out) = &result {
                prior.push((g.name.clone(), out.text.clone()));
            }
            results.push((step_idxs[k], g.name.clone(), result));
        }
        (main_index, results)
    }

    /// 并行跑一批主 agent(首轮 = 全部;补做轮 = 打回指定子集/子目标,目标文本附补做指令)。
    /// 主 agent 内按子目标逐个独立执行,每子目标完成即推进其 plan 步骤
    ///(running→done/error,写库成功才发事件由 set_plan 保证)。
    /// 各主产出**在收尾时从 plan 统一重建**(而非就地拼接本轮 sections):plan 是子目标
    /// 结果的唯一真相源,重建保证补做替换旧版本、未打回主保留原产出、不出现新旧并存
    ///(实跑问题 2)。
    /// 返回 (各主产出, 是否被任务取消中断);某主部分子目标失败时产出保留成功部分
    ///(附失败说明供审计覆盖度判定),全部子目标失败才记 None。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_mains_parallel(
        &self,
        ctx: &TaskRunContext,
        mains: &[TeamMain],
        step_ranges: &[Vec<usize>],
        plan: &mut [TaskStep],
        total: &mut TokenUsage,
        kickbacks: Option<&[Kickback]>,
        ownership: &Arc<WriteOwnership>,
        // 波间截止期收尾文案:团队总预算与任务总预算要能区分(TMR3-4)
        deadline_reason: &'static str,
        // SKY-7:本轮的预算池截止期(初始执行 = 全局截止期 - 审计预留;
        // 补做轮 = 全局截止期)。None = 无墙钟预算,行为与旧实现一致。
        share_pool: Option<Instant>,
        // SKY-7:被份额收窄的主在启动下一子目标前使用的原因文案
        // (与 `deadline_reason` 区分「团队/任务总预算用尽」与「份额用尽」)。
        share_reason: &'static str,
    ) -> (Vec<Option<String>>, bool) {
        let svc = &self.svc;
        // SKY-8:单文件目标的最终入口(plan 里最后一个 HTML)。中间模板步骤的终态
        // 以它为准,不因模板里的 `@@` 单独判 partial。
        let final_entry: Option<String> = if single_file_requested(&ctx.goal) {
            crate::services::task_engine::delivery::required_from_plan_for_goal(plan, &ctx.goal)
                .into_iter()
                .next()
                .map(|(path, _)| path)
                .filter(|path| {
                    let lower = path.to_ascii_lowercase();
                    lower.ends_with(".html") || lower.ends_with(".htm")
                })
        } else {
            None
        };
        // 确定本轮要跑的主 agent 子集:(主序号, 补做指令, 限定的子目标下标集)。
        // 子目标下标先按该主实际子目标数过滤:越界/为空 → 退回整主(审计 step 越界
        // 或该主全部被点名时都不至于漏跑或空跑)。
        let batch: Vec<(usize, Option<String>, Option<Vec<usize>>)> = match kickbacks {
            None => (0..mains.len()).map(|i| (i, None, None)).collect(),
            Some(list) => {
                // 同一主的多个打回条目合并为一轮 spawn:step=None 视为整主(不设过滤),
                // 有具体 step 的取并集(指令合并);避免同一虚拟 session 被并发 spawn。
                let mut merged: Vec<(usize, Option<Vec<usize>>, Vec<String>)> = Vec::new();
                for k in list {
                    match merged.iter_mut().find(|(m, _, _)| *m == k.main) {
                        Some((_, steps, ins)) => {
                            match (&mut *steps, k.step) {
                                // 任一条目未指明子目标 = 整主补做(放宽为不过滤)
                                (s @ Some(_), None) => *s = None,
                                (Some(s), Some(step)) => {
                                    if !s.contains(&step) {
                                        s.push(step);
                                    }
                                }
                                (None, _) => {}
                            }
                            ins.push(k.instruction.clone());
                        }
                        None => merged.push((
                            k.main,
                            k.step.map(|s| vec![s]),
                            vec![k.instruction.clone()],
                        )),
                    }
                }
                merged
                    .into_iter()
                    .map(|(i, steps, ins)| {
                        // 过滤越界子目标:全部越界 → 整主(不设过滤),否则只跑命中项
                        let filtered = steps.and_then(|s| {
                            let max = mains[i].goals.len();
                            let kept: Vec<usize> = s.into_iter().filter(|x| *x < max).collect();
                            if kept.is_empty() {
                                None
                            } else {
                                Some(kept)
                            }
                        });
                        (i, Some(ins.join("\n")), filtered)
                    })
                    .collect()
            }
        };
        // 补做轮的 prior 种子在此快照:必须早于下方「置 running」——否则被打回子目标
        // 自身已被置为 Running,会被 before_kickback_prior 的 Done 过滤跳过,补做者
        // 看不到自己的旧版本(实测踩坑:只剩同门未打回子目标的产出)。
        // 按 batch 顺序与 spawn 一一对应;整主/首轮为空 vec(首轮走循环内自然累积)。
        let prior_seeds: Vec<Vec<(String, String)>> = batch
            .iter()
            .map(|(i, _, filter)| before_kickback_prior(step_ranges, plan, *i, filter.as_deref()))
            .collect();
        // SKY-7:每个主本轮**实际执行**的子目标数 = 预算份额的权重。补做轮只算
        // 被点名的子目标(被跳过的步骤不参与分配);首轮 = 该主全部子目标。
        let units_by_pos: Vec<usize> = batch
            .iter()
            .map(|(i, _, filter)| match filter {
                None => mains[*i].goals.len(),
                Some(steps) => steps.len(),
            })
            .collect();
        // TMR3-3:按主级依赖分波。首轮按 `plan_waves`;补做轮依赖已在首轮满足,
        // 保持单波(与 TMR3 之前一致)。无依赖时 waves = 一波,行为逐字节不变。
        let waves: Vec<Vec<usize>> = if kickbacks.is_none() {
            plan_waves(mains)
                .into_iter()
                .map(|wave| {
                    wave.into_iter()
                        .filter_map(|mi| batch.iter().position(|(i, _, _)| *i == mi))
                        .collect::<Vec<usize>>()
                })
                .filter(|w| !w.is_empty())
                .collect()
        } else {
            vec![(0..batch.len()).collect()]
        };
        if waves.len() > 1 {
            let desc = waves
                .iter()
                .enumerate()
                .map(|(n, w)| {
                    let names: Vec<String> = w
                        .iter()
                        .map(|pos| format!("主Agent-{}", batch[*pos].0 + 1))
                        .collect();
                    format!("第{}波:{}", n + 1, names.join(","))
                })
                .collect::<Vec<_>>()
                .join(";");
            svc.emit_event(
                TaskEventKind::Status,
                &ctx.task_id,
                None,
                None,
                Some(format!("团队依赖调度:{desc}")),
            );
        }
        let mut cancelled = false;
        for (wave_no, wave) in waves.iter().enumerate() {
            // 波间取消/截止期:未启动的条目统一标终态,不留 pending/running
            if *ctx.cancel.borrow()
                || crate::services::task_engine::deadline_exhausted(ctx.deadline)
            {
                let reason = if *ctx.cancel.borrow() {
                    cancelled = true;
                    "任务已停止"
                } else {
                    deadline_reason
                };
                for pos in waves[wave_no..].iter().flat_map(|w| w.iter().copied()) {
                    let (i, _, filter) = &batch[pos];
                    for &si in &step_ranges[*i] {
                        let will_run = match filter {
                            None => true,
                            Some(steps) => main_goal_index(step_ranges, *i, si)
                                .is_some_and(|k| steps.contains(&k)),
                        };
                        if will_run {
                            plan[si].status = TaskStepStatus::Error;
                            plan[si].result = reason.to_string();
                        }
                    }
                }
                svc.set_plan(&ctx.task_id, plan);
                break;
            }
            // 步骤置 running 并落库(事件随 set_plan 发射):只置本波真正会跑的步骤——
            // 子目标粒度补做时被跳过的步骤保持原终态,不得留 running(实跑问题 2)。
            for &pos in wave {
                let (i, _, filter) = &batch[pos];
                for &si in &step_ranges[*i] {
                    let will_run = match filter {
                        None => true,
                        Some(steps) => {
                            // filter 存的是主内子目标下标;映射回全局步骤下标比对
                            main_goal_index(step_ranges, *i, si).is_some_and(|k| steps.contains(&k))
                        }
                    };
                    if will_run {
                        plan[si].status = TaskStepStatus::Running;
                    }
                }
            }
            svc.set_plan(&ctx.task_id, plan);

            // SKY-7:按「本波 + 后续未启动波」的实际执行子目标数分配预算池。
            // 前波提前结束的余额会自然回流后续波(每波从当前剩余重新计算);
            // 同一波内各主按步数比例分,主 1 再也不能吃掉后续波的全部预算。
            let remaining_units: usize = waves[wave_no..]
                .iter()
                .flat_map(|w| w.iter())
                .map(|pos| units_by_pos[*pos])
                .sum();
            // pos → 本主本轮的截止期;None = 不设额外闸门(仅在无预算池时)。
            let mut allocations: Vec<Option<Instant>> = vec![None; batch.len()];
            if let Some(pool) = share_pool {
                let now = Instant::now();
                let pool_remaining = pool.saturating_duration_since(now);
                for &pos in wave {
                    let share = fair_share(pool_remaining, units_by_pos[pos], remaining_units);
                    allocations[pos] = Some(pool.min(now.checked_add(share).unwrap_or(pool)));
                }
                // 多主同波或仍有后续波时把分配事实透给用户/证据;单主单波不刷屏。
                let has_future = waves[wave_no + 1..].iter().any(|w| !w.is_empty());
                if wave.len() > 1 || has_future {
                    let parts: Vec<String> = wave
                        .iter()
                        .map(|pos| {
                            format!(
                                "主Agent-{} {} 秒({} 步)",
                                batch[*pos].0 + 1,
                                allocations[*pos]
                                    .map(|d| d.saturating_duration_since(now).as_secs())
                                    .unwrap_or(0),
                                units_by_pos[*pos]
                            )
                        })
                        .collect();
                    svc.emit_event(
                        TaskEventKind::AgentStatus,
                        &ctx.task_id,
                        None,
                        None,
                        Some(format!(
                            "团队预算分配(第{}波):{}(预算池剩余 {} 秒)",
                            wave_no + 1,
                            parts.join(";"),
                            pool_remaining.as_secs()
                        )),
                    );
                }
            }

            let mut join: JoinSet<MainJoinOutput> = JoinSet::new();
            for &pos in wave {
                let (i, extra, goal_filter) = batch[pos].clone();
                // SKY-7:本主的有效截止期(份额优先,无池时回退全局截止期);
                // 份额早于全局时用「份额用尽」文案,否则沿用团队/任务总预算文案。
                let main_deadline = allocations[pos].or(ctx.deadline);
                let main_reason = match (main_deadline, ctx.deadline) {
                    (Some(md), Some(gd)) if md < gd => share_reason,
                    _ => deadline_reason,
                };
                let prior_seed = prior_seeds[pos].clone();
                // 依赖主已完成子目标的产出摘要:让总装主在动手前看到上游成果;
                // 文件本身已落盘,提示模型用 fs_read 核对(不是只凭摘要复述)。
                let upstream_seed = if kickbacks.is_none() {
                    upstream_seed_for(
                        mains,
                        step_ranges,
                        plan,
                        i,
                        ctx.scope.as_ref().map(|s| s.workspace()),
                    )
                } else {
                    Vec::new()
                };
                // SKY-8:依赖主声明的上游文件(补做轮沿用首轮已落盘的产物,同样可取;
                // 这里按 plan 现况重算,不依赖 kickbacks)。
                let upstream_files = upstream_files_for(mains, step_ranges, plan, i);
                let this = Self {
                    svc: self.svc.clone(),
                    engine: self.engine.clone(),
                };
                let ctx2 = TaskRunContext {
                    task_id: ctx.task_id.clone(),
                    token: ctx.token,
                    goal: ctx.goal.clone(),
                    settings: ctx.settings.clone(),
                    executor_id: ctx.executor_id.clone(),
                    character_id: ctx.character_id.clone(),
                    cancel: ctx.cancel.clone(),
                    connection_id: ctx.connection_id.clone(),
                    // 工作区作用域(编码通道批次 1):打回补做轮沿用同一工作区
                    scope: ctx.scope.clone(),
                    // SKY-7:本主的份额截止期(无预算池时回退全局截止期)
                    deadline: main_deadline,
                };
                let main = mains[i].clone();
                // SKY-7:只有依赖 DAG 汇点(总装终点)豁免两个计数守卫
                let integration_main = is_integration_main(mains, i);
                let step_idxs = step_ranges[i].clone();
                let ownership = ownership.clone();
                join.spawn(async move {
                    this.run_main(
                        &ctx2,
                        i,
                        &main,
                        &step_idxs,
                        extra.as_deref(),
                        goal_filter.as_deref(),
                        prior_seed,
                        upstream_seed,
                        upstream_files,
                        main_reason,
                        integration_main,
                        ownership,
                    )
                    .await
                });
            }

            // 本波的主序号留底:JoinError 兜底置 error 时用(spawn 循环已消耗 wave 的引用)
            let wave_batch_idx: Vec<usize> = wave.iter().map(|pos| batch[*pos].0).collect();
            while let Some(res) = join.join_next().await {
                // 主序号不再参与聚合(产出统一从 plan 重建),仅供 JoinError 分支追溯留底
                let (_i, sub_results) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        // JoinError(panic 等):按该主失败处理,不中断其余主;JoinError 不携带
                        // 业务下标,无法反查是哪一主 panic——保守地把本波中仍 running 的
                        // 步骤统一置 error(终态至少 partial,不留永远 running)。
                        // TMR3-L7:该主**已完成**子目标的 usage 已在 run_main 内完成即入账,
                        // 不随 panic 丢失;panic 发生在进行中的调用时,那一次的用量仍不可得
                        // (同 TMR3-L1 的 future drop 边界)。
                        tracing::warn!(error = e.to_string(), "team 主 agent 任务异常终止");
                        fail_batch_running_steps(plan, &wave_batch_idx, step_ranges);
                        svc.set_plan(&ctx.task_id, plan);
                        continue;
                    }
                };
                // 逐子目标回写各自步骤(未在本轮 results 中的子目标保持原状态/产出)
                for (si, _goal_name, result) in sub_results {
                    match result {
                        Ok(out) => {
                            let usage = out.usage;
                            total.prompt_tokens += usage.prompt_tokens;
                            total.completion_tokens += usage.completion_tokens;
                            total.total_tokens += usage.total_tokens;
                            // usage 行已由 run_main 在子目标完成时落库(TMR3-L7);
                            // 这里只汇入返回值,不再重复 record_usage(否则会双计)。
                            // SKY-8:终态由「收尾方式 + 本步声明文件磁盘事实」共同决定
                            // ——守卫截断但文件齐全可记 done,预算收尾但文件缺失必须记
                            // partial;原因写进步骤结果,审计/前端都能看到。
                            let (status, result) = step_outcome(
                                ctx.scope.as_ref().map(|s| s.workspace()),
                                &plan[si],
                                &out.text,
                                out.guard_stopped.as_deref(),
                                out.budget_stopped,
                                final_entry.as_deref(),
                            );
                            plan[si].status = status;
                            plan[si].result = result;
                        }
                        Err(e) => {
                            if *ctx.cancel.borrow() {
                                cancelled = true;
                            }
                            plan[si].status = TaskStepStatus::Error;
                            plan[si].result = e.clone();
                        }
                    }
                }
                svc.set_plan(&ctx.task_id, plan);
            }
        }
        // 统一从 plan 重建各主产出(唯一真相源;补做已替换旧版本,未打回主保留原产出)
        (rebuild_outputs(mains, step_ranges, plan), cancelled)
    }
}
