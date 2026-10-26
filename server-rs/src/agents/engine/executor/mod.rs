// 生成执行：计算器启发式工具触发、单次流式生成与 AGENT 模式工具循环。
// C-2C：工具循环已拆到 `engine::tool_loop`（编排 / 单轮执行 / 守卫判定）。
// C-2F-2：生产代码按职责分区（generation / transport / heal / guard）；本文件保留
// 执行结果类型与 `generate_text`，并再导出既有路径；测试在同目录 tests.rs。
use super::*;

use super::tool_loop::LoopStop;
pub(crate) use super::tool_loop::{run_tool_loop, ToolLoopRequest};

mod generation;
mod guard;
mod heal;
mod transport;

// 再导出保持既有调用路径 `engine::executor::…` 不变（纯代码移动，不改调用方）。
pub(crate) use generation::execute_generation;
pub(super) use generation::{generate_round, maybe_run_tool, tool_origin_str, trim_round_history};
// 仅测试直接消费的内部小函数（生产侧在 generation 内自用）。
#[cfg(test)]
pub(super) use generation::{add_usage, process_chunk};
pub(crate) use guard::ToolGate;
pub(super) use guard::{
    budget_reached, budget_stop_content, has_declared_change, is_declared_file_write,
    resolve_max_rounds, step_budget_reached,
};
// 自愈纯函数的唯一消费方是 generation/transport 与测试（测试经 super::* 取用）。
#[cfg(test)]
pub(super) use heal::{
    doubled_heal_budget, heal_budget_for, is_truncated_tool_call_error, truncation_heal_cause,
    TRUNCATION_HEAL_MAX_TOKENS_CAP,
};
pub(super) use transport::resolve_call_watchdog;
pub(crate) use transport::TASK_TOOL_LOOP_CALL_TIMEOUT;
#[cfg(test)]
pub(super) use transport::{
    call_watchdog_for, should_retry_transport, with_call_watchdog, MAX_TRANSPORT_RETRIES,
};

/// 执行结果(executor 输出);content/usage/interrupted 由父模块 run() 主流程读取
/// pub(crate):任务引擎(task_engine)直调 run_tool_loop 后读取本结构(批次 4.2)。
pub(crate) struct ExecutorResult {
    pub(crate) content: String,
    pub(crate) usage: TokenUsage,
    pub(crate) interrupted: bool,
    /// 模型本轮请求的工具调用(AGENT 模式由 run_tool_loop 执行)
    pub(crate) tool_calls: Vec<ToolCallArgs>,
    /// 本轮思考模式推理内容(回传用)
    pub(crate) reasoning: String,
    /// 上游 finish_reason(stop/length 等;可观测性问题①):任务模式落库用,
    /// 上游未下发/中断未完成时为 None;聊天路径不消费本字段
    pub(crate) finish_reason: Option<String>,
    /// 本轮内发生的截断自愈记录(问题①,2026-08-31 deepseek 实测修复):
    /// 仅 run_tool_loop 的单轮自愈路径产出,其余构造点恒空;聊天路径不消费。
    pub(crate) self_heals: Vec<SelfHealRecord>,
    /// 是否因**预算类闸门**而提前停止工具循环:token 预算(HB-1,聊天与任务共用)或
    /// 步骤墙钟预算(D3,任务侧专属,见 `GenerationParams.step_budget`)。收尾端据此在
    /// 消息 extra 里留痕(extra.budget_exceeded),刷新后仍能看出「这轮是被成本/时间
    /// 闸门收掉的」。聊天路径不传 `step_budget`,故聊天侧该位只可能来自 token 预算。
    pub(crate) budget_stopped: bool,
    /// 工具循环的提前停止记录(C-2C):7 类终止条件统一为 `LoopStopKind`。
    /// None = 正常收尾。预算类/轮次上限属于正常收尾,只有 4 个空转守卫经
    /// [`Self::guard_stopped`] 透出给任务侧降 partial(SKY-6 第 2 项)。
    ///
    /// **不要用 `stop.is_some()` 判定「守卫截断」**:预算/轮次上限同样会写入本字段,
    /// 任务侧降 partial 必须走 [`Self::guard_stopped`](它按 kind 过滤)。
    pub(crate) stop: Option<LoopStop>,
}

impl ExecutorResult {
    /// 所有「提前停止」出口的唯一构造点(C-2C):保证 kind/message/事件三者同源,
    /// 并让 `budget_stopped` 由 kind 派生,避免两处各写一份映射。
    ///
    /// 停止时本轮工具已全部执行完(轮末闸门),因此 `usage` 归零——整轮用量已由
    /// 调用方累进 `total_usage`;`tool_calls`/`reasoning` 也已在事件与消息中收尾。
    pub(crate) fn stopped(
        content: String,
        finish_reason: Option<String>,
        self_heals: Vec<SelfHealRecord>,
        stop: LoopStop,
    ) -> Self {
        Self {
            content,
            usage: TokenUsage::default(),
            interrupted: false,
            tool_calls: Vec::new(),
            reasoning: String::new(),
            finish_reason,
            self_heals,
            budget_stopped: stop.kind.is_budget(),
            stop: Some(stop),
        }
    }

    /// 守卫类截断的短原因(任务侧据此把步骤降 partial);预算/轮次上限返回 None。
    pub(crate) fn guard_stopped(&self) -> Option<&str> {
        self.stop
            .as_ref()
            .filter(|s| s.kind.is_guard())
            .map(|s| s.message.as_str())
    }
}

/// 截断自愈记录(问题①):单轮生成被 max_tokens 截断到不可用(空正文/半截
/// tool_call JSON)时「翻倍预算原样重发一次」的留痕。供任务模式(run_agent_loop /
/// custom 步骤)把被截断的那次调用补落 task_llm_calls,调用情况面板可见
/// 「截断 → 提高预算重发 → 成功/失败」完整链路。
pub(crate) struct SelfHealRecord {
    /// 触发原因描述(落 response_summary;如「返回空内容(已达 token 上限)」
    /// 「工具参数 JSON 截断(工具 "calculator")」)
    pub(crate) note: String,
    /// 被截断调用的 token 用量(Err 形态拿不到,记 0)
    pub(crate) prompt_tokens: i64,
    pub(crate) completion_tokens: i64,
    /// 被截断调用的 finish_reason(Ok 形态恒 Some("length");
    /// Err 形态 finish_reason 随连接器错误丢失,None)
    pub(crate) finish_reason: Option<String>,
    /// 重发使用的 max_tokens(翻倍后,上限 TRUNCATION_HEAL_MAX_TOKENS_CAP)
    pub(crate) retried_max_tokens: u32,
}

// ===== 非流式静默生成(自 engine/mod.rs 拆分迁入,纯代码移动,逻辑不变)=====
impl AgentEngine {
    /// 阶段六 6g-1:非流式静默生成(复刻 generate_reflect_advice 模式)。
    /// 供后端脚本 TavernHelper.generate 与外部调用;不入聊天记录、不推 SSE。
    pub async fn generate_text(
        &self,
        messages: &[LlmMessage],
        params: GenerationParams,
        abort: watch::Receiver<bool>,
    ) -> Result<(String, TokenUsage), String> {
        if *abort.borrow() {
            return Err("生成已中断".into());
        }
        let connector = self.connector.read().await.clone();
        // 该公开接口(脚本 TavernHelper.generate)以 String 报错,分类在此落回文案
        let chunks = connector
            .generate(messages, params, abort)
            .await
            .map_err(|e| e.message().to_string())?;
        drop(connector);
        let mut out = String::new();
        let mut usage = TokenUsage::default();
        for chunk in chunks {
            match chunk {
                LlmStreamChunk::Token(t) => out.push_str(&t),
                LlmStreamChunk::Usage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens,
                    prompt_cache_hit_tokens,
                    prompt_cache_miss_tokens,
                    ..
                } => {
                    usage.prompt_tokens += prompt_tokens;
                    usage.completion_tokens += completion_tokens;
                    usage.total_tokens += total_tokens;
                    usage.prompt_cache_hit_tokens += prompt_cache_hit_tokens;
                    usage.prompt_cache_miss_tokens += prompt_cache_miss_tokens;
                }
                _ => {}
            }
        }
        if out.trim().is_empty() {
            Err("生成返回空内容".into())
        } else {
            Ok((out, usage))
        }
    }
}

#[cfg(test)]
mod tests;
