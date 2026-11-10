// team 审计/终审的**只读工具**通道(TMR-2,2026-10-19)。
//
// 背景:team 的规划/审计/终审/汇总此前全部走 `generate_text + Vec::new()`,审计员只能
// 读主 agent 的自述,无法核对工作区里的文件是否存在、内容是否一致。实测里主 3 的
// 「拒绝伪造」是模型自觉的兜底,不是系统能力。
//
// 审计只消费文件、版本绑定截图与已执行命令证据，不下发 bash 或写工具。
// 工作区工具面仍与任务授权取交；只读工具不可用时沿用既有纯文本审计入口。
use super::context::TaskRunContext;
use super::sink;
use crate::agents::engine::executor::{run_tool_loop, ToolGate, ToolLoopRequest};
use crate::agents::engine::{AbortFlag, AgentEngine};
use crate::agents::state_machine::{AgentState, StateMachine};
use crate::models::types::{
    GenerationParams, LlmMessage, TokenUsage, ToolBudget, ToolChoice, ToolContext,
};
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
    // 工具面：任务策略编译后只保留文件、图像及命令证据读取——即使策略放全，
    // 审计也拿不到写工具/bash;白名单是硬边界(ToolGate::listed)。
    let policy = super::tool_policy::compile(
        &ctx.settings.task_tool_policy,
        &ctx.settings.task_tool_allowlist,
        &engine.tool_registry(),
        true,
        ctx.settings.task_coding_bundle_enabled,
        engine
            .resolve_connector(ctx.connection_id.as_deref())
            .await?
            .0
            .capabilities()
            .supports_vision,
        ctx.settings.vision_screenshot_enabled,
    );
    let defs: Vec<_> = policy
        .defs
        .into_iter()
        .filter(|d| {
            crate::tools::tool_sets::WORKSPACE_READONLY_TOOLS.contains(&d.name.as_str())
                || matches!(
                    d.name.as_str(),
                    "exec_log_read" | "view_image" | "zoom_image" | "image_diff"
                )
        })
        .collect();
    let allowed: Vec<_> = defs.iter().map(|d| d.name.clone()).collect();
    if defs.is_empty() {
        return Err("审计工具不可用:任务策略未放行工作区只读工具".into());
    }

    let (configured_step_budget, semantic_guard) = super::task_loop_limits(&ctx.settings);
    // SKY-6 第 4 项:审计/终审的工具面只有工作区只读三件,HB-2 与「一轮并发多次读」
    // 天然冲突——2026-10-23 复跑里终审在 fs_read 上 8 次调用 / 2 种输出即被熔断
    // (seq 1298),结论靠无工具收尾轮兜底。本路径已有 AUDIT_MAX_ROUNDS /
    // FINAL_AUDIT_MAX_ROUNDS 与 300s 墙钟兜底,故把语义熔断置**关闭位**。
    let semantic_guard = audit_semantic_guard(semantic_guard);
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
    let record = super::verification::load_current(
        &super::verification::evidence_dir(&svc.scratch_dir_for(&ctx.task_id)?),
        scope.workspace(),
        &ctx.goal,
    );
    let mut evidence = LlmMessage::plain(
        "user",
        &crate::services::prompt_kit::untrusted_boundary_task(
            "运行环境采集的版本绑定验收证据",
            &serde_json::to_string(&record).map_err(|e| e.to_string())?,
        ),
    );
    if engine
        .resolve_connector(ctx.connection_id.as_deref())
        .await?
        .0
        .capabilities()
        .supports_vision
    {
        for mut image in record.screenshots {
            image.data_url = engine.image_service().resolve_data_url(&image.id)?;
            evidence.images.push(image);
        }
    } else if !record.screenshots.is_empty() {
        evidence
            .content
            .push_str("\n本审计连接没有视觉输入能力，视觉效果待验。");
    }
    messages.push(evidence);
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
        // 审计是只读工具面:连续只读与无进展两个计数闸门必须关闭(否则核对几次文件
        // 就会被提醒/熔断);语义熔断同样关闭——只读面下「重复读」不是空转信号,
        // 由上面的轮次上限与 300s 墙钟兜底(SKY-6 第 4 项)。
        budget: Some(ToolBudget {
            execution: None,
            verification: None,
            input_token_budget: crate::agents::engine::executor::task_input_token_budget(
                ctx.settings.max_context_tokens,
                ctx.settings.tool_history_budget_tokens,
            ),
            semantic_guard,
            readonly_streak_limit: None,
            no_progress_limit: None,
            // 审计/终审不是集成主：无分片读取软提醒。
            shard_read_nudge_after: None,
            // 审计/终审只跑只读工具（Step 不会拦只读，Workspace 不涉及命令执行）；
            // 仍原样带着档位，避免未来审计工具面扩张时漏判。
            task_authorization: ctx.tool_authorization,
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
    let result = run_tool_loop(ToolLoopRequest {
        engine,
        state_machine: &mut state_machine,
        agent_session: None, // 任务模式无 agent_sessions 行
        session_id: &session_id,
        llm_messages: &mut messages,
        params: &params,
        tool_ctx: &tool_ctx,
        tx: &tx,
        abort: &ctx.cancel,
        flag: &flag,
        total_usage: &mut usage,
        self_heals: &mut self_heals,
        run_id: &run_id,
        gate: ToolGate::listed(&allowed),
        call_timeout: None,
        emit_tokens: false, // 审计正文不流式透出(工具调用事件照常)
        // 但思考与工具轨迹要留(TMSHOW-1):这正是「不要正文、要思考」的组合场景
        emit_reasoning: true,
    })
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

/// 审计/终审的语义熔断配置(SKY-6 第 4 项):保留窗口/去重上限,把同工具次数下限
/// 置 0(= 关闭位)。**不能**用 `None`:引擎侧 `GenerationParams.semantic_guard` 的
/// `None` 语义是「回退到聊天侧扁平设置」,不是关闭(`agents/engine/executor.rs`)。
fn audit_semantic_guard(guard: Option<(usize, usize, usize)>) -> Option<(usize, usize, usize)> {
    guard.map(|(window, _min_calls, max_distinct)| (window, 0, max_distinct))
}

#[cfg(test)]
mod tests {
    use super::audit_semantic_guard;

    /// SKY-6 第 4 项:审计/终审必须落到 min_calls=0 的关闭位;`None` 会被引擎
    /// 回退成聊天侧扁平值(默认 16/12/2),不是关闭。
    #[test]
    fn audit_semantic_guard_uses_off_sentinel() {
        assert_eq!(audit_semantic_guard(Some((8, 4, 2))), Some((8, 0, 2)));
        assert_eq!(audit_semantic_guard(None), None, "None 无源可关,原样透传");
    }
}
