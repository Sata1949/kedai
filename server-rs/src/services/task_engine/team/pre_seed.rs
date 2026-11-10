// team 单文件入口预置：确定性调用同一工具执行器，不直接读写工作区。
use super::TeamExecutor;
use crate::agents::engine::executor::{execute_tool_round, ToolGate, ToolRoundContext};
use crate::agents::engine::AbortFlag;
use crate::agents::state_machine::StateMachine;
use crate::models::types::{ExecScope, SseEvent, ToolBudget, ToolCallArgs, ToolContext};
use crate::services::task_engine::context::TaskRunContext;
use crate::services::task_engine::wiring::single_file_requested;
use crate::services::task_engine::{deadline_exhausted, sink, tool_policy};
use crate::tools::agent_tools_fs_inline::INLINE_TOOL;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

impl TeamExecutor {
    /// 预置与模型调用共用策略、逐步确认、取消、路径、所有权及变更台账。
    /// 只准备参数；分片读取和“已有入口不覆盖”均在授权后的工具内执行。
    pub(super) async fn pre_seed_single_file(
        &self,
        ctx: &TaskRunContext,
        session_id: &str,
        scope: Option<&Arc<ExecScope>>,
        declared_files: &[String],
        upstream_files: &[String],
    ) -> Option<String> {
        if !single_file_requested(&ctx.goal) || upstream_files.is_empty() {
            return None;
        }
        let scope = scope?;
        let html = declared_files.iter().rfind(|file| {
            let lower = file.to_ascii_lowercase();
            lower.ends_with(".html") || lower.ends_with(".htm")
        })?;
        if *ctx.cancel.borrow() {
            return Some("任务已停止，未执行入口预置".into());
        }
        if deadline_exhausted(ctx.deadline) {
            return Some("任务预算已耗尽，未执行入口预置".into());
        }
        let vision = match self
            .engine
            .resolve_connector(ctx.connection_id.as_deref())
            .await
        {
            Ok((connector, _)) => connector.capabilities().supports_vision,
            Err(error) => return Some(format!("入口预置未执行：{error}")),
        };
        let settings = &ctx.settings;
        let registry = self.engine.tool_registry();
        let policy = tool_policy::compile_with_authorization(
            &settings.task_tool_policy,
            &settings.task_tool_allowlist,
            &registry,
            true,
            settings.task_coding_bundle_enabled,
            vision,
            settings.vision_screenshot_enabled,
            ctx.tool_authorization,
        );
        let gate = match ctx.tool_authorization {
            Some(mode) if mode.can_wait_for_user() => ToolGate::listed_waiting(&policy.allowed),
            _ => ToolGate::listed(&policy.allowed),
        };
        let input = json!({
            "path": html,
            "sources": upstream_files,
            "title": "Kedai 单文件交付（引擎预置骨架）",
            "pre_seed": true,
            "include_three": ctx.goal.to_ascii_lowercase().contains("three"),
        });
        let call = ToolCallArgs {
            id: Uuid::new_v4().to_string(),
            name: INLINE_TOOL.into(),
            arguments: input.to_string(),
        };
        let tool_ctx = ToolContext {
            session_id: session_id.into(),
            character_id: ctx.character_id.clone().unwrap_or_default(),
            agent_depth: 0,
            scope: Some(scope.clone()),
            budget: Some(ToolBudget {
                execution: None,
                verification: None,
                input_token_budget: crate::agents::engine::executor::task_input_token_budget(
                    settings.max_context_tokens,
                    settings.tool_history_budget_tokens,
                ),
                semantic_guard: None,
                readonly_streak_limit: None,
                no_progress_limit: None,
                shard_read_nudge_after: None,
                task_authorization: ctx.tool_authorization,
            }),
        };
        let (tx, drain) = sink::spawn(
            self.svc.clone(),
            ctx.task_id.clone(),
            "入口预置",
            "agent",
            None,
        );
        let mut state_machine = StateMachine::new(session_id);
        let (flag, _flag_rx) = AbortFlag::new();
        let run_id = Uuid::new_v4().to_string();
        let mut messages = Vec::new();
        let result = async {
            tx.send(SseEvent::ToolCall {
                name: INLINE_TOOL.into(),
                input,
                call_id: Some(call.id.clone()),
                render_kind: Some("generic".into()),
                origin: Some("builtin".into()),
            })
            .await
            .map_err(|e| format!("预置工具事件发送失败:{e}"))?;
            execute_tool_round(
                ToolRoundContext {
                    engine: &self.engine,
                    state_machine: &mut state_machine,
                    agent_session: None,
                    session_id,
                    llm_messages: &mut messages,
                    tool_ctx: &tool_ctx,
                    tx: &tx,
                    abort: &ctx.cancel,
                    flag: &flag,
                    run_id: &run_id,
                    gate,
                },
                &[call],
            )
            .await
            .map_err(|e| e.to_string())
        }
        .await;
        drop(tx);
        if let Err(error) = drain.await {
            return Some(format!("入口预置事件转发失败:{error}"));
        }
        Some(match result {
            Ok(outcome) if outcome.interrupted => "任务已停止，入口预置已中断".into(),
            Ok(outcome) => {
                let output = &outcome.outputs[0].output;
                match output.get("error").and_then(|v| v.as_str()) {
                    Some(reason) => format!("入口预置未完成:{reason}"),
                    None => format!(
                        "{}；请先 fs_read 核对，再补全入口调用，不要重写整份内联。",
                        output
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| output.to_string())
                    ),
                }
            }
            Err(error) => format!("入口预置未完成:{error}"),
        })
    }
}

#[cfg(test)]
#[path = "pre_seed_tests.rs"]
mod tests;
