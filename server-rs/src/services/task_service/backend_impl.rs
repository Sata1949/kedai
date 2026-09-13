// TaskBackend 宿主侧实现(批次 B.3 依赖倒置):一行转调 TaskService 既有方法,
// 本体零改动、零行为变化。任务引擎只依赖 task_core::TaskBackend 接口,
// 真实读写能力仍全部落在 TaskService/各子模块。
use super::executor::{generate_step_retry, plan_task_retry, summarize_task_retry};
use super::{TaskGenOutput, TaskService};
use crate::models::types::{
    CharacterRecord, LlmMessage, TaskEventKind, TaskRecord, TaskStatus, TaskStep,
    TaskSubtaskStatus, ToolDefinition,
};
use crate::services::agent_flow_service::AgentFlowService;
use crate::services::settings_service::RuntimeSettings;
use crate::services::task_core::{DeltaBatcher, TaskBackend, TaskTerminal};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

impl TaskBackend for TaskService {
    fn get(&self, id: &str) -> Option<TaskRecord> {
        TaskService::get(self, id)
    }

    fn task_settings(&self) -> RuntimeSettings {
        TaskService::task_settings(self)
    }

    fn assemble_executor_system_prompt(
        &self,
        settings: &RuntimeSettings,
        character_id: Option<&str>,
        user_goal: &str,
    ) -> String {
        TaskService::assemble_executor_system_prompt(self, settings, character_id, user_goal)
    }

    fn world_context(&self, character_id: Option<&str>) -> String {
        TaskService::world_context(self, character_id)
    }

    fn agent_flow(&self) -> Arc<Mutex<AgentFlowService>> {
        TaskService::agent_flow(self)
    }

    fn render_agent_prompt(
        &self,
        prompt: &str,
        character: Option<&CharacterRecord>,
        world_text: &str,
        user_goal: &str,
    ) -> String {
        TaskService::render_agent_prompt(self, prompt, character, world_text, user_goal)
    }

    fn set_status(&self, id: &str, status: TaskStatus) -> bool {
        TaskService::set_status(self, id, status)
    }

    fn set_plan(&self, id: &str, plan: &[TaskStep]) -> bool {
        TaskService::set_plan(self, id, plan)
    }

    fn set_subtask_status(
        &self,
        id: &str,
        status: TaskSubtaskStatus,
        result: Option<&str>,
        error: Option<&str>,
    ) -> bool {
        TaskService::set_subtask_status(self, id, status, result, error)
    }

    fn create_subtask(
        &self,
        task_id: &str,
        name: &str,
        instruction: &str,
    ) -> Result<String, String> {
        TaskService::create_subtask(self, task_id, name, instruction)
    }

    fn record_usage(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        out: &TaskGenOutput,
    ) {
        TaskService::record_usage(self, task_id, phase, step_index, out)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_llm_call(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        model: &str,
        messages: &[LlmMessage],
        response: &str,
        out: Option<&TaskGenOutput>,
        elapsed: Duration,
        status: &str,
    ) {
        TaskService::record_llm_call(
            self, task_id, phase, step_index, model, messages, response, out, elapsed, status,
        )
    }

    fn record_self_heals(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        model: &str,
        messages: &[LlmMessage],
        self_heals: &[crate::agents::engine::executor::SelfHealRecord],
    ) {
        TaskService::record_self_heals(
            self, task_id, phase, step_index, model, messages, self_heals,
        )
    }

    fn emit_event(
        &self,
        kind: TaskEventKind,
        task_id: &str,
        title: Option<String>,
        status: Option<TaskStatus>,
        detail: Option<String>,
    ) {
        TaskService::emit_event(self, kind, task_id, title, status, detail)
    }

    fn delta_batcher(&self, task_id: &str, phase: &str, step_index: Option<usize>) -> DeltaBatcher {
        TaskService::delta_batcher(self, task_id, phase, step_index)
    }

    fn finalize_terminal(
        &self,
        task_id: &str,
        token: u64,
        terminal: TaskTerminal,
        ended_by_cancel: bool,
    ) {
        TaskService::finalize_terminal(self, task_id, token, terminal, ended_by_cancel)
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_text<'a>(
        &'a self,
        task_id: &'a str,
        phase: &'a str,
        step_index: Option<usize>,
        messages: Vec<LlmMessage>,
        tools: Vec<ToolDefinition>,
        max_tokens: u32,
        temperature: f64,
        top_p: f64,
        cancel: watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        Box::pin(TaskService::generate_text(
            self,
            task_id,
            phase,
            step_index,
            messages,
            tools,
            max_tokens,
            temperature,
            top_p,
            cancel,
        ))
    }

    fn generate_step_retry<'a>(
        &'a self,
        task: &'a TaskRecord,
        step: &'a TaskStep,
        step_index: Option<usize>,
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        Box::pin(generate_step_retry(self, task, step, step_index, cancel))
    }

    fn plan_task_retry<'a>(
        &'a self,
        task_id: &'a str,
        title: &'a str,
        character_id: Option<&'a str>,
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<(Vec<TaskStep>, TaskGenOutput), String>> {
        Box::pin(plan_task_retry(self, task_id, title, character_id, cancel))
    }

    fn summarize_task_retry<'a>(
        &'a self,
        task: &'a TaskRecord,
        plan: &'a [TaskStep],
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        Box::pin(summarize_task_retry(self, task, plan, cancel))
    }
}
