// team 审计/终审的**只读工具**通道(TMR-2,2026-10-19)。
//
// 背景:team 的规划/审计/终审/汇总此前全部走 `generate_text + Vec::new()`,审计员只能
// 读主 agent 的自述,无法核对工作区里的文件是否存在、内容是否一致。实测里主 3 的
// 「拒绝伪造」是模型自觉的兜底,不是系统能力。
//
// 本模块只给审计/终审挂**工作区只读三件**(fs_read/fs_glob/fs_grep),不挂 bash 与任何
// 写工具:审计的职责是核对,不是执行;命令级验收(node --check/测试)留作遗留 TMR-L3。
// 任一环节失败(无作用域、工具面为空、循环 Err/空输出)由调用方回退既有纯文本审计,
// 保证新增能力不会让审计整体失效。
use super::context::TaskRunContext;
use super::sink;
use crate::agents::engine::executor::{run_tool_loop, ToolGate};
use crate::agents::engine::{AbortFlag, AgentEngine};
use crate::agents::state_machine::{AgentState, StateMachine};
use crate::models::types::{
    GenerationParams, LlmMessage, TokenUsage, ToolBudget, ToolChoice, ToolContext,
};
use crate::services::settings_service::vision_enabled;
use crate::services::task_core::{TaskBackend, TaskGenOutput};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// 审计工具循环的墙钟上限(单次审计的完整循环,不是每轮):审计是核对环节,
/// 不应比一个执行步骤更贵;与任务总预算取小(见 `effective_step_budget`)。
const AUDIT_TOOL_BUDGET: Duration = Duration::from_secs(300);

/// 审计工具循环轮次上限(显式小预算):只读核对不需要无限轮;达到后按既有路径收尾,
/// 产出仍会进入 JSON 解析(解析失败按未通过兜底,口径不变)。
pub(crate) const AUDIT_MAX_ROUNDS: u32 = 12;

/// 终审工具循环轮次上限:只核对补做后的最终产出,比首审更聚焦。
pub(crate) const FINAL_AUDIT_MAX_ROUNDS: u32 = 8;

/// 以只读工作区工具跑一轮审计/终审生成。
///
/// 成功返回的 `TaskGenOutput` 已含正文、finish_reason 与累计 token;调用方只需把它
/// 送进既有 `parse_audit`。`total` 在本函数内累加(与 `generate_text_healed` 同口径:
/// 每次调用都 `record_usage` 并计入整任务用量)。
///
/// **不负责回退**:工具不可用/循环失败一律 Err,由调用方回退纯文本审计——回退策略是
/// 编排层知识,放在这里会让本模块同时承担两条路径。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn generate_with_readonly_tools(
    svc: &Arc<dyn TaskBackend>,
    engine: &Arc<AgentEngine>,
    ctx: &TaskRunContext,
    phase: &'static str,
    label: &str,
    system: &str,
    user: &str,
    max_rounds: u32,
    max_tokens: u32,
    temperature: f64,
    total: &mut TokenUsage,
) -> Result<TaskGenOutput, String> {
    // 没有作用域就没有可核对的工作区文件:直接 Err,让调用方走纯文本审计。
    // (未绑定工作区的任务在 run_inner 里也有 scratch 作用域,因此实际只有在
    //  作用域构造失败/测试替身等异常形态才会走到这里。)
    let Some(scope) = ctx.scope.clone() else {
        return Err("审计工具不可用:本任务没有工作区作用域".into());
    };
    // 工具面:任务策略编译后只保留工作区只读三件——即使策略是 all/deny_dangerous,
    // 审计也拿不到写工具/bash;白名单是硬边界(ToolGate::listed)。
    let policy = super::tool_policy::compile(
        &ctx.settings.task_tool_policy,
        &ctx.settings.task_tool_allowlist,
        &engine.tool_registry(),
        true,
        ctx.settings.task_coding_bundle_enabled,
        vision_enabled(&ctx.settings),
        ctx.settings.vision_screenshot_enabled,
    );
    let allowed: Vec<String> = crate::tools::tool_sets::WORKSPACE_READONLY_TOOLS
        .iter()
        .map(|s| s.to_string())
        .collect();
    let defs = crate::tools::tool_sets::filter_by_const(
        policy.defs,
        crate::tools::tool_sets::WORKSPACE_READONLY_TOOLS,
    );
    if defs.is_empty() {
        return Err("审计工具不可用:任务策略未放行工作区只读工具".into());
    }

    let (configured_step_budget, semantic_guard) = super::task_loop_limits(&ctx.settings);
    // 审计自带 300s 上限;任务单步预算更小时取更小;总预算截止期再取小。
    let base = configured_step_budget.map_or(AUDIT_TOOL_BUDGET, |b| b.min(AUDIT_TOOL_BUDGET));
    let step_budget = super::effective_step_budget(Some(base), ctx.deadline);
    let params = GenerationParams {
        temperature,
        top_p: ctx.settings.default_top_p,
        max_tokens,
        stop: None,
        tools: defs,
        max_tool_rounds: Some(max_rounds),
        tool_choice: ToolChoice::Auto,
        connection_id: ctx.connection_id.clone(),
        parallel_tool_calls: None,
        step_budget,
        semantic_guard,
        response_format: None,
    };

    let session_id = format!("task:{}:{}", ctx.task_id, phase);
    let mut messages = vec![
        LlmMessage::plain("system", system),
        LlmMessage::plain("user", user),
    ];
    let mut state_machine = StateMachine::new(&session_id);
    if let Err(e) = state_machine.transition(AgentState::Executing, &session_id) {
        tracing::warn!(error = %e, session = %session_id, "审计状态迁移被拒");
    }
    let run_id = Uuid::new_v4().to_string();
    let (flag, _flag_rx) = AbortFlag::new();
    let tool_ctx = ToolContext {
        session_id: session_id.clone(),
        character_id: ctx.character_id.clone().unwrap_or_default(),
        agent_depth: 0,
        scope: Some(scope),
        // 审计是只读工具面:连续只读守卫必须关闭(否则核对几次文件就会被提醒/熔断);
        // 语义熔断沿用任务侧收紧值,防审计在同一份输出上空转。
        budget: Some(ToolBudget {
            semantic_guard,
            readonly_streak_limit: None,
            no_progress_limit: None,
        }),
    };
    let (tx, drain) = sink::spawn(svc.clone(), ctx.task_id.clone(), label, phase, None);
    // 调用追踪行的 model 必须与本次实际走的任务连接一致(不能用聊天引擎的当前模型):
    // 与 `generate_text` 同一解析入口;解析失败时循环本身也会失败,此处用引擎当前模型
    // 兜底只为错误行有可读值。
    let model = engine
        .resolve_connector(ctx.connection_id.as_deref())
        .await
        .map(|(_, m)| m)
        .unwrap_or_else(|_| engine.model());
    let mut usage = TokenUsage::default();
    // TMR3-2:截断自愈留痕由调用方持有,失败路径也能补落(不再随 Err 丢弃)
    let mut self_heals = Vec::new();
    let started = Instant::now();
    let result = run_tool_loop(
        engine,
        &mut state_machine,
        None, // 任务模式无 agent_sessions 行
        &session_id,
        &mut messages,
        &params,
        &tool_ctx,
        &tx,
        &ctx.cancel,
        &flag,
        &mut usage,
        &mut self_heals,
        &run_id,
        ToolGate::listed(&allowed),
        None,
        false, // 审计正文不流式透出(工具调用事件照常)
    )
    .await;
    drop(tx);
    let _ = drain.await;
    let elapsed = started.elapsed();

    match result {
        Ok(res) => {
            // TMR3-2:截断自愈留痕与 usage 同口径补落(审计成功后仍可回看截断细节)
            svc.record_self_heals(
                &ctx.task_id,
                phase,
                None,
                &model,
                &messages,
                &super::executor::to_self_heals(&res.self_heals),
            );
            let text = res.content.trim().to_string();
            let status = if text.is_empty() { "empty" } else { "ok" };
            let mut out = super::executor::usage_as_output(&usage);
            out.finish_reason = res.finish_reason.clone();
            svc.record_llm_call(
                &ctx.task_id,
                phase,
                None,
                &model,
                &messages,
                &text,
                Some(&out),
                elapsed,
                status,
            );
            svc.record_usage(&ctx.task_id, phase, None, &out);
            total.prompt_tokens += out.prompt_tokens;
            total.completion_tokens += out.completion_tokens;
            total.total_tokens += out.prompt_tokens + out.completion_tokens;
            total.context_tokens += usage.context_tokens;
            total.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;
            total.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;
            if text.is_empty() {
                // 工具循环跑完但正文为空:不能退回只看主 agent 自述的纯文本审计——
                // 那会丢掉刚刚读到的文件证据。把含 tool 结果的完整消息带进一次
                // **无工具收尾轮**,要求直接出 JSON;这次调用自身会落一行调用追踪。
                let mut follow_messages = messages.clone();
                follow_messages.push(LlmMessage::plain(
                    "user",
                    crate::services::task_core::prompt_consts::TEAM_AUDIT_FINAL_NUDGE,
                ));
                let follow = svc
                    .generate_text(
                        &ctx.task_id,
                        phase,
                        None,
                        follow_messages,
                        Vec::new(),
                        max_tokens,
                        temperature,
                        ctx.settings.default_top_p,
                        ctx.connection_id.as_deref(),
                        ctx.cancel.clone(),
                    )
                    .await;
                match follow {
                    Ok(follow_out) if !follow_out.text.trim().is_empty() => {
                        svc.record_usage(&ctx.task_id, phase, None, &follow_out);
                        total.prompt_tokens += follow_out.prompt_tokens;
                        total.completion_tokens += follow_out.completion_tokens;
                        total.total_tokens +=
                            follow_out.prompt_tokens + follow_out.completion_tokens;
                        let _ = state_machine.transition(AgentState::Finished, &session_id);
                        return Ok(follow_out);
                    }
                    Ok(_) => {
                        let _ = state_machine.transition(AgentState::Error, &session_id);
                        return Err(format!("{label}返回空内容(无工具收尾轮仍为空)"));
                    }
                    Err(e) => {
                        let _ = state_machine.transition(AgentState::Error, &session_id);
                        return Err(format!("{label}无工具收尾轮失败:{e}"));
                    }
                }
            }
            let _ = state_machine.transition(AgentState::Finished, &session_id);
            out.text = text;
            Ok(out)
        }
        Err(e) => {
            // TMR3-2:审计失败循环已消耗的 token 与截断自愈此前完全不入账;
            // 现在与 solo/multi 同口径:自愈行 + 错误行 + usage 三者求和一致。
            let msg = e.message().to_string();
            svc.record_self_heals(
                &ctx.task_id,
                phase,
                None,
                &model,
                &messages,
                &super::executor::to_self_heals(&self_heals),
            );
            let out = super::executor::usage_as_output(&usage);
            svc.record_llm_call(
                &ctx.task_id,
                phase,
                None,
                &model,
                &messages,
                &msg,
                Some(&out),
                elapsed,
                "error",
            );
            svc.record_usage(&ctx.task_id, phase, None, &out);
            total.prompt_tokens += out.prompt_tokens;
            total.completion_tokens += out.completion_tokens;
            total.total_tokens += out.prompt_tokens + out.completion_tokens;
            let _ = state_machine.transition(AgentState::Error, &session_id);
            Err(msg)
        }
    }
}
