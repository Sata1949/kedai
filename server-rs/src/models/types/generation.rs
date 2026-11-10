//! 本轮生成与工具选择的内部契约，不是设置或 HTTP 线格式。
use super::ToolDefinition;

/// 工具选择策略(function calling):决定模型是否/如何调用工具。
/// OpenAI 兼容语义:auto = 模型自行决定;none = 禁止调用;required = 强制调用(至少一个);
/// function(name) = 强制调用指定工具。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Function(String),
}

/// 结构化输出请求(2026-10-03 能力位接入;VISION-L6 收口)。
/// 调用方声明「本产物希望是 JSON」的**意图**,是否真正下发协议参数由连接器
/// 按自身能力位(`ConnectorCapabilities::supports_structured_output`)与方言决定:
/// chat 方言下发 `response_format:{type:"json_object"}`、responses 下发
/// `text.format`,anthropic 无等价参数不下发(维持提示词契约路径)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFormat {
    /// JSON 模式(OpenAI json_object 语义;要求提示词里出现「JSON」字样)
    Json,
}

#[derive(Debug, Clone)]
pub struct GenerationParams {
    pub temperature: f64,
    pub top_p: f64,
    pub max_tokens: u32,
    pub stop: Option<Vec<String>>,
    /// 工具定义(function calling);空 = 不启用工具
    pub tools: Vec<ToolDefinition>,
    /// 工具循环轮次上限:None = 引擎默认 32(聊天路径);Some(0) = **不限轮次**
    /// (任务模式哨兵,2026-10-19);Some(n>0) = 显式上限(聊天设置/custom 节点级)。
    /// 显式上限达到后不再发起新的模型请求;任务模式取消默认轮次上限后,
    /// 仍由步骤墙钟预算、空闲超时、语义/重复熔断与取消通道兜底。
    pub max_tool_rounds: Option<u32>,
    /// 工具选择策略;默认 Auto(保持原行为)。None/Required/Function 仅 tools 非空时生效。
    pub tool_choice: ToolChoice,
    /// 该调用走哪一套连接(`ConnectionProfile.id`;None = 默认连接)。
    /// 二维批次 5b:节点级 provider 路由的内部通道——本结构**未实现 Serialize**,
    /// 故新增字段不动任何线格式。消费点见 `agents/engine/executor.rs::execute_generation`
    /// 与 `services/task_service/executor.rs::generate_text`(同一处解析,不各自实现)。
    pub connection_id: Option<String>,
    /// 是否允许模型单轮返回多个工具调用(None = 使用后端默认)
    pub parallel_tool_calls: Option<bool>,
    /// 步骤墙钟预算(提交 3 · D3):单次 `run_tool_loop` 的墙钟上限,到点**带着已有产出
    /// 收尾**(返回 Ok,步骤照常记 done,不制造失败)。None = 不设预算——聊天路径恒 None
    /// (行为逐字节不变);唯一来源是任务侧设置 `task_step_budget_secs`
    /// (装配见 `task_engine::task_loop_limits`,消费点 `agents/engine/executor.rs`)。
    pub step_budget: Option<std::time::Duration>,
    /// 语义熔断三值覆盖(提交 3 · D3;窗口/同工具次数下限/输出指纹去重上限):
    /// None = 用引擎设置快照里的扁平值(聊天路径恒 None → 行为逐字节不变);
    /// Some = 任务侧经 `utils::loop_guard::clamp_for_task` **收紧**后的值(只收不放,
    /// 0 = 关闭位原样保持)。引擎不做模式嗅探:是不是任务由调用方显式传值表达。
    pub semantic_guard: Option<(usize, usize, usize)>,
    /// 结构化输出意图(None = 不要求,既有行为;见 [`ResponseFormat`])。
    /// 现注入点:mvu 两步生成的状态调用(mvu.rs);连接器按能力位决定是否下发。
    pub response_format: Option<ResponseFormat>,
    /// 编码包的可写执行轮优先完成具体动作。编排层按真实工具面设置，
    /// 连接器只在实测或已声明支持的路由控制推理强度；不进入设置或公开线格式。
    pub focused_action: bool,
}

impl GenerationParams {
    /// 派生调用可能清空工具或改成强制指定工具；此时不能继承自动执行策略。
    pub(crate) fn is_focused_action_round(&self) -> bool {
        self.focused_action && self.tool_choice == ToolChoice::Auto && !self.tools.is_empty()
    }
}
