// 生成执行:计算器启发式工具触发、单次流式生成与 AGENT 模式工具循环
use super::*;
use std::sync::atomic::{AtomicI64, Ordering};

// C-2C:工具循环已拆到 `engine::tool_loop`(编排 / 单轮执行 / 守卫判定)。
// 此处重新导出保持既有调用路径 `engine::executor::run_tool_loop` 不变。
use super::tool_loop::LoopStop;
pub(crate) use super::tool_loop::{run_tool_loop, ToolLoopRequest};

/// 进程级 LLM 请求快照序号(单调递增;第四点·主题 A 的 llm_requests.seq 用,
/// 跨 run 也单调,比「run 内自增」更强,prune 按 id 删除不受影响)。
static LLM_REQUEST_SEQ: AtomicI64 = AtomicI64::new(0);

/// 工具来源串(`builtin`/`plugin`/`mcp`;PLGM 3.3 的 SSE 工具事件 origin 字段填充)。
/// 未注册/未知工具返回 None(字段随 `skip_serializing_if` 省略,旧客户端零变化)。
pub(super) fn tool_origin_str(
    registry: &crate::tools::registry::ToolRegistry,
    name: &str,
) -> Option<String> {
    registry.origin_of(name).map(|o| o.as_str().to_string())
}

/// 工具触发(每个生成周期最多一次;失败不致命)
#[allow(clippy::too_many_arguments)]
pub(super) async fn maybe_run_tool(
    engine: &AgentEngine,
    state_machine: &mut StateMachine,
    tool_triggered: &mut bool,
    agent_session_id: &str,
    session_id: &str,
    user_input: &str,
    tool_ctx: &ToolContext,
    llm_messages: &mut Vec<LlmMessage>,
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
) -> Result<(), String> {
    if *tool_triggered || !looks_like_calculation(user_input) {
        return Ok(());
    }
    *tool_triggered = true;
    state_machine.transition_best_effort(AgentState::ToolCall, session_id);
    let _ = engine
        .agent_sessions
        .update(agent_session_id, Some("tool_call"), None, None, None);
    let expression = extract_expression(user_input);
    send_event(
        SseEvent::ToolCall {
            name: "calculator".into(),
            input: json!({ "expression": expression }),
            call_id: None,
            render_kind: Some("generic".into()),
            origin: tool_origin_str(&engine.tool_registry, "calculator"),
        },
        tx,
        abort,
        flag,
    )
    .await?;

    let start = std::time::Instant::now();
    let output: Value = match engine
        .tool_registry
        .execute(
            "calculator",
            &json!({ "expression": expression }).to_string(),
            tool_ctx.clone(),
        )
        .await
    {
        Ok(r) => serde_json::from_str(&r).unwrap_or(Value::Null),
        Err(e) => {
            let _ = send_event(
                step_evt("工具调用失败,改用直接生成", Some(e.clone()), None, None),
                tx,
                abort,
                flag,
            )
            .await;
            json!({ "error": e })
        }
    };
    let _ = engine.agent_sessions.add_tool_call(
        agent_session_id,
        "calculator",
        json!({ "expression": expression }),
        output.clone(),
        start.elapsed().as_millis() as i64,
        // 直算路径无回退快照(calculator 非写工具)
        None,
    );
    send_event(
        SseEvent::ToolResult {
            name: "calculator".into(),
            output: output.clone(),
            call_id: None,
            render_kind: Some("generic".into()),
            origin: tool_origin_str(&engine.tool_registry, "calculator"),
            snapshot_id: None,
        },
        tx,
        abort,
        flag,
    )
    .await?;
    // 计算结果回填到 LLM 消息(否则模型看不到结果,只能瞎猜答案,工具对提示无实际作用)。
    // 以 user 角色的中性旁白注入,让模型当作「外部提供的事实」而非自己的发言,
    // 避免「[计算器结果]」元文本污染正文、破坏角色扮演沉浸感。
    if let Some(result) = output.get("result") {
        llm_messages.push(LlmMessage {
            role: "user".into(),
            content: format!("(旁白:计算结果为 {result})"),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            images: Vec::new(),
        });
    }
    state_machine.transition_best_effort(AgentState::Executing, session_id);
    let _ = engine
        .agent_sessions
        .update(agent_session_id, Some("executing"), None, None, None);
    Ok(())
}
/// 任务模式「单次 LLM 调用」总时长上限(第二次看门狗,2026-09-18)。
///
/// 为什么需要:姊妹路径 `TaskService::generate_text`(规划/步骤/汇总等纯生成)有 300s
/// 总时长看门狗(`task_service/mod.rs` 的 `TASK_LLM_TOTAL_TIMEOUT`),而工具循环
/// (`run_tool_loop` → `execute_generation`)此前只有连接器层的**空闲**看门狗与轮次上限,
/// 没有总时长上限。两个缺口叠加可让任务无限停在 running:
///   ① 引擎侧无总时长;② 空闲判定对「慢速滴流」无效——上游每 <120s 吐几个字节即可
///   每次重置空闲计时,单轮生成拖到无穷。
/// 本轮把任务模式补齐到与纯生成同档(300s/次),超时让任务进 error 终态而非静默卡死。
///
/// 常量与 `task_service` 的同名值口径一致但不跨层 import(L3 引擎不依赖 L2 服务;
/// 两处各自声明、注释互指,改动时须同步)。
///
/// 风险:会中断「极慢但最终能返回」的上游请求。取 300s 而非更小值,是因为推理模型
/// 单轮生成数十秒属正常;与 planner/step 既有 300s 口径一致,不对同类调用双标。
///
/// `pub(crate)` 的用途(2026-09-30 批次 2):任务空闲看守的**合法下限**按
/// 「单条命令上限 + 单次模型调用上限 + 1」推导,这里就是第二项的唯一出处
/// (消费点 `services::settings_service::params::task_idle_floor_secs`)——
/// 此前该下限被硬编码成 601 三份,抬 bash 上限时漏改任何一份都会误杀合法长命令。
pub(crate) const TASK_TOOL_LOOP_CALL_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(300);

/// 总时长看门狗包装:`watchdog=None` 时行为与裸调完全一致;`Some(limit)` 时超过
/// `limit` 未完成即放弃 future 并返回 `timeout_error()`。
///
/// 抽成泛型是为了让单测能用毫秒级阈值覆盖「超时/未超时」两条分支——生产阈值 300s
/// 不可能在测试里真等。
async fn with_call_watchdog<F, T>(
    fut: F,
    watchdog: Option<std::time::Duration>,
    timeout_error: impl FnOnce() -> EngineError,
) -> Result<T, EngineError>
where
    F: std::future::Future<Output = Result<T, EngineError>>,
{
    let Some(limit) = watchdog else {
        return fut.await;
    };
    match tokio::time::timeout(limit, fut).await {
        Ok(r) => r,
        Err(_) => Err(timeout_error()),
    }
}

/// 该会话是否启用工具循环总时长看门狗(任务模式虚拟 session 为 `task:` 前缀)。
///
/// 抽成纯函数以便单测锁死「只有任务模式启用」这条边界:聊天路径的长回复是正常形态,
/// 加总时长上限会把「模型慢慢写长文」误判为失败。
fn call_watchdog_for(session_id: &str) -> Option<std::time::Duration> {
    session_id
        .starts_with("task:")
        .then_some(TASK_TOOL_LOOP_CALL_TIMEOUT)
}

/// 单次调用看门狗的最终取值(A 批 A1):**节点级覆盖优先**,缺省回落上面那条既有判定。
///
/// 抽成纯函数的原因与 `call_watchdog_for` 相同——生产阈值是 300s(或节点配的 30~3600s),
/// 不可能在测试里真等,故把「取哪个值」与「等多久」分开锁:单测锁取值,引擎侧只按值包超时。
/// 覆盖**同时具备收紧与放宽两种用法**(60s 的严节点 / 900s 的慢节点),故不是 `min` 语义。
pub(super) fn resolve_call_watchdog(
    session_id: &str,
    call_timeout: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    call_timeout.or_else(|| call_watchdog_for(session_id))
}

/// 任务模式单次调用遇到可重试上游失败(超时/限流/流中断)时的最大重试次数(TMR3-1,
/// 2026-10-22;单一出处)。
///
/// 为什么只重试**单次调用**而不是重启整个子目标:2026-10-21 实测主 Agent 2 跑完
/// 234 轮后上游错误,子目标级重启会丢掉全部工具历史并重跑已完成的副作用;单次
/// 重试保留 `llm_messages` 与工具结果,只把「这一次没有返回的调用」再做一遍。
///
/// 为什么上限是 1:任务模式的 300s 看门狗一次已很贵,重试一次最坏多花 300s;
/// 团队/单步墙钟预算与取消信号是外层兜底,不能让上游持续停滞把预算吃光。
const MAX_TRANSPORT_RETRIES: usize = 1;

/// 可重试失败与重试之间的固定退避(TMR3-1):只做一次,无需指数;给上游一个
/// 恢复窗口,同时不让测试等太久(连接器层重试另有 500ms×2^n 的退避)。
const TRANSPORT_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);

/// 是否对本次失败发起单次调用重试(TMR3-1 的单一判定出处,纯函数便于单测):
///   - 只有任务模式启用(`call_watchdog.is_some()`;聊天路径的长回复是正常形态);
///   - 只重试已分类为可重试的上游失败(`Timeout`/`RateLimited`/`Upstream`);
///   - 已达到重试上限或用户已取消时不重试。
///
/// 内部错误、截断自愈失败、鉴权/参数类错误都不走这里:截断错误有自己的「翻倍预算」
/// 路径,传输重试只会原样重发同一截断;后两者重试无意义(与 `EngineError::retryable`
/// 的口径一致)。
fn should_retry_transport(
    err: &EngineError,
    call_watchdog: Option<std::time::Duration>,
    retries_used: usize,
    cancelled: bool,
) -> bool {
    !is_truncated_tool_call_error(err.message())
        && err.retryable()
        && call_watchdog.is_some()
        && retries_used < MAX_TRANSPORT_RETRIES
        && !cancelled
}

/// 流式执行 LLM 生成(与 Node 版 executor.ts executeGeneration 对齐)
/// pub(crate):任务引擎 custom 模式(批次 4.3b)按步骤直调;聊天路径行为不变。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_generation(
    engine: &AgentEngine,
    session_id: &str,
    run_id: &str,
    messages: &[LlmMessage],
    params: &GenerationParams,
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
    // 是否把正文增量透出为 Token 事件(RPFLOW-1):草稿步/归档步等隐藏步骤传 false,
    // 产物不进消息气泡;工具调用/重试等其余事件照常透出。
    emit_tokens: bool,
    // 调用外部的 usage 实时累加器(TMR3-L1):每收到一个 Usage 分块就同步写入。
    // 单次调用被看门狗超时 drop 时,本函数栈上的局部 `usage` 随 future 消失,
    // 只有外部累加器能保住「超时前已收到的用量」;成功路径由返回值统一入账,
    // 调用方不得再读它,避免双计。聊天路径恒 None,行为不变。
    mut live_usage: Option<&mut TokenUsage>,
) -> Result<ExecutorResult, EngineError> {
    // LLM 请求快照(第四点·主题 A):开关开启时,把真正下发的完整消息数组落盘,
    // 供回放/调试「模型到底看到了什么」。失败仅告警,不阻塞生成。
    // seq 总是分配:缓存观测(usage 落库)不依赖快照开关。
    let seq = LLM_REQUEST_SEQ.fetch_add(1, Ordering::Relaxed);
    // 任务模式(session_id 带 task: 前缀)跳过 llm_requests 落库:该表 FK 到 sessions(id),
    // 虚拟 id 会 FK 失败刷 warn;任务侧调用追踪统一走 task_llm_calls(批次 3 起)。
    let is_task_run = session_id.starts_with("task:");

    // 连接器按本次调用的 connection_id 解析(二维批次 5b):None = 默认连接(5b 之前
    // 的读锁快照口径不变);节点级连接由 `resolve_connector` 单点解析,两条执行路径共用。
    // (先于快照与拆分解析:大图拆分要以目标连接的 image_auto_split 能力位为准。)
    let (connector, _model) = engine
        .resolve_connector(params.connection_id.as_deref())
        .await?;

    // 大图自动拆分(视觉能力包 D3):目标连接勾了 `image_auto_split` 且本轮带图时,
    // 把超阈值图像替换为「总览 + 行/列块」(派生块走磁盘缓存)。图像解码/重编码是
    // CPU 重活,park_worker 让出 async worker;未勾选/无图时零拷贝走原引用。
    let prepared: Option<Vec<LlmMessage>> = if connector.capabilities().image_auto_split
        && messages.iter().any(|m| !m.images.is_empty())
    {
        let mut v = messages.to_vec();
        crate::utils::blocking::park_worker(|| engine.images.expand_for_auto_split(&mut v));
        Some(v)
    } else {
        None
    };
    let messages: &[LlmMessage] = prepared.as_deref().unwrap_or(messages);

    {
        // 设置快照:不留锁跨 await(锁在 settings_snapshot 内即释放)
        let log_enabled = engine.settings_snapshot().llm_request_log;
        if log_enabled && !is_task_run {
            // 图像 data URL 可达数十 MB(视觉能力包 D2):快照只留文字与图像引用元数据,
            // 清空 data_url 再序列化——排查「模型看到了什么文字/哪张图」不需要重复存像素
            // (像素真身在 DATA_DIR/images/,按引用可复现;拆分标注 label 保留,便于诊断)。
            let mut log_messages = messages.to_vec();
            for m in &mut log_messages {
                for img in &mut m.images {
                    img.data_url.clear();
                }
            }
            if let Ok(payload) = serde_json::to_string(&log_messages) {
                if let Err(e) = engine.sessions.save_llm_request(
                    session_id,
                    run_id,
                    seq,
                    &payload,
                    &engine.model(),
                ) {
                    tracing::warn!(error = e, "LLM 请求快照落库失败");
                }
            }
        }
    }
    let mut content = String::new();
    let mut usage = TokenUsage::default();
    // 按 index 聚合后的完整工具调用(由连接器在流结束时输出)
    let mut tool_calls: Vec<ToolCallArgs> = Vec::new();
    // 思考模式推理内容(多轮工具调用需随 assistant 消息回传)
    let mut reasoning = String::new();
    // 上游 finish_reason(stop/length 等;可观测性问题①):聚合到 ExecutorResult,
    // 任务模式经 run_tool_loop 一路带到 task_llm_calls 落库点;聊天路径不消费本字段。
    let mut finish_reason: Option<String> = None;

    let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel();
    let generate = connector.generate_stream(messages, params.clone(), abort.clone(), chunk_tx);
    tokio::pin!(generate);
    let mut generation_result = None;

    loop {
        tokio::select! {
            result = &mut generate, if generation_result.is_none() => {
                generation_result = Some(result);
            }
            chunk = chunk_rx.recv() => match chunk {
                Some(chunk) => {
                    if process_chunk(chunk, &mut content, &mut reasoning, &mut tool_calls, &mut usage, live_usage.as_deref_mut(), &mut finish_reason, &engine.tool_registry, tx, abort, flag, emit_tokens).await? {
                        return Ok(ExecutorResult { content, usage, interrupted: true, tool_calls, reasoning, finish_reason, self_heals: Vec::new(), budget_stopped: false, stop: None });
                    }
                }
                None => break,
            }
        }
        if generation_result.is_some() && chunk_rx.is_empty() {
            break;
        }
    }
    if let Some(Err(e)) = generation_result {
        if *abort.borrow() {
            return Ok(ExecutorResult {
                content,
                usage,
                interrupted: true,
                tool_calls,
                reasoning,
                finish_reason,
                self_heals: Vec::new(),
                budget_stopped: false,
                stop: None,
            });
        }
        // 连接器失败:分类原样带出(不丢分类信息)
        return Err(EngineError::Llm(e));
    }

    // 缓存观测落库(缓存感知管线):每轮请求的命中/未命中 token 记入 llm_requests,
    // 与快照开关解耦;失败仅告警,不影响生成结果。
    // 任务模式(task: 前缀)同样跳过:llm_requests FK 到 sessions 表,任务追踪走 task_llm_calls。
    if !is_task_run {
        if let Err(e) = engine.sessions.save_llm_cache_usage(
            session_id,
            run_id,
            seq,
            &engine.model(),
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.prompt_cache_hit_tokens,
            usage.prompt_cache_miss_tokens,
        ) {
            tracing::warn!(error = e, "LLM 缓存统计落库失败");
        }
    }

    Ok(ExecutorResult {
        content,
        usage,
        interrupted: *abort.borrow(),
        tool_calls,
        reasoning,
        finish_reason,
        self_heals: Vec::new(),
        budget_stopped: false,
        stop: None,
    })
}

/// 把一段用量并入累计(TMR3-L1 的单一累加实现:返回体入账、失败尝试补记、
/// 外部实时累加器三处共用,避免字段遗漏)。
fn add_usage(dst: &mut TokenUsage, src: &TokenUsage) {
    dst.prompt_tokens += src.prompt_tokens;
    dst.completion_tokens += src.completion_tokens;
    dst.total_tokens += src.total_tokens;
    dst.context_tokens += src.context_tokens;
    dst.prompt_cache_hit_tokens += src.prompt_cache_hit_tokens;
    dst.prompt_cache_miss_tokens += src.prompt_cache_miss_tokens;
}

#[allow(clippy::too_many_arguments)]
async fn process_chunk(
    chunk: LlmStreamChunk,
    content: &mut String,
    reasoning: &mut String,
    tool_calls: &mut Vec<ToolCallArgs>,
    usage: &mut TokenUsage,
    // 外部实时累加器(TMR3-L1):说明见 execute_generation 的同名参数。
    live_usage: Option<&mut TokenUsage>,
    finish_reason: &mut Option<String>,
    registry: &crate::tools::registry::ToolRegistry,
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
    // 是否透出 Token 事件(RPFLOW-1;草稿/归档等隐藏步骤传 false)
    emit_tokens: bool,
) -> Result<bool, String> {
    if *abort.borrow() {
        return Ok(true);
    }
    match chunk {
        LlmStreamChunk::Token(text) => {
            content.push_str(&text);
            if emit_tokens {
                send_event(SseEvent::Token { text }, tx, abort, flag).await?;
            }
        }
        LlmStreamChunk::Reasoning(rc) => reasoning.push_str(&rc),
        LlmStreamChunk::ToolCall(call) => {
            if !call.id.is_empty() && !call.name.is_empty() {
                tool_calls.push(call.clone());
            }
            send_event(
                SseEvent::ToolCall {
                    name: if call.name.is_empty() {
                        "unknown".into()
                    } else {
                        call.name.clone()
                    },
                    input: Value::String(call.arguments),
                    call_id: (!call.id.is_empty()).then_some(call.id),
                    render_kind: crate::tools::registry::render_kind_for(&call.name)
                        .map(|s| s.to_string()),
                    origin: tool_origin_str(registry, &call.name),
                },
                tx,
                abort,
                flag,
            )
            .await?;
        }
        LlmStreamChunk::Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            prompt_cache_hit_tokens,
            prompt_cache_miss_tokens,
            ..
        } => {
            let delta = TokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens,
                context_tokens: 0,
                prompt_cache_hit_tokens,
                prompt_cache_miss_tokens,
            };
            add_usage(usage, &delta);
            if let Some(live) = live_usage {
                add_usage(live, &delta);
            }
        }
        // finish_reason 聚合到结果(可观测性问题①):任务模式据此落 task_llm_calls,
        // 区分「正常收尾(stop)」与「max_tokens 截断(length)」;聊天引擎不据此动作
        LlmStreamChunk::Finish { reason } => *finish_reason = Some(reason),
        // 连接器重试提示(HB-4,2026-09-18):透出为顶层 Retry 事件,界面显示
        // 「正在重试 (n/m)…」。此前重试只写 tracing,用户看到的是「停几十秒
        // 然后报错」的无解释等待。非终态事件,落到这里即视为可继续的普通块。
        LlmStreamChunk::Retry {
            attempt,
            max,
            reason,
        } => {
            send_event(
                SseEvent::Retry {
                    attempt,
                    max,
                    reason,
                },
                tx,
                abort,
                flag,
            )
            .await?;
        }
    }
    Ok(false)
}

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

/// 截断自愈重发的 max_tokens 上限(问题①):与设置页 max_tokens 校验上限
/// (`1..=131072`)同口径。2026-09-15 修正:该封顶原为 8192,低于子任务输出上限的
/// 下限(16384),导致「下限之上的单轮一旦截断,翻倍结果仍被 8192 压回」——
/// `doubled_heal_budget` 见 `min(current*2, cap) <= current` 即判定重发无意义,
/// 子 agent 的自愈被静默放弃。封顶抬到 131072 后与子任务区间自洽。
const TRUNCATION_HEAL_MAX_TOKENS_CAP: u32 = 131_072;

/// 计算自愈重发的 max_tokens(翻倍+封顶);已封顶返回 None(重发无意义,走原错误路径)。
/// 算法收敛在 `utils::retry::doubled_heal_budget`(2026-09-13 批次 4.1 四路合一),
/// 此处仅绑定本路径专属封顶值;是否重发仍由外层 `truncation_heal_cause` 判定。
fn doubled_heal_budget(current: u32) -> Option<u32> {
    crate::utils::retry::doubled_heal_budget(current, TRUNCATION_HEAL_MAX_TOKENS_CAP)
}

/// Ok 形态的截断判定(问题①):finish_reason=length 且该轮产出不可用——
/// 任一 tool_call 的 arguments 非空但非法 JSON(参数被预算切成半截,执行必败),
/// 或空正文且无 tool_call(推理烧光预算,正文为零)。普通文本截断(非空正文、
/// 无/合法工具调用)不算:半截文本也是产出,维持既有「截断仍 done」语义
/// (task_llm_call_finish_reason_marks_length_truncation 锁定)。
/// 空正文但带合法 tool_call 也不算:模型本轮只想调工具,工具调用本身就是产出
/// (heal_cause_ignores_healthy_results 锁定),误触发自愈会白白重发一轮。
fn truncation_heal_cause(res: &ExecutorResult) -> Option<String> {
    if res.finish_reason.as_deref() != Some("length") {
        return None;
    }
    // 先查半截 tool_call(成因更具体:正文为空也可能是预算被工具参数烧光)
    if let Some(bad) = res.tool_calls.iter().find(|c| {
        !c.arguments.trim().is_empty() && serde_json::from_str::<Value>(&c.arguments).is_err()
    }) {
        return Some(format!("工具参数 JSON 截断(工具 \"{}\")", bad.name));
    }
    if res.content.trim().is_empty() && res.tool_calls.is_empty() {
        // 2026-09-14 细化:区分「推理烧光预算」与「输出预算本身不足」。
        // 思考模型(DeepSeek 系等)会把 max_tokens 全花在 reasoning_content 上,
        // 正文为空但推理非空——此时成因是推理挤占,单纯翻倍往往仍需再来一轮
        // (实测 agent 模式两次自愈吃掉 37 秒,占该次请求 54%)。
        // 识别出来才能给出可诊断的提示与更合理的提升幅度。
        if !res.reasoning.trim().is_empty() {
            return Some(format!(
                "推理耗尽输出预算(推理 {} 字符,正文为空)",
                res.reasoning.chars().count()
            ));
        }
        return Some("返回空内容(已达 token 上限)".into());
    }
    None
}

/// 自愈重发的 max_tokens 计算(2026-09-14 推理感知):
/// - 普通截断(输出预算不足):沿用翻倍 + 封顶(既有语义不变);
/// - 推理耗尽预算:按「已消耗推理 + 原预算」给足,使正文有与原预算等宽的空间。
///   实测中该形态下翻倍往往一次不够、要再自愈一轮(每次一个完整 LLM 往返),
///   一次给足可省掉后续轮次。仍受 CAP 封顶,不改变既有上限纪律。
///
/// 返回 None = 已无法再提升(重发无意义,走原错误路径)。
fn heal_budget_for(res: &ExecutorResult, current: u32) -> Option<u32> {
    let doubled = doubled_heal_budget(current)?;
    // 仅当本轮确认是「推理挤占」时做加强提升;其余形态维持翻倍语义。
    let reasoning_exhausted = res.finish_reason.as_deref() == Some("length")
        && res.content.trim().is_empty()
        && res.tool_calls.is_empty()
        && !res.reasoning.trim().is_empty();
    if !reasoning_exhausted {
        return Some(doubled);
    }
    // 推理消耗以 completion_tokens 近似(思考模型该值含推理);
    // 目标 = 已消耗 + 原预算(让正文有与原预算等宽的空间)
    let spent = res.usage.completion_tokens.max(0) as u32;
    let target = spent.saturating_add(current).max(doubled);
    let capped = target.min(TRUNCATION_HEAL_MAX_TOKENS_CAP);
    (capped > current).then_some(capped)
}

/// Err 形态的截断判定(问题①):真实连接器(openai_compatible)在 finish_reason=length
/// 时把半截 tool_call 留到流尾 flush 校验,报「工具 "X" 的 arguments 不是合法 JSON」,
/// finish_reason 随错误丢失,只能按该专属错误文案判定。
/// (跨层文案耦合点:connectors/openai_compatible/sse_parser.rs flush_tool_calls,
/// 该文案变更时此处须同步。)
fn is_truncated_tool_call_error(err: &str) -> bool {
    err.contains("arguments 不是合法 JSON")
}

/// AGENT 模式工具循环:生成 → 有 tool_calls 则逐个执行并回填消息 → 重新生成。
/// 轮次上限:None = 聊天默认 32;Some(0) = 任务模式不限;Some(n>0) = 显式上限。
/// 无 tool_calls 时返回最终正文。每轮 usage 已累加进 total_usage。
/// 截断自愈(问题①):单轮生成被 max_tokens 截断到不可用(空正文 / 半截 tool_call
/// JSON / 连接器流尾 flush 校验报错)时,本轮 max_tokens 翻倍(上限
/// TRUNCATION_HEAL_MAX_TOKENS_CAP)原样重发一次,重发仍失败才透出原结果;
/// 触发自愈的记录经 ExecutorResult.self_heals 透出(任务模式补落 task_llm_calls)。
/// SSE 语义:模型发出调用时由 execute_generation 推送 ToolCall,执行完成后此处推送 ToolResult。
/// 轮次语义(修复原 8 轮 off-by-one):第 N 轮(含 N=上限)生成的工具调用照常执行,
/// 只是执行完后停止再发起新的模型请求——工具调用不会被静默丢弃,卡片不会无终态悬挂。
/// 白名单模式:步骤配置了 tools 白名单时,名单内工具自动放行(不再弹授权框)。
/// agent_session 为 None 表示任务模式(不建影子 agent_sessions 行,
/// 工具授权闸门:替代裸白名单,把「名单内放行」与「名单外如何处理」分开表达。
/// - `whitelist`:None = 非白名单模式(未放行工具走授权等待);
///   Some(list) = 仅名单内工具放行,**空名单 = 拒绝一切**。
///   名单由「本轮下发的工具集」派生,空集即「该节点一个工具都不该被调用」;
///   旧语义「空 = 全量放行」是 fail-open 的越权面(见 `遗留.md` IFW-12 附带发现,
///   2026-09-24 修正)。
/// - `no_ui_authorization`:true = 未放行工具立即拒绝并回灌错误,不进入授权等待。
///   任务模式没有 UI 授权上下文,若走等待会空等 300 秒超时,故必须置 true。
#[derive(Clone, Copy)]
pub(crate) struct ToolGate<'a> {
    pub whitelist: Option<&'a [String]>,
    pub no_ui_authorization: bool,
}

impl<'a> ToolGate<'a> {
    /// 非白名单模式,未放行则等待授权(聊天 agent 路径)
    pub(crate) fn wait() -> Self {
        Self {
            whitelist: None,
            no_ui_authorization: false,
        }
    }

    /// 显式白名单,不等待(任务 custom 步骤 / 子 agent)
    pub(crate) fn listed(whitelist: &'a [String]) -> Self {
        Self {
            whitelist: Some(whitelist),
            no_ui_authorization: true,
        }
    }

    /// 该工具是否被名单放行。
    ///
    /// **空名单不放行任何工具**:名单由「本轮下发的工具集」派生,空集意味着该节点
    /// 一个工具都没下发,此时放行一切与下发意图相反——模型臆造的工具调用会绕过
    /// 策略剔除被执行(fail-open;任务策略 all 时下发集本身非空,不依赖该语义)。
    pub(super) fn authorizes(&self, name: &str) -> bool {
        self.whitelist
            .is_some_and(|wl| wl.iter().any(|n| n == name))
    }

    /// 该工具是否被闸门硬性排除。仅对「有名单 + 不等待授权」的路径成立(任务模式):
    /// 名单是能力的硬边界,不在名单内的工具必须拒绝——否则文件规则可能因「宽松模式
    /// 写文件放行」而放过被任务策略排除的危险工具(模型幻觉调用即越权)。
    /// 聊天路径(no_ui_authorization=false)不硬性排除:名单外工具走授权等待,与改造前一致。
    /// 空名单同样走本判定(拒绝一切)——这正是 2026-09-24 修正的形态。
    pub(super) fn excludes(&self, name: &str) -> bool {
        self.no_ui_authorization && self.whitelist.is_some() && !self.authorizes(name)
    }
}

/// 单次生成 token 预算是否已达上限(HB-1,纯函数便于边界单测):
/// 预算 0 = 关闭;口径 = 本 run 内工具循环累计 prompt+completion,「达到」即算超限
/// (等于预算也停,与轮次上限的 `round >= max_rounds` 同口径)。
/// token 预算是否已达(达到即算超限)。pub(super):run_loop 的归档步闸门复用同一判定
/// (预算越线后不再追加自动调用),避免两处各写一份比较逻辑而漂移。
pub(super) fn budget_reached(used_tokens: i64, budget: u32) -> bool {
    budget > 0 && used_tokens >= budget as i64
}

/// 步骤墙钟预算是否已用尽(D3,纯函数便于边界单测):None = 不设预算(聊天路径恒此);
/// 口径 = 单次 `run_tool_loop` 的墙钟耗时,「达到」即算(与 token 预算/轮次上限同族)。
pub(super) fn step_budget_reached(
    elapsed: std::time::Duration,
    budget: Option<std::time::Duration>,
) -> bool {
    budget.is_some_and(|b| elapsed >= b)
}

/// 预算收尾时的正文选择(纯函数便于单测):本轮正文非空即用它;本轮只有工具调用
/// (工具型模型的常见形态)则回退到最近一次非空正文——预算的承诺是「带着已有产出收尾」,
/// 回一个空串会被上游判成「返回空内容」,与承诺相反。
/// 两者都空时如实返回空串(不伪造正文,口径同提交 2 的「不伪造成果」)。
pub(super) fn budget_stop_content(current: String, last_non_empty: &str) -> String {
    if current.trim().is_empty() {
        last_non_empty.to_string()
    } else {
        current
    }
}

/// 工具循环轮次上限解析(2026-10-19 任务模式取消轮次限制):
/// - `None` → 旧调用方/聊天路径缺省 32(行为不变);
/// - `Some(0)` → **不限轮次**(任务模式显式哨兵,不再因读文件/探索耗尽轮次);
/// - `Some(n>0)` → 显式上限(聊天设置、custom 节点级预算)。
///
/// 不限轮次不等于无界:步骤墙钟预算、空闲超时、语义/重复熔断、token 预算、
/// 任务总预算(若开启)与用户取消仍然生效。
pub(super) fn resolve_max_rounds(configured: Option<u32>) -> Option<usize> {
    match configured {
        Some(0) => None,
        Some(n) => Some(n.max(1) as usize),
        None => Some(32),
    }
}

/// 本次工具调用是否是「成功写入本主声明文件」的候选(P0-2 纯函数)。
///
/// 只认带 `path` 参数的工作区写工具(`fs_write`/`fs_edit`)+ 路径命中
/// `scope.declared_files()`。`bash` 刻意不算:它可写可读,且自检循环正是用
/// `bash` 跑探针;临时文件工具调用也不算——它们不推进交付物。`fs_patch` 的
/// 补丁目标暂不解析(其参数是补丁文本,不是单一路径),因此不会计为进展。
/// 结果是否成功由调用方用 `output` 判定(本函数只看调用意图与目标路径)。
pub(super) fn is_declared_file_write(name: &str, arguments: &str, declared: &[String]) -> bool {
    if !crate::tools::tool_sets::is_workspace_write_tool(name) || declared.is_empty() {
        return false;
    }
    let Ok(args) = serde_json::from_str::<Value>(arguments) else {
        return false;
    };
    let Some(raw) = args.get("path").and_then(|v| v.as_str()) else {
        return false;
    };
    let Some(key) = crate::models::ownership::normalize_workspace_rel(raw) else {
        return false;
    };
    declared.iter().any(|d| {
        crate::models::ownership::normalize_workspace_rel(d).as_deref() == Some(key.as_str())
    })
}

/// 台账增量里是否出现了「本主声明文件」的改动(TMR3-L6 纯函数)。
///
/// 台账路径是执行侧扫出的工作区相对路径,与声明侧同用 `normalize_workspace_rel`
/// 归一化后再比对;任一非法路径只跳过该行(不让单条脏数据否决整轮进展)。
pub(super) fn has_declared_change(declared: &[String], changed_paths: &[String]) -> bool {
    if declared.is_empty() {
        return false;
    }
    changed_paths.iter().any(|raw| {
        let Some(key) = crate::models::ownership::normalize_workspace_rel(raw) else {
            return false;
        };
        declared.iter().any(|d| {
            crate::models::ownership::normalize_workspace_rel(d).as_deref() == Some(key.as_str())
        })
    })
}

/// 每轮生成前的工具历史回灌裁剪(R3b,2026-09-02 实测修复:每轮全量回灌
/// assistant/tool 交替历史,3 步任务 prompt 3,119→26,679 token 无界膨胀)。
///
/// 最老轮 tool 结果原地摘要化(保留最近 K 轮完整 + token 预算双闸门;配对不破坏、
/// 幂等)。仅存在工具循环历史时触发;聊天主链路 compaction(compaction.rs)/
/// protected_tail 注入语义不受影响(不同层)。自循环体抽出的纯搬运(C-2C)。
pub(super) fn trim_round_history(
    engine: &AgentEngine,
    session_id: &str,
    round: usize,
    llm_messages: &mut [LlmMessage],
) {
    if !llm_messages.iter().any(|m| m.role == "tool") {
        return;
    }
    let (keep_rounds, budget_tokens) = {
        // 设置快照:不留锁跨 await
        let s = engine.settings_snapshot();
        (
            s.tool_history_keep_rounds as usize,
            s.tool_history_budget_tokens,
        )
    };
    let summarized_count = |msgs: &[LlmMessage]| {
        msgs.iter()
            .filter(|m| m.content.starts_with(TOOL_HISTORY_SUMMARY_PREFIX))
            .count()
    };
    let before = summarized_count(llm_messages);
    let model = engine.model();
    let outcome = {
        let mut ts = engine
            .token_service
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        trim_tool_history(llm_messages, keep_rounds, budget_tokens, &mut ts, &model)
    };
    let after = summarized_count(llm_messages);
    if after > before || outcome.reclaimed_argument_rounds > 0 {
        tracing::info!(
            session_id = session_id.to_string(),
            newly_summarized = after - before,
            reclaimed_argument_rounds = outcome.reclaimed_argument_rounds,
            keep_rounds = keep_rounds,
            budget_tokens = budget_tokens,
            total_tokens = outcome.total_tokens,
            "工具循环历史回灌截断(旧轮 tool 结果/参数已摘要化)"
        );
    }
    // 未能收敛必须可诊断,不能静默(2026-09-14):体量超过预算闸门可压缩范围
    // 时继续跑下去会持续放大 prompt,最终表现为任务长时间不收敛、token 失控。
    if !outcome.converged {
        tracing::warn!(
            session_id = session_id.to_string(),
            round = round,
            budget_tokens = budget_tokens,
            total_tokens = outcome.total_tokens,
            "工具历史压缩后仍超预算:单轮体量过大,上下文可能持续膨胀"
        );
    }
}

/// 单轮生成:看门狗 + 传输重试 + 截断自愈(C-2C 自循环体抽出,行为逐字节不变)。
///
/// 单轮内最多自愈一次:该轮被 max_tokens 截断到不可用时,把本轮 max_tokens 翻倍
/// (上限 TRUNCATION_HEAL_MAX_TOKENS_CAP)原样重发;重发仍失败(同形态再现)
/// 才透出原结果走既有错误路径。与 task_service 的空输出分级重试是同问题不同层:
/// 那边管无工具纯生成(plan/step/summary),这边管工具循环内的单轮。
///
/// 失败尝试已收到的 usage 在这里并入 `total_usage`(TMR3-L1:看门狗超时会把
/// future 直接 drop,栈上局部 usage 随 future 消失);成功路径由调用方按返回值
/// 统一入账,此处**不**并入,避免双计。截断自愈记录在成功/失败两条路径都写入
/// `self_heals`(TMR3-2:失败路径不再丢弃)。
#[allow(clippy::too_many_arguments)]
pub(super) async fn generate_round(
    engine: &AgentEngine,
    session_id: &str,
    run_id: &str,
    llm_messages: &[LlmMessage],
    params: &GenerationParams,
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
    emit_tokens: bool,
    call_watchdog: Option<std::time::Duration>,
    round: usize,
    total_usage: &mut TokenUsage,
    self_heals: &mut Vec<SelfHealRecord>,
) -> Result<ExecutorResult, EngineError> {
    let mut attempt_params = params.clone();
    let mut healed: Option<SelfHealRecord> = None;
    // TMR3-1:本轮已用掉的上游重试次数(每轮重置;上限见 MAX_TRANSPORT_RETRIES)
    let mut transport_retries = 0usize;
    let result = loop {
        // 单次调用总时长看门狗(2026-09-18):任务模式下包 300s 上限,超时按超时分类
        // 透出(任务侧照既有错误路径落 error 行并进 error 终态,不留静默挂起);
        // 聊天路径 call_watchdog=None,行为与裸调完全一致。
        let attempt_started = std::time::Instant::now();
        // TMR3-L1:本次尝试的实时 usage 累加器。看门狗超时会把 future 直接 drop,
        // execute_generation 栈上的局部 usage 随之丢失;这里的累加器在 future 外,
        // 「超时前已收到的分块」能保留下来,Err 分支再并入整轮 total_usage。
        let mut attempt_usage = TokenUsage::default();
        let one = with_call_watchdog(
            execute_generation(
                engine,
                session_id,
                run_id,
                llm_messages,
                &attempt_params,
                tx,
                abort,
                flag,
                emit_tokens,
                Some(&mut attempt_usage),
            ),
            call_watchdog,
            || {
                let limit = call_watchdog.expect("看门狗为 Some 时才会走到超时分支"); // 见 call_watchdog_for
                tracing::warn!(
                    session_id = session_id.to_string(),
                    timeout_s = limit.as_secs(),
                    elapsed_ms = attempt_started.elapsed().as_millis() as u64,
                    max_tokens = attempt_params.max_tokens,
                    "任务模式工具循环单次生成超时(总时长看门狗触发)"
                );
                crate::models::llm_error::LlmError::timeout(format!(
                    "模型调用超过 {}s 未完成(上游停滞或输出过慢),已中止本轮;可重新执行任务",
                    limit.as_secs()
                ))
                .into()
            },
        )
        .await;
        // 失败尝试(timed out / 流中断 / 鉴权失败等)没有返回值,usage 只能从这里补:
        // 成功路径由返回值统一入账,此处再并会双计。
        // 仅任务模式落到 total_usage(聊天路径的 total_usage 语义保持逐字节不变)。
        if one.is_err() && call_watchdog.is_some() {
            add_usage(&mut *total_usage, &attempt_usage);
        }
        // 已自愈过(重发仍失败):透出原结果,不再重发
        if healed.is_some() {
            break one;
        }
        // TMR3-1:单次调用遇到可重试上游失败(超时/限流/流中断)时原地重试一次。
        // 只重发**这一次**调用:llm_messages 与已完成的工具结果原样保留,不重启
        // 子目标、不重跑副作用。失败尝试已收到的 usage 由上方 attempt_usage 补记
        // (TMR3-L1);没有收到的部分上游也未计费到可见分块,无从补。
        let transport_failure: Option<String> = match &one {
            Err(e)
                if should_retry_transport(e, call_watchdog, transport_retries, *abort.borrow()) =>
            {
                Some(e.message().to_string())
            }
            _ => None,
        };
        if let Some(msg) = transport_failure {
            transport_retries += 1;
            tracing::warn!(
                session_id = session_id.to_string(),
                round = round,
                attempt = transport_retries,
                max_attempts = MAX_TRANSPORT_RETRIES,
                error = msg.as_str(),
                "任务模式单次调用失败(可重试),退避后原地重试"
            );
            let _ = send_event(
                // 用连接器同款 Retry 事件:任务侧事件桥据此清流式缓冲并播报重试状态
                // (TMR3-L2);聊天侧已有「正在重试」展示,语义一致。
                SseEvent::Retry {
                    attempt: transport_retries,
                    max: MAX_TRANSPORT_RETRIES,
                    reason: msg,
                },
                tx,
                abort,
                flag,
            )
            .await;
            if *abort.borrow() {
                break one;
            }
            tokio::time::sleep(TRANSPORT_RETRY_BACKOFF).await;
            if *abort.borrow() {
                break one;
            }
            continue;
        }
        match one {
            Ok(res) => {
                let Some(cause) = truncation_heal_cause(&res) else {
                    break Ok(res);
                };
                // 中断轮不自愈(用户停止语义优先;interrupted 时 finish_reason
                // 也可能残留 length,不得误判)
                if res.interrupted {
                    break Ok(res);
                }
                // 预算提升:推理挤占形态一次给足(见 heal_budget_for),其余走翻倍;
                // None = 已封顶,重发无意义 → 透出原结果走既有错误路径
                let Some(next_budget) = heal_budget_for(&res, attempt_params.max_tokens) else {
                    break Ok(res);
                };
                tracing::info!(
                    session_id = session_id.to_string(),
                    cause = cause.clone(),
                    max_tokens = attempt_params.max_tokens,
                    retry_max_tokens = next_budget,
                    "工具循环单轮截断,提高输出上限原样重发(截断自愈)"
                );
                let _ = send_event(
                    step_evt(
                        "截断自愈",
                        Some(format!(
                            "{cause},输出上限 {}→{next_budget} 重发",
                            attempt_params.max_tokens
                        )),
                        None,
                        None,
                    ),
                    tx,
                    abort,
                    flag,
                )
                .await;
                healed = Some(SelfHealRecord {
                    note: cause,
                    prompt_tokens: res.usage.prompt_tokens,
                    completion_tokens: res.usage.completion_tokens,
                    finish_reason: res.finish_reason.clone(),
                    retried_max_tokens: next_budget,
                });
                attempt_params.max_tokens = next_budget;
                continue;
            }
            Err(e) => {
                if !is_truncated_tool_call_error(e.message()) {
                    break Err(e);
                }
                // Err 形态拿不到本轮结果(连接器流尾校验失败),
                // 只能沿用「翻倍 + 封顶」;None = 已封顶 → 透出原错误
                let Some(next_budget) = doubled_heal_budget(attempt_params.max_tokens) else {
                    break Err(e);
                };
                tracing::info!(
                    session_id = session_id.to_string(),
                    max_tokens = attempt_params.max_tokens,
                    retry_max_tokens = next_budget,
                    "工具调用参数 JSON 截断,提高输出上限原样重发(截断自愈)"
                );
                let _ = send_event(
                    step_evt(
                        "截断自愈",
                        Some(format!(
                            "工具参数 JSON 截断,输出上限 {}→{next_budget} 重发",
                            attempt_params.max_tokens
                        )),
                        None,
                        None,
                    ),
                    tx,
                    abort,
                    flag,
                )
                .await;
                healed = Some(SelfHealRecord {
                    note: "工具参数 JSON 截断".into(),
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    finish_reason: None,
                    retried_max_tokens: next_budget,
                });
                attempt_params.max_tokens = next_budget;
                continue;
            }
        }
    };
    // Err 传播前包装截断错误文案(问题③):自愈重发仍失败时,错误带「截断」定性,
    // 任务模式据此把可读的失败原因写进步骤 result(而非连接器原始半截 JSON)。
    match result {
        Ok(r) => {
            if let Some(h) = healed {
                self_heals.push(h);
            }
            Ok(r)
        }
        Err(e) => {
            if let Some(h) = healed {
                self_heals.push(h);
                // 包装截断定性文案:分类保持原错误分类(截断不是新的失败类型)
                return Err(match e {
                    EngineError::Llm(e) => {
                        EngineError::Llm(crate::models::llm_error::LlmError::new(
                            e.kind(),
                            format!("工具参数 JSON 截断(已达 token 上限,提高预算重发仍失败): {e}"),
                        ))
                    }
                    other => EngineError::Internal(format!(
                        "工具参数 JSON 截断(已达 token 上限,提高预算重发仍失败): {other}"
                    )),
                });
            }
            Err(e)
        }
    }
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
