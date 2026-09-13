// 模式执行器抽象:每种任务模式一个实现,共享 TaskRunContext 输入。
// 风格对齐 tools 的 ToolExecutor(BoxFuture);能力缝雏形(docs/任务引擎六模式.md 第六节)。
// 产出为「终态值 + 整轮 token 累计」:执行器只描述到达什么终态(task_core::TaskTerminal),
// 落库由 TaskService::finalize_terminal 统一消费(批次 B 依赖倒置,规则 C 断环)。
use super::context::TaskRunContext;
use crate::models::types::TokenUsage;
use crate::services::task_core::{TaskGenOutput, TaskTerminal};
use futures::future::BoxFuture;

/// 模式执行器(solo/plan/multi/team/custom/legacy/followup 共用同一缝)。
/// `Ok((terminal, usage))` = 已到达终态(usage 仅供完成日志);`Err` = 引擎兜底失败收尾。
pub(crate) trait ModeExecutor: Send + Sync {
    fn run<'a>(
        &'a self,
        ctx: TaskRunContext,
    ) -> BoxFuture<'a, Result<(TaskTerminal, TokenUsage), String>>;
}

/// 由整轮累计 TokenUsage 构造 record_usage 入参(六模式执行器统一口径;
/// reasoning 无分项观测记 0,详情以 task_llm_calls 行为准)。
pub(crate) fn usage_as_output(usage: &TokenUsage) -> TaskGenOutput {
    TaskGenOutput {
        text: String::new(),
        finish_reason: None,
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        reasoning_tokens: 0,
        reasoning_chars: 0,
        tool_calls: Vec::new(),
    }
}
