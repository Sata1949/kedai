// 任务核心共享类型(批次 B.3 依赖倒置):任务引擎与任务服务两侧共用的纯数据类型,
// 自 task_service 机械搬迁至此,使 task_engine 只依赖 task_core、不再反向依赖宿主服务。
use crate::models::types::ToolCallArgs;

/// 任务模式单次 LLM 调用的完整产出:正文 + 诊断信息。
/// 诊断字段来自流式 Usage/Finish/Reasoning 块,用于日志排障、空输出分级重试与 usage 落库。
pub(crate) struct TaskGenOutput {
    pub text: String,
    /// stop/length/content_filter 等;上游未下发时为 None
    pub finish_reason: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// completion 中推理消耗的 token(推理模型;0 = 无观测)
    pub reasoning_tokens: i64,
    /// 推理正文字符数(reasoning_content 流;与 reasoning_tokens 互补,部分上游只给其一)
    pub reasoning_chars: usize,
    /// 本次调用模型请求的工具调用(仅下发 tools 的调用可能非空;问题②规划器侦察轮用,
    /// 其余调用方恒空)。来自流式 ToolCall 块聚合(与 execute_generation 同口径)。
    pub tool_calls: Vec<ToolCallArgs>,
}
