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
// 手动拓扑(用户指定主 agent 数量/人设/分工)预留,待前端入口批次(docs/任务引擎六模式.md)。
use super::context::TaskRunContext;
use super::executor::{usage_as_output, ModeExecutor, TaskOutcome};
use super::solo::{run_agent_loop, AgentLoopCall};
use crate::agents::engine::AgentEngine;
use crate::models::types::{LlmMessage, TaskStatus, TaskStep, TaskStepStatus, TokenUsage};
use crate::services::prompt_kit::untrusted_boundary;
use crate::services::task_service::prompt::{
    SUMMARIZER_PROMPT, TEAM_AUDIT_PROMPT, TEAM_FINAL_AUDIT_PROMPT, TEAM_PLANNER_PROMPT,
};
use crate::services::task_service::TaskService;
use futures::future::BoxFuture;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinSet;

/// run_mains_parallel 并行 JoinSet 的单主产出:(主序号, [(子目标序号, 子目标名, 生成结果)])
type MainJoinOutput = (
    usize,
    Vec<(usize, String, Result<(String, TokenUsage), String>)>,
);

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
/// 重试预算翻倍上限(同 RETRY_MAX_TOKENS_CAP)
const TEAM_RETRY_MAX_TOKENS_CAP: u32 = 65536;

/// 一个主 agent 的分工:分工名 + 子目标(1~4 个)
#[derive(Debug, Clone)]
struct TeamMain {
    name: String,
    goals: Vec<TaskStep>,
}

/// 审计结论:通过/打回列表/结论文本(解析失败兜底 pass=true,审计不沉任务)
#[derive(Debug, Clone)]
struct AuditVerdict {
    pass: bool,
    /// (主 agent 序号 0-based, 补做指令)
    kickbacks: Vec<(usize, String)>,
    conclusion: String,
}

/// 规划器输出拓扑的原始 JSON 形态
#[derive(Deserialize)]
struct TeamTopologyRaw {
    mains: Vec<TeamMainRaw>,
}

#[derive(Deserialize)]
struct TeamMainRaw {
    name: String,
    #[serde(default)]
    goals: Vec<TaskStep>,
}

/// 解析 team 规划输出:剥 markdown 代码块 → 取首个 '{' 到末个 '}' 子串 → serde 解析;
/// 过滤空分工/空子目标,归并超编主 agent(>4 并入第 4 位)、截断超编子目标(每主 >4
/// 截前 4),再做全局 ≤15 封顶(4×4=16 可超,从末位主倒序截断,每主至少留 1)。
/// mains 为空报错(交给重试);仅 1 个主 agent 时接受(降级拓扑,记日志),不强行拆分。
fn parse_team_topology(text: &str) -> Result<Vec<TeamMain>, String> {
    let t = text.trim();
    let stripped = t
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let candidate = match (stripped.find('{'), stripped.rfind('}')) {
        (Some(lo), Some(hi)) if lo < hi => &stripped[lo..=hi],
        _ => stripped,
    };
    let raw: TeamTopologyRaw =
        serde_json::from_str(candidate).map_err(|e| format!("解析团队拓扑失败: {e}"))?;
    let mut mains: Vec<TeamMain> = raw
        .mains
        .into_iter()
        .filter(|m| !m.name.trim().is_empty())
        .map(|m| TeamMain {
            name: m.name.trim().to_string(),
            goals: m
                .goals
                .into_iter()
                .filter(|g| !g.name.trim().is_empty() || !g.goal.trim().is_empty())
                .collect(),
        })
        .filter(|m| !m.goals.is_empty())
        .collect();
    if mains.is_empty() {
        return Err("解析团队拓扑失败: 规划输出不含任何有效主 agent 分工".into());
    }
    // 超编归并:第 4 位之后的主 agent 子目标并入第 4 位
    if mains.len() > TEAM_MAX_MAINS {
        let tail = mains.split_off(TEAM_MAX_MAINS);
        let merged: usize = tail.iter().map(|m| m.goals.len()).sum();
        for m in tail {
            mains[TEAM_MAX_MAINS - 1].goals.extend(m.goals);
        }
        crate::utils::logger::warn(
            "team 规划主 agent 超编,已归并进末位主 agent",
            &[("merged_goals", serde_json::Value::from(merged))],
        );
    }
    // 每主子目标超编截断(保留前 TEAM_MAX_GOALS_PER_MAIN)
    for (i, m) in mains.iter_mut().enumerate() {
        if m.goals.len() > TEAM_MAX_GOALS_PER_MAIN {
            crate::utils::logger::warn(
                "team 规划子目标超编,已截断",
                &[
                    ("main", serde_json::Value::from(i + 1)),
                    (
                        "dropped",
                        serde_json::Value::from(m.goals.len() - TEAM_MAX_GOALS_PER_MAIN),
                    ),
                ],
            );
            m.goals.truncate(TEAM_MAX_GOALS_PER_MAIN);
        }
    }
    // 全局子目标 ≤15 封顶(4 主 × 4 子 = 16 可超):从末位主倒序截断,每主至少留 1
    let mut total: usize = mains.iter().map(|m| m.goals.len()).sum();
    if total > TEAM_MAX_GOALS_GLOBAL {
        let mut dropped = 0usize;
        for m in mains.iter_mut().rev() {
            if total <= TEAM_MAX_GOALS_GLOBAL {
                break;
            }
            let overflow = total - TEAM_MAX_GOALS_GLOBAL;
            // 每主至少保留 1 个子目标(空分工已在前面过滤,此处守住不再制造空主)
            let drop = overflow.min(m.goals.len().saturating_sub(1));
            m.goals.truncate(m.goals.len() - drop);
            total -= drop;
            dropped += drop;
        }
        crate::utils::logger::warn(
            "team 规划子目标全局超编,已按 ≤15 封顶截断",
            &[("dropped", serde_json::Value::from(dropped))],
        );
    }
    if mains.len() == 1 {
        crate::utils::logger::warn("team 规划仅产出 1 个主 agent,按降级拓扑执行", &[]);
    }
    Ok(mains)
}

/// 解析审计输出:{"通过":bool,"打回":[{"main":1-based 序号,"instruction":"..."}],"结论":"..."}。
/// 解析失败兜底 pass=true、结论=原文(审计是增强环节,格式异常不应沉掉整个任务);
/// 打回序号越界的条目丢弃(防 LLM 幻觉引用不存在的主 agent);同一主重复打回只保留
/// 首条(防同一虚拟 session task:{id}:main:{n} 在补做轮被并发 spawn 两次)。
fn parse_audit(text: &str, mains_count: usize) -> AuditVerdict {
    let fallback = || AuditVerdict {
        pass: true,
        kickbacks: Vec::new(),
        conclusion: text.trim().to_string(),
    };
    let t = text.trim();
    let stripped = t
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let candidate = match (stripped.find('{'), stripped.rfind('}')) {
        (Some(lo), Some(hi)) if lo < hi => &stripped[lo..=hi],
        _ => stripped,
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(candidate) else {
        // 腰斩 JSON(推理模型烧光预算的截断形态,自愈重发仍截断时的末层防线)
        // 与纯文本回复区别对待:前者结论段给干净说明——半截 JSON 原样进
        // result「## 审计结论」卡既难看又误导;原文记 warn 字段备查
        if stripped.starts_with('{') {
            crate::utils::logger::warn(
                "team 审计输出为截断 JSON,按通过兜底",
                &[(
                    "head",
                    serde_json::Value::from(stripped.chars().take(120).collect::<String>()),
                )],
            );
            return AuditVerdict {
                pass: true,
                kickbacks: Vec::new(),
                conclusion: "(审计输出不完整,按通过兜底)".to_string(),
            };
        }
        crate::utils::logger::warn("team 审计输出非 JSON,按通过兜底", &[]);
        return fallback();
    };
    let pass = v.get("通过").and_then(|b| b.as_bool()).unwrap_or(true);
    let conclusion = v
        .get("结论")
        .and_then(|s| s.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| text.trim().to_string());
    let mut kickbacks: Vec<(usize, String)> = Vec::new();
    if let Some(arr) = v.get("打回").and_then(|a| a.as_array()) {
        for item in arr {
            let main = item.get("main").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
            let instruction = item
                .get("instruction")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if main >= 1 && main <= mains_count && !instruction.is_empty() {
                let idx = main - 1;
                // 同一主重复打回只保留首条(补做轮每主一个 spawn,重复条目会并发复用同一虚拟 session)
                if kickbacks.iter().any(|(m, _)| *m == idx) {
                    crate::utils::logger::warn(
                        "team 审计打回条目与既有条目同主,已保留首条",
                        &[("main", serde_json::Value::from(main))],
                    );
                    continue;
                }
                kickbacks.push((idx, instruction));
            } else {
                crate::utils::logger::warn(
                    "team 审计打回条目无效(序号越界或指令为空),已丢弃",
                    &[("main", serde_json::Value::from(main))],
                );
            }
        }
    }
    AuditVerdict {
        pass,
        kickbacks,
        conclusion,
    }
}

/// 截断自愈预算决策(纯函数):仅 finish_reason=length 触发,预算翻倍并封顶。
/// 对齐 solo/custom 逐步执行的自愈语义(2026-09-03 实测:team 审计/终审/汇总
/// 直调 generate_text 无自愈,推理模型 reasoning 烧光 1024 预算产出腰斩 JSON)。
fn trunc_heal_budget(finish_reason: Option<&str>, max_tokens: u32) -> Option<u32> {
    if finish_reason == Some("length") && max_tokens < TEAM_RETRY_MAX_TOKENS_CAP {
        Some(max_tokens.saturating_mul(2).min(TEAM_RETRY_MAX_TOKENS_CAP))
    } else {
        None
    }
}

/// 审计/终审/汇总共用的纯生成出口:先按原预算调 generate_text,finish=length
/// 时翻倍重发一次(截断自愈)。两次调用均经 generate_text 落 task_llm_calls,
/// 调用情况面板可见「截断 → 提高预算重发」完整链路;token 用量两次都入 total,
/// record_usage 两次都落(与 solo 自愈留痕同口径)。
#[allow(clippy::too_many_arguments)]
async fn generate_text_healed(
    svc: &TaskService,
    task_id: &str,
    phase: &str,
    messages: Vec<LlmMessage>,
    max_tokens: u32,
    temperature: f64,
    top_p: f64,
    cancel: &watch::Receiver<bool>,
    total: &mut TokenUsage,
) -> Result<crate::services::task_service::TaskGenOutput, String> {
    let mut out = svc
        .generate_text(
            task_id,
            phase,
            None,
            messages.clone(),
            Vec::new(),
            max_tokens,
            temperature,
            top_p,
            cancel.clone(),
        )
        .await?;
    svc.record_usage(task_id, phase, None, &out);
    total.prompt_tokens += out.prompt_tokens;
    total.completion_tokens += out.completion_tokens;
    total.total_tokens += out.prompt_tokens + out.completion_tokens;
    let Some(retry_budget) = trunc_heal_budget(out.finish_reason.as_deref(), max_tokens) else {
        return Ok(out);
    };
    crate::utils::logger::warn(
        "team 纯生成调用截断,输出上限翻倍重发",
        &[
            ("phase", serde_json::Value::from(phase)),
            ("max_tokens", serde_json::Value::from(max_tokens)),
            ("retry_max_tokens", serde_json::Value::from(retry_budget)),
        ],
    );
    svc.emit_event(
        "agent_status",
        task_id,
        None,
        None,
        Some(format!(
            "(截断自愈){phase} 输出截断,输出上限翻倍至 {retry_budget} 重发"
        )),
    );
    out = svc
        .generate_text(
            task_id,
            phase,
            None,
            messages,
            Vec::new(),
            retry_budget,
            temperature,
            top_p,
            cancel.clone(),
        )
        .await?;
    svc.record_usage(task_id, phase, None, &out);
    total.prompt_tokens += out.prompt_tokens;
    total.completion_tokens += out.completion_tokens;
    total.total_tokens += out.prompt_tokens + out.completion_tokens;
    Ok(out)
}

/// team 执行器:任务服务(规划/审计/汇总纯生成出口 + 落库)+ 聊天引擎(主 agent 工具循环)。
pub(crate) struct TeamExecutor {
    svc: Arc<TaskService>,
    engine: Arc<AgentEngine>,
}

impl TeamExecutor {
    pub(crate) fn new(svc: Arc<TaskService>, engine: Arc<AgentEngine>) -> Self {
        TeamExecutor { svc, engine }
    }

    /// team 规划:TEAM_PLANNER_PROMPT + 世界书背景(untrusted 包裹),解析失败按
    /// finish_reason 分级重试(截断/空输出翻倍预算,最多 TEAM_PLAN_MAX_ATTEMPTS 次)。
    /// 调用追踪经 generate_text 统一出口落 task_llm_calls(phase=planner)。
    async fn plan_team(
        &self,
        ctx: &TaskRunContext,
    ) -> Result<(Vec<TeamMain>, crate::services::task_service::TaskGenOutput), String> {
        let mut sys = String::from(TEAM_PLANNER_PROMPT);
        let world = self.svc.world_context(ctx.character_id.as_deref());
        if !world.is_empty() {
            sys.push_str(&format!("\n\n{}", untrusted_boundary("world_book", &world)));
        }
        let messages = vec![
            LlmMessage::plain("system", &sys),
            LlmMessage::plain("user", &ctx.goal),
        ];
        let mut max_tokens = TEAM_PLAN_INITIAL_MAX_TOKENS;
        let mut last_err = String::from("规划器未产出有效拓扑");
        for attempt in 1..=TEAM_PLAN_MAX_ATTEMPTS {
            if attempt > 1 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if *ctx.cancel.borrow() {
                    return Err("任务已停止".into());
                }
            }
            let out = self
                .svc
                .generate_text(
                    &ctx.task_id,
                    "planner",
                    None,
                    messages.clone(),
                    Vec::new(),
                    max_tokens,
                    TEAM_JSON_TEMPERATURE,
                    ctx.settings.default_top_p,
                    ctx.cancel.clone(),
                )
                .await?;
            match parse_team_topology(&out.text) {
                Ok(mains) => return Ok((mains, out)),
                Err(e) => last_err = e,
            }
            let reason = out.finish_reason.as_deref().unwrap_or("");
            crate::utils::logger::warn(
                "team 规划输出解析失败,准备重试",
                &[
                    ("attempt", serde_json::Value::from(attempt)),
                    ("finish_reason", serde_json::Value::from(reason)),
                    ("error", serde_json::Value::from(last_err.clone())),
                ],
            );
            if reason == "length" || out.text.trim().is_empty() {
                max_tokens = (max_tokens.saturating_mul(2)).min(TEAM_RETRY_MAX_TOKENS_CAP);
            }
        }
        Err(format!(
            "{last_err}(已重试 {} 次)",
            TEAM_PLAN_MAX_ATTEMPTS - 1
        ))
    }

    /// 一个主 agent 的完整执行:按子目标逐个独立调用 run_agent_loop(每子目标一次
    /// agent 调用,同一主的多个子目标复用同一虚拟 session task:{id}:main:{n} 保持
    /// 人设一致)。返回 (主序号 0-based, 各子目标结果[全局步骤下标, 子目标名, 结果]),
    /// 由调用方逐个回写 plan 步骤并聚合进审计/汇总输入。
    async fn run_main(
        &self,
        ctx: &TaskRunContext,
        main_index: usize,
        main: &TeamMain,
        step_idxs: &[usize],
        extra_instruction: Option<&str>,
    ) -> (
        usize,
        Vec<(usize, String, Result<(String, TokenUsage), String>)>,
    ) {
        let n = main_index + 1;
        let count = main.goals.len();
        let session_id = format!("task:{}:main:{}", ctx.task_id, n);
        let mut results = Vec::with_capacity(count);
        for (k, g) in main.goals.iter().enumerate() {
            // 取消(含上一子目标被中断后):剩余子目标统一标记中断,保证每步都有终态
            if *ctx.cancel.borrow() {
                for (k2, g2) in main.goals.iter().enumerate().skip(k) {
                    results.push((step_idxs[k2], g2.name.clone(), Err("任务已停止".into())));
                }
                break;
            }
            let mut goal = format!(
                "总体目标:\n{}\n\n你是主 Agent-{n},负责分工「{}」。当前子目标(第 {}/{} 个):{}\n{}\n",
                ctx.goal,
                main.name,
                k + 1,
                count,
                g.name,
                g.goal
            );
            if let Some(extra) = extra_instruction {
                goal.push_str(&format!("\n审计补做指令:\n{extra}\n"));
            }
            goal.push_str("请完成该子目标并直接产出最终结果。");
            let call = AgentLoopCall {
                task_id: ctx.task_id.clone(),
                session_id: session_id.clone(),
                goal,
                settings: ctx.settings.clone(),
                character_id: ctx.character_id.clone(),
                phase: "agent",
                step_index: Some(step_idxs[k]),
                label: format!("主 agent {n} 子目标 {}", k + 1),
            };
            let result = run_agent_loop(
                self.svc.clone(),
                self.engine.clone(),
                call,
                ctx.cancel.clone(),
            )
            .await;
            results.push((step_idxs[k], g.name.clone(), result));
        }
        (main_index, results)
    }

    async fn run_inner(&self, ctx: TaskRunContext) -> Result<TaskOutcome, String> {
        let svc = &self.svc;
        let mut total = TokenUsage::default();

        // ===== a. 规划器:自动拓扑(reset_task 已置 planning;规划阶段归执行器所有) =====
        svc.set_status(&ctx.task_id, TaskStatus::Planning);
        let (mains, plan_out) = self.plan_team(&ctx).await?;
        svc.record_usage(&ctx.task_id, "planner", None, &plan_out);
        total.prompt_tokens += plan_out.prompt_tokens;
        total.completion_tokens += plan_out.completion_tokens;
        total.total_tokens += plan_out.prompt_tokens + plan_out.completion_tokens;
        if *ctx.cancel.borrow() {
            return Err("任务已停止".into());
        }

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
                });
            }
            step_ranges.push(idxs);
        }
        svc.set_plan(&ctx.task_id, &plan);
        svc.set_status(&ctx.task_id, TaskStatus::Running);

        // ===== b. 各主 agent 并行(JoinSet;独立 run_id/虚拟 session,cancel 同源) =====
        let outputs: Vec<Option<String>> = vec![None; mains.len()];
        let mut had_error = false;
        let (outputs, cancelled) = self
            .run_mains_parallel(
                &ctx,
                &mains,
                &step_ranges,
                &mut plan,
                outputs,
                &mut total,
                &mut had_error,
                None,
            )
            .await;
        if cancelled {
            return Err("任务已停止".into());
        }
        let mut outputs = outputs;
        if outputs.iter().all(|o| o.is_none()) {
            return Err("全部主 agent 执行失败,团队无产出".into());
        }

        // ===== c. 审计 agent:一致性/质量/覆盖度审查;缺漏打回指定主补做(最多 1 轮) =====
        let audit_input = build_audit_input(&ctx.goal, &mains, &outputs);
        let audit_messages = vec![
            LlmMessage::plain("system", TEAM_AUDIT_PROMPT),
            LlmMessage::plain("user", &audit_input),
        ];
        // 截断自愈:审计是结构化 JSON 输出,推理模型烧光预算会腰斩 JSON(实测)
        let audit_out = generate_text_healed(
            svc,
            &ctx.task_id,
            "audit",
            audit_messages,
            ctx.settings.default_max_tokens,
            TEAM_JSON_TEMPERATURE,
            ctx.settings.default_top_p,
            &ctx.cancel,
            &mut total,
        )
        .await?;
        if *ctx.cancel.borrow() {
            return Err("任务已停止".into());
        }
        let verdict = parse_audit(&audit_out.text, mains.len());
        // 进入最终 result「## 审计结论」段的文本:无打回 = 首次审计结论;
        // 有打回 = 补做后的终审结论(修复:旧实现永远挂首次打回原文,任务 done
        // 而 result 结尾仍显示「需要补全」)
        let mut final_conclusion = verdict.conclusion.clone();

        // 打回补做:仅一轮,补做完成后追加终审(只产出结论文本,不再打回)
        if !verdict.pass && !verdict.kickbacks.is_empty() {
            svc.emit_event(
                "agent_status",
                &ctx.task_id,
                None,
                None,
                Some(format!(
                    "审计打回 {} 个主 agent 补做",
                    verdict.kickbacks.len()
                )),
            );
            let (new_outputs, cancelled) = self
                .run_mains_parallel(
                    &ctx,
                    &mains,
                    &step_ranges,
                    &mut plan,
                    outputs,
                    &mut total,
                    &mut had_error,
                    Some(&verdict.kickbacks),
                )
                .await;
            outputs = new_outputs;
            if cancelled {
                return Err("任务已停止".into());
            }
            if outputs.iter().all(|o| o.is_none()) {
                return Err("补做后全部主 agent 无产出".into());
            }

            // 终审:基于补做后的最终产出给出结论文本(空输出兜底回退首次审计结论,
            // 与审计「增强环节异常不沉任务」同口径)
            let review_input = build_final_review_input(&ctx.goal, &mains, &outputs, &verdict);
            let review_messages = vec![
                LlmMessage::plain("system", TEAM_FINAL_AUDIT_PROMPT),
                LlmMessage::plain("user", &review_input),
            ];
            let review_out = generate_text_healed(
                svc,
                &ctx.task_id,
                "final_audit",
                review_messages,
                ctx.settings.default_max_tokens,
                TEAM_JSON_TEMPERATURE,
                ctx.settings.default_top_p,
                &ctx.cancel,
                &mut total,
            )
            .await?;
            if *ctx.cancel.borrow() {
                return Err("任务已停止".into());
            }
            let conclusion = review_out.text.trim();
            if conclusion.is_empty() {
                crate::utils::logger::warn("team 终审返回空内容,审计结论段回退为首次审计结论", &[]);
            } else {
                final_conclusion = conclusion.to_string();
            }
        }

        // ===== d. 升华整合:整合文本 + 审计结论段(前端拆卡契约) =====
        let summary_input = build_audit_input(&ctx.goal, &mains, &outputs);
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
        let mut summary_out = generate_text_healed(
            svc,
            &ctx.task_id,
            "summary",
            summary_messages.clone(),
            ctx.settings.default_max_tokens,
            ctx.settings.default_temperature,
            ctx.settings.default_top_p,
            &ctx.cancel,
            &mut total,
        )
        .await?;
        if summary_out.text.trim().is_empty() && !*ctx.cancel.borrow() {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            summary_out = svc
                .generate_text(
                    &ctx.task_id,
                    "summary",
                    None,
                    summary_messages,
                    Vec::new(),
                    ctx.settings.default_max_tokens,
                    ctx.settings.default_temperature,
                    ctx.settings.default_top_p,
                    ctx.cancel.clone(),
                )
                .await?;
            svc.record_usage(&ctx.task_id, "summary", None, &summary_out);
            total.prompt_tokens += summary_out.prompt_tokens;
            total.completion_tokens += summary_out.completion_tokens;
            total.total_tokens += summary_out.prompt_tokens + summary_out.completion_tokens;
        }
        if summary_out.text.trim().is_empty() {
            return Err("团队汇总返回空内容".into());
        }

        // ===== e. 收尾:有主失败但有产出 → partial(对齐 legacy partial 语义) =====
        let text = format!(
            "{}\n\n## 审计结论\n{}",
            summary_out.text.trim(),
            final_conclusion
        );
        let status = if had_error {
            Some(TaskStatus::Partial)
        } else {
            None
        };
        Ok(TaskOutcome {
            text,
            usage: total,
            status,
        })
    }

    /// 并行跑一批主 agent(首轮 = 全部;补做轮 = kickbacks 指定子集,目标文本附补做指令)。
    /// 主 agent 内按子目标逐个独立执行,每子目标完成即推进其 plan 步骤
    ///(running→done/error,写库成功才发事件由 set_plan 保证)。
    /// 返回 (各主产出, 是否被任务取消中断);某主部分子目标失败时产出保留成功部分
    ///(附失败说明供审计覆盖度判定),全部子目标失败才记 None。
    #[allow(clippy::too_many_arguments)]
    async fn run_mains_parallel(
        &self,
        ctx: &TaskRunContext,
        mains: &[TeamMain],
        step_ranges: &[Vec<usize>],
        plan: &mut [TaskStep],
        mut outputs: Vec<Option<String>>,
        total: &mut TokenUsage,
        had_error: &mut bool,
        kickbacks: Option<&[(usize, String)]>,
    ) -> (Vec<Option<String>>, bool) {
        let svc = &self.svc;
        // 确定本轮要跑的主 agent 子集
        let batch: Vec<(usize, Option<String>)> = match kickbacks {
            None => (0..mains.len()).map(|i| (i, None)).collect(),
            Some(list) => list
                .iter()
                .map(|(i, ins)| (*i, Some(ins.clone())))
                .collect(),
        };
        // 步骤置 running 并落库(事件随 set_plan 发射)
        for (i, _) in &batch {
            for &si in &step_ranges[*i] {
                plan[si].status = TaskStepStatus::Running;
            }
        }
        svc.set_plan(&ctx.task_id, plan);
        // 本轮 batch 的主序号留底:JoinError 兜底置 error 时用(spawn 循环会消耗 batch)
        let batch_idx: Vec<usize> = batch.iter().map(|(i, _)| *i).collect();

        let mut join: JoinSet<MainJoinOutput> = JoinSet::new();
        for (i, extra) in batch {
            let this = Self {
                svc: self.svc.clone(),
                engine: self.engine.clone(),
            };
            let ctx2 = TaskRunContext {
                task_id: ctx.task_id.clone(),
                goal: ctx.goal.clone(),
                settings: ctx.settings.clone(),
                character_id: ctx.character_id.clone(),
                cancel: ctx.cancel.clone(),
            };
            let main = mains[i].clone();
            let step_idxs = step_ranges[i].clone();
            join.spawn(async move {
                this.run_main(&ctx2, i, &main, &step_idxs, extra.as_deref())
                    .await
            });
        }

        let mut cancelled = false;
        while let Some(res) = join.join_next().await {
            let (i, sub_results) = match res {
                Ok(v) => v,
                Err(e) => {
                    // JoinError(panic 等):按该主失败处理,不中断其余主;JoinError 不携带
                    // 业务下标,无法反查是哪一主 panic——保守地把本轮 batch 中仍 running 的
                    // 步骤统一置 error 并置 had_error(终态至少 partial,不留永远 running)
                    crate::utils::logger::warn(
                        "team 主 agent 任务异常终止",
                        &[("error", serde_json::Value::from(e.to_string()))],
                    );
                    fail_batch_running_steps(plan, &batch_idx, step_ranges, had_error);
                    svc.set_plan(&ctx.task_id, plan);
                    continue;
                }
            };
            // 逐子目标回写各自步骤;聚合该主产出(成功段 + 失败说明)
            let mut sections: Vec<String> = Vec::new();
            let mut succeeded = 0usize;
            for (si, goal_name, result) in sub_results {
                match result {
                    Ok((text, usage)) => {
                        total.prompt_tokens += usage.prompt_tokens;
                        total.completion_tokens += usage.completion_tokens;
                        total.total_tokens += usage.total_tokens;
                        // usage 落库(phase=agent,step_index=该子目标的全局步骤下标;
                        // 子 agent 行在 run_subtask 内落)
                        svc.record_usage(&ctx.task_id, "agent", Some(si), &usage_as_output(&usage));
                        plan[si].status = TaskStepStatus::Done;
                        plan[si].result = text.clone();
                        sections.push(format!("子目标「{goal_name}」:\n{text}"));
                        succeeded += 1;
                    }
                    Err(e) => {
                        if *ctx.cancel.borrow() {
                            cancelled = true;
                        }
                        *had_error = true;
                        plan[si].status = TaskStepStatus::Error;
                        plan[si].result = e.clone();
                        sections.push(format!("子目标「{goal_name}」:(执行失败:{e})"));
                    }
                }
            }
            // 补做轮某主全败时保留其首轮产出是有意的既定兜底:历史产出优于丢空,
            // 审计/汇总仍可基于首轮内容判定覆盖度——此时该主步骤态(error)与送审内容
            //(首轮产出)不一致,属既定兜底口径,不做对齐
            if succeeded > 0 {
                outputs[i] = Some(sections.join("\n\n"));
            }
            svc.set_plan(&ctx.task_id, plan);
        }
        (outputs, cancelled)
    }
}

/// JoinError(panic 等)兜底:无法从 JoinError 反查是哪一主 panic(不携带业务下标),
/// 保守地把本轮 batch 中仍 running 的步骤统一置 error 并置 had_error(任务终态至少
/// partial,不留永远 running 的步骤)。其余主若仍在跑,其步骤会在完成回写时覆盖为
/// 最终态,此处写的只是瞬时兜底;已终态步骤与 batch 外步骤不动。返回置 error 的步骤数。
fn fail_batch_running_steps(
    plan: &mut [TaskStep],
    batch_idx: &[usize],
    step_ranges: &[Vec<usize>],
    had_error: &mut bool,
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
    if failed > 0 {
        *had_error = true;
    }
    failed
}

/// 审计/汇总的输入文本:总体目标 + 各主产出(失败主保留错误文本,供覆盖度判定)
fn build_audit_input(goal: &str, mains: &[TeamMain], outputs: &[Option<String>]) -> String {
    let mut user = format!("用户目标:\n{goal}\n\n各主 agent 产出:\n");
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

/// 终审的输入文本:补做后的各主产出 + 首轮审计意见(结论与打回清单),
/// 供终审员判断首轮指出的问题是否已解决(终审只产出结论文本,不再打回)。
fn build_final_review_input(
    goal: &str,
    mains: &[TeamMain],
    outputs: &[Option<String>],
    verdict: &AuditVerdict,
) -> String {
    let mut user = build_audit_input(goal, mains, outputs);
    user.push_str("首轮审计结论:\n");
    user.push_str(&verdict.conclusion);
    user.push_str("\n\n首轮打回(均已完成补做):\n");
    for (i, ins) in &verdict.kickbacks {
        user.push_str(&format!("- 主 Agent-{}:{}\n", i + 1, ins));
    }
    user
}

impl ModeExecutor for TeamExecutor {
    fn run<'a>(&'a self, ctx: TaskRunContext) -> BoxFuture<'a, Result<TaskOutcome, String>> {
        Box::pin(async move { self.run_inner(ctx).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合法拓扑直接解析;过滤空分工/空子目标
    #[test]
    fn parse_team_topology_plain() {
        let text = r#"{"mains":[{"name":"调研","goals":[{"name":"子一","goal":"做一"},{"name":"子二","goal":"做二"}]},{"name":"写作","goals":[{"name":"子三","goal":"做三"}]}]}"#;
        let mains = parse_team_topology(text).expect("合法拓扑应解析成功");
        assert_eq!(mains.len(), 2);
        assert_eq!(mains[0].goals.len(), 2);
        assert_eq!(mains[1].name, "写作");
    }

    /// 剥 markdown 代码块 + 前言后记容忍
    #[test]
    fn parse_team_topology_fenced_and_prose() {
        let text = "好的,拓扑如下:\n```json\n{\"mains\":[{\"name\":\"甲\",\"goals\":[{\"name\":\"g\",\"goal\":\"x\"}]}]}\n```\n以上。";
        let mains = parse_team_topology(text).unwrap();
        assert_eq!(mains.len(), 1, "单主降级拓扑应被接受");
    }

    /// 超编归并与截断:6 主 → 4 主(尾部并入第 4 位),每主上限 4(归并后第 4 位
    /// 9 个子目标 → 截前 4)
    #[test]
    fn parse_team_topology_clamps_overflow() {
        let mut text = String::from(r#"{"mains":["#);
        for i in 1..=6 {
            if i > 1 {
                text.push(',');
            }
            text.push_str(&format!(
                r#"{{"name":"主{i}","goals":[{{"name":"a","goal":"x"}},{{"name":"b","goal":"y"}},{{"name":"c","goal":"z"}}]}}"#
            ));
        }
        text.push_str("]}");
        let mains = parse_team_topology(&text).unwrap();
        assert_eq!(mains.len(), 4, "超编主 agent 应归并为 4");
        for m in &mains {
            assert!(m.goals.len() <= 4, "每主子目标应截断到 4");
        }
        assert_eq!(mains[3].goals.len(), 4, "归并后第 4 位应截断到 4");
        let total: usize = mains.iter().map(|m| m.goals.len()).sum();
        assert!(total <= 15, "全局子目标应 ≤15");
    }

    /// 全局 ≤15 封顶真实生效:每主上限调至 4 后 4×4=16 可超,从末位主倒序截断,
    /// 每主至少保留 1 个子目标(4×2=8<15 时旧上限永远触不到,属逻辑矛盾修复)
    #[test]
    fn parse_team_topology_global_cap_fifteen() {
        let mut text = String::from(r#"{"mains":["#);
        for i in 1..=4 {
            if i > 1 {
                text.push(',');
            }
            text.push_str(&format!(
                r#"{{"name":"主{i}","goals":[{{"name":"a","goal":"x"}},{{"name":"b","goal":"y"}},{{"name":"c","goal":"z"}},{{"name":"d","goal":"w"}}]}}"#
            ));
        }
        text.push_str("]}");
        let mains = parse_team_topology(&text).unwrap();
        let total: usize = mains.iter().map(|m| m.goals.len()).sum();
        assert_eq!(total, 15, "全局子目标应封顶 15: {mains:?}");
        for m in &mains {
            assert!(!m.goals.is_empty(), "每主至少保留 1 个子目标: {mains:?}");
        }
        assert_eq!(mains[3].goals.len(), 3, "应从末位主截断: {mains:?}");
    }

    /// 空拓扑/垃圾文本报错(交给重试)
    #[test]
    fn parse_team_topology_rejects_empty() {
        assert!(parse_team_topology("{\"mains\":[]}").is_err());
        assert!(parse_team_topology("这不是 JSON").is_err());
        assert!(parse_team_topology("").is_err());
        // 空分工/全空子目标被过滤后为空 → 报错
        assert!(parse_team_topology(
            r#"{"mains":[{"name":"","goals":[]},{"name":"甲","goals":[]}]}"#
        )
        .is_err());
    }

    /// 审计输出:通过/打回/结论;序号 1-based 转 0-based;越界条目丢弃
    #[test]
    fn parse_audit_kickbacks() {
        let v = parse_audit(
            r#"{"通过":false,"打回":[{"main":2,"instruction":"补数据"},{"main":9,"instruction":"越界"},{"main":1,"instruction":""}],"结论":"主二缺数据"}"#,
            3,
        );
        assert!(!v.pass);
        assert_eq!(
            v.kickbacks,
            vec![(1, "补数据".to_string())],
            "越界与空指令条目应丢弃"
        );
        assert_eq!(v.conclusion, "主二缺数据");

        let ok = parse_audit(r#"{"通过":true,"打回":[],"结论":"覆盖完整"}"#, 2);
        assert!(ok.pass && ok.kickbacks.is_empty());
    }

    /// 审计输出非 JSON:兜底通过、结论取原文(审计异常不沉任务)
    #[test]
    fn parse_audit_fallback_passes() {
        let v = parse_audit("看起来都不错", 2);
        assert!(v.pass);
        assert_eq!(v.conclusion, "看起来都不错");
    }

    /// 截断自愈预算决策(2026-09-03 实测:推理模型 reasoning 烧光
    /// default_max_tokens=1024,审计 finish=length 产出腰斩 JSON):
    /// 仅 finish=length 触发翻倍,封顶 TEAM_RETRY_MAX_TOKENS_CAP
    #[test]
    fn trunc_heal_budget_only_on_length() {
        assert_eq!(trunc_heal_budget(Some("length"), 1024), Some(2048));
        assert_eq!(trunc_heal_budget(Some("length"), 40000), Some(65536));
        assert_eq!(trunc_heal_budget(Some("length"), 65536), None);
        assert_eq!(trunc_heal_budget(Some("stop"), 1024), None);
        assert_eq!(trunc_heal_budget(None, 1024), None);
    }

    /// 腰斩 JSON 兜底(自愈重发仍截断的末层防线):按通过兜底,但结论段
    /// 不落半截 JSON(原样进 result「## 审计结论」卡既难看又误导),给干净说明
    #[test]
    fn audit_truncated_json_fallback_clean_conclusion() {
        let v = parse_audit(r#"{"通过":false,"打回":[{"main":2"#, 2);
        assert!(v.pass);
        assert!(v.kickbacks.is_empty());
        assert!(
            !v.conclusion.contains('{'),
            "截断 JSON 不应原样进结论段,实际:{}",
            v.conclusion
        );
    }

    /// 审计打回按主序号去重:同一主重复打回只保留首条
    ///(防同一虚拟 session task:{id}:main:{n} 在补做轮被并发 spawn 两次)
    #[test]
    fn parse_audit_dedups_same_main() {
        let v = parse_audit(
            r#"{"通过":false,"打回":[{"main":1,"instruction":"首条指令"},{"main":2,"instruction":"主二补做"},{"main":1,"instruction":"重复条目应丢弃"}],"结论":"x"}"#,
            3,
        );
        assert!(!v.pass);
        assert_eq!(
            v.kickbacks,
            vec![(0, "首条指令".to_string()), (1, "主二补做".to_string())],
            "同一主重复打回应只保留首条"
        );
    }

    /// JoinError(panic)兜底:无法从 JoinError 反查是哪一主 panic,保守地把本轮 batch 中
    /// 仍 running 的步骤统一置 error 并置 had_error(任务终态至少 partial,不留永远
    /// running 的步骤);已终态步骤与 batch 外步骤不动。
    ///(集成侧不可注入:mock 链路无法让某一主 panic,故抽取 fail_batch_running_steps 单测覆盖。)
    #[test]
    fn fail_batch_running_steps_marks_running_error() {
        fn mk(status: TaskStepStatus) -> TaskStep {
            TaskStep {
                name: "子目标".into(),
                goal: String::new(),
                status,
                result: String::new(),
            }
        }
        let mut plan = vec![
            mk(TaskStepStatus::Done),
            mk(TaskStepStatus::Running),
            mk(TaskStepStatus::Running),
            mk(TaskStepStatus::Pending),
        ];
        // 4 主各领 1 步;本轮 batch = 主 0/1/3(主 2 不在本轮)
        let step_ranges = vec![vec![0], vec![1], vec![2], vec![3]];
        let mut had_error = false;
        let failed = fail_batch_running_steps(&mut plan, &[0, 1, 3], &step_ranges, &mut had_error);
        assert_eq!(failed, 1, "仅 batch 内仍 running 的步骤应置 error");
        assert_eq!(plan[0].status, TaskStepStatus::Done, "已终态步骤不动");
        assert_eq!(plan[1].status, TaskStepStatus::Error);
        assert!(!plan[1].result.is_empty(), "置 error 应附原因文本");
        assert_eq!(plan[2].status, TaskStepStatus::Running, "batch 外步骤不动");
        assert_eq!(
            plan[3].status,
            TaskStepStatus::Pending,
            "非 running 步骤不动"
        );
        assert!(had_error, "有步骤被置 error 时 had_error 应置位");

        // 幂等:无 running 步骤时不重复置位、不污染 had_error 以外的状态
        let mut had_error2 = false;
        let failed2 = fail_batch_running_steps(&mut plan, &[0], &step_ranges, &mut had_error2);
        assert_eq!(failed2, 0, "无 running 步骤应返回 0");
        assert!(!had_error2, "无 running 步骤不应置 had_error");
        assert_eq!(plan[0].status, TaskStepStatus::Done);
    }
}
