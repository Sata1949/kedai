// solo 模式:单主 agent 工具自循环——目标直接进 run_tool_loop,工具集按
// task_tool_policy 编译(默认 deny_dangerous:危险级与元工具除外、bash 例外),
// 步数上限 max_tool_rounds(docs/功能.md 第一节)。
// 复用聊天引擎 run_tool_loop,不建影子 sessions 行(session_id 用 task: 前缀虚拟 id,
// llm_requests 落库在引擎侧据此跳过;任务侧追踪走 task_llm_calls,phase=agent)。
// run_agent_loop 为「单主 agent 工具自循环」共享骨架:solo/multi 执行器与
// team 模式的各主 agent 复用同一实现(批次 4.3b),差异仅在运行身份/追踪字段。
use super::context::TaskRunContext;
use super::executor::{usage_as_output, ModeExecutor};
use super::sink;
use crate::agents::engine::executor::run_tool_loop;
use crate::agents::engine::{AbortFlag, AgentEngine};
use crate::agents::state_machine::{AgentState, StateMachine};
use crate::models::types::{
    GenerationParams, LlmMessage, TaskStatus, TokenUsage, ToolChoice, ToolContext,
};
use crate::services::settings_service::RuntimeSettings;
use crate::services::task_core::{TaskBackend, TaskTerminal};
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;
use uuid::Uuid;

/// 单主 agent 工具自循环的入参(owned,便于 tokio::spawn/JoinSet 跨 'static 边界)。
pub(crate) struct AgentLoopCall {
    /// 任务 id(调用追踪/usage 落库/事件桥用)
    pub task_id: String,
    /// 运行身份(虚拟 session_id):solo/multi = task:{id};team 主 agent = task:{id}:main:{n}
    pub session_id: String,
    /// user 消息正文(solo/multi = 任务目标;team 主 = 总体目标 + 该主分工文本)
    pub goal: String,
    /// 任务模式有效设置快照(执行全程读快照,与 legacy 同语义)
    pub settings: RuntimeSettings,
    /// 执行者库 id(空 = 通用执行者)。与 character_id 的优先级见
    /// TaskPromptKit::assemble_executor_system_prompt:executor_id 命中即独占身份段。
    pub executor_id: Option<String>,
    /// 执行者人设角色 id(空 = 通用执行者)。兼容字段,仅为旧任务保留。
    pub character_id: Option<String>,
    /// 调用追踪 phase(solo/multi/team 主 agent 均为 "agent")
    pub phase: &'static str,
    /// 调用追踪 step_index(team/plan 续跑 = 子目标/步骤在 plan 中的全局下标;
    /// solo/multi = None)
    pub step_index: Option<usize>,
    /// 事件桥文案称谓(「主 agent」/team 的「主 agent N」)
    pub label: String,
    /// 任务级连接(A 批 B1;空 = 跟随设置的默认连接)。
    /// 与节点级 `PlanStep.connection_id` 不同:solo/multi/team/plan 无节点概念,
    /// 这里是本任务模型调用的缺省连接。
    pub connection_id: Option<String>,
    /// **工作区作用域**(编码通道批次 1;空 = 未绑定工作区)。从执行上下文原样下传,
    /// 供工具循环构造 `ToolContext.scope`(fs_* 工具族与 bash 的 cwd jail 消费它)。
    pub scope: Option<Arc<crate::models::types::ExecScope>>,
}

/// 单主 agent 工具自循环(solo/multi 执行器主体;team 各主 agent 复用):
/// system 组装(内置执行者指令 → 人设 → 世界书 → 提示词注入 → Agent 提示词,
/// 外部段落逐一 untrusted 包裹)→ run_tool_loop(工具按 task_tool_policy 编译 +
/// ToolGate 闸门,恒不等待授权:名单外工具立即拒绝)→ 调用追踪统一出口落 task_llm_calls(成功/空/中断/错误均一行)。
/// 成功返回 (正文, 整轮累计 usage);中断/错误/空内容返回 Err。
pub(crate) async fn run_agent_loop(
    svc: Arc<dyn TaskBackend>,
    engine: Arc<AgentEngine>,
    call: AgentLoopCall,
    cancel: watch::Receiver<bool>,
) -> Result<(String, TokenUsage), String> {
    let settings = &call.settings;

    // 工具按任务策略下发(批次授权改造):默认拒绝危险工具;元工具不入正文列表。
    // 任务模式无 UI 授权上下文,闸门恒 no_ui_authorization = true:
    // 名单外工具立即拒绝并回灌错误,不会空等 300 秒授权超时。
    let policy = super::tool_policy::compile(
        &settings.task_tool_policy,
        &settings.task_tool_allowlist,
        &engine.tool_registry(),
        // 工作区工具族随「作用域是否存在」下发:任务自 D1 起恒有作用域(绑定工作区用
        // 工作区,未绑定则用任务 scratch,见 task_engine::run_inner),故 fs_* 对任务恒可见;
        // 传 `scope.is_some()` 而非恒 true,是为守住「没有作用域就不给文件工具」这条不变量
        // (作用域缺失时那些工具必然报错,不如不给)。
        call.scope.is_some(),
        // 包专属工具(fs_patch)随编码能力包开关下发(task 合并值;关包整族剔除,
        // 见 tool_policy::coding_pack_gate)
        settings.task_coding_bundle_enabled,
        // 视觉三件随生效连接的「视觉输入」能力位(口径单一出处;见 tool_policy::vision_gate)
        crate::services::settings_service::vision_enabled(settings),
        // 截图工具随「视觉与截图」总开关(默认关;见 tool_policy::screenshot_gate)
        settings.vision_screenshot_enabled,
    );
    // 闸门名单先取出(allowed 借用生命周期需覆盖整个工具循环),再取走 defs
    let allowed = policy.allowed;
    // 本轮**实际**下发的工具是否非空(策略收窄到空集时不得声称有工具):
    // 既决定 system 是否追加工具纪律段,也与「能力事实」的实际形态一致。
    // 必须在 `tools: policy.defs` 把 defs 移走之前算。
    let has_tools = !policy.defs.is_empty();
    // 视觉验证纪律段的条件(D4):工具面含视觉三件才追加(单一出处 tool_sets::VISION_TOOLS)
    let has_vision_tools = allowed
        .iter()
        .any(|n| crate::tools::tool_sets::VISION_TOOLS.contains(&n.as_str()));
    // system 提示词组装:统一走 TaskBackend::assemble_executor_system_prompt
    // (单一实现,宿主侧 prompt.rs);拼装顺序与 untrusted 包裹纪律同 legacy,
    // 勿在本文件复制实现(WP7)。
    let sys = svc.assemble_executor_system_prompt(
        settings,
        call.executor_id.as_deref(),
        call.character_id.as_deref(),
        &call.goal,
        has_tools,
        has_vision_tools,
    );

    let mut messages = vec![
        LlmMessage::plain("system", &sys),
        LlmMessage::plain("user", &call.goal),
    ];

    // 任务侧工具循环的两道限制(提交 3 · D3):单步墙钟预算 + 语义熔断收紧,
    // 与 custom 的工具节点共用同一映射(单一出处,见 task_loop_limits 文档)。
    let (step_budget, semantic_guard) = super::task_loop_limits(settings);
    // 执行者建议温度(TM-GEN-1):命中执行者库且配置了温度时优先于任务有效缺省温度;
    // 未配置/无执行者 = 沿用缺省(单点解析式见 TaskPromptKit::executor_temperature,
    // generate_step 侧同一口径)。
    let temperature = svc
        .executor_temperature(call.executor_id.as_deref())
        .unwrap_or(settings.default_temperature);
    let params = GenerationParams {
        temperature,
        top_p: settings.default_top_p,
        max_tokens: settings.default_max_tokens,
        stop: None,
        tools: policy.defs,
        max_tool_rounds: Some(settings.max_tool_rounds),
        tool_choice: ToolChoice::Auto,
        // 任务级连接(A 批 B1):solo 无节点级连接可配,直接用任务绑定的那条
        // (None = 默认连接,与本批之前一致)
        connection_id: call.connection_id.clone(),
        parallel_tool_calls: None,
        step_budget,
        semantic_guard,
    };
    let gate = crate::agents::engine::executor::ToolGate::listed(&allowed);

    // 运行身份:虚拟 session_id(task: 前缀,不建 sessions/agent_sessions 影子行);
    // 状态迁移校验:非法迁移记 warn 日志(StateMachine 会返回 Err,不得静默丢弃);
    // run_id 标识本轮(工具授权等待的 key,白名单模式下用不到)。
    let mut state_machine = StateMachine::new(&call.session_id);
    let run_id = Uuid::new_v4().to_string();
    // AbortFlag 仅供 send_event 的「通道断开」置位;中断信号本体是任务取消通道
    // cancel(register_cancel 登记,stop 经 signal_cancel 触发)。
    let (flag, _flag_rx) = AbortFlag::new();
    let tool_ctx = ToolContext {
        session_id: call.session_id.clone(),
        character_id: call.character_id.clone().unwrap_or_default(),
        agent_depth: 0,
        scope: call.scope.clone(),
    };
    // 事件桥:引擎事件 → 任务事件(agent_status);drain 持续消费到 tx drop。
    // phase/step_index 随桥传入(批次 R4):Token 攒批 delta 携带调用归属,
    // 与下方 record_llm_call 落库行同口径(前端按 key 对齐暂态与权威)
    let (tx, drain) = sink::spawn(
        svc.clone(),
        call.task_id.clone(),
        &call.label,
        call.phase,
        call.step_index,
    );
    let mut total_usage = TokenUsage::default();
    let started = Instant::now();
    // Idle → Executing 为任务/工具路径的合法首迁(无独立规划阶段,见 state_machine.rs);
    // 仍记录非法迁移,避免静默吞掉状态机校验结果。
    if let Err(e) = state_machine.transition(AgentState::Executing, &call.session_id) {
        tracing::warn!(error = %e, session = %call.session_id, "状态迁移被拒");
    }
    let mut result = run_tool_loop(
        &engine,
        &mut state_machine,
        None, // 任务模式无 agent_sessions 行:跳过状态/工具调用落库
        &call.session_id,
        &mut messages,
        &params,
        &tool_ctx,
        &tx,
        &cancel,
        &flag,
        &mut total_usage,
        &run_id,
        gate,
        // 单次调用超时覆盖(A 批 A1):solo 无节点级配置,恒走宿主既有判定
        None,
    )
    .await;
    let model = engine.model();

    // ===== 空正文收尾重试(TM-EMPTY-1)=====
    // 工具循环正常结束(未中断/未取消)但正文 trim 后为空时——主成因是推理吃光输出预算,
    // 或「只发工具调用、不给正文」的收尾轮——追加**一次**无工具单轮收尾:user 提醒 +
    // 与 `generate_step_retry` 同款的分级参数(length→预算翻倍含下限、温度保持;
    // 其他/无原因→同预算、温度 0.7)。仍空才走原 `empty_output_error` 错误路径;
    // 硬错误(Err)/用户中断不重试(既有语义)。影响面:本函数是 solo/multi/plan 续跑/
    // team 主 agent/followup 五条链路的单一收口点。
    // 记账:首轮(空)先单独落一行(usage=首轮累计),最终行取**增量**(total − 首轮)
    // ——usage_total 由调用方按整轮累计一次入账,「各行 token 求和 == usage_total」不变量保持。
    let mut retry_budget_used: Option<u32> = None;
    let mut usage_before_retry = TokenUsage::default();
    if let Ok(res) = &result {
        if !res.interrupted && !*cancel.borrow() && res.content.trim().is_empty() {
            svc.record_self_heals(
                &call.task_id,
                call.phase,
                call.step_index,
                &model,
                &messages,
                &super::executor::to_self_heals(&res.self_heals),
            );
            let mut out = usage_as_output(&total_usage);
            out.finish_reason = res.finish_reason.clone();
            svc.record_llm_call(
                &call.task_id,
                call.phase,
                call.step_index,
                &model,
                &messages,
                "",
                Some(&out),
                started.elapsed(),
                "empty",
            );
            let reason = res.finish_reason.as_deref().unwrap_or("");
            let (retry_max_tokens, retry_temperature) = if reason == "length" {
                (
                    super::retry::truncated_retry_budget(params.max_tokens),
                    params.temperature,
                )
            } else {
                (params.max_tokens, 0.7)
            };
            tracing::warn!(
                task_id = call.task_id.as_str(),
                finish_reason = reason,
                max_tokens = params.max_tokens,
                retry_max_tokens,
                "任务执行者空正文:追加一次无工具收尾轮"
            );
            usage_before_retry = total_usage.clone();
            retry_budget_used = Some(retry_max_tokens);
            messages.push(LlmMessage::plain(
                "user",
                crate::services::task_core::prompt_consts::EXECUTOR_FINAL_NUDGE,
            ));
            let mut retry_params = params.clone();
            retry_params.tools = Vec::new();
            retry_params.max_tool_rounds = Some(1);
            retry_params.max_tokens = retry_max_tokens;
            retry_params.temperature = retry_temperature;
            // 不下发任何工具:空名单闸门(fail-closed)——意外工具调用被拒,不放行
            let retry_gate = crate::agents::engine::executor::ToolGate::listed(&[]);
            result = run_tool_loop(
                &engine,
                &mut state_machine,
                None,
                &call.session_id,
                &mut messages,
                &retry_params,
                &tool_ctx,
                &tx,
                &cancel,
                &flag,
                &mut total_usage,
                &run_id,
                retry_gate,
                None,
            )
            .await;
        }
    }
    // 先关通道再等 drain 收尾,保证进度事件全部转发完毕
    drop(tx);
    let _ = drain.await;

    // 调用追踪统一出口:成功/空/中断/错误均落一行 task_llm_calls;
    // token 用整轮累计 total_usage(重试场景取**增量**:首轮已单独落行,防双计);
    // messages 此时含完整工具循环历史(含收尾提醒),摘要自截断。
    let elapsed = started.elapsed();
    let row_usage = if retry_budget_used.is_some() {
        TokenUsage {
            prompt_tokens: total_usage.prompt_tokens - usage_before_retry.prompt_tokens,
            completion_tokens: total_usage.completion_tokens - usage_before_retry.completion_tokens,
            total_tokens: total_usage.total_tokens - usage_before_retry.total_tokens,
            context_tokens: total_usage.context_tokens - usage_before_retry.context_tokens,
            prompt_cache_hit_tokens: total_usage.prompt_cache_hit_tokens
                - usage_before_retry.prompt_cache_hit_tokens,
            prompt_cache_miss_tokens: total_usage.prompt_cache_miss_tokens
                - usage_before_retry.prompt_cache_miss_tokens,
        }
    } else {
        total_usage.clone()
    };
    match result {
        Ok(res) if !res.interrupted => {
            // 截断自愈留痕落库(问题①):被截断的调用补落一行 + 补 usage,
            // 统一实现见 TaskService::record_self_heals(与 custom 共用)。
            svc.record_self_heals(
                &call.task_id,
                call.phase,
                call.step_index,
                &model,
                &messages,
                &super::executor::to_self_heals(&res.self_heals),
            );
            let text = res.content.trim().to_string();
            let status = if text.is_empty() { "empty" } else { "ok" };
            // token/字段构造统一走 usage_as_output(批次 B.4 单一出处);text 由
            // record_llm_call 的 response 参数单独承载,finish_reason 是本调用特有的
            // 诊断(问题①)故单独覆盖——上游未下发时为 None,落库 ''。
            let mut out = usage_as_output(&row_usage);
            out.finish_reason = res.finish_reason.clone();
            svc.record_llm_call(
                &call.task_id,
                call.phase,
                call.step_index,
                &model,
                &messages,
                &text,
                Some(&out),
                elapsed,
                status,
            );
            if text.is_empty() {
                // 空内容错误文案统一走单一出处(提交 3 · D6):finish_reason + 思考占输出
                // 比例 +「提高上限/改非推理模型」建议,区分「预算被推理吃光」(实测主因)
                // 与其他成因。`out` 的 completion/reasoning token 来自本轮累计(工具循环
                // 聚合口径不含推理拆分,故那边恒 0,文案会退化为「占比未知」——不报假数)。
                // TM-EMPTY-1:收尾重试已发起过(若条件满足),仍空才走到这里。
                if let Err(e) = state_machine.transition(AgentState::Error, &call.session_id) {
                    tracing::warn!(error = %e, session = %call.session_id, "状态迁移被拒");
                }
                return Err(
                    crate::services::task_core::prompt_consts::empty_output_error(
                        &call.label,
                        &out,
                        Some(retry_budget_used.unwrap_or(params.max_tokens)),
                    ),
                );
            }
            if let Err(e) = state_machine.transition(AgentState::Finished, &call.session_id) {
                tracing::warn!(error = %e, session = %call.session_id, "状态迁移被拒");
            }
            Ok((text, total_usage))
        }
        Ok(_) => {
            // 中断(用户 stop):与 legacy 的 connector Err 同款记 error 行
            svc.record_llm_call(
                &call.task_id,
                call.phase,
                call.step_index,
                &model,
                &messages,
                "(已中断)",
                None,
                elapsed,
                "error",
            );
            if let Err(e) = state_machine.transition(AgentState::Interrupted, &call.session_id) {
                tracing::warn!(error = %e, session = %call.session_id, "状态迁移被拒");
            }
            Err("任务已停止".into())
        }
        Err(e) => {
            // 对外契约是字符串错误(任务追踪列/步骤 result),分类在此落回文案;
            // 分类只服务聊天路径的 SSE 错误终态。
            let msg = e.message().to_string();
            svc.record_llm_call(
                &call.task_id,
                call.phase,
                call.step_index,
                &model,
                &messages,
                &msg,
                None,
                elapsed,
                "error",
            );
            if let Err(e) = state_machine.transition(AgentState::Error, &call.session_id) {
                tracing::warn!(error = %e, session = %call.session_id, "状态迁移被拒");
            }
            Err(msg)
        }
    }
}

/// solo 执行器:持有任务后端(提示词助手/状态落库/调用追踪)与聊天引擎(工具循环)。
pub(crate) struct SoloExecutor {
    svc: Arc<dyn TaskBackend>,
    engine: Arc<AgentEngine>,
}

impl SoloExecutor {
    pub(crate) fn new(svc: Arc<dyn TaskBackend>, engine: Arc<AgentEngine>) -> Self {
        SoloExecutor { svc, engine }
    }

    async fn run_inner(&self, ctx: TaskRunContext) -> Result<(TaskTerminal, TokenUsage), String> {
        // solo 无规划阶段:进入即执行(run 入口 reset_task 已置 planning,此处推进到 running)
        self.svc.set_status(&ctx.task_id, TaskStatus::Running);

        let call = AgentLoopCall {
            task_id: ctx.task_id.clone(),
            session_id: format!("task:{}", ctx.task_id),
            goal: ctx.goal.clone(),
            settings: ctx.settings.clone(),
            executor_id: ctx.executor_id.clone(),
            character_id: ctx.character_id.clone(),
            phase: "agent",
            step_index: None,
            label: "主 agent".into(),
            // 任务级连接(A 批 B1):从执行上下文原样下传
            connection_id: ctx.connection_id.clone(),
            scope: ctx.scope.clone(),
        };
        let (text, usage) = run_agent_loop(
            self.svc.clone(),
            self.engine.clone(),
            call,
            ctx.cancel.clone(),
        )
        .await?;
        // usage 落库(批次 4.3b 口径补齐:solo 整轮一行,phase=agent;approve 续跑
        // 自 2026-08 起由 ApprovedPlanExecutor 逐步落库,本执行器仅作 plan 为空时的兜底)
        self.svc
            .record_usage(&ctx.task_id, "agent", None, &usage_as_output(&usage));
        // 成功终态:result 文本 + done(无覆盖状态/原因)
        Ok((
            TaskTerminal::Complete {
                result: text,
                status: TaskStatus::Done,
                error: None,
            },
            usage,
        ))
    }
}

impl ModeExecutor for SoloExecutor {
    fn run<'a>(
        &'a self,
        ctx: TaskRunContext,
    ) -> BoxFuture<'a, Result<(TaskTerminal, TokenUsage), String>> {
        Box::pin(async move { self.run_inner(ctx).await })
    }
}
