// 生成执行：计算器启发式工具触发、单次流式生成与 AGENT 模式工具循环
// （C-2F-2 自 executor.rs 纯搬运，逻辑不变）。
use super::super::*;
use super::heal::{
    doubled_heal_budget, heal_budget_for, is_truncated_tool_call_error, truncation_heal_cause,
};
use super::transport::{
    should_retry_transport, with_call_watchdog, MAX_TRANSPORT_RETRIES, TRANSPORT_RETRY_BACKOFF,
};
use super::{ExecutorResult, SelfHealRecord};
use std::sync::atomic::{AtomicI64, Ordering};

/// 进程级 LLM 请求快照序号(单调递增;第四点·主题 A 的 llm_requests.seq 用,
/// 跨 run 也单调,比「run 内自增」更强,prune 按 id 删除不受影响)。
static LLM_REQUEST_SEQ: AtomicI64 = AtomicI64::new(0);

/// 工具来源串(`builtin`/`plugin`/`mcp`;PLGM 3.3 的 SSE 工具事件 origin 字段填充)。
/// 未注册/未知工具返回 None(字段随 `skip_serializing_if` 省略,旧客户端零变化)。
pub(crate) fn tool_origin_str(
    registry: &crate::tools::registry::ToolRegistry,
    name: &str,
) -> Option<String> {
    registry.origin_of(name).map(|o| o.as_str().to_string())
}

/// 工具触发(每个生成周期最多一次;失败不致命)
#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_run_tool(
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
pub(crate) fn add_usage(dst: &mut TokenUsage, src: &TokenUsage) {
    dst.prompt_tokens += src.prompt_tokens;
    dst.completion_tokens += src.completion_tokens;
    dst.total_tokens += src.total_tokens;
    dst.context_tokens += src.context_tokens;
    dst.prompt_cache_hit_tokens += src.prompt_cache_hit_tokens;
    dst.prompt_cache_miss_tokens += src.prompt_cache_miss_tokens;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn process_chunk(
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

/// 每轮生成前的工具历史回灌裁剪(R3b,2026-09-02 实测修复:每轮全量回灌
/// assistant/tool 交替历史,3 步任务 prompt 3,119→26,679 token 无界膨胀)。
///
/// 最老轮 tool 结果原地摘要化(保留最近 K 轮完整 + token 预算双闸门;配对不破坏、
/// 幂等)。仅存在工具循环历史时触发;聊天主链路 compaction(compaction.rs)/
/// protected_tail 注入语义不受影响(不同层)。自循环体抽出的纯搬运(C-2C)。
pub(crate) fn trim_round_history(
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
pub(crate) async fn generate_round(
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
