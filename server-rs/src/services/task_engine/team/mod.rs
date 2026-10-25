// team 模式(批次 4.3b,首版只做自动拓扑):规划器拆目标并归并 2~4 个主 agent
//(每主 1~4 子目标,全局 ≤15 封顶)→ 各主并行跑 solo 同款工具自循环(独立 run_id/
// task:{id}:main:{n} 虚拟 session、AbortFlag、cancel 同源;可经 agentgo 派子 agent,
// 子 agent 路径与 multi 相同);主 agent 内按子目标逐个独立执行(每子目标一次调用,
// 各写各自步骤的 result/status,同一主复用同一虚拟 session 保持人设一致)→
// 审计 agent 一致性/质量/覆盖度审查(缺漏打回指定主补做,最多 1 轮;补做完成后
// 追加一次终审——只产出结论文本,不再打回)→ 升华整合输出。
// 结果契约:result = 整合文本 + "\n\n## 审计结论\n" + 审计文本(前端按此拆卡);
// 无打回时审计文本 = 首次审计结论,有打回时 = 终审结论。
// plan 步骤名 = 「【主Agent-N】子目标名」(前端分工卡按此前缀分组)。
// 手动拓扑(用户指定主 agent 数量/人设/分工)预留,待前端入口批次(docs/功能.md)。

// C-2D(2026-10-25):本模块由单文件拆为目录模块(纯搬运,行为零变化)——mod.rs 只留
// 执行器骨架与 `run_inner` 编排;topology(规划/拓扑/依赖)、budget(墙钟份额/输出预算)、
// parallel(主 agent 并行执行/补做/产出重建)、audit(审计与终审)、assemble(审计输入/
// 工作区实况/终态原因)按职责分文件;单元测试搬到 `tests.rs`。外部路径
// `task_engine::team::TeamExecutor` 零改动。

mod assemble;
mod audit;
mod budget;
mod parallel;
mod topology;

#[cfg(test)]
mod tests;

use self::assemble::{
    build_audit_input, build_final_review_input, build_summary_input, error_step_labels,
    has_error_steps, partial_step_labels, workspace_manifest,
};
use self::audit::{generate_text_healed, parse_audit};
use self::budget::{main_pool_deadline, post_exec_reserve, structured_budget};
use self::topology::build_write_ownership;
use super::context::TaskRunContext;
use super::executor::ModeExecutor;
use super::solo::AgentLoopOutput;
use crate::agents::engine::AgentEngine;
use crate::models::types::{
    LlmMessage, TaskEventKind, TaskStatus, TaskStep, TaskStepStatus, TokenUsage,
};
use crate::services::prompt_kit::untrusted_boundary;
use crate::services::task_core::prompt_consts::{
    team_audit_tool_guidance, SUMMARIZER_PROMPT, TEAM_AUDIT_PROMPT, TEAM_FINAL_AUDIT_PROMPT,
};
use crate::services::task_core::{fallback_terminal, TaskBackend, TaskTerminal};
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Instant;

/// run_mains_parallel 并行 JoinSet 的单主产出:(主序号, [(子目标序号, 子目标名, 生成结果)])
type MainJoinOutput = (usize, Vec<(usize, String, Result<AgentLoopOutput, String>)>);

/// 主 agent 分工数上限(规划器约定 2~4;超出部分归并进末位主 agent)
const TEAM_MAX_MAINS: usize = 4;
/// 每主 agent 子目标数上限(规划器约定 1~4;超出部分截断并记日志)
const TEAM_MAX_GOALS_PER_MAIN: usize = 4;
/// 全局子目标数上限(4 主 × 4 子 = 16 可超,超出部分从末位主倒序截断,每主至少留 1)
const TEAM_MAX_GOALS_GLOBAL: usize = 15;
/// 规划/审计调用的温度(结构化 JSON 输出,与 legacy planner 同口径)
const TEAM_JSON_TEMPERATURE: f64 = 0.3;
/// 规划调用初始 max_tokens(推理模型 reasoning 与正文共用预算,同 PLAN_INITIAL_MAX_TOKENS)
const TEAM_PLAN_INITIAL_MAX_TOKENS: u32 = 2048;
/// 规划解析最大尝试次数(截断/空输出翻倍预算,同 PLAN_MAX_ATTEMPTS)
const TEAM_PLAN_MAX_ATTEMPTS: u32 = 3;
/// 重试预算翻倍上限(与设置页 max_tokens 上限一致)
const TEAM_RETRY_MAX_TOKENS_CAP: u32 = 131_072;
/// 结构化/长文本阶段(审计、终审、汇总)的初始输出预算下限。
/// 2026-09-15 实录:审计 `max_tokens=10000` 被 reasoning 吃掉 9894、正文仅 178 字符
/// 即被截断,自愈翻倍到 20000 重发才成功——首发即给足可省掉这一整个 50 秒左右
/// 的空烧往返。默认 `default_max_tokens=1024` 时更小,结构化 JSON 必然腰斩。
const TEAM_STRUCTURED_MIN_TOKENS: u32 = 16384;

/// 主执行池给审计/终审/汇总预留的秒数下限/上限(SKY-7,见 `post_exec_reserve`)。
/// 下限 10s 覆盖一次快速审计/汇总往返;上限 180s 防止长任务把 5% 无界放大。
const TEAM_POST_EXEC_RESERVE_MIN_SECS: u64 = 10;
const TEAM_POST_EXEC_RESERVE_MAX_SECS: u64 = 180;

/// 每个子目标最多声明的负责文件数(TMR-4;规划器输出不可信,防超长/超量)。
const TEAM_MAX_DECLARED_FILES_PER_GOAL: usize = 32;

/// 一个主 agent 的分工:分工名 + 子目标(1~4 个)+ 依赖的其他主 agent。
#[derive(Debug, Clone)]
struct TeamMain {
    name: String,
    goals: Vec<TaskStep>,
    /// 依赖的主 agent 下标(0-based;已去自环/越界/重复)。空 = 无依赖,按现状并行。
    after: Vec<usize>,
}

/// 审计打回条目:定位粒度到「主 agent 的某个子目标」。
/// `step` 为 1-based 的主内子目标序号(审计输出新增可选字段);缺省(None)= 审计未
/// 指明子目标,退回整主补做,保持对旧审计输出的兼容。定位到具体子目标可避免
/// 「打回一个子目标却重跑该主全部子目标、各再造一版」的版本爆炸(实跑问题 2)。
#[derive(Debug, Clone, PartialEq)]
struct Kickback {
    /// 主 agent 序号(0-based)
    main: usize,
    /// 主内子目标序号(0-based;None = 整主)
    step: Option<usize>,
    /// 补做指令
    instruction: String,
}

/// 审计结论:通过/打回列表/结论文本。
/// 解析失败或输出截断时**按未通过兜底**(实跑问题 2:旧实现默认 pass=true,审计可
/// 静默失效;审计是质量闸门,读不出明确通过就不应判定通过)——结论段给干净说明,
/// 不把半截 JSON 原样灌进 result;打回为空时由调用方映射为 partial 终态。
#[derive(Debug, Clone)]
struct AuditVerdict {
    pass: bool,
    kickbacks: Vec<Kickback>,
    conclusion: String,
}

/// team 执行器:任务后端(规划/审计/汇总纯生成出口 + 落库)+ 聊天引擎(主 agent 工具循环)。
pub(crate) struct TeamExecutor {
    svc: Arc<dyn TaskBackend>,
    engine: Arc<AgentEngine>,
}

impl TeamExecutor {
    pub(crate) fn new(svc: Arc<dyn TaskBackend>, engine: Arc<AgentEngine>) -> Self {
        TeamExecutor { svc, engine }
    }

    async fn run_inner(
        &self,
        mut ctx: TaskRunContext,
    ) -> Result<(TaskTerminal, TokenUsage), String> {
        let svc = &self.svc;
        let mut total = TokenUsage::default();

        // ===== a. 规划器:自动拓扑(reset_task 已置 planning;规划阶段归执行器所有) =====
        svc.set_status(&ctx.task_id, TaskStatus::Planning);
        let (mains, plan_out) = self.plan_team(&ctx).await?;
        // TMR-4:声明表只构造一次,首轮与补做轮共用(认领记录也随之延续)。
        let ownership = build_write_ownership(&mains);
        svc.record_usage(&ctx.task_id, "planner", None, &plan_out);
        total.prompt_tokens += plan_out.prompt_tokens;
        total.completion_tokens += plan_out.completion_tokens;
        total.total_tokens += plan_out.prompt_tokens + plan_out.completion_tokens;
        if *ctx.cancel.borrow() {
            return Err("任务已停止".into());
        }

        // TMR3-4:团队总预算从**规划完成后**起算(规划自身有 3×300s 的重试上限);
        // 与任务总预算取小,团队预算更早时标记绑定,用于区分收尾文案。
        let (deadline, team_binding) =
            super::team_deadline(ctx.deadline, ctx.settings.task_team_budget_secs);
        ctx.deadline = deadline;
        // SKY-7:主执行池 = 截止期 - 审计/汇总预留。主 agent 只能分到池内份额,
        // 审计/终审/汇总继续用 ctx.deadline——主执行再也吃不掉质量闸门的最后窗口。
        let main_pool = main_pool_deadline(deadline);
        let share_reason = if team_binding {
            "团队预算份额用尽(未执行)"
        } else {
            "任务预算份额用尽(未执行)"
        };
        // TMR3-L4:默认 1200s 是**安全阀**而不是目标值,但它是 TMR3 之后的新默认行为
        // ——团队任务不再无限跑。开工时先透出一条预算事实,让用户在看到「团队总预算
        // 用尽」之前就知道有这条闸门、以及去哪里调(设置页;0 = 关闭)。
        // SKY-7:同时说明按主/步分配与审计预留,避免用户误以为每主都能用满总预算。
        if team_binding {
            let reserve_secs = deadline
                .map(|d| post_exec_reserve(d.saturating_duration_since(Instant::now())).as_secs())
                .unwrap_or(0);
            let mut msg = format!(
                "团队执行预算:{} 秒(规划完成后起算;设置页可调大或设 0 关闭)",
                ctx.settings.task_team_budget_secs
            );
            if reserve_secs > 0 {
                msg.push_str(&format!(";按主/步分配,审计/汇总预留 {reserve_secs} 秒"));
            }
            svc.emit_event(
                TaskEventKind::AgentStatus,
                &ctx.task_id,
                None,
                None,
                Some(msg),
            );
        }
        let deadline_reason = if team_binding {
            "团队总预算用尽(未执行)"
        } else {
            "任务总预算用尽(未执行)"
        };

        // plan 步骤:「【主Agent-N】子目标名」(前端分工卡分组契约);
        // step_ranges[n] = 主 agent n 在 plan 中的步骤下标区间
        let mut plan: Vec<TaskStep> = Vec::new();
        let mut step_ranges: Vec<Vec<usize>> = Vec::new();
        for (i, m) in mains.iter().enumerate() {
            let mut idxs = Vec::new();
            for g in &m.goals {
                idxs.push(plan.len());
                plan.push(TaskStep {
                    name: format!("【主Agent-{}】{}", i + 1, g.name),
                    goal: g.goal.clone(),
                    status: TaskStepStatus::Pending,
                    result: String::new(),
                    node_id: None,
                    files: g.files.clone(),
                });
            }
            step_ranges.push(idxs);
        }
        svc.set_plan(&ctx.task_id, &plan);
        svc.set_status(&ctx.task_id, TaskStatus::Running);

        // ===== b. 各主 agent 并行(JoinSet;独立 run_id/虚拟 session,cancel 同源) =====
        let (mut outputs, cancelled) = self
            .run_mains_parallel(
                &ctx,
                &mains,
                &step_ranges,
                &mut plan,
                &mut total,
                None,
                &ownership,
                deadline_reason,
                // SKY-7:首轮用「全局截止期 - 审计预留」作为主执行池
                main_pool,
                share_reason,
            )
            .await;
        if cancelled {
            return Err("任务已停止".into());
        }
        // 任务总预算用尽(PRODCAP-2):各主 agent 的循环已被截止期收窄的步骤预算收口;
        // 截止期已过则审计/汇总不再发起新调用(总预算的目的就是停止继续开销),
        // 以已完成部分收尾(无产出时 fallback_terminal 自然回落 Failed)。
        if super::deadline_exhausted(ctx.deadline) {
            let msg = if team_binding {
                "团队总预算用尽:审计与汇总未执行,以已完成部分收尾"
            } else {
                "任务总预算用尽:审计与汇总未执行,以已完成部分收尾"
            };
            svc.emit_event(
                TaskEventKind::Status,
                &ctx.task_id,
                None,
                None,
                Some(msg.to_string()),
            );
            return Ok((fallback_terminal(&plan, msg.to_string()), total));
        }
        if outputs.iter().all(|o| o.is_none()) {
            // 全灭时 helper 自然回落 Failed(无 done 步骤可拼),措辞与旧实现一致
            return Ok((
                fallback_terminal(&plan, "全部主 agent 执行失败,团队无产出".into()),
                total,
            ));
        }

        // ===== c. 审计 agent:一致性/质量/覆盖度审查;缺漏打回指定主补做(最多 1 轮) =====
        // SKY-3:审计前按 TaskStep.files 的**全类型**路径 stat 一次,把文件实况写进输入。
        // 审计员自选的 fs_glob 模式(如 "*.js")不能作为存在性判据。
        let manifest = ctx
            .scope
            .as_ref()
            .map(|s| workspace_manifest(s.workspace(), &mains, &ctx.goal))
            .unwrap_or_default();
        let audit_input = build_audit_input(&ctx.goal, &mains, &outputs, &manifest);
        // TMR-2:审计先走「只读工作区工具」通道,让审计员能核对文件真实存在与内容;
        // 工具不可用/循环失败/空输出时回退既有纯文本审计,保证审计整体不会因新能力失效。
        let mut audit_system = String::from(TEAM_AUDIT_PROMPT);
        audit_system.push_str("\n\n");
        audit_system.push_str(team_audit_tool_guidance(ctx.scope.is_some()));
        let audit_out = match super::team_audit::generate_with_readonly_tools(
            svc,
            &self.engine,
            &ctx,
            "audit",
            "审计 agent",
            &audit_system,
            &audit_input,
            super::team_audit::AUDIT_MAX_ROUNDS,
            structured_budget(ctx.settings.default_max_tokens),
            TEAM_JSON_TEMPERATURE,
            &mut total,
        )
        .await
        {
            Ok(o) => o,
            // 取消:保持 Err,由引擎按 ended 收尾(终态兜底从库里的 plan 补写已完成产出)
            Err(e) if *ctx.cancel.borrow() => return Err(e),
            Err(tool_err) => {
                tracing::warn!(
                    error = tool_err.as_str(),
                    "team 审计只读工具通道不可用,回退纯文本审计"
                );
                let audit_messages = vec![
                    LlmMessage::plain("system", &audit_system),
                    LlmMessage::plain("user", &audit_input),
                ];
                // 截断自愈:审计是结构化 JSON 输出,推理模型烧光预算会腰斩 JSON(实测)
                match generate_text_healed(
                    svc,
                    &ctx.task_id,
                    "audit",
                    audit_messages,
                    structured_budget(ctx.settings.default_max_tokens),
                    TEAM_JSON_TEMPERATURE,
                    ctx.settings.default_top_p,
                    &ctx.cancel,
                    &mut total,
                )
                .await
                {
                    Ok(o) => o,
                    Err(e) if *ctx.cancel.borrow() => return Err(e),
                    // 审计调用失败但各主已产出(提交 2 部分成果兜底):不再把成果整体丢掉
                    Err(e) => {
                        return Ok((fallback_terminal(&plan, format!("审计调用失败:{e}")), total))
                    }
                }
            }
        };
        if *ctx.cancel.borrow() {
            return Err("任务已停止".into());
        }
        let verdict = parse_audit(&audit_out.text, mains.len());
        // 进入最终 result「## 审计结论」段的文本:无打回 = 首次审计结论;
        // 有打回 = 补做后的终审结论(修复:旧实现永远挂首次打回原文,任务 done
        // 而 result 结尾仍显示「需要补全」)
        let mut final_conclusion = verdict.conclusion.clone();
        // 终态闸门(实跑问题 2):审计/终审未通过则任务不得判 done。
        // 无打回时即首次审计结论;有打回时以补做后的终审裁定覆盖。
        let mut final_pass = verdict.pass;

        // 打回补做:仅一轮,补做完成后追加终审(只产出结论文本,不再打回)
        if !verdict.pass && !verdict.kickbacks.is_empty() {
            svc.emit_event(
                TaskEventKind::AgentStatus,
                &ctx.task_id,
                None,
                None,
                Some(format!("审计打回 {} 处补做", verdict.kickbacks.len())),
            );
            let (new_outputs, cancelled) = self
                .run_mains_parallel(
                    &ctx,
                    &mains,
                    &step_ranges,
                    &mut plan,
                    &mut total,
                    Some(&verdict.kickbacks),
                    &ownership,
                    deadline_reason,
                    // SKY-7:补做轮不再二次预留(补做优先),但仍在全局截止期内
                    // 按被打回主的子目标数分配,避免一个补做主吃光终审窗口。
                    ctx.deadline,
                    share_reason,
                )
                .await;
            outputs = new_outputs;
            if cancelled {
                return Err("任务已停止".into());
            }
            // TMR3-4:补做轮可能把团队预算用尽;到点后不再发起终审/汇总。
            if super::deadline_exhausted(ctx.deadline) {
                let msg = if team_binding {
                    "团队总预算用尽:终审与汇总未执行,以已完成部分收尾"
                } else {
                    "任务总预算用尽:终审与汇总未执行,以已完成部分收尾"
                };
                svc.emit_event(
                    TaskEventKind::Status,
                    &ctx.task_id,
                    None,
                    None,
                    Some(msg.to_string()),
                );
                return Ok((fallback_terminal(&plan, msg.to_string()), total));
            }
            if outputs.iter().all(|o| o.is_none()) {
                // 补做后全灭:首轮通过的主产出仍可能已在 plan 里(helper 会捞回来),
                // 一步都没有才回落 Failed
                return Ok((
                    fallback_terminal(&plan, "补做后全部主 agent 无产出".into()),
                    total,
                ));
            }

            // SKY-5:补做轮末执行器复查(2026-10-24)。补做是否真的修好了,不再等终审:
            // 先按最终 plan 重算「必需文件缺失 + 内容体检 + 静态接线 + 单文件外链」。
            // 仍有确定性问题时直接跳过终审——任务终态必然是 partial(交付门槛同源),
            // 再花一次 16K+ 的结构化调用没有信息增量。无问题时维持既有终审。
            let recheck_required = super::delivery::required_from_plan_for_goal(&plan, &ctx.goal);
            let recheck_problems: Vec<String> = match ctx.scope.as_ref() {
                Some(scope) => {
                    let ws = scope.workspace();
                    let mut problems: Vec<String> =
                        super::delivery::missing_files(ws, &recheck_required)
                            .into_iter()
                            .map(|path| format!("缺少必需文件:{path}"))
                            .collect();
                    problems.extend(super::delivery::content_issues(
                        ws,
                        &recheck_required,
                        &ctx.goal,
                    ));
                    problems
                }
                None => Vec::new(),
            };
            if !recheck_problems.is_empty() {
                let detail = recheck_problems.join(";");
                svc.emit_event(
                    TaskEventKind::Status,
                    &ctx.task_id,
                    None,
                    None,
                    Some(format!("补做后交付内容复查未通过:{detail}")),
                );
                final_pass = false;
                final_conclusion =
                    format!("补做后交付内容复查未通过:{detail};已跳过终审,任务以 partial 交付。");
            }
            if recheck_problems.is_empty() {
                // 终审:基于补做后的最终产出给出结论文本(空输出兜底回退首次审计结论,
                // 与审计「增强环节异常不沉任务」同口径)。TMR-2:与首审同款只读工具通道,
                // 失败回退纯文本。SKY-3:补做可能改动文件,终审前**重新**枚举工作区实况,
                // 不复用首审时点的清单。
                let review_manifest = ctx
                    .scope
                    .as_ref()
                    .map(|s| workspace_manifest(s.workspace(), &mains, &ctx.goal))
                    .unwrap_or_default();
                let review_input = build_final_review_input(
                    &ctx.goal,
                    &mains,
                    &outputs,
                    &verdict,
                    &review_manifest,
                );
                let mut review_system = String::from(TEAM_FINAL_AUDIT_PROMPT);
                review_system.push_str("\n\n");
                review_system.push_str(team_audit_tool_guidance(ctx.scope.is_some()));
                let review_out = match super::team_audit::generate_with_readonly_tools(
                    svc,
                    &self.engine,
                    &ctx,
                    "final_audit",
                    "终审 agent",
                    &review_system,
                    &review_input,
                    super::team_audit::FINAL_AUDIT_MAX_ROUNDS,
                    structured_budget(ctx.settings.default_max_tokens),
                    TEAM_JSON_TEMPERATURE,
                    &mut total,
                )
                .await
                {
                    Ok(o) => o,
                    Err(e) if *ctx.cancel.borrow() => return Err(e),
                    Err(tool_err) => {
                        tracing::warn!(
                            error = tool_err.as_str(),
                            "team 终审只读工具通道不可用,回退纯文本终审"
                        );
                        let review_messages = vec![
                            LlmMessage::plain("system", &review_system),
                            LlmMessage::plain("user", &review_input),
                        ];
                        match generate_text_healed(
                            svc,
                            &ctx.task_id,
                            "final_audit",
                            review_messages,
                            structured_budget(ctx.settings.default_max_tokens),
                            TEAM_JSON_TEMPERATURE,
                            ctx.settings.default_top_p,
                            &ctx.cancel,
                            &mut total,
                        )
                        .await
                        {
                            Ok(o) => o,
                            Err(e) if *ctx.cancel.borrow() => return Err(e),
                            // 终审调用失败但补做产出已在手(提交 2 部分成果兜底):不再整体丢成果
                            Err(e) => {
                                return Ok((
                                    fallback_terminal(&plan, format!("终审调用失败:{e}")),
                                    total,
                                ))
                            }
                        }
                    }
                };
                if *ctx.cancel.borrow() {
                    return Err("任务已停止".into());
                }
                // 终审:基于补做后的最终产出做裁定(结构化 JSON);解析失败/空输出按未通过
                // 兜底(终审是终态闸门,读不出明确通过就不判 done);结论段空输出时回退首次
                // 审计结论,避免「## 审计结论」段空白
                let review_verdict = parse_audit(&review_out.text, mains.len());
                final_pass = review_verdict.pass;
                if review_out.text.trim().is_empty() {
                    tracing::warn!("team 终审返回空内容,审计结论段回退为首次审计结论");
                } else {
                    final_conclusion = review_verdict.conclusion;
                }
            }
        }

        // ===== d. 升华整合:整合文本 + 审计结论段(前端拆卡契约) =====
        // 汇总输入携带审计结论与已选定版本,并要求「只整合、不另起一版」——旧实现
        // 汇总与审计互不可见,汇总自由再生产一版,导致正文与审计结论不同源
        //(实跑问题 2)。SKY-3:汇总同样带**实时**工作区清单,不复述旧文本。
        let summary_manifest = ctx
            .scope
            .as_ref()
            .map(|s| workspace_manifest(s.workspace(), &mains, &ctx.goal))
            .unwrap_or_default();
        let summary_input = build_summary_input(
            &ctx.goal,
            &mains,
            &outputs,
            &final_conclusion,
            &summary_manifest,
        );
        let mut sum_sys = String::from(SUMMARIZER_PROMPT);
        let world = svc.world_context(ctx.character_id.as_deref());
        if !world.is_empty() {
            sum_sys.push_str(&format!("\n\n{}", untrusted_boundary("world_book", &world)));
        }
        let summary_messages = vec![
            LlmMessage::plain("system", &sum_sys),
            LlmMessage::plain("user", &summary_input),
        ];
        // 空输出重试一次(对齐 legacy 汇总空输出语义;此处简化为同参数单重重试)
        // 截断自愈与 record_usage/total 入账由 generate_text_healed 承担
        let mut summary_out = match generate_text_healed(
            svc,
            &ctx.task_id,
            "summary",
            summary_messages.clone(),
            structured_budget(ctx.settings.default_max_tokens),
            ctx.settings.default_temperature,
            ctx.settings.default_top_p,
            &ctx.cancel,
            &mut total,
        )
        .await
        {
            Ok(o) => o,
            Err(e) if *ctx.cancel.borrow() => return Err(e),
            // 汇总调用失败但各主已产出(提交 2 部分成果兜底):不再整体丢成果
            Err(e) => return Ok((fallback_terminal(&plan, format!("团队汇总失败:{e}")), total)),
        };
        if summary_out.text.trim().is_empty() && !*ctx.cancel.borrow() {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            summary_out = match svc
                .generate_text(
                    &ctx.task_id,
                    "summary",
                    None,
                    summary_messages,
                    Vec::new(),
                    structured_budget(ctx.settings.default_max_tokens),
                    ctx.settings.default_temperature,
                    ctx.settings.default_top_p,
                    None, // team 模式无节点级连接
                    ctx.cancel.clone(),
                )
                .await
            {
                Ok(o) => o,
                Err(e) if *ctx.cancel.borrow() => return Err(e),
                Err(e) => {
                    return Ok((fallback_terminal(&plan, format!("团队汇总失败:{e}")), total))
                }
            };
            svc.record_usage(&ctx.task_id, "summary", None, &summary_out);
            total.prompt_tokens += summary_out.prompt_tokens;
            total.completion_tokens += summary_out.completion_tokens;
            total.total_tokens += summary_out.prompt_tokens + summary_out.completion_tokens;
        }
        if summary_out.text.trim().is_empty() {
            // 汇总空产出同样降级(提交 2):各主成果保留
            return Ok((fallback_terminal(&plan, "团队汇总返回空内容".into()), total));
        }

        // ===== e. 收尾:终态失败步骤或审计/终审未通过 → partial;全部通过 → done =====
        // 实跑问题 2:审计不通过(含解析失败兜底未通过、补做后终审仍不过)不得判定
        // done——审计是质量闸门,其结论必须反映到终态;未通过时任务以 partial 交付,
        // 由用户决定是否追加指令继续修。
        // 失败步骤判定以 plan 终态为准(而非累加式 had_error):补做成功的步骤已被覆盖为
        // Done,不应再拖累终态——旧实现 had_error 一旦置位永不复位,导致「失败 → 打回补做
        // 成功 → 终审通过」仍判 partial 且 error 为空、用户无从解释(实跑问题 2)。
        let failed_steps = error_step_labels(&plan);
        let failed = has_error_steps(&plan);
        // SKY-6 第 2 项/SKY-8:未完成(partial)步骤即使审计通过也不得让任务记 done
        // ——步骤没跑完就是没跑完。SKY-8 起 partial 有两个来源:空转守卫截断,以及
        // 预算收尾时声明文件缺失;补做成功的步骤已被覆盖回 done,不会拖累终态。
        let truncated_steps = partial_step_labels(&plan);
        let text = format!(
            "{}\n\n## 审计结论\n{}",
            summary_out.text.trim(),
            final_conclusion
        );
        // partial 原因:失败子目标 > 审计/终审未通过;守卫截断原因**独立追加**
        // (不是二选一——两者可能同时存在,用户需要知道哪些子目标没跑完)。
        // Done 时为 None(complete_mode_run 写空串清掉上一轮残留 error)
        let mut reasons: Vec<String> = Vec::new();
        if failed {
            reasons.push(format!(
                "以下子目标执行失败,成果不完整:{}",
                failed_steps.join("、")
            ));
        } else if !final_pass {
            reasons.push("审计/终审未通过,未达交付标准(详见「审计结论」)".to_string());
        }
        if !truncated_steps.is_empty() {
            reasons.push(format!(
                "以下子目标未完成(守卫截断或预算收尾),成果可能不完整:{};可追加指令继续完成",
                truncated_steps.join("、")
            ));
        }
        let error = (!reasons.is_empty()).then(|| reasons.join("\n"));
        // 有可解释 error(失败子目标 / 审计未通过 / 未完成步骤)即 partial,否则 done
        let status = if error.is_some() {
            TaskStatus::Partial
        } else {
            TaskStatus::Done
        };
        Ok((
            TaskTerminal::Complete {
                result: text,
                status,
                error,
            },
            total,
        ))
    }
}

impl ModeExecutor for TeamExecutor {
    fn run<'a>(
        &'a self,
        ctx: TaskRunContext,
    ) -> BoxFuture<'a, Result<(TaskTerminal, TokenUsage), String>> {
        Box::pin(async move { self.run_inner(ctx).await })
    }
}
