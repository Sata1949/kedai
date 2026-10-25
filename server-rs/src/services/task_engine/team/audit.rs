// team 审计:审计输出解析,以及审计/终审/汇总共用的纯生成出口(截断自愈)。
// 由原 `team.rs` 整段搬来(C-2D,纯搬运)。
use super::budget::{structured_budget, trunc_heal_budget_reasoning_aware};
use super::{AuditVerdict, Kickback, TeamExecutor, TEAM_JSON_TEMPERATURE};
use crate::models::types::{LlmMessage, TaskEventKind, TokenUsage};
use crate::services::task_core::{TaskBackend, TaskGenOutput};
use crate::services::task_engine::context::TaskRunContext;
use std::sync::Arc;
use tokio::sync::watch;

/// 解析审计输出:{"通过":bool,"打回":[{"main":1-based 序号,"step":主内 1-based 子目标
/// 序号(可选),"instruction":"..."}],"结论":"..."}。
/// 解析失败/截断一律按**未通过**兜底(见 AuditVerdict 文档;实跑问题 2 修复);
/// 打回序号越界的条目丢弃(防 LLM 幻觉引用不存在的主 agent);同一定位(主 + 子目标)
/// 重复打回只保留首条(防同一虚拟 session 在补做轮被并发 spawn 两次)。
pub(super) fn parse_audit(text: &str, mains_count: usize) -> AuditVerdict {
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
        // 解析失败(含推理模型烧光预算的腰斩 JSON):审计质量闸门读不出明确通过,
        // 按未通过处理;结论段给干净说明——半截 JSON 原样进 result「## 审计结论」
        // 卡既难看又误导;原文记 warn 字段备查
        tracing::warn!(
            head = stripped.chars().take(120).collect::<String>(),
            "team 审计输出无法解析,按未通过兜底"
        );
        let conclusion = if stripped.starts_with('{') {
            "(审计输出不完整,无法确认通过)".to_string()
        } else {
            t.to_string()
        };
        return AuditVerdict {
            pass: false,
            kickbacks: Vec::new(),
            conclusion,
        };
    };
    let pass = v.get("通过").and_then(|b| b.as_bool()).unwrap_or(false);
    let conclusion = v
        .get("结论")
        .and_then(|s| s.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        // JSON 合法但缺「结论」:回填固定文案,不把裸 JSON(整段原文是 `{"通过":…}`)
        // 灌进 result 的「## 审计结论」卡——那既难看又误导(实跑问题 2 连带给修复)
        .unwrap_or_else(|| "(审计未给出结论)".to_string());
    let mut kickbacks: Vec<Kickback> = Vec::new();
    if let Some(arr) = v.get("打回").and_then(|a| a.as_array()) {
        for item in arr {
            let main = item.get("main").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
            // step 为可选的主内 1-based 子目标序号;缺省 = 整主补做(兼容旧输出)
            let step = item
                .get("step")
                .and_then(|n| n.as_u64())
                .map(|n| n as usize);
            let instruction = item
                .get("instruction")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if main >= 1 && main <= mains_count && !instruction.is_empty() {
                let idx = main - 1;
                // step 越界(0)视为未指明 → 整主补做,不错失问题上报;先归一到 0-based
                // 再做去重比较(否则 1-based 与已存 0-based 不一致,去重失效)
                let step0 = step.filter(|s| *s >= 1).map(|s| s - 1);
                // 同一定位(主 + 子目标)重复打回只保留首条(补做轮每定位一个 spawn)
                if kickbacks.iter().any(|k| k.main == idx && k.step == step0) {
                    tracing::warn!(main = main, "team 审计打回条目与既有条目同定位,已保留首条");
                    continue;
                }
                kickbacks.push(Kickback {
                    main: idx,
                    step: step0,
                    instruction,
                });
            } else {
                tracing::warn!(
                    main = main,
                    "team 审计打回条目无效(序号越界或指令为空),已丢弃"
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

/// 审计/终审/汇总共用的纯生成出口:先按原预算调 generate_text,finish=length
/// 时按「翻倍(不低于下限)+ 推理感知」重发一次(截断自愈)。两次调用均经
/// generate_text 落 task_llm_calls,调用情况面板可见「截断 → 提高预算重发」
/// 完整链路;token 用量两次都入 total,record_usage 两次都落(与 solo 自愈留痕同口径)。
#[allow(clippy::too_many_arguments)]
pub(super) async fn generate_text_healed(
    svc: &Arc<dyn TaskBackend>,
    task_id: &str,
    phase: &str,
    messages: Vec<LlmMessage>,
    max_tokens: u32,
    temperature: f64,
    top_p: f64,
    cancel: &watch::Receiver<bool>,
    total: &mut TokenUsage,
) -> Result<TaskGenOutput, String> {
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
            None, // team 模式无节点级连接
            cancel.clone(),
        )
        .await?;
    svc.record_usage(task_id, phase, None, &out);
    total.prompt_tokens += out.prompt_tokens;
    total.completion_tokens += out.completion_tokens;
    total.total_tokens += out.prompt_tokens + out.completion_tokens;
    let Some(retry_budget) = trunc_heal_budget_reasoning_aware(
        out.finish_reason.as_deref(),
        max_tokens,
        out.completion_tokens,
        out.reasoning_tokens,
    ) else {
        return Ok(out);
    };
    tracing::warn!(
        phase = phase,
        max_tokens = max_tokens,
        reasoning_tokens = out.reasoning_tokens,
        completion_tokens = out.completion_tokens,
        retry_max_tokens = retry_budget,
        "team 纯生成调用截断,提高输出上限重发"
    );
    svc.emit_event(
        TaskEventKind::AgentStatus,
        task_id,
        None,
        None,
        Some(format!(
            "(截断自愈){phase} 输出截断,输出上限提高至 {retry_budget} 重发"
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
            None, // team 模式无节点级连接
            cancel.clone(),
        )
        .await?;
    svc.record_usage(task_id, phase, None, &out);
    total.prompt_tokens += out.prompt_tokens;
    total.completion_tokens += out.completion_tokens;
    total.total_tokens += out.prompt_tokens + out.completion_tokens;
    Ok(out)
}

impl TeamExecutor {
    /// 审计/终审共用的结构化阶段出口(C-2D 去重):先走只读工作区工具通道,
    /// 不可用时回退纯文本生成(带截断自愈)。取消保持 Err(由引擎按 ended 收尾);
    /// 其它失败也返回 Err,由调用方决定 `fallback_terminal` 的文案(审计/终审不同)。
    ///
    /// `short` 只用于回退日志的措辞(「审计」/「终审」),`label` 是调用追踪/事件里的
    /// 「审计 agent」/「终审 agent」。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_audit_phase(
        &self,
        ctx: &TaskRunContext,
        phase: &'static str,
        label: &str,
        short: &str,
        system: &str,
        input: &str,
        max_rounds: u32,
        total: &mut TokenUsage,
    ) -> Result<TaskGenOutput, String> {
        match crate::services::task_engine::team_audit::generate_with_readonly_tools(
            &self.svc,
            &self.engine,
            ctx,
            phase,
            label,
            system,
            input,
            max_rounds,
            structured_budget(ctx.settings.default_max_tokens),
            TEAM_JSON_TEMPERATURE,
            total,
        )
        .await
        {
            Ok(o) => Ok(o),
            // 取消:保持 Err,由引擎按 ended 收尾(终态兜底从库里的 plan 补写已完成产出)
            Err(e) if *ctx.cancel.borrow() => Err(e),
            Err(tool_err) => {
                tracing::warn!(
                    error = tool_err.as_str(),
                    "team {short}只读工具通道不可用,回退纯文本{short}"
                );
                let messages = vec![
                    LlmMessage::plain("system", system),
                    LlmMessage::plain("user", input),
                ];
                // 截断自愈:审计/终审是结构化 JSON 输出,推理模型烧光预算会腰斩 JSON(实测)
                generate_text_healed(
                    &self.svc,
                    &ctx.task_id,
                    phase,
                    messages,
                    structured_budget(ctx.settings.default_max_tokens),
                    TEAM_JSON_TEMPERATURE,
                    ctx.settings.default_top_p,
                    &ctx.cancel,
                    total,
                )
                .await
            }
        }
    }
}
