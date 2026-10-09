// 生成执行:计算器启发式工具触发、单次流式生成与 AGENT 模式工具循环
use super::*;
use std::sync::atomic::{AtomicI64, Ordering};

/// 进程级 LLM 请求快照序号(单调递增;第四点·主题 A 的 llm_requests.seq 用,
/// 跨 run 也单调,比「run 内自增」更强,prune 按 id 删除不受影响)。
static LLM_REQUEST_SEQ: AtomicI64 = AtomicI64::new(0);

/// 工具来源串(`builtin`/`plugin`/`mcp`;PLGM 3.3 的 SSE 工具事件 origin 字段填充)。
/// 未注册/未知工具返回 None(字段随 `skip_serializing_if` 省略,旧客户端零变化)。
fn tool_origin_str(registry: &crate::tools::registry::ToolRegistry, name: &str) -> Option<String> {
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
    );
    send_event(
        SseEvent::ToolResult {
            name: "calculator".into(),
            output: output.clone(),
            call_id: None,
            render_kind: Some("generic".into()),
            origin: tool_origin_str(&engine.tool_registry, "calculator"),
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
fn resolve_call_watchdog(
    session_id: &str,
    call_timeout: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    call_timeout.or_else(|| call_watchdog_for(session_id))
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
                    if process_chunk(chunk, &mut content, &mut reasoning, &mut tool_calls, &mut usage, &mut finish_reason, &engine.tool_registry, tx, abort, flag, emit_tokens).await? {
                        return Ok(ExecutorResult { content, usage, interrupted: true, tool_calls, reasoning, finish_reason, self_heals: Vec::new(), budget_stopped: false });
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
    })
}

#[allow(clippy::too_many_arguments)]
async fn process_chunk(
    chunk: LlmStreamChunk,
    content: &mut String,
    reasoning: &mut String,
    tool_calls: &mut Vec<ToolCallArgs>,
    usage: &mut TokenUsage,
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
            usage.prompt_tokens += prompt_tokens;
            usage.completion_tokens += completion_tokens;
            usage.total_tokens += total_tokens;
            usage.prompt_cache_hit_tokens += prompt_cache_hit_tokens;
            usage.prompt_cache_miss_tokens += prompt_cache_miss_tokens;
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

/// AGENT 模式工具循环:生成 → 有 tool_calls 则逐个执行并回填消息 → 重新生成,
/// 轮次上限默认 32(params.max_tool_rounds 可调,settings 页配置);无 tool_calls 时返回最终正文。/// 每轮 usage 已累加进 total_usage。
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
    fn authorizes(&self, name: &str) -> bool {
        self.whitelist
            .is_some_and(|wl| wl.iter().any(|n| n == name))
    }

    /// 该工具是否被闸门硬性排除。仅对「有名单 + 不等待授权」的路径成立(任务模式):
    /// 名单是能力的硬边界,不在名单内的工具必须拒绝——否则文件规则可能因「宽松模式
    /// 写文件放行」而放过被任务策略排除的危险工具(模型幻觉调用即越权)。
    /// 聊天路径(no_ui_authorization=false)不硬性排除:名单外工具走授权等待,与改造前一致。
    /// 空名单同样走本判定(拒绝一切)——这正是 2026-09-24 修正的形态。
    fn excludes(&self, name: &str) -> bool {
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
fn step_budget_reached(elapsed: std::time::Duration, budget: Option<std::time::Duration>) -> bool {
    budget.is_some_and(|b| elapsed >= b)
}

/// 预算收尾时的正文选择(纯函数便于单测):本轮正文非空即用它;本轮只有工具调用
/// (工具型模型的常见形态)则回退到最近一次非空正文——预算的承诺是「带着已有产出收尾」,
/// 回一个空串会被上游判成「返回空内容」,与承诺相反。
/// 两者都空时如实返回空串(不伪造正文,口径同提交 2 的「不伪造成果」)。
fn budget_stop_content(current: String, last_non_empty: &str) -> String {
    if current.trim().is_empty() {
        last_non_empty.to_string()
    } else {
        current
    }
}

/// 跳过状态/工具调用落库;docs/功能.md 第三节);聊天路径恒 Some,行为不变。
/// pub(crate):任务引擎 solo/custom 模式直调(批次 4.2 起)。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_tool_loop(
    engine: &AgentEngine,
    state_machine: &mut StateMachine,
    agent_session: Option<&AgentSessionRecord>,
    session_id: &str,
    llm_messages: &mut Vec<LlmMessage>,
    params: &GenerationParams,
    tool_ctx: &ToolContext,
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
    total_usage: &mut TokenUsage,
    run_id: &str,
    // 授权闸门(替代 step_whitelist):见 ToolGate 文档
    gate: ToolGate<'_>,
    // 单次调用超时覆盖(A 批 A1):None = 用 call_watchdog_for 的既有判定(行为不变);
    // Some = 该节点每次调用的时间预算(可收紧也可放宽),唯一来源是自定义流程节点的
    // `call_timeout_secs`。放在末位是为了让既有 5 个调用点只在传值处改动。
    call_timeout: Option<std::time::Duration>,
    // 是否透出正文增量(RPFLOW-1):隐藏步骤(草稿/归档)传 false,工具调用照常透出
    emit_tokens: bool,
) -> Result<ExecutorResult, EngineError> {
    // 轮次上限:缺省 32(settings 可调);至少 1 轮,防止配置异常导致死循环
    let max_rounds = params.max_tool_rounds.unwrap_or(32).max(1) as usize;
    let mut round = 0usize;
    // 重复调用熔断(P0-2,2026-09-14):普通 agent 工具循环此前**没有任何重复调用检测**,
    // 唯一终止条件是 max_rounds。实测 plan 模式任务在单步反复调用同一工具(同参数)时,
    // 7 分钟烧 150 万 prompt token 仍未收敛,必须人工 stop。
    // 契约多步路径已有同类熔断(contracts/multi_step.rs),但只覆盖 PatchOp;
    // 此处用 utils::loop_guard 的同口径算法补上工具循环的守卫。
    // 口径 N=8 / K=3:窗口内同一「工具名+参数」指纹出现 ≥3 次即中止该步。
    let mut loop_guard = crate::utils::loop_guard::LoopGuard::with_defaults();
    // 单次调用总时长看门狗(2026-09-18):仅任务模式启用(见 call_watchdog_for 文档)。
    // 聊天路径保持无总时长上限——长回复是正常形态,加限会误伤。
    // A 批 A1:节点级覆盖优先(自定义流程节点的 `call_timeout_secs`),缺省回落既有判定。
    let call_watchdog = resolve_call_watchdog(session_id, call_timeout);
    // token 预算(HB-1):循环前取一次设置快照(与上面工具历史裁剪同款口径,
    // 不留锁跨 await);0 = 关闭。budget_noticed 保证 warn 档只提示一次。
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
    // 语义熔断三值:任务侧由调用方经 `GenerationParams.semantic_guard` 显式传入
    // **收紧后**的值(只收不放,0 = 关闭位原样保持);None = 聊天路径,用上面的扁平快照
    // ——不存在按 `task:` 前缀的嗅探,模式由调用方传值表达(见 D3 口径裁定 C2)。
    let (semantic_window, semantic_min_calls, semantic_max_distinct) = params
        .semantic_guard
        .unwrap_or((sem_window, sem_min_calls, sem_max_distinct));
    // 步骤墙钟预算(D3):None = 不设(聊天路径恒 None,行为逐字节不变);
    // 任务侧来源是设置项 `task_step_budget_secs`(装配见 task_engine::task_loop_limits)。
    let step_budget = params.step_budget;
    // 本步墙钟起点:闸门在**轮末**判定(与 token 预算/轮次上限同构:本轮工具已执行完,
    // 不发起下一轮模型请求,也不会留下「无 tool_result 的悬挂 tool_call」)。
    let loop_started = std::time::Instant::now();
    let mut budget_noticed = false;
    // 轮级滚动:最近一次非空正文(墙钟预算收尾的回退值,见轮末更新点)
    let mut last_non_empty_content = String::new();
    // 语义熔断(HB-2):看「输出的实质变化」而非参数指纹——参数每轮略变即绕开指纹熔断,
    // 实测 54 轮不同命令烧 137 万 token(遗留 L22)。登记点在工具执行收尾(输出此时可得)。
    let mut semantic_guard = crate::utils::loop_guard::SemanticGuard::new(
        semantic_window,
        semantic_min_calls,
        semantic_max_distinct,
    );
    let mut semantic_break: Option<(String, usize, usize)> = None;
    // 截断自愈留痕(问题①):各轮触发的自愈记录,随最终 ExecutorResult 透出;
    // Err 传播(?)时丢弃——失败路径由调用方落 error 行,截断细节含在错误文案内
    let mut self_heals: Vec<SelfHealRecord> = Vec::new();
    loop {
        // ===== 工具历史回灌上限(R3b,2026-09-02 实测修复:每轮全量回灌
        // assistant/tool 交替历史,3 步任务 prompt 3,119→26,679 token 无界膨胀)=====
        // 每轮生成前:最老轮 tool 结果原地摘要化(保留最近 K 轮完整 + token 预算
        // 双闸门;配对不破坏、幂等)。仅存在工具循环历史时触发;聊天主链路
        // compaction(compaction.rs)/protected_tail 注入语义不受影响(不同层)。
        if llm_messages.iter().any(|m| m.role == "tool") {
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
        // ===== 截断自愈(问题①,2026-08-31 deepseek 实测:max_tokens 被推理/长参数
        // 烧光,tool_call 参数 JSON 被切成半截直接判步骤 error)=====
        // 单轮内最多自愈一次:该轮被 max_tokens 截断到不可用时,把本轮 max_tokens 翻倍
        // (上限 TRUNCATION_HEAL_MAX_TOKENS_CAP)原样重发;重发仍失败(同形态再现)
        // 才透出原结果走既有错误路径。与 task_service 的空输出分级重试是同问题不同层:
        // 那边管无工具纯生成(plan/step/summary),这边管工具循环内的单轮。
        let mut attempt_params = params.clone();
        let mut healed: Option<SelfHealRecord> = None;
        let result = loop {
            // 单次调用总时长看门狗(2026-09-18):任务模式下包 300s 上限,超时按超时分类
            // 透出(任务侧照既有错误路径落 error 行并进 error 终态,不留静默挂起);
            // 聊天路径 call_watchdog=None,行为与裸调完全一致。
            let attempt_started = std::time::Instant::now();
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
            // 已自愈过(重发仍失败):透出原结果,不再重发
            if healed.is_some() {
                break one;
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
        // 任务模式据此把可读的失败原因写进步骤 result(而非连接器原始半截 JSON)
        let result = match result {
            Ok(r) => r,
            Err(e) => {
                if let Some(h) = healed {
                    self_heals.push(h);
                    // 包装截断定性文案:分类保持原错误分类(截断不是新的失败类型)
                    return Err(match e {
                        EngineError::Llm(e) => {
                            EngineError::Llm(crate::models::llm_error::LlmError::new(
                                e.kind(),
                                format!(
                                    "工具参数 JSON 截断(已达 token 上限,提高预算重发仍失败): {e}"
                                ),
                            ))
                        }
                        other => EngineError::Internal(format!(
                            "工具参数 JSON 截断(已达 token 上限,提高预算重发仍失败): {other}"
                        )),
                    });
                }
                return Err(e);
            }
        };
        if let Some(h) = healed {
            self_heals.push(h);
        }
        total_usage.prompt_tokens += result.usage.prompt_tokens;
        total_usage.completion_tokens += result.usage.completion_tokens;
        total_usage.total_tokens += result.usage.total_tokens;
        total_usage.prompt_cache_hit_tokens += result.usage.prompt_cache_hit_tokens;
        total_usage.prompt_cache_miss_tokens += result.usage.prompt_cache_miss_tokens;
        if result.interrupted {
            // 本轮 usage 已累进 total_usage(上方);返回体归零,避免调用方(HB-3 之后
            // 中断分支也会合并 usage)重复计数。与「无工具调用短路」的归零同理由。
            return Ok(ExecutorResult {
                usage: TokenUsage::default(),
                self_heals: std::mem::take(&mut self_heals),
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
                self_heals: std::mem::take(&mut self_heals),
                budget_stopped: false,
            });
        }
        round += 1;
        // 最近一次**非空**正文(轮级滚动):工具型模型常见「只发工具调用、不带正文」的轮,
        // 而墙钟预算闸门收尾时若恰好停在这样一轮上,返回值就是空串 → 上游按「返回空内容」
        // 判失败,与「带着已有产出收尾」的承诺相反(2026-09-26 真模型实测:该模型整轮
        // 工具调用几乎不带可见正文,delta 事件极少)。故预算收尾时回退到这里。
        if !result.content.trim().is_empty() {
            last_non_empty_content = result.content.clone();
        }
        // 本轮是否已达上限:是则执行完本轮工具后停止,不再发起新的模型请求
        let last_round = round >= max_rounds;
        // ===== 重复调用熔断(P0-2)=====
        // 对本轮每个工具调用做「工具名 + 参数」指纹登记;窗口内同一指纹累计达阈值
        // 即判定模型在原地打转,执行本轮工具后停止,理由显式推给用户(不静默截断)。
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
        let loop_broken = repeated.is_some();
        // 回填 OpenAI 标准结构:先追加一条 assistant 消息,携带本轮完整 tool_calls[] 与
        // reasoning_content,再逐条追加 tool 结果消息。旧实现为每个 call 单独追加一条
        // assistant(tool_calls=[call]),违反 OpenAI 多工具调用格式,并行工具调用时可能被
        // 严格后端拒绝(400)或丢失 reasoning_content 对应关系。
        let reasoning = (!result.reasoning.is_empty()).then_some(result.reasoning.clone());
        llm_messages.push(LlmMessage {
            role: "assistant".into(),
            content: String::new(),
            reasoning_content: reasoning,
            tool_calls: Some(result.tool_calls.clone()),
            tool_call_id: None,
            images: Vec::new(),
        });
        // ===== 本轮工具执行 =====
        // 1) 裁决(顺序,无副作用):白名单工具自动放行,其余走三档授权模式裁决
        // (任务模式无 UI 授权上下文时,未放行工具在下方执行阶段直接拒绝,不空等)
        let mut executed: Vec<ExecutedTool> = result
            .tool_calls
            .iter()
            .map(|call| {
                let registered = engine.tool_registry.get(&call.name).is_some();
                let custom_authorized = gate.authorizes(&call.name);
                let (mode, always_required) = {
                    // 设置快照:不留锁跨 await
                    let settings = engine.settings_snapshot();
                    (
                        settings.authorization_mode,
                        settings
                            .bypass_blacklist
                            .iter()
                            .any(|name| name == &call.name),
                    )
                };
                // 操作分类:读/写/删文件与系统路径区域,决定三档矩阵的走向
                let origin = engine
                    .tool_registry
                    .origin_of(&call.name)
                    .unwrap_or(crate::tools::action_class::ToolOrigin::Builtin);
                let action =
                    crate::tools::action_class::classify(&call.name, &call.arguments, origin);
                let permission = if gate.excludes(&call.name) {
                    // 任务模式名单是硬边界:名单外工具直接拒绝,不进入三档文件规则
                    // (否则宽松模式会放过被任务策略排除的写类工具)
                    crate::tools::permissions::PermissionDecision {
                        allowed: false,
                        risk: engine.tool_registry.permissions().risk_for(&call.name),
                        reason: "该工具不在当前任务策略允许的工具清单内".into(),
                    }
                } else {
                    engine.tool_registry.permissions().decide_with_policy(
                        &call.name,
                        tool_ctx,
                        registered,
                        custom_authorized,
                        mode,
                        &action,
                        always_required,
                    )
                };
                tracing::info!(
                    tool = call.name.clone(),
                    risk = format!("{:?}", permission.risk).to_lowercase(),
                    allowed = permission.allowed,
                    result = permission.reason.clone(),
                    "tool_permission"
                );
                ExecutedTool {
                    call: call.clone(),
                    permission,
                    output: Value::Null,
                    duration_ms: 0,
                }
            })
            .collect();
        // 2) 执行:按模型顺序分组调度——连续 Safe 工具组内并发(join_all 保序),
        //    非 Safe/未注册工具单独成组、串行执行(含授权等待),组间串行,
        //    保证授权语义与副作用顺序稳定。
        let parallel_safe: Vec<bool> = executed
            .iter()
            .map(|e| {
                engine.tool_registry.get(&e.call.name).is_some()
                    && engine.tool_registry.permissions().risk_for(&e.call.name)
                        == crate::tools::permissions::ToolRisk::Safe
            })
            .collect();
        let groups = split_parallel_groups(&parallel_safe);
        for group in groups {
            if group.len() > 1 || parallel_safe[group[0]] {
                // 并发组:组内全部为 Safe 工具,并发执行并按原顺序收集
                let futures: Vec<_> = group
                    .iter()
                    .map(|&idx| {
                        let e = &executed[idx];
                        let call = &e.call;
                        let perm = &e.permission;
                        let ctx = tool_ctx.clone();
                        async move { execute_call(engine, call, perm, &ctx, tx, abort, flag).await }
                    })
                    .collect();
                let outputs = futures::future::join_all(futures).await;
                for (&idx, out) in group.iter().zip(outputs) {
                    executed[idx].output = out.0;
                    executed[idx].duration_ms = out.1;
                }
            } else {
                // 串行组:单个非 Safe/未注册工具,含授权等待
                let interrupted = execute_serial_tool(
                    engine,
                    &mut executed[group[0]],
                    tool_ctx,
                    tx,
                    abort,
                    flag,
                    run_id,
                    gate,
                )
                .await?;
                if interrupted {
                    return Ok(ExecutorResult {
                        content: String::new(),
                        usage: TokenUsage::default(),
                        interrupted: true,
                        tool_calls: Vec::new(),
                        reasoning: String::new(),
                        // 工具执行期中断:生成未完成,finish_reason 不适用
                        finish_reason: None,
                        self_heals: std::mem::take(&mut self_heals),
                        budget_stopped: false,
                    });
                }
            }
        }
        // 3) 收尾(按原调用顺序):状态机/持久化/SSE 推送/模型消息回填。
        //    ToolResult 与 ToolAuthorizationRequired 均在此按序推送,保证前端配对稳定。
        for e in &executed {
            state_machine.transition_best_effort(AgentState::ToolCall, session_id);
            // 任务模式 agent_session=None(不建影子会话行):跳过 agent_sessions 落库;
            // 聊天路径恒 Some,落库顺序与原实现逐字节一致。
            if let Some(agent_session) = agent_session {
                let _ = engine.agent_sessions.update(
                    &agent_session.id,
                    Some("tool_call"),
                    None,
                    None,
                    None,
                );
                let input_val: Value = serde_json::from_str(&e.call.arguments)
                    .unwrap_or_else(|_| Value::String(e.call.arguments.clone()));
                let _ = engine.agent_sessions.add_tool_call(
                    &agent_session.id,
                    &e.call.name,
                    input_val,
                    e.output.clone(),
                    e.duration_ms,
                );
            }
            state_machine.transition_best_effort(AgentState::Executing, session_id);
            if let Some(agent_session) = agent_session {
                let _ = engine.agent_sessions.update(
                    &agent_session.id,
                    Some("executing"),
                    None,
                    None,
                    None,
                );
            }
            // 授权已在执行前等待;此处统一发送最终结果,允许与拒绝都按 call_id 收束状态。
            send_event(
                SseEvent::ToolResult {
                    name: e.call.name.clone(),
                    output: e.output.clone(),
                    call_id: Some(e.call.id.clone()),
                    render_kind: crate::tools::registry::render_kind_for(&e.call.name)
                        .map(|s| s.to_string()),
                    origin: tool_origin_str(&engine.tool_registry, &e.call.name),
                },
                tx,
                abort,
                flag,
            )
            .await?;
            // tool 结果消息:与循环前的 assistant(tool_calls) 成对,供下一轮生成参考。
            // 工具图像通道(视觉能力包 D4):约定式返回 `{text, images:[引用]}` 时,
            // 把引用解析成 data URL 挂到 tool 消息上(内容取 text 字段);其它返回形状
            // 原样透传(既有工具零改动)。SSE 的 ToolResult 沿用原值,前端按 images 渲染缩略图。
            let (tool_content, tool_images) = engine.images.parse_tool_output(&e.output);
            llm_messages.push(LlmMessage {
                role: "tool".into(),
                content: tool_content,
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some(e.call.id.clone()),
                images: tool_images,
            });
            // 语义熔断登记(HB-2):输出经归一化(抹时间戳/耗时/计数)后取指纹;
            // 同一工具在窗口内调用 ≥N 次且指纹去重 ≤K 即判定空转。只记录首个命中,
            // 收尾按与「重复调用熔断」同款方式中止(本轮工具已执行完,无悬挂)。
            if semantic_break.is_none() {
                let fp = crate::utils::loop_guard::output_fingerprint(&e.output.to_string());
                semantic_break = semantic_guard.record(&e.call.name, fp);
            }
        }
        // ===== token 预算闸门(HB-1,2026-09-18):按**成本**而非轮数收口 =====
        // 轮次上限(max_tool_rounds 默认 32)只管轮数:实测 54 轮各不相同命令的空转仍烧
        // 137 万 prompt token(遗留 L22)。预算口径 = 本轮 run 内工具循环累计
        // prompt+completion(与 Finish 事件透出的 total_tokens 同口径);0 = 关闭(默认)。
        // 判定点放在**轮末**(本轮工具已执行完,与轮次上限/熔断同构):不发起下一轮
        // 模型请求,也不会留下「无 tool_result 的悬挂 tool_call」。warn 档只提示一次
        // 后继续;stop 档跳出循环并保留已有产出。
        if token_budget > 0 {
            let used_tokens = total_usage.prompt_tokens + total_usage.completion_tokens;
            if budget_reached(used_tokens, token_budget) {
                let stop = budget_action == "stop";
                if !budget_noticed {
                    budget_noticed = true;
                    tracing::info!(
                        session_id = session_id.to_string(),
                        used_tokens,
                        budget = token_budget,
                        rounds = round + 1,
                        action = budget_action.as_str(),
                        "单次生成 token 预算超限"
                    );
                    send_event(
                        step_evt(
                            if stop {
                                "Token 预算超限,停止工具循环"
                            } else {
                                "Token 预算超限"
                            },
                            Some(format!(
                                "已用 {used_tokens} token / 预算 {token_budget}(第 {} 轮):{}",
                                round + 1,
                                if stop {
                                    "已停止工具循环,本轮产出与用量照常保留"
                                } else {
                                    "仅提示(warn 档),循环继续"
                                }
                            )),
                            None,
                            None,
                        ),
                        tx,
                        abort,
                        flag,
                    )
                    .await?;
                }
                if stop {
                    return Ok(ExecutorResult {
                        content: result.content,
                        usage: TokenUsage::default(),
                        interrupted: false,
                        tool_calls: Vec::new(),
                        reasoning: String::new(),
                        finish_reason: result.finish_reason,
                        self_heals: std::mem::take(&mut self_heals),
                        budget_stopped: true,
                    });
                }
            }
        }
        // 步骤墙钟预算(D3,2026-09-26 实测):时间/成本闸门优先于轮数闸门——
        // 任务模式每个 agent 循环各自重置轮次上限,实测单步 10 条命令反复自检(重跑测试/
        // pwd/ls/git status)、单轮往返 1.5~3 分钟,15 分钟不收敛。到点**带着已有产出收尾**:
        // 返回 Ok 让步骤照常记 done(不制造失败——失败会把已完成的工作一起丢掉)。
        if round > 0 {
            let elapsed = loop_started.elapsed();
            if step_budget_reached(elapsed, step_budget) {
                let budget_secs = step_budget.map(|b| b.as_secs()).unwrap_or(0);
                send_event(
                    step_evt(
                        "步骤墙钟预算用尽",
                        Some(format!(
                            "已用 {}s / 预算 {budget_secs}s(第 {round} 轮):已停止工具循环,本轮产出与用量照常保留",
                            elapsed.as_secs()
                        )),
                        None,
                        None,
                    ),
                    tx,
                    abort,
                    flag,
                )
                .await?;
                return Ok(ExecutorResult {
                    content: budget_stop_content(result.content, &last_non_empty_content),
                    usage: TokenUsage::default(),
                    interrupted: false,
                    tool_calls: Vec::new(),
                    reasoning: String::new(),
                    finish_reason: result.finish_reason,
                    self_heals: std::mem::take(&mut self_heals),
                    budget_stopped: true,
                });
            }
        }
        // 已达轮次上限:本轮工具已全部执行(含终态推送),停止发起新的模型请求并输出当前结果
        if last_round {
            send_event(
                step_evt(
                    "达到工具调用轮次上限",
                    Some(format!(
                        "已执行 {round} 轮工具调用,停止继续调用工具,输出当前结果"
                    )),
                    None,
                    None,
                ),
                tx,
                abort,
                flag,
            )
            .await?;
            return Ok(ExecutorResult {
                content: result.content,
                usage: TokenUsage::default(),
                interrupted: false,
                tool_calls: Vec::new(),
                reasoning: String::new(),
                finish_reason: result.finish_reason,
                self_heals: std::mem::take(&mut self_heals),
                budget_stopped: false,
            });
        }
        // 语义熔断(HB-2):同一工具在窗口内反复调用且输出实质无变化(参数可以每轮都变)。
        // 与「重复调用熔断」互补——后者抓同参数空转,本项抓「参数在变、结果不变」的空转。
        // 同样置于 last_round 之后:轮次上限是更明确的终止条件。
        if let Some((tool, calls, distinct)) = semantic_break.take() {
            let detail = format!(
                "检测到重复空转:工具 \"{tool}\" 在最近 {semantic_window} 次调用中出现 {calls} 次,输出实质无变化(去重后 {distinct} 种),已在第 {round} 轮中止。请调整策略后重试。"
            );
            tracing::warn!(
                session_id = session_id.to_string(),
                tool = tool.as_str(),
                calls,
                distinct,
                "工具循环重复空转熔断(HB-2)"
            );
            send_event(
                step_evt("重复空转熔断", Some(detail), None, None),
                tx,
                abort,
                flag,
            )
            .await?;
            return Ok(ExecutorResult {
                content: result.content,
                usage: TokenUsage::default(),
                interrupted: false,
                tool_calls: Vec::new(),
                reasoning: String::new(),
                finish_reason: result.finish_reason,
                self_heals: std::mem::take(&mut self_heals),
                budget_stopped: false,
            });
        }
        // 重复调用熔断:本轮工具已执行完(与轮次上限同款收尾),显式告知中止理由。
        // 置于 last_round 之后:轮次上限是更明确的终止条件,优先按其文案收尾。
        if loop_broken {
            let (tool, times) = repeated.expect("loop_broken 蕴含 repeated 为 Some");
            let detail = format!(
                "检测到重复调用:工具 \"{tool}\" 以相同参数在最近 {} 轮内出现 {times} 次,已在第 {round} 轮中止以避免空转。请调整策略后重试。",
                crate::utils::loop_guard::DEFAULT_WINDOW
            );
            tracing::warn!(
                session_id = session_id.to_string(),
                tool = tool.as_str(),
                repeat_count = times,
                round = round,
                "工具循环重复调用熔断"
            );
            send_event(
                step_evt("重复调用熔断", Some(detail), None, None),
                tx,
                abort,
                flag,
            )
            .await?;
            return Ok(ExecutorResult {
                // 尽量保留已产出的正文(可能为思考内容),不丢模型已有成果
                content: result.content,
                usage: TokenUsage::default(),
                interrupted: false,
                tool_calls: Vec::new(),
                reasoning: String::new(),
                finish_reason: result.finish_reason,
                self_heals: std::mem::take(&mut self_heals),
                budget_stopped: false,
            });
        }
    }
}

/// 单轮中一个待执行/已执行的工具调用(裁决结果 + 执行输出 + 耗时)
struct ExecutedTool {
    call: ToolCallArgs,
    permission: crate::tools::permissions::PermissionDecision,
    output: Value,
    /// 工具执行耗时(毫秒;未执行时为 0)
    duration_ms: i64,
}

/// 按模型顺序把工具分成「并行组」:连续 Safe 工具为同一组(组内并发),
/// 非 Safe/未注册工具各自成组(串行屏障)。返回每组在 executed 里的下标集合。
fn split_parallel_groups(parallel_safe: &[bool]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (idx, &safe) in parallel_safe.iter().enumerate() {
        if safe {
            // Safe 且上一组也是 Safe 组 → 并入,否则新开一组
            if let Some(last) = groups.last_mut() {
                if parallel_safe[last[0]] {
                    last.push(idx);
                    continue;
                }
            }
            groups.push(vec![idx]);
        } else {
            // 非 Safe → 单独成组(屏障)
            groups.push(vec![idx]);
        }
    }
    groups
}

/// 串行执行单个工具调用(含未授权时的等待授权)。返回是否被中止。
/// 与并发组不同,非 Safe/未注册工具必须走此路径,保证授权语义与副作用顺序稳定。
/// `gate.no_ui_authorization = true`(任务模式)时,未放行工具直接拒绝并回灌错误,
/// 不进入 300 秒授权等待——任务模式没有 UI 授权上下文,等待必然超时。
#[allow(clippy::too_many_arguments)]
async fn execute_serial_tool(
    engine: &AgentEngine,
    e: &mut ExecutedTool,
    tool_ctx: &ToolContext,
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
    run_id: &str,
    gate: ToolGate<'_>,
) -> Result<bool, String> {
    if *abort.borrow() {
        return Ok(true);
    }
    if !e.permission.allowed && engine.tool_registry.get(&e.call.name).is_some() {
        // 任务模式(无 UI 授权上下文):立即拒绝,不空等
        if gate.no_ui_authorization {
            e.output = json!({
                "error": e.permission.reason,
                "code": "tool_policy_denied"
            });
            return Ok(false);
        }
        let receiver = engine.tool_registry.permissions().begin_wait(
            run_id,
            &e.call.id,
            &tool_ctx.session_id,
            &e.call.name,
        );
        send_event(
            SseEvent::ToolAuthorizationRequired {
                name: e.call.name.clone(),
                risk: e.permission.risk,
                reason: e.permission.reason.clone(),
                run_id: run_id.to_string(),
                call_id: e.call.id.clone(),
                origin: tool_origin_str(&engine.tool_registry, &e.call.name),
            },
            tx,
            abort,
            flag,
        )
        .await?;
        let mut abort_wait = abort.clone();
        let timeout_secs = {
            let s = engine.settings_snapshot();
            s.tool_authorization_timeout_secs as u64
        };
        let resolved = tokio::select! {
            decision = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), receiver) => {
                match decision {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(_)) => Err(("authorization_disconnected", "授权通道已断开")),
                    Err(_) => Err(("authorization_timeout", "等待授权超时")),
                }
            }
            changed = abort_wait.changed() => {
                let _ = changed;
                Err(("authorization_disconnected", "生成连接已断开"))
            }
        };
        engine
            .tool_registry
            .permissions()
            .cancel_wait(run_id, &e.call.id);
        match resolved {
            Ok(crate::tools::permissions::PendingAuthorizationDecision::AllowOnce) => {
                e.permission = crate::tools::permissions::PermissionDecision::allowed(
                    e.permission.risk,
                    "用户允许本次调用".into(),
                );
            }
            Ok(crate::tools::permissions::PendingAuthorizationDecision::AllowSession) => {
                // 授权落盘失败(如匿名会话/磁盘异常)不应中断整轮生成:降级为「仅本次允许」
                // 并留痕,否则用户看到的是生成失败而非授权失败。
                match engine.tool_registry.permissions().authorize(
                    &e.call.name,
                    "session",
                    &tool_ctx.session_id,
                ) {
                    Ok(()) => {
                        e.permission = crate::tools::permissions::PermissionDecision::allowed(
                            e.permission.risk,
                            "用户授权当前会话".into(),
                        );
                    }
                    Err(err) => {
                        tracing::warn!(tool = e.call.name, error = %err, "会话授权落盘失败,降级为仅本次允许");
                        e.permission = crate::tools::permissions::PermissionDecision::allowed(
                            e.permission.risk,
                            format!("用户允许本次调用(会话授权未保存:{err})"),
                        );
                    }
                }
            }
            Ok(crate::tools::permissions::PendingAuthorizationDecision::AllowRole) => {
                match engine.tool_registry.permissions().authorize(
                    &e.call.name,
                    "role",
                    &tool_ctx.character_id,
                ) {
                    Ok(()) => {
                        e.permission = crate::tools::permissions::PermissionDecision::allowed(
                            e.permission.risk,
                            "用户授权当前角色".into(),
                        );
                    }
                    Err(err) => {
                        tracing::warn!(tool = e.call.name, error = %err, "角色授权落盘失败,降级为仅本次允许");
                        e.permission = crate::tools::permissions::PermissionDecision::allowed(
                            e.permission.risk,
                            format!("用户允许本次调用(角色授权未保存:{err})"),
                        );
                    }
                }
            }
            Ok(crate::tools::permissions::PendingAuthorizationDecision::Deny) => {
                e.output =
                    json!({ "error": "用户拒绝工具调用", "code": "tool_authorization_denied" });
                emit_authorization_outcome(tx, abort, flag, &e.call.name, "已拒绝").await;
                return Ok(false);
            }
            Err((code, message)) => {
                e.output = json!({ "error": message, "code": code });
                emit_authorization_outcome(tx, abort, flag, &e.call.name, message).await;
                return Ok(false);
            }
        }
    }
    let (output, duration_ms) =
        execute_call(engine, &e.call, &e.permission, tool_ctx, tx, abort, flag).await;
    e.output = output;
    e.duration_ms = duration_ms;
    Ok(false)
}

/// 授权终态事件(拒绝/超时/断开):原实现只回灌 error JSON,前端工具卡片显示为
/// 普通失败,用户无法区分「授权被拒」与「工具报错」。发一条 step 事件补足可见性。
async fn emit_authorization_outcome(
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
    tool: &str,
    outcome: &str,
) {
    let _ = send_event(
        step_evt(
            &format!("工具 {tool} 未执行"),
            Some(format!("授权结果:{outcome}")),
            None,
            None,
        ),
        tx,
        abort,
        flag,
    )
    .await;
}

/// 工具执行的取消闸门(HARNESS3-1):工具 future 与 abort 信号竞争,先到的胜出。
///
/// 为什么在**调用侧**竞争,而不是给 `ToolRegistry::execute*` 加取消形参:
/// 执行器类型 `ToolExecutor` 是 L1 公共契约(`models::types::ToolExecutor`,
/// `tools/registry.rs:17-21` 的下沉说明写明「`plugins/` 与 `mcp/` 都要构造它」),
/// 加形参等于动 L1 类型 + 全部内置工具闭包 + 两个 L3 构造点,与本批「取消只在工具循环生效」
/// 的需求不成比例。
///
/// `None` = 已取消。竞争赢时工具 future 被 drop:对 `bash` 而言这意味着 exec 层的
/// future 被丢弃,进程由 `kill_on_drop` 与 Job Object 句柄关闭收掉
/// (见 `services/exec/desktop.rs`)。
///
/// 循环而非单次 `select!` 的原因:`changed()` 返回 `Err` 只表示**发送端已全部 drop**
/// (引擎 run 收尾),那不是取消信号——若把它当取消,正常完成的工具会被误判成
/// 「已取消」;若不管它,`changed()` 会立刻重复就绪导致忙循环。故 `Err` 分支改为
/// 等 future 自己跑完,`Ok` 分支回到循环顶重读当前值。
async fn await_or_cancel<F>(fut: F, abort: &watch::Receiver<bool>) -> Option<F::Output>
where
    F: std::future::Future,
{
    tokio::pin!(fut);
    let mut wait = abort.clone();
    loop {
        // borrow() 不标记 seen:标记会让克隆出的接收端再也等不到「变化」,
        // 于是「起跑前已取消」反而要等工具自然结束(正是本条要修的行为)。
        if *wait.borrow() {
            return None;
        }
        tokio::select! {
            biased;
            value = &mut fut => return Some(value),
            changed = wait.changed() => match changed {
                Ok(()) => continue,
                Err(_) => return Some((&mut fut).await),
            }
        }
    }
}

/// 执行单个工具调用(已裁决)。未授权不执行:未注册 → 错误 JSON;已注册 → 错误 JSON
/// (授权事件由收尾阶段按序推送)。执行失败 → 结构化错误 JSON + step 提示,不终止整轮。
/// 返回 (输出, 耗时毫秒)。
#[allow(clippy::too_many_arguments)]
async fn execute_call(
    engine: &AgentEngine,
    call: &ToolCallArgs,
    permission: &crate::tools::permissions::PermissionDecision,
    tool_ctx: &ToolContext,
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
) -> (Value, i64) {
    if !permission.allowed {
        return (
            if engine.tool_registry.get(&call.name).is_none() {
                json!({ "error": format!("工具未注册:{}", call.name), "code": "tool_not_registered" })
            } else {
                json!({ "error": permission.reason, "code": "tool_authorization_required" })
            },
            0,
        );
    }
    let start = std::time::Instant::now();
    // 白名单放行:已裁决执行,避免 execute 二次权限裁决拒绝白名单危险工具
    // (旧实现外层 allowed=true 但 execute 内部再裁决,敏感/危险工具实际仍被拒)
    let executed = await_or_cancel(
        engine.tool_registry.execute_with_decision(
            &call.name,
            &call.arguments,
            tool_ctx.clone(),
            permission,
        ),
        abort,
    )
    .await;
    let out = match executed {
        // 取消:回填结构化错误(形状与「执行失败」同款,不终止整轮),真正的收场
        // 由调用侧既有的轮末 / 串行路径 abort 检查负责。这里**不**返回 Err,
        // 否则整轮被判失败会把已完成的工作一起丢掉(口径同步骤预算的「带着产出收尾」)。
        None => {
            tracing::info!(tool = call.name.as_str(), "工具执行中被取消,提前结束等待");
            return (
                json!({
                    "error": "已取消:工具在执行中被中止(用户停止)",
                    "code": "tool_cancelled"
                }),
                start.elapsed().as_millis() as i64,
            );
        }
        Some(Ok(r)) => serde_json::from_str(&r).unwrap_or(Value::String(r)),
        Some(Err(e)) => {
            let _ = send_event(
                step_evt(
                    &format!("工具 {} 调用失败", call.name),
                    Some(e.clone()),
                    None,
                    None,
                ),
                tx,
                abort,
                flag,
            )
            .await;
            json!({ "error": e })
        }
    };
    (out, start.elapsed().as_millis() as i64)
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
mod tests {
    use super::*;

    /// 任务模式名单是硬边界:名单外工具必须被闸门排除。
    /// 回归用例——曾因三档文件规则(宽松模式写文件放行)而放过被策略排除的 write。
    #[test]
    fn task_gate_hard_excludes_tools_outside_whitelist() {
        let allowed = vec!["read".to_string(), "search".to_string()];
        let gate = ToolGate::listed(&allowed);
        assert!(gate.excludes("write"), "名单外 write 必须被排除");
        assert!(!gate.excludes("read"), "名单内 read 不应被排除");
    }

    /// 聊天路径不硬性排除名单外工具:走授权等待(与改造前行为一致)
    #[test]
    fn chat_gate_does_not_hard_exclude() {
        let allowed = vec!["read".to_string()];
        let chat = ToolGate {
            whitelist: Some(&allowed),
            no_ui_authorization: false,
        };
        assert!(!chat.excludes("write"), "聊天路径名单外工具应走授权等待");
    }

    /// 非白名单模式(whitelist=None)不排除任何工具
    #[test]
    fn wait_gate_excludes_nothing() {
        assert!(!ToolGate::wait().excludes("write"));
    }

    /// 空名单 = 拒绝一切(2026-09-24 修正)。回归用例——旧语义「空 = 全量放行」曾让
    /// `custom` 节点在「策略 ∩ 白名单 = 空集」时退化成全开:模型臆造的工具调用被放行。
    #[test]
    fn empty_whitelist_denies_all() {
        let gate = ToolGate::listed(&[]);
        assert!(gate.excludes("write"), "空名单必须硬性排除一切工具");
        assert!(!gate.authorizes("write"));
    }

    #[test]
    fn split_parallel_groups_groups_safe_runs() {
        // [safe, safe, danger, safe] → [[0,1],[2],[3]]
        let flags = vec![true, true, false, true];
        assert_eq!(
            split_parallel_groups(&flags),
            vec![vec![0, 1], vec![2], vec![3]]
        );
    }

    #[test]
    fn split_parallel_groups_all_safe_single_group() {
        let flags = vec![true, true, true];
        assert_eq!(split_parallel_groups(&flags), vec![vec![0, 1, 2]]);
    }

    #[test]
    fn split_parallel_groups_no_safe_each_solo() {
        let flags = vec![false, false];
        assert_eq!(split_parallel_groups(&flags), vec![vec![0], vec![1]]);
    }

    #[test]
    fn split_parallel_groups_empty() {
        assert!(split_parallel_groups(&[]).is_empty());
    }

    // ===== 截断自愈(问题①)判定纯函数 =====

    fn mk_result(
        content: &str,
        finish: Option<&str>,
        tool_calls: Vec<(&str, &str)>,
    ) -> ExecutorResult {
        ExecutorResult {
            content: content.into(),
            usage: TokenUsage::default(),
            interrupted: false,
            tool_calls: tool_calls
                .into_iter()
                .map(|(name, arguments)| ToolCallArgs {
                    id: "c1".into(),
                    name: name.into(),
                    arguments: arguments.into(),
                })
                .collect(),
            reasoning: String::new(),
            finish_reason: finish.map(|s| s.into()),
            self_heals: Vec::new(),
            budget_stopped: false,
        }
    }

    /// 半截 tool_call JSON + finish=length → 触发,成因指向工具参数截断
    #[test]
    fn heal_cause_detects_truncated_tool_call() {
        let r = mk_result(
            "",
            Some("length"),
            vec![("calculator", "{\"expression\": \"12*3")],
        );
        let cause = truncation_heal_cause(&r).expect("半截 tool_call 应触发自愈");
        assert!(cause.contains("工具参数 JSON 截断"), "{cause}");
        assert!(cause.contains("calculator"), "{cause}");
        // 正文非空但 tool_call 半截:同样触发(工具调用不可执行)
        let r2 = mk_result(
            "思考残余",
            Some("length"),
            vec![("agentgo", "{\"tasks\": [")],
        );
        assert!(truncation_heal_cause(&r2).is_some());
    }

    /// 空正文 + finish=length(推理烧光预算)→ 触发,成因为空内容
    #[test]
    fn heal_cause_detects_empty_content_length() {
        let r = mk_result("", Some("length"), vec![]);
        let cause = truncation_heal_cause(&r).unwrap();
        assert!(cause.contains("空内容"), "{cause}");
    }

    /// 不自愈的形态:普通文本截断(半截文本也是产出,既有语义)、正常收尾、
    /// 合法 tool_call、无 finish_reason
    #[test]
    fn heal_cause_ignores_healthy_results() {
        // 半截文本截断:不触发(维持「截断仍 done」语义)
        assert!(
            truncation_heal_cause(&mk_result("这段成果被截断,后半", Some("length"), vec![]))
                .is_none()
        );
        // 正常 stop:不触发
        assert!(truncation_heal_cause(&mk_result("", Some("stop"), vec![])).is_none());
        // length 但 tool_call 参数完整:正常执行,不触发
        assert!(truncation_heal_cause(&mk_result(
            "",
            Some("length"),
            vec![("calculator", "{\"expression\":\"1+1\"}")],
        ))
        .is_none());
        // 无 finish_reason:不触发
        assert!(truncation_heal_cause(&mk_result("", None, vec![])).is_none());
    }

    /// P2-1(2026-09-14):思考模型「推理耗尽预算」的成因必须被识别出来,
    /// 与「输出预算本身不足」区分——前者需要一次给足预算,后者翻倍即可。
    #[test]
    fn heal_cause_identifies_reasoning_exhausted_budget() {
        let mut r = mk_result("", Some("length"), vec![]);
        r.reasoning = "推理内容很长".repeat(100);
        let cause = truncation_heal_cause(&r).unwrap();
        assert!(
            cause.contains("推理耗尽输出预算"),
            "应识别为推理挤占: {cause}"
        );
        assert!(cause.contains("字符"), "应带上推理长度便于诊断: {cause}");
    }

    /// 推理挤占时预算一次给足(而非仅翻倍),减少一次完整 LLM 往返;
    /// 且严格受 CAP 封顶、不超上限。
    #[test]
    fn heal_budget_gives_enough_for_reasoning_exhaustion() {
        let mut r = mk_result("", Some("length"), vec![]);
        r.reasoning = "思考".repeat(200);
        // 已消耗 6000(推理占满),原预算 4000 → 翻倍仅 8000;
        // 推理感知应给 6000+4000=10000(新 CAP 131072 之下一路放行)
        r.usage.completion_tokens = 6000;
        let next = heal_budget_for(&r, 4000).unwrap();
        assert_eq!(next, 10000, "推理挤占时按已消耗+原预算给足");
        assert!(next > 8000, "应比单纯翻倍给得更多: {next}");
        // 封顶仍生效:给足值超过 CAP 时收敛到 CAP
        r.usage.completion_tokens = 200_000;
        assert_eq!(
            heal_budget_for(&r, 4000),
            Some(TRUNCATION_HEAL_MAX_TOKENS_CAP),
            "推理感知结果同样受 CAP 约束"
        );
    }

    /// 非推理形态维持既有「翻倍」语义(不改变原有行为)
    #[test]
    fn heal_budget_keeps_doubling_for_plain_truncation() {
        let r = mk_result("", Some("length"), vec![]);
        assert_eq!(heal_budget_for(&r, 1000), Some(2000), "无推理时仅翻倍");
        // 已封顶 → None(重发无意义)
        assert_eq!(heal_budget_for(&r, TRUNCATION_HEAL_MAX_TOKENS_CAP), None);
        assert_eq!(
            heal_budget_for(&r, 100_000),
            Some(TRUNCATION_HEAL_MAX_TOKENS_CAP),
            "翻倍后受 CAP 收敛"
        );
    }

    /// Err 形态判定:仅匹配连接器半截 tool_call flush 的专属文案
    #[test]
    fn heal_err_matches_connector_truncation_wording() {
        assert!(is_truncated_tool_call_error(
            "工具 \"agentgo\" 的 arguments 不是合法 JSON: {\"tasks\": ["
        ));
        assert!(!is_truncated_tool_call_error("上游连接超时"));
        assert!(!is_truncated_tool_call_error("生成已中断"));
    }

    /// 预算翻倍+封顶:1024→2048;8192→16384;封顶值 131072 之上不再重发(None)
    #[test]
    fn heal_budget_doubles_with_cap() {
        assert_eq!(doubled_heal_budget(1024), Some(2048));
        assert_eq!(doubled_heal_budget(4096), Some(8192));
        assert_eq!(doubled_heal_budget(8192), Some(16384));
        assert_eq!(doubled_heal_budget(131_072), None, "已封顶不重发");
        assert_eq!(doubled_heal_budget(u32::MAX), None, "饱和相乘不得溢出");
    }

    /// 2026-09-15 回归:封顶抬到 131072 后,子任务下限(16384)之上的单轮截断
    /// 仍能翻倍重发;旧封顶 8192 会让 16384 判定「重发无意义」而静默放弃自愈。
    #[test]
    fn heal_budget_still_grows_above_new_subagent_floor() {
        assert_eq!(
            doubled_heal_budget(16_384),
            Some(32_768),
            "下限之上的截断必须仍可自愈(旧封顶 8192 会返回 None)"
        );
        assert_eq!(doubled_heal_budget(65_536), Some(131_072));
    }

    /// token 预算判定边界(HB-1):预算 0 恒不触发;「达到」即算超限(与轮次上限同口径)
    #[test]
    fn budget_reached_boundaries() {
        assert!(!budget_reached(0, 0), "预算 0 = 关闭");
        assert!(!budget_reached(9_999, 0), "预算 0 时与用量无关");
        assert!(!budget_reached(1023, 1024), "低于预算不触发");
        assert!(budget_reached(1024, 1024), "等于预算即触发(达到即算超限)");
        assert!(budget_reached(1025, 1024), "超过预算触发");
        assert!(budget_reached(i64::MAX, 1), "极值不溢出");
    }

    /// 步骤墙钟预算判定边界(D3):None 恒不触发(聊天路径不放这个字段,行为不变);
    /// 「达到」即算用尽(与 token 预算/轮次上限同族口径)
    #[test]
    fn step_budget_reached_boundaries() {
        use std::time::Duration;
        assert!(
            !step_budget_reached(Duration::from_secs(9_999), None),
            "None = 不设预算,永不由本闸门收口"
        );
        assert!(
            !step_budget_reached(Duration::from_millis(999), Some(Duration::from_secs(1))),
            "低于预算不触发"
        );
        assert!(
            step_budget_reached(Duration::from_secs(1), Some(Duration::from_secs(1))),
            "等于预算即触发(达到即算用尽)"
        );
        assert!(
            step_budget_reached(Duration::from_secs(2), Some(Duration::from_secs(1))),
            "超过预算触发"
        );
    }

    /// 预算收尾的正文选择(D3):本轮有正文用本轮;只有工具调用则回退最近一次非空;
    /// 两者皆空如实返回空(不伪造)
    #[test]
    fn budget_stop_content_prefers_current_then_falls_back() {
        assert_eq!(
            budget_stop_content("本轮正文".into(), "更早的正文"),
            "本轮正文",
            "本轮非空即用本轮"
        );
        assert_eq!(
            budget_stop_content(String::new(), "更早的正文"),
            "更早的正文",
            "本轮只有工具调用时回退最近非空正文(否则会被判「返回空内容」)"
        );
        assert_eq!(
            budget_stop_content("   \n".into(), "更早的正文"),
            "更早的正文",
            "纯空白等同空"
        );
        assert_eq!(
            budget_stop_content(String::new(), ""),
            "",
            "两者皆空如实返回空:不伪造正文"
        );
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::await_or_cancel;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::watch;

    /// **正常路径必须仍然放行**(与下面两条同权重):取消守卫一旦误判,表现不是报错
    /// 而是「工具永远返回已取消」——比现状(取消打断不了工具)更糟,所以必须单独钉。
    #[tokio::test]
    async fn not_cancelled_returns_tool_output() {
        let (_tx, rx) = watch::channel(false);
        let got = await_or_cancel(async { "工具输出" }, &rx).await;
        assert_eq!(got, Some("工具输出"), "未取消时必须原样带回工具输出");
    }

    /// 本批的正面断言:工具执行途中取消 → **立即**返回,不等工具自身时长。
    /// 修复前该用例要等满 5s(工具自身 sleep)才返回,故时间余量给到 1s 足以判别。
    #[tokio::test]
    async fn cancel_during_tool_execution_returns_immediately() {
        let (tx, rx) = watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let _ = tx.send(true);
        });
        let start = Instant::now();
        let got = await_or_cancel(tokio::time::sleep(Duration::from_secs(5)), &rx).await;
        let elapsed = start.elapsed();
        assert!(got.is_none(), "取消途中应返回 None(实际: {got:?})");
        assert!(
            elapsed < Duration::from_secs(1),
            "取消应在毫秒级生效,实际等了一轮工具时长: {elapsed:?}"
        );
    }

    /// 起跑前已置位:直接判取消,且**不轮询**工具 future(否则副作用已经发生才说「取消了」)。
    #[tokio::test]
    async fn already_aborted_before_start_skips_tool() {
        let (tx, rx) = watch::channel(true);
        let polled = Arc::new(AtomicBool::new(false));
        let flag = polled.clone();
        let got = await_or_cancel(
            async move {
                flag.store(true, Ordering::SeqCst);
                1u8
            },
            &rx,
        )
        .await;
        assert_eq!(got, None);
        assert!(
            !polled.load(Ordering::SeqCst),
            "已取消时不该把工具放下去跑(否则取消只是事后通知)"
        );
        drop(tx);
    }

    /// `changed()` 返回 Err 只代表**发送端已全部 drop**(run 收尾的正常路径),那不是取消
    /// 信号——必须等工具跑完并带回结果。此用例同时兜住「Err 分支被误写成 continue 造成忙循环」。
    #[tokio::test]
    async fn dropped_sender_is_not_treated_as_cancel() {
        let (tx, rx) = watch::channel(false);
        drop(tx);
        let start = Instant::now();
        let got = await_or_cancel(tokio::time::sleep(Duration::from_millis(40)), &rx).await;
        assert!(got.is_some(), "发送端断开不等于取消,应等工具完成");
        assert!(
            start.elapsed() >= Duration::from_millis(30),
            "应真的等完工具,而不是立刻返回"
        );
    }
}

#[cfg(test)]
mod watchdog_tests {
    use super::{
        call_watchdog_for, resolve_call_watchdog, with_call_watchdog, EngineError,
        TASK_TOOL_LOOP_CALL_TIMEOUT,
    };
    use std::time::Duration;

    /// 任务模式虚拟 session(`task:` 前缀)必须启用总时长看门狗——这是「任务无限停在
    /// running」缺口的封堵点,漏接线则该缺口原样存在。
    #[test]
    fn watchdog_enabled_for_task_sessions_only() {
        assert_eq!(
            call_watchdog_for("task:57cf15f9-0508-440a-b1a3-ae2762b58581"),
            Some(TASK_TOOL_LOOP_CALL_TIMEOUT),
            "任务模式必须启用"
        );
        assert_eq!(
            call_watchdog_for("task:abc:main:3:sub:def"),
            Some(TASK_TOOL_LOOP_CALL_TIMEOUT),
            "team/子 agent 的层叠 session 同样以 task: 开头,应一并启用"
        );
    }

    /// 聊天路径不得启用:长回复是正常形态,加总时长会把正常生成判成失败。
    #[test]
    fn watchdog_disabled_for_chat_sessions() {
        assert_eq!(call_watchdog_for("session-123"), None);
        assert_eq!(call_watchdog_for(""), None);
        // 边界:仅前缀匹配,含 task 但不以其开头的不算
        assert_eq!(call_watchdog_for("mytask:1"), None);
    }

    /// 节点级覆盖(A 批 A1):配了就必须**原样照用**——收紧(60s)与放宽(900s)都是合法用法,
    /// 故实现不能写成 `min(覆盖, 缺省)`,否则「给慢节点放宽」这条诉求会被静默吃掉。
    #[test]
    fn node_call_timeout_overrides_default() {
        let tight = Duration::from_secs(60);
        let loose = Duration::from_secs(900);
        assert_eq!(
            resolve_call_watchdog("task:t1", Some(tight)),
            Some(tight),
            "任务会话:节点级覆盖优先于既有 300s"
        );
        assert_eq!(
            resolve_call_watchdog("task:t1", Some(loose)),
            Some(loose),
            "放宽用法同样生效(不是 min 语义)"
        );
        assert_eq!(
            resolve_call_watchdog("session-1", Some(tight)),
            Some(tight),
            "聊天会话:节点级覆盖也照用(该字段只由任务侧 PlanStep 提供,聊天侧恒为 None)"
        );
    }

    /// 未配覆盖时逐字节维持既有判定(任务会话 300s / 聊天会话无上限)。
    #[test]
    fn absent_node_call_timeout_keeps_default() {
        assert_eq!(
            resolve_call_watchdog("task:t1", None),
            Some(TASK_TOOL_LOOP_CALL_TIMEOUT)
        );
        assert_eq!(resolve_call_watchdog("session-1", None), None);
        assert_eq!(resolve_call_watchdog("", None), None);
    }

    /// 未超时:结果原样透出,错误路径不受影响。
    #[tokio::test]
    async fn watchdog_passes_through_when_within_limit() {
        let out: Result<u32, EngineError> =
            with_call_watchdog(async { Ok(42) }, Some(Duration::from_millis(500)), || {
                EngineError::Internal("不应触发".into())
            })
            .await;
        assert_eq!(out.unwrap(), 42);
    }

    /// 超时:放弃 future 并返回分类化超时错误(不是字符串猜测)。
    #[tokio::test]
    async fn watchdog_times_out_and_reports_timeout_kind() {
        let out: Result<u32, EngineError> = with_call_watchdog(
            async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(1)
            },
            Some(Duration::from_millis(50)),
            || {
                EngineError::Llm(crate::models::llm_error::LlmError::timeout(
                    "模型调用超过 300s 未完成",
                ))
            },
        )
        .await;
        let err = out.expect_err("超过阈值必须超时");
        assert!(
            err.message().contains("超过 300s 未完成"),
            "实际:{}",
            err.message()
        );
        // 分类须为超时:任务侧据此落 error 行,聊天路径据此映射错误码
        match err {
            EngineError::Llm(e) => assert_eq!(
                e.kind(),
                crate::models::llm_error::LlmErrorKind::Timeout,
                "必须是超时分类,而不是内部错误"
            ),
            other => panic!("应为 EngineError::Llm(超时分类),实际 {other:?}"),
        }
    }

    /// None = 不限时:future 按原样等待,行为与裸调一致(聊天路径语义)。
    #[tokio::test]
    async fn watchdog_none_awaits_without_limit() {
        let out: Result<u32, EngineError> = with_call_watchdog(
            async {
                tokio::time::sleep(Duration::from_millis(80)).await;
                Ok(7)
            },
            None,
            || EngineError::Internal("不应触发".into()),
        )
        .await;
        assert_eq!(out.unwrap(), 7);
    }
}
