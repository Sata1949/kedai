//! 任务阶段压缩的模型调用边界；所有失败都保留原历史，不能回落到盲裁剪。
use super::context_stage::TaskContext;
use crate::agents::engine::executor::{add_usage, execute_generation, ExecutorResult};
use crate::agents::engine::util::{send_event, step_evt};
use crate::agents::engine::{AbortFlag, AgentEngine, EngineError};
use crate::models::types::{
    GenerationParams, LlmMessage, ResponseFormat, SseEvent, TokenUsage, ToolChoice,
};
use std::future::Future;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

pub(super) struct TaskContextRuntime {
    context: TaskContext,
    supports_vision: bool,
    supports_structured_output: bool,
}

pub(super) struct CompactRequest<'a> {
    pub engine: &'a AgentEngine,
    pub session_id: &'a str,
    pub run_id: &'a str,
    pub messages: &'a mut Vec<LlmMessage>,
    pub params: &'a GenerationParams,
    pub tx: &'a mpsc::Sender<SseEvent>,
    pub abort: &'a watch::Receiver<bool>,
    pub flag: &'a AbortFlag,
    pub call_watchdog: Option<Duration>,
    pub remaining_budget: Option<Duration>,
    pub total_usage: &'a mut TokenUsage,
}

async fn await_summary_call(
    call: impl Future<Output = Result<ExecutorResult, EngineError>>,
    timeout: Option<Duration>,
    abort: &watch::Receiver<bool>,
) -> Result<ExecutorResult, EngineError> {
    let call = async {
        match timeout {
            Some(limit) => tokio::time::timeout(limit, call).await.map_err(|_| {
                EngineError::from(crate::models::llm_error::LlmError::timeout(format!(
                    "任务阶段摘要超过 {} 秒，历史保持原样。",
                    limit.as_secs_f64()
                )))
            })?,
            None => call.await,
        }
    };
    super::round::await_or_cancel(call, abort)
        .await
        .ok_or(EngineError::Interrupted)?
}

fn account_summary_call(
    outcome: Result<ExecutorResult, EngineError>,
    live_usage: &TokenUsage,
    total_usage: &mut TokenUsage,
) -> Result<ExecutorResult, EngineError> {
    match outcome {
        Ok(result) => {
            add_usage(total_usage, &result.usage);
            Ok(result)
        }
        Err(error) => {
            add_usage(total_usage, live_usage);
            Err(error)
        }
    }
}

fn summary_params(params: &GenerationParams, structured_output: bool) -> GenerationParams {
    let mut summary = params.clone();
    summary.tools.clear();
    summary.tool_choice = ToolChoice::None;
    summary.parallel_tool_calls = None;
    summary.temperature = 0.0;
    summary.response_format = structured_output.then_some(ResponseFormat::Json);
    // 输出额度沿用调用方；推理模型需要把推理与可见 JSON 都纳入 max_tokens。
    summary
}

impl TaskContextRuntime {
    pub(super) async fn new(
        engine: &AgentEngine,
        params: &GenerationParams,
        budget: u32,
    ) -> Result<Self, EngineError> {
        let (connector, model) = engine
            .resolve_connector(params.connection_id.as_deref())
            .await?;
        let capabilities = connector.capabilities();
        let context = {
            let mut ts = engine
                .token_service
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            TaskContext::new(model, budget, &params.tools, &mut ts)
        };
        Ok(Self {
            context,
            supports_vision: capabilities.supports_vision,
            supports_structured_output: capabilities.supports_structured_output,
        })
    }

    pub(super) async fn compact(&self, req: CompactRequest<'_>) -> Result<(), EngineError> {
        if !self.supports_vision && req.messages.iter().any(|m| !m.images.is_empty()) {
            return Err(
                "任务选择的连接不支持图像输入；不能把图像当作已验证文字继续执行。"
                    .to_string()
                    .into(),
            );
        }
        let stage = {
            let mut ts = req
                .engine
                .token_service
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            self.context.prepare(req.messages, &mut ts)?
        };
        let Some(stage) = stage else {
            return Ok(());
        };
        send_event(step_evt("任务上下文阶段压缩", Some("保留原始目标、最新完整工具轮与错误证据；旧图像保留持久引用。输入预算计文本、工具定义及图像保守余量。".into()), None, None), req.tx, req.abort, req.flag).await?;
        let params = summary_params(req.params, self.supports_structured_output);
        // live_usage 的所有者在 timeout/cancel future 外；中止流时已收到的用量不能丢失。
        let mut live_usage = TokenUsage::default();
        let timeout = match (req.call_watchdog, req.remaining_budget) {
            (Some(call), Some(remaining)) => Some(call.min(remaining)),
            (call, remaining) => call.or(remaining),
        };
        let call = execute_generation(
            req.engine,
            req.session_id,
            req.run_id,
            &stage.request,
            &params,
            req.tx,
            req.abort,
            req.flag,
            false,
            false,
            Some(&mut live_usage),
        );
        let outcome = await_summary_call(call, timeout, req.abort).await;
        let result = account_summary_call(outcome, &live_usage, req.total_usage)?;
        if result.interrupted {
            return Err(EngineError::Interrupted);
        }
        if !result.tool_calls.is_empty() || result.finish_reason.as_deref() == Some("length") {
            return Err(
                "任务阶段摘要没有完整返回 JSON（截断或仍请求工具）；历史未替换。"
                    .to_string()
                    .into(),
            );
        }
        let candidate = {
            let mut ts = req
                .engine
                .token_service
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            self.context.finish(stage, &result.content, &mut ts)?
        };
        *req.messages = candidate;
        Ok(())
    }
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
