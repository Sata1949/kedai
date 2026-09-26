// TaskBackend 宿主侧实现(批次 B.3 依赖倒置;批次 4.2 拆窄接口):一行转调 TaskService
// 既有方法,本体零改动、零行为变化。任务引擎只依赖 task_core 的窄接口,
// 真实读写能力仍全部落在 TaskService/各子模块。
//
// 批次 4.2 变更:
// - 按 ISP 分别实现 TaskStore/TaskTrace/TaskSettings/TaskPromptKit/TaskFlowAccess/
//   TaskEvents/TaskTerminalSink/TaskGenerator(不再实现聚合别名 TaskBackend,该别名由
//   task_core 的 blanket impl 自动满足);
// - `record_self_heals` 改收中性 DTO `TruncationHeal`,不再引用 agents 层类型;
// - `agent_flow()`(返回 Arc<Mutex<AgentFlowService>>)收敛为 `resolve_task_flow()`
//   (二维批次 5a:绑定优先 → 冻结快照;未绑定 → 当时的当前流程),
//   加锁与校验在能力内部完成,不再泄漏锁纪律。
use super::{TaskGenOutput, TaskService};
use crate::models::types::{
    CharacterRecord, LlmMessage, TaskEventKind, TaskMessageRecord, TaskRecord, TaskStatus,
    TaskStep, TaskSubtaskStatus, ToolDefinition,
};
use crate::services::agent_flow_service::FlowSnapshot;
use crate::services::settings_service::RuntimeSettings;
use crate::services::task_core::{
    DeltaBatcher, TaskEvents, TaskFlowAccess, TaskGenerator, TaskPromptKit, TaskScratch,
    TaskSettings, TaskStore, TaskTerminal, TaskTerminalSink, TaskTrace, TruncationHeal,
};
use futures::future::BoxFuture;
use std::time::Duration;
use tokio::sync::watch;

// ==================== TaskStore:任务/子任务持久化读写 ====================

impl TaskStore for TaskService {
    fn get(&self, id: &str) -> Option<TaskRecord> {
        TaskService::get(self, id)
    }

    fn set_flow_snapshot(&self, id: &str, snapshot: &FlowSnapshot) -> bool {
        TaskService::set_flow_snapshot(self, id, snapshot)
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
}

// ==================== TaskTrace:调用追踪与用量留痕 ====================

impl TaskTrace for TaskService {
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
        self_heals: &[TruncationHeal],
    ) {
        TaskService::record_self_heals(
            self, task_id, phase, step_index, model, messages, self_heals,
        )
    }
}

// ==================== TaskSettings:任务模式有效设置 ====================

impl TaskSettings for TaskService {
    fn task_settings(&self) -> RuntimeSettings {
        TaskService::task_settings(self)
    }
}

// ==================== TaskPromptKit:提示词组装 ====================

impl TaskPromptKit for TaskService {
    fn assemble_executor_system_prompt(
        &self,
        settings: &RuntimeSettings,
        executor_id: Option<&str>,
        character_id: Option<&str>,
        user_goal: &str,
        has_tools: bool,
    ) -> String {
        TaskService::assemble_executor_system_prompt(
            self,
            settings,
            executor_id,
            character_id,
            user_goal,
            has_tools,
        )
    }

    fn world_context(&self, character_id: Option<&str>) -> String {
        TaskService::world_context(self, character_id)
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
}

// ==================== TaskFlowAccess:自定义 Agent 流程访问 ====================

impl TaskFlowAccess for TaskService {
    fn resolve_task_flow(&self, task: Option<&TaskRecord>) -> Result<FlowSnapshot, String> {
        // 规则(绑定优先 → 冻结快照自校验;未绑定 → 当时的当前流程)收在 TaskService
        // 的单一出处里;approve 的前置校验调的是同一个方法,避免两条分叉的判断。
        TaskService::resolve_task_flow(self, task)
    }
}

// ==================== TaskEvents:任务事件发射 ====================

impl TaskEvents for TaskService {
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
}

// ==================== TaskTerminalSink:执行终态落库 ====================

impl TaskTerminalSink for TaskService {
    fn finalize_terminal(
        &self,
        task_id: &str,
        token: u64,
        terminal: TaskTerminal,
        ended_by_cancel: bool,
    ) {
        TaskService::finalize_terminal(self, task_id, token, terminal, ended_by_cancel)
    }
}

// ==================== TaskScratch:任务临时工作区 ====================

impl TaskScratch for TaskService {
    fn scratch_dir_for(&self, task_id: &str) -> Result<std::path::PathBuf, String> {
        TaskService::scratch_dir_for(self, task_id)
    }
}

// ==================== TaskGenerator:LLM 生成与分级重试 ====================

impl TaskGenerator for TaskService {
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
        connection_id: Option<&'a str>,
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
            connection_id,
            cancel,
        ))
    }

    /// 带单次调用超时覆盖的生成(A 批 A1):宿主是唯一持有看门狗的实现,故只有这里
    /// 需要覆盖 trait 的默认委托(见 `TaskGenerator::generate_text_with_timeout`)。
    #[allow(clippy::too_many_arguments)]
    fn generate_text_with_timeout<'a>(
        &'a self,
        task_id: &'a str,
        phase: &'a str,
        step_index: Option<usize>,
        messages: Vec<LlmMessage>,
        tools: Vec<ToolDefinition>,
        max_tokens: u32,
        temperature: f64,
        top_p: f64,
        connection_id: Option<&'a str>,
        timeout: Option<Duration>,
        cancel: watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        Box::pin(TaskService::generate_text_timed(
            self,
            task_id,
            phase,
            step_index,
            messages,
            tools,
            max_tokens,
            temperature,
            top_p,
            connection_id,
            timeout,
            cancel,
        ))
    }

    fn generate_step<'a>(
        &'a self,
        task: &'a TaskRecord,
        step: &'a TaskStep,
        step_index: Option<usize>,
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        Box::pin(TaskService::generate_step(
            self, task, step, step_index, cancel,
        ))
    }

    fn generate_step_with<'a>(
        &'a self,
        task: &'a TaskRecord,
        step: &'a TaskStep,
        step_index: Option<usize>,
        max_tokens: u32,
        temperature: f64,
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        Box::pin(TaskService::generate_step_with(
            self,
            task,
            step,
            step_index,
            max_tokens,
            temperature,
            cancel,
        ))
    }

    fn summarize_task<'a>(
        &'a self,
        task: &'a TaskRecord,
        plan: &'a [TaskStep],
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        Box::pin(TaskService::summarize_task(self, task, plan, cancel))
    }

    fn summarize_task_with<'a>(
        &'a self,
        task: &'a TaskRecord,
        plan: &'a [TaskStep],
        max_tokens: u32,
        temperature: f64,
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        Box::pin(TaskService::summarize_task_with(
            self,
            task,
            plan,
            max_tokens,
            temperature,
            cancel,
        ))
    }

    fn plan_task<'a>(
        &'a self,
        task_id: &'a str,
        title: &'a str,
        character_id: Option<&'a str>,
        max_tokens: u32,
        cancel: &'a watch::Receiver<bool>,
        scope: Option<std::sync::Arc<crate::models::types::ExecScope>>,
        capability: crate::services::task_core::prompt_consts::StepCapability,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        Box::pin(TaskService::plan_task(
            self,
            task_id,
            title,
            character_id,
            max_tokens,
            cancel,
            scope,
            capability,
        ))
    }

    fn plan_revise<'a>(
        &'a self,
        task: &'a TaskRecord,
        history: &'a [TaskMessageRecord],
        feedback: &'a str,
        max_tokens: u32,
        cancel: &'a watch::Receiver<bool>,
        scope: Option<std::sync::Arc<crate::models::types::ExecScope>>,
        capability: crate::services::task_core::prompt_consts::StepCapability,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        Box::pin(TaskService::plan_revise(
            self, task, history, feedback, max_tokens, cancel, scope, capability,
        ))
    }
}
