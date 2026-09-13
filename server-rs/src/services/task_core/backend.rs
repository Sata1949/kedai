// TaskBackend 能力缝(批次 B.3 依赖倒置):任务引擎执行器对宿主能力的唯一依赖面。
// 执行器只按本接口调用「读任务/写库/发射事件/LLM 生成」等宿主能力,由 TaskService
// 一侧实现薄委托(一行转调既有方法),从而 task_engine 不再反向依赖 task_service,
// 规则 C 的环被打断。
//
// 方法签名逐字对齐 TaskService 既有方法(返回类型不得漂移,见 db.rs/events.rs/
// executor.rs):写库类保持 bool / Result 返回,异步生成类用 BoxFuture(与
// ModeExecutor 同风格,不引入新依赖)。
use crate::models::types::{
    CharacterRecord, LlmMessage, TaskEventKind, TaskRecord, TaskStatus, TaskStep,
    TaskSubtaskStatus, ToolDefinition,
};
use crate::services::agent_flow_service::AgentFlowService;
use crate::services::settings_service::RuntimeSettings;
use crate::services::task_core::{DeltaBatcher, TaskGenOutput, TaskTerminal};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// 任务引擎所需的宿主能力(由 TaskService 实现)。
pub(crate) trait TaskBackend: Send + Sync {
    // —— 读 ——

    /// 按 id 读取任务记录。
    fn get(&self, id: &str) -> Option<TaskRecord>;

    /// 任务模式合并后的有效设置快照。
    fn task_settings(&self) -> RuntimeSettings;

    /// 执行者 system 提示词组装(内置执行者指令 → 人设 → 世界书 → 提示词注入 →
    /// 用户可编辑 Agent 提示词)。
    fn assemble_executor_system_prompt(
        &self,
        settings: &RuntimeSettings,
        character_id: Option<&str>,
        user_goal: &str,
    ) -> String;

    /// 世界书常驻条目文本。
    fn world_context(&self, character_id: Option<&str>) -> String;

    /// 自定义 Agent 流程库句柄(custom 模式读取当前启用流程)。
    fn agent_flow(&self) -> Arc<Mutex<AgentFlowService>>;

    /// 渲染 Agent 系统提示词占位符。
    fn render_agent_prompt(
        &self,
        prompt: &str,
        character: Option<&CharacterRecord>,
        world_text: &str,
        user_goal: &str,
    ) -> String;

    // —— 写(返回类型照抄既有方法) ——

    /// 设置任务状态;返回是否命中并更新。
    fn set_status(&self, id: &str, status: TaskStatus) -> bool;

    /// 覆盖任务计划;返回是否命中并更新。
    fn set_plan(&self, id: &str, plan: &[TaskStep]) -> bool;

    /// 更新子任务状态(result/error 为 None 时保持原值);返回是否命中并更新。
    fn set_subtask_status(
        &self,
        id: &str,
        status: TaskSubtaskStatus,
        result: Option<&str>,
        error: Option<&str>,
    ) -> bool;

    /// 创建子任务行;失败返回错误文本。
    fn create_subtask(&self, task_id: &str, name: &str, instruction: &str)
        -> Result<String, String>;

    /// 落一次 LLM 调用的 usage。
    fn record_usage(&self, task_id: &str, phase: &str, step_index: Option<usize>, out: &TaskGenOutput);

    /// 落一次 LLM 调用追踪行。
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
    );

    /// 落截断自愈留痕(补落被截断调用行 + 补 usage)。
    fn record_self_heals(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        model: &str,
        messages: &[LlmMessage],
        self_heals: &[crate::agents::engine::executor::SelfHealRecord],
    );

    /// 发射任务事件。
    fn emit_event(
        &self,
        kind: TaskEventKind,
        task_id: &str,
        title: Option<String>,
        status: Option<TaskStatus>,
        detail: Option<String>,
    );

    /// 构造挂在任务事件广播通道上的 delta 攒批器。
    fn delta_batcher(&self, task_id: &str, phase: &str, step_index: Option<usize>) -> DeltaBatcher;

    /// 任务执行终态统一落库(按 TaskTerminal 变体分派既有写入形态)。
    fn finalize_terminal(&self, task_id: &str, token: u64, terminal: TaskTerminal, ended_by_cancel: bool);

    // —— 异步(BoxFuture,沿用 ModeExecutor 既有风格,不引入新依赖) ——

    /// 非流式生成统一出口(含调用追踪/delta 旁路落库)。
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
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>>;

    /// 生成步骤(含空输出分级重试)。
    fn generate_step_retry<'a>(
        &'a self,
        task: &'a TaskRecord,
        step: &'a TaskStep,
        step_index: Option<usize>,
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>>;

    /// 规划(含解析、截断打捞与分级重试)。
    fn plan_task_retry<'a>(
        &'a self,
        task_id: &'a str,
        title: &'a str,
        character_id: Option<&'a str>,
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<(Vec<TaskStep>, TaskGenOutput), String>>;

    /// 汇总(含空输出分级重试)。
    fn summarize_task_retry<'a>(
        &'a self,
        task: &'a TaskRecord,
        plan: &'a [TaskStep],
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>>;
}
