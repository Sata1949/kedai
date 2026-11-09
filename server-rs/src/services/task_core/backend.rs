// 任务模式宿主能力缝(批次 B.3 依赖倒置;批次 4.2 按 ISP 拆窄接口)。
//
// 任务引擎执行器只按这些**窄接口**调用宿主能力,由 TaskService 一侧实现薄委托
// (一行转调既有方法),从而 task_engine 不再反向依赖 task_service,规则 C 的环被打断。
//
// 拆分原则(批次 4.2):原 `TaskBackend` 是 20 方法的上帝接口,把「读任务/写库/追踪/
// 提示词/流程/事件/终态/生成重试」全部混在一起;消费方按实际所需依赖对应能力面即可。
// `TaskBackend` 保留为**聚合别名**(blanket impl:满足全部窄接口即自动实现),
// 使既有 `Arc<dyn TaskBackend>` 构造点零改动;新代码应优先依赖窄接口。
//
// 方法签名逐字对齐 TaskService 既有方法(返回类型不得漂移,见 db.rs/events.rs/
// executor.rs):写库类保持 bool / Result 返回,异步生成类用 BoxFuture(与
// ModeExecutor 同风格,不引入新依赖)。
//
// ## 读代码须知:这些窄接口的存在理由(2026-09-14 明确,防误读)
//
// **存在理由是「断开 task_engine → task_service 的反向依赖」,不是「为将来多实现预留」。**
// 当前 8 个窄接口**各自只有 1 个实现**(全在 `task_service/backend_impl.rs`),
// 经 blanket `TaskBackend` 以 `Arc<dyn TaskBackend>` 消费——即就「多态」而言,
// 它是一层纯转发,没有第二个实现者,也**不建议**为「万一将来换实现」而继续细拆。
//
// 它的价值是**结构性的**:把「引擎需要宿主提供什么」写成不可绕过的清单,
// 使规则 C(L1 边界门禁)可机器验证,并让引擎与宿主可以独立演进/测试。
// 对比:`ModeExecutor`(task_engine/executor.rs)有 8 个**真实**实现(solo/multi/plan/
// team/custom/legacy/followup),那一处的 trait 拆分才兼具「多态」价值。
//
// 结论:**新增窄接口前先问「是否真的多了一个实现者或一条要断的依赖」**;
// 两个都不是,就不要拆(trait 数量本身不是架构收益)。
use crate::models::types::{
    CharacterRecord, LlmMessage, TaskEventKind, TaskMessageRecord, TaskRecord, TaskStatus,
    TaskStep, TaskSubtaskStatus, ToolDefinition,
};
use crate::services::agent_flow_service::FlowSnapshot;
use crate::services::settings_service::RuntimeSettings;
use crate::services::task_core::{DeltaBatcher, TaskGenOutput, TaskTerminal, TruncationHeal};
use futures::future::BoxFuture;
use std::time::Duration;
use tokio::sync::watch;

// ==================== 窄接口(1/9):任务/子任务持久化读写 ====================

/// 任务与子任务的持久化读写(task 主表 + task_subtasks 表)。
pub(crate) trait TaskStore: Send + Sync {
    /// 按 id 读取任务记录。
    fn get(&self, id: &str) -> Option<TaskRecord>;

    /// 覆写任务的流程快照(未绑定任务在**执行开始时**记录本轮实际编排);
    /// 返回是否命中并更新。读侧不经本接口:执行器要的是**解析后**的快照
    /// (绑定优先 → 冻结快照;否则当时的当前流程),那由 `TaskFlowAccess` 单一提供。
    fn set_flow_snapshot(&self, id: &str, snapshot: &FlowSnapshot) -> bool;

    /// 仅当前未取消 token 可写运行期自检问题(空切片 = 清空);返回是否更新。
    /// 详情侧读同一列由宿主 `TaskService::runtime_issues` 提供,引擎只需写。
    fn set_runtime_issues(&self, id: &str, token: u64, issues: &[String]) -> bool;

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
    fn create_subtask(
        &self,
        task_id: &str,
        name: &str,
        instruction: &str,
    ) -> Result<String, String>;

    /// 某任务累计的非回滚文件变更行数(TMR-3 文件交付门槛)。
    ///
    /// 语义:任务台账(`task_file_changes`)里 `op != 'rollback'` 的行数;任务终身累计,
    /// 调用方在执行前后各取一次、比较差值来判定**本次执行**有没有产出文件。
    /// 查询失败返回 0(失败方向 = 判无产物,宁可 partial 也不伪装 done)。
    fn file_change_count(&self, task_id: &str) -> usize;
}

// ==================== 窄接口(2/9):调用追踪与用量留痕 ====================

/// LLM 调用追踪(task_llm_calls)与用量(task_usage)留痕。
pub(crate) trait TaskTrace: Send + Sync {
    /// 落一次 LLM 调用的 usage。
    fn record_usage(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        out: &TaskGenOutput,
    );

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
    /// 入参为中性 DTO(`TruncationHeal`),不再引用 agents 层 SelfHealRecord(批次 4.2)。
    fn record_self_heals(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        model: &str,
        messages: &[LlmMessage],
        self_heals: &[TruncationHeal],
    );
}

// ==================== 窄接口(3/9):任务模式有效设置 ====================

/// 任务模式合并后的有效设置快照读取。
pub(crate) trait TaskSettings: Send + Sync {
    /// 任务模式合并后的有效设置快照。
    fn task_settings(&self) -> RuntimeSettings;
}

// ==================== 窄接口(4/9):提示词组装 ====================

/// 执行者提示词组装能力(内置指令/执行者身份/世界书/注入/Agent 提示词)。
pub(crate) trait TaskPromptKit: Send + Sync {
    /// 执行者 system 提示词组装(内置执行者指令 → 执行者身份段 → 世界书 → 提示词注入 →
    /// 用户可编辑 Agent 提示词)。
    ///
    /// 两个身份入参不是「二选一必填」而是**优先级**:`executor_id` 命中执行者库时
    /// 独占身份段(角色卡不参与),否则回退 `character_id` 的旧角色卡路径——
    /// 后者保留仅为旧任务零回归,新任务不再写 character_id。
    ///
    /// `has_tools`(提交 3 · D3-c):本轮执行者**实际**有没有下发工具(调用方按策略
    /// 编译结果算,策略收窄到空集时为 false)。true 时在内置指令后追加
    /// [`crate::services::task_core::prompt_consts::EXECUTOR_TOOL_DISCIPLINE`];
    /// legacy 的步骤没有工具(传 false),不追加——那条纪律讲的是怎么用工具。
    /// `has_write_tools`(TMR-1,2026-10-19):本轮工具面是否含工作区写工具
    /// (`fs_write`/`fs_edit`/`fs_patch`)——true 时追加
    /// [`crate::services::task_core::prompt_consts::EXECUTOR_WRITE_FIRST_DISCIPLINE`],
    /// 把「产出文件类任务先落盘」的优先级压进 system;只读策略/无工作区不追加。
    /// `has_image_tools`(视觉能力包 D4;修复批次并含截图):本轮工具面是否含任一
    /// 图像工具(`tool_sets::is_image_tool`:视觉三件 ∪ 截图)——true 时再追加
    /// [`crate::services::task_core::prompt_consts::VISION_VERIFY_DISCIPLINE`]。
    #[allow(clippy::too_many_arguments)]
    fn assemble_executor_system_prompt(
        &self,
        settings: &RuntimeSettings,
        executor_id: Option<&str>,
        character_id: Option<&str>,
        user_goal: &str,
        has_tools: bool,
        has_write_tools: bool,
        has_image_tools: bool,
    ) -> String;

    /// 世界书常驻条目文本。
    fn world_context(&self, character_id: Option<&str>) -> String;

    /// 渲染 Agent 系统提示词占位符。
    fn render_agent_prompt(
        &self,
        prompt: &str,
        character: Option<&CharacterRecord>,
        world_text: &str,
        user_goal: &str,
    ) -> String;

    /// 执行者「建议温度」(TM-GEN-1,2026-10-01):`Some` = 执行者库命中且显式配置了温度,
    /// 调用方以它优先于任务有效缺省温度;`None` = 无执行者/未配置/已删除(沿用缺省)。
    /// 与 [`Self::assemble_executor_system_prompt`] 同一消费面:只有注入执行者身份段的
    /// 执行路径(solo/plan 续跑/team 主 agent/followup 经 `run_agent_loop`;legacy/plan
    /// 步骤经 `generate_step`)才消费。前端占位「留空 = 沿用任务模式默认温度」即此语义。
    fn executor_temperature(&self, executor_id: Option<&str>) -> Option<f64>;
}

// ==================== 窄接口(5/9):自定义 Agent 流程访问 ====================

/// 自定义 Agent 流程库访问(custom 模式读取本轮要跑的流程)。
///
/// 批次 4.2 起不再返回 `Arc<Mutex<AgentFlowService>>`(那会把锁纪律泄漏给调用方,
/// 逼迫 task_engine 处理 `lock().unwrap()`):改由宿主侧在能力内部完成「加锁 →
/// 取流程 → 启用校验 → 注册工具集校验」,只把结果快照交给调用方。
/// 二维批次 5a 起返回的是**流程快照**(入口 + 可达子流程闭包),不再返回单个配置:
/// 运行期子图解析只能在这份闭包内进行,否则改一个被挂载的子流程仍会改变已冻结任务的行为。
pub(crate) trait TaskFlowAccess: Send + Sync {
    /// 解析本任务**本轮要跑**的流程快照:
    /// 绑定任务取其创建时冻结的快照(**不看流程库**;快照缺失即报错,不静默回退),
    /// 未绑定任务按当时的当前流程解析(要求存在且启用,与旧 `current_flow` 语义一致)。
    ///
    /// `task` 为 None(任务行已消失)时按「未绑定」处理:此时绑定信息无从谈起,
    /// 后续的写库/收尾自会失败并收口。
    fn resolve_task_flow(&self, task: Option<&TaskRecord>) -> Result<FlowSnapshot, String>;
}

// ==================== 窄接口(6/9):任务事件发射 ====================

/// 任务事件广播发射能力。
pub(crate) trait TaskEvents: Send + Sync {
    /// 发射任务事件。
    fn emit_event(
        &self,
        kind: TaskEventKind,
        task_id: &str,
        title: Option<String>,
        status: Option<TaskStatus>,
        detail: Option<String>,
    );

    /// 广播「工具待确认」任务事件(2026-11-06 三档授权;瞬态,不落库)。
    ///
    /// 引擎桥把 `SseEvent::ToolAuthorizationRequired` 翻译成本事件——任务模式没有聊天
    /// SSE 客户端,不翻译的话前端只能看到一行 `agent_status` 文案,无从点确认。
    fn emit_tool_authorization_required(
        &self,
        task_id: &str,
        auth: crate::models::types::TaskToolAuthorization,
    );

    /// 广播「待确认已收束」任务事件(2026-11-06;瞬态,不落库)。
    fn emit_tool_authorization_resolved(
        &self,
        task_id: &str,
        decision: crate::models::types::TaskToolAuthorizationDecision,
    );

    /// 构造挂在任务事件广播通道上的 delta 攒批器。
    fn delta_batcher(&self, task_id: &str, phase: &str, step_index: Option<usize>) -> DeltaBatcher;
}

// ==================== 窄接口(7/9):执行终态落库 ====================

/// 任务执行终态统一落库能力。
pub(crate) trait TaskTerminalSink: Send + Sync {
    /// 任务执行终态统一落库(按 TaskTerminal 变体分派既有写入形态)。
    fn finalize_terminal(
        &self,
        task_id: &str,
        token: u64,
        terminal: TaskTerminal,
        ended_by_cancel: bool,
    );
}

// ==================== 窄接口(8/9):LLM 单次生成原语 ====================

/// 任务模式 LLM 单次生成原语。
///
/// 批次 4.2:重试与解析**算法**已上移 `task_engine::retry`(那是任务引擎的职责),
/// 本接口只保留「发一次调用」的原语(宿主侧负责提示词组装、连接器调用、调用追踪落库);
/// 引擎侧据此自行决定重试策略与解析判定。
pub(crate) trait TaskGenerator: Send + Sync {
    /// 非流式生成统一出口(含调用追踪/delta 旁路落库)。
    ///
    /// `connection_id`(二维批次 5b):本次调用走哪一套连接(`ConnectionProfile.id`);
    /// `None` = 默认连接。只有自定义流程的节点会带值(节点级 provider 路由),
    /// 其余模式恒传 `None`(行为与 5b 之前一致)。
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
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>>;

    /// 带**单次调用超时覆盖**的生成(A 批 A1):`timeout` 覆盖宿主侧缺省看门狗
    /// (任务侧两条路径各自的 300s);`None` = 与 `generate_text` 逐字节一致。
    ///
    /// 节点级 `call_timeout_secs` 是唯一消费者(自定义流程节点),故不走 `generate_text`
    /// 加参的老路——那要动 8 个既有调用点与所有测试替身。默认实现**忽略覆盖、直接委托**:
    /// 只有真正持有看门狗的实现(任务宿主)需要覆盖它,其余实现(含测试替身)零改动。
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
        timeout: Option<std::time::Duration>,
        cancel: watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>> {
        let _ = timeout;
        self.generate_text(
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
        )
    }

    /// 执行单个步骤的单次生成(注入执行者指令/世界书/Agent 提示词;
    /// 提示词注入按 TM-SET-2 恒隔离,任务侧不继承 `prompt_floors.json`)。
    fn generate_step<'a>(
        &'a self,
        task: &'a TaskRecord,
        step: &'a TaskStep,
        step_index: Option<usize>,
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>>;

    /// 带生成参数覆盖的步骤生成(重试时提高 max_tokens 或调整温度)。
    #[allow(clippy::too_many_arguments)]
    fn generate_step_with<'a>(
        &'a self,
        task: &'a TaskRecord,
        step: &'a TaskStep,
        step_index: Option<usize>,
        max_tokens: u32,
        temperature: f64,
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>>;

    /// 汇总的单次生成。
    fn summarize_task<'a>(
        &'a self,
        task: &'a TaskRecord,
        plan: &'a [TaskStep],
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>>;

    /// 带生成参数覆盖的汇总生成。
    #[allow(clippy::too_many_arguments)]
    fn summarize_task_with<'a>(
        &'a self,
        task: &'a TaskRecord,
        plan: &'a [TaskStep],
        max_tokens: u32,
        temperature: f64,
        cancel: &'a watch::Receiver<bool>,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>>;

    /// 规划的单次生成(含只读侦察;解析与重试在 task_engine::retry)。
    ///
    /// `scope`(D1):侦察轮的只读作用域——绑定了工作区/scratch 的任务,规划器可以
    /// 用只读工作区工具看真实文件,而不是盲规划;`capability`:执行阶段的工具面事实,
    /// 用于给规划器提示「执行者能做什么」(见 prompt_consts::capability_note)。
    #[allow(clippy::too_many_arguments)]
    fn plan_task<'a>(
        &'a self,
        task_id: &'a str,
        title: &'a str,
        character_id: Option<&'a str>,
        max_tokens: u32,
        cancel: &'a watch::Receiver<bool>,
        scope: Option<std::sync::Arc<crate::models::types::ExecScope>>,
        capability: crate::services::task_core::prompt_consts::StepCapability,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>>;

    /// 规划对话修订的单次生成(解析与重试在 task_engine::retry)。
    #[allow(clippy::too_many_arguments)]
    fn plan_revise<'a>(
        &'a self,
        task: &'a TaskRecord,
        history: &'a [TaskMessageRecord],
        feedback: &'a str,
        max_tokens: u32,
        cancel: &'a watch::Receiver<bool>,
        scope: Option<std::sync::Arc<crate::models::types::ExecScope>>,
        capability: crate::services::task_core::prompt_consts::StepCapability,
    ) -> BoxFuture<'a, Result<TaskGenOutput, String>>;
}

// ==================== 窄接口(9/9):任务临时工作区 ====================

/// 任务 scratch 工作区(任务模式 D1):未绑定工作区的任务的可写草稿区。
///
/// 语义归宿主:目录按任务 id 隔离、首次使用时创建、**失败必须 Err**——调用方据此让任务
/// 以明确错误终止;降级(例如回落到数据目录)会让模型在用户真实数据里写垃圾文件,
/// 是更坏的结果(2026-09 实测:一轮 77 条命令 / 12 个垃圾文件)。
pub(crate) trait TaskScratch: Send + Sync {
    /// 任务 scratch 目录:`<scratch_root>/<task_id>`,不存在则创建。
    fn scratch_dir_for(&self, task_id: &str) -> Result<std::path::PathBuf, String>;

    /// 任务附件清单(ATTACH-1)。
    ///
    /// 挂在窄接口 9 而不是新开一条:附件与 scratch 同属「任务的文件作用域」,
    /// 消费方也只有同一处(`task_engine::run_inner` 拼目标文本)。
    /// 返回类型用**无结构的 `(name, rel_path, size, mime)` 四元组**,不把
    /// `TaskAttachmentRecord`(宿主侧的类型)泄进 task_core:`task_core` 是窄接口层,
    /// 不认识 DB 行;真需要整行时再另立接口(与 `TaskTrace` / `TaskTimeline` 的
    /// 「名字相近、职责不同」同一处置)。
    fn list_task_attachments(&self, task_id: &str) -> Result<Vec<TaskAttachmentBrief>, String>;
}

/// 目标注入只需要这四个字段:名字(给人看)、相对路径(给 `fs_read` 用)、
/// 大小(给模型判断要不要读)与 MIME(决定 fs_read 还是 view_image)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskAttachmentBrief {
    pub name: String,
    pub rel_path: String,
    pub size: i64,
    pub mime: Option<String>,
}

// ==================== 窄接口(10/10):任务执行轨迹(TMSHOW-1) ====================

/// 任务执行轨迹落库(`task_trace`):思考正文 / 工具入出参 / 节点产出。
///
/// **与 [`TaskTrace`] 的区别(名字相近,职责不同)**:
/// - `TaskTrace`(窄接口 2)是**调用记账**——`task_llm_calls` 每次调用一行、token/耗时/摘要,
///   回答「这次调用花了多少、看到了什么」;
/// - 本接口是**执行内容**——`task_trace` 一条轨迹一行完整正文,回答「这一轮想了什么、
///   调了哪个工具、bash 回了什么」。前者是成本与诊断视角,后者是渲染视角。
///
/// 只有真的多了一种**要渲染的内容**才走本接口;只是想多记一个统计量,应加在 `TaskTrace`。
pub(crate) trait TaskTimeline: Send + Sync {
    /// 落一条轨迹并广播 `kind=trace` 事件。
    ///
    /// 空正文由实现侧早退(不落库不广播);正文超上限在实现侧截断并置标记。
    /// 返回值 = 轨迹序号(未落库时为 None),供调用方观测/测试断言。
    fn record_trace(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        kind: crate::models::types::TaskTraceKind,
        name: Option<&str>,
        content: &str,
    ) -> Option<i64>;
}

// ==================== 聚合别名(兼容既有构造点) ====================

/// 宿主能力全集(聚合别名,非上帝接口):同时满足全部窄接口即自动实现。
/// 既有 `Arc<dyn TaskBackend>` 构造点零改动;消费方应优先依赖上面的窄接口。
pub(crate) trait TaskBackend:
    TaskStore
    + TaskTrace
    + TaskSettings
    + TaskPromptKit
    + TaskFlowAccess
    + TaskEvents
    + TaskTerminalSink
    + TaskGenerator
    + TaskScratch
    + TaskTimeline
{
}

impl<T> TaskBackend for T where
    T: TaskStore
        + TaskTrace
        + TaskSettings
        + TaskPromptKit
        + TaskFlowAccess
        + TaskEvents
        + TaskTerminalSink
        + TaskGenerator
        + TaskScratch
        + TaskTimeline
{
}
