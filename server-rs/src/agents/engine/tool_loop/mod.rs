// 工具循环编排(C-2C,自 `executor.rs::run_tool_loop` 抽出)。
//
// 分层:
//   `generation`(仍在 executor.rs)一次生成 + 看门狗 + 传输重试 + 截断自愈;
//   `round` 一轮工具执行与副作用收尾;
//   `guard` 守卫计数与 7 类停止条件判定;
//   本文件只保留「循环骨架」:生成 → 执行工具 → 登记事实 → 判定 → 发事件/收尾。
//
// 行为契约(与抽取前逐字节一致):停止条件顺序、事件标题/明细、`guard_stopped`
// 文案、任务终态(预算类 done / 守卫类 partial)均由 `guard.rs` 与既有集成用例锁定。
mod guard;
mod round;

pub(crate) use guard::{LoopStop, LoopStopKind};

use super::executor::{
    budget_stop_content, generate_round, has_declared_change, is_declared_file_write,
    resolve_call_watchdog, resolve_max_rounds, trim_round_history, ExecutorResult, SelfHealRecord,
    ToolGate,
};
use super::util::{send_event, step_evt};
use super::{AbortFlag, AgentEngine, EngineError};
use crate::agents::state_machine::StateMachine;
use crate::models::types::{
    AgentSessionRecord, GenerationParams, LlmMessage, SseEvent, TokenUsage, ToolContext,
};
use crate::utils::loop_guard::{LoopGuard, SemanticGuard};

use guard::{GuardConfig, GuardNotice, GuardState, RoundFacts};
use round::RoundContext;

/// 工具循环的输入聚合(C-2C 第 5 步:把 16 个位置参数收成一个结构体)。
///
/// 可变引用也收在本结构体内:调用方一次构造、函数内解构为独立局部变量,
/// 后续对 `llm_messages` / `total_usage` / `self_heals` 的借用互不影响。
pub(crate) struct ToolLoopRequest<'a> {
    pub(crate) engine: &'a AgentEngine,
    pub(crate) state_machine: &'a mut StateMachine,
    pub(crate) agent_session: Option<&'a AgentSessionRecord>,
    pub(crate) session_id: &'a str,
    pub(crate) llm_messages: &'a mut Vec<LlmMessage>,
    pub(crate) params: &'a GenerationParams,
    pub(crate) tool_ctx: &'a ToolContext,
    pub(crate) tx: &'a tokio::sync::mpsc::Sender<SseEvent>,
    pub(crate) abort: &'a tokio::sync::watch::Receiver<bool>,
    pub(crate) flag: &'a AbortFlag,
    pub(crate) total_usage: &'a mut TokenUsage,
    /// 截断自愈留痕(TMR3-2):由调用方持有,成功/失败都能拿到记录。
    pub(crate) self_heals: &'a mut Vec<SelfHealRecord>,
    pub(crate) run_id: &'a str,
    pub(crate) gate: ToolGate<'a>,
    /// 单次调用超时覆盖(A 批 A1):None = 宿主既有判定。
    pub(crate) call_timeout: Option<std::time::Duration>,
    /// 是否透出正文增量(RPFLOW-1):隐藏步骤传 false,工具事件照常透出。
    pub(crate) emit_tokens: bool,
}

/// 循环开始前解析一次的限制与守卫快照(循环内不再读设置锁,不留锁跨 await)。
struct LoopLimits {
    call_watchdog: Option<std::time::Duration>,
    declared_files: Vec<String>,
    /// 声明文件无进展守卫是否启用(有预算阈值且有声明文件)。
    no_progress_enabled: bool,
    guards: GuardState,
    semantic_guard: SemanticGuard,
}

fn resolve_loop_limits(
    engine: &AgentEngine,
    session_id: &str,
    params: &GenerationParams,
    tool_ctx: &ToolContext,
    gate: ToolGate<'_>,
    call_timeout: Option<std::time::Duration>,
) -> LoopLimits {
    // 单次调用总时长看门狗:仅任务模式启用(节点级 `call_timeout` 可覆盖)。
    let call_watchdog = resolve_call_watchdog(session_id, call_timeout);
    // token 预算(HB-1)与语义熔断默认值:循环前取一次设置快照。
    let (token_budget, budget_action, sem_window, sem_min_calls, sem_max_distinct) = {
        let s = engine.settings_snapshot();
        (
            s.session_token_budget,
            s.session_budget_action,
            s.loop_guard_semantic_window as usize,
            s.loop_guard_semantic_min_calls as usize,
            s.loop_guard_semantic_max_distinct as usize,
        )
    };
    // 任务侧由调用方显式传入收紧后的三值(只收不放,0 = 关闭位原样保持)。
    let (semantic_window, semantic_min_calls, semantic_max_distinct) = params
        .semantic_guard
        .unwrap_or((sem_window, sem_min_calls, sem_max_distinct));
    // 步骤墙钟预算(D3):None = 不设(聊天路径恒 None)。
    let step_budget = params.step_budget;
    // 轮次上限:None = 旧调用方/聊天路径默认 32;Some(0) = 显式不限(任务模式哨兵)。
    let max_rounds = resolve_max_rounds(params.max_tool_rounds);
    // 连续只读检测(TMR-5):任务侧特有,载体是 ToolBudget(聊天路径 None)。
    // 只有本轮确实下发工作区写工具时才启用——只读白名单/审计面/聊天路径不误提醒。
    let readonly_limit = tool_ctx
        .budget
        .and_then(|b| b.readonly_streak_limit)
        .filter(|n| *n > 0)
        .filter(|_| {
            gate.whitelist.is_some_and(|wl| {
                wl.iter()
                    .any(|n| crate::tools::tool_sets::is_workspace_write_tool(n))
            })
        });
    // 声明文件无进展守卫(P0-2):只在 scope 带声明文件(team 主 agent)时启用。
    let declared_files: Vec<String> = tool_ctx
        .scope
        .as_ref()
        .map(|s| s.declared_files().to_vec())
        .unwrap_or_default();
    let no_progress_limit = tool_ctx
        .budget
        .and_then(|b| b.no_progress_limit)
        .filter(|n| *n > 0)
        .filter(|_| !declared_files.is_empty());

    LoopLimits {
        call_watchdog,
        declared_files,
        no_progress_enabled: no_progress_limit.is_some(),
        guards: GuardState::new(GuardConfig {
            max_rounds,
            token_budget,
            stop_on_budget: budget_action == "stop",
            step_budget,
            readonly_limit,
            no_progress_limit,
            semantic_window,
        }),
        semantic_guard: SemanticGuard::new(
            semantic_window,
            semantic_min_calls,
            semantic_max_distinct,
        ),
    }
}

/// AGENT 模式工具循环:生成 → 有 tool_calls 则逐个执行并回填消息 → 重新生成。
///
/// 轮次上限:None = 聊天默认 32;Some(0) = 任务模式不限;Some(n>0) = 显式上限。
/// 无 tool_calls 时返回最终正文;每轮 usage 累加进 `total_usage`。
/// 停止条件与顺序见 `guard.rs` 模块头。
pub(crate) async fn run_tool_loop(req: ToolLoopRequest<'_>) -> Result<ExecutorResult, EngineError> {
    let ToolLoopRequest {
        engine,
        state_machine,
        agent_session,
        session_id,
        llm_messages,
        params,
        tool_ctx,
        tx,
        abort,
        flag,
        total_usage,
        self_heals,
        run_id,
        gate,
        call_timeout,
        emit_tokens,
    } = req;

    let LoopLimits {
        call_watchdog,
        declared_files,
        no_progress_enabled,
        mut guards,
        mut semantic_guard,
    } = resolve_loop_limits(engine, session_id, params, tool_ctx, gate, call_timeout);
    let mut round = 0usize;
    // 重复调用熔断(P0-2):窗口内同一「工具名+参数」指纹 ≥3 次即中止。
    let mut loop_guard = LoopGuard::with_defaults();
    let loop_started = std::time::Instant::now();
    // 轮级滚动:最近一次非空正文(墙钟/只读/无进展收尾的回退值)。
    let mut last_non_empty_content = String::new();
    let mut semantic_break: Option<(String, usize, usize)> = None;
    loop {
        // ===== 工具历史回灌上限(R3b):最老轮的 tool 结果原地摘要化 =====
        trim_round_history(engine, session_id, round, llm_messages);

        // ===== 单轮生成(看门狗 + 传输重试 + 截断自愈)=====
        let result = generate_round(
            engine,
            session_id,
            run_id,
            llm_messages,
            params,
            tx,
            abort,
            flag,
            emit_tokens,
            call_watchdog,
            round,
            total_usage,
            self_heals,
        )
        .await?;

        total_usage.prompt_tokens += result.usage.prompt_tokens;
        total_usage.completion_tokens += result.usage.completion_tokens;
        total_usage.total_tokens += result.usage.total_tokens;
        total_usage.prompt_cache_hit_tokens += result.usage.prompt_cache_hit_tokens;
        total_usage.prompt_cache_miss_tokens += result.usage.prompt_cache_miss_tokens;
        if result.interrupted {
            // 本轮 usage 已累进 total_usage;返回体归零,避免调用方重复计数。
            return Ok(ExecutorResult {
                usage: TokenUsage::default(),
                self_heals: std::mem::take(self_heals),
                ..result
            });
        }
        if result.tool_calls.is_empty() {
            return Ok(ExecutorResult {
                content: result.content,
                usage: TokenUsage::default(),
                interrupted: false,
                tool_calls: Vec::new(),
                reasoning: String::new(),
                // 末轮(无工具调用)的 finish_reason 透出:任务模式落库截断标记用
                finish_reason: result.finish_reason,
                self_heals: std::mem::take(self_heals),
                budget_stopped: false,
                stop: None,
            });
        }
        round += 1;
        // 最近一次**非空**正文:工具型模型常见「只发工具调用、不带正文」的轮,
        // 预算类收尾若停在这样一轮,回退到这里避免返回空串。
        if !result.content.trim().is_empty() {
            last_non_empty_content = result.content.clone();
        }
        // ===== 重复调用熔断(P0-2):本轮每个工具调用做「工具名+参数」指纹登记 =====
        let repeated: Option<(String, usize)> = {
            let mut hit: Option<(String, usize)> = None;
            for call in &result.tool_calls {
                let fp = crate::utils::loop_guard::fnv1a_hash(&[&call.name, &call.arguments]);
                if loop_guard.record(fp) {
                    hit = Some((call.name.clone(), loop_guard.count_of(fp)));
                    break;
                }
            }
            hit
        };
        // ===== 回填 assistant 消息(完整 tool_calls[] + reasoning_content)=====
        // 旧实现为每个 call 单独追加一条 assistant,违反 OpenAI 多工具调用格式。
        let reasoning = (!result.reasoning.is_empty()).then_some(result.reasoning.clone());
        llm_messages.push(LlmMessage {
            role: "assistant".into(),
            content: String::new(),
            reasoning_content: reasoning,
            tool_calls: Some(result.tool_calls.clone()),
            tool_call_id: None,
            images: Vec::new(),
        });

        // TMR3-L6:本轮执行前的台账基线(bash 写目标只覆盖显式形态,台账补判)。
        let ledger_mark: Option<(String, i64)> = if no_progress_enabled {
            crate::services::task_change_service::task_id_of(&tool_ctx.session_id).and_then(|tid| {
                crate::services::task_change_service::max_id(&engine.db, &tid).map(|id| (tid, id))
            })
        } else {
            None
        };

        // ===== 本轮工具执行(裁决/并发分组/状态机/SSE/tool 消息)=====
        let outcome = round::execute_round(
            RoundContext {
                engine,
                state_machine,
                agent_session,
                session_id,
                llm_messages,
                tool_ctx,
                tx,
                abort,
                flag,
                run_id,
                gate,
            },
            &result.tool_calls,
        )
        .await?;
        if outcome.interrupted {
            return Ok(ExecutorResult {
                content: String::new(),
                usage: TokenUsage::default(),
                interrupted: true,
                tool_calls: Vec::new(),
                reasoning: String::new(),
                // 工具执行期中断:生成未完成,finish_reason 不适用
                finish_reason: None,
                self_heals: std::mem::take(self_heals),
                budget_stopped: false,
                stop: None,
            });
        }

        // ===== 在成果里判定「是否写入了声明文件」(P0-2 / TMR3-L6)=====
        let mut round_progress = false;
        if no_progress_enabled {
            for e in &outcome.outputs {
                // 权限拒绝(output=Null)与错误对象不算;只认带 `path` 参数的写工具。
                if is_declared_file_write(&e.call.name, &e.call.arguments, &declared_files)
                    && !e.output.is_null()
                    && e.output.get("error").is_none()
                {
                    round_progress = true;
                }
            }
            // 没有可识别的成功写调用时,用台账增量补判——bash 重定向/tee/cp 等
            // 实际落盘的声明文件同样算进展;临时文件与越界路径不算。
            if !round_progress {
                if let Some((task_id, mark)) = ledger_mark.as_ref() {
                    let changed = crate::services::task_change_service::paths_since(
                        &engine.db, task_id, *mark,
                    );
                    round_progress = has_declared_change(&declared_files, &changed);
                }
            }
        }

        // ===== 无进展守卫的中途提醒(先提醒,后硬收尾)=====
        if let Some(notice) = guards.observe_round_progress(round_progress) {
            emit_notice(notice, llm_messages, tx, abort, flag).await?;
        }
        // ===== 连续只读计数(TMR-5):按本轮模型请求的工具调用统计 =====
        guards.observe_tool_calls(&result.tool_calls);
        // ===== 语义熔断登记(HB-2):看输出的实质变化,参数可以每轮都变 =====
        if semantic_break.is_none() {
            for e in &outcome.outputs {
                let fp = crate::utils::loop_guard::semantic_fingerprint(
                    &e.call.name,
                    &e.output.to_string(),
                    &e.call.arguments,
                );
                // SKY-6 第 4 项:纯读工具按整窗阈值判定(少量文件反复读是任务常态)。
                let readonly_class =
                    crate::tools::tool_sets::is_workspace_readonly_tool(&e.call.name);
                semantic_break = semantic_guard.record_classified(&e.call.name, fp, readonly_class);
                if semantic_break.is_some() {
                    break;
                }
            }
        }

        // ===== 轮末判定(7 类停止条件;顺序见 guard.rs)=====
        // 两阶段:token 预算的提醒事件先发出,步骤墙钟再采样 elapsed——
        // 与抽取前「先 await token warn,再取墙钟」的时点一致。
        let mut evaluation = guards.evaluate_token(
            session_id,
            total_usage.prompt_tokens + total_usage.completion_tokens,
            round,
        );
        emit_notices(&mut evaluation.notices, llm_messages, tx, abort, flag).await?;
        if evaluation.stop.is_none() {
            evaluation = guards.evaluate_rest(
                session_id,
                RoundFacts {
                    round,
                    elapsed: loop_started.elapsed(),
                    used_tokens: total_usage.prompt_tokens + total_usage.completion_tokens,
                    semantic_break: semantic_break.take(),
                    repeated,
                },
            );
            emit_notices(&mut evaluation.notices, llm_messages, tx, abort, flag).await?;
        }
        if let Some(stop) = evaluation.stop {
            let (title, detail) = stop.event();
            send_event(
                step_evt(title, Some(detail.to_string()), None, None),
                tx,
                abort,
                flag,
            )
            .await?;
            // 预算/轮次收尾用本轮正文;墙钟/只读/无进展承诺「带着已有产出」,
            // 本轮只有工具调用时回退到最近一次非空正文。
            let content = match stop.kind {
                LoopStopKind::StepBudget | LoopStopKind::Readonly | LoopStopKind::NoProgress => {
                    budget_stop_content(result.content, &last_non_empty_content)
                }
                LoopStopKind::TokenBudget
                | LoopStopKind::RoundLimit
                | LoopStopKind::Semantic
                | LoopStopKind::RepeatedCall => result.content,
            };
            return Ok(ExecutorResult::stopped(
                content,
                result.finish_reason,
                std::mem::take(self_heals),
                stop,
            ));
        }
    }
}

/// 发一条守卫提醒:只读/无进展会把提醒文本回灌给模型(先回灌再发事件,
/// 与抽取前顺序一致);token 预算 warn 档只有事件。
async fn emit_notice(
    notice: GuardNotice,
    llm_messages: &mut Vec<LlmMessage>,
    tx: &tokio::sync::mpsc::Sender<SseEvent>,
    abort: &tokio::sync::watch::Receiver<bool>,
    flag: &AbortFlag,
) -> Result<(), EngineError> {
    if let Some(text) = notice.nudge_text() {
        llm_messages.push(LlmMessage::plain("user", text));
    }
    let (title, detail) = notice.event();
    send_event(step_evt(title, Some(detail), None, None), tx, abort, flag).await?;
    Ok(())
}

/// 按顺序发出阶段内的全部提醒(两阶段判定共用;drain 避免调用方再管所有权)。
async fn emit_notices(
    notices: &mut Vec<GuardNotice>,
    llm_messages: &mut Vec<LlmMessage>,
    tx: &tokio::sync::mpsc::Sender<SseEvent>,
    abort: &tokio::sync::watch::Receiver<bool>,
    flag: &AbortFlag,
) -> Result<(), EngineError> {
    for notice in notices.drain(..) {
        emit_notice(notice, llm_messages, tx, abort, flag).await?;
    }
    Ok(())
}
