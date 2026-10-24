// solo 模式:单主 agent 工具自循环——目标直接进 run_tool_loop,工具集按
// task_tool_policy 编译(默认 deny_dangerous:危险级与元工具除外、bash 例外),
// 轮次默认不限(2026-10-19 取消任务模式轮次上限;步骤墙钟/空闲/熔断仍兜底)。
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
    GenerationParams, LlmMessage, TaskEventKind, TaskStatus, TokenUsage, ToolBudget, ToolChoice,
    ToolContext,
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
    /// **任务级总预算截止期**(PRODCAP-2;None = 未开启)。
    /// 消费点两处:① 本步墙钟预算按 `min(步骤预算, deadline - now)` 取小
    /// (见 `effective_step_budget`);② 多步执行器在启动下一步前判定是否已到点。
    pub deadline: Option<Instant>,
    /// 是否启用「连续只读」守卫(TMR-5,`ToolBudget.readonly_streak_limit`)。
    ///
    /// 默认 true(solo/multi/team/plan 续跑)。followup 置 false:终态追加指令
    /// 常见形态是「先通读既有产物再修」,一次总装可能合法读取 9+ 个分片;8 次只读
    /// 即提醒、再次达到即熔断会把「还没读完就开始修」误判成探索空转(2026-10-23
    /// 空岛 followup 实测)。语义熔断/步骤预算/轮次上限仍照常生效。
    ///
    /// team 的**集成/总装主**(依赖 DAG 汇点,见 `team.rs::is_integration_main`)同置
    /// false:总装必须先读全部上游分片再写大文件,2026-10-23 复跑中主 Agent-4 的四次
    /// 循环全部被该守卫截断(SKY-6)。SKY-7 起只豁免汇点——有 `after` 但下游还有主的
    /// 中段生产者仍受守卫保护(2026-10-24 全题复跑主 2 342 轮零写盘)。
    pub readonly_guard: bool,
    /// 是否启用「声明文件无进展」守卫(P0-2,`ToolBudget.no_progress_limit`)。
    ///
    /// 默认 true;仅在有声明文件的 scope 上实际生效。team 的依赖汇点主置 false——
    /// 读上游分片的 16+ 轮内本就不会写自己的交付文件,该守卫会把合法集成形态
    /// 当成空转截断(SKY-6;步骤墙钟与团队总预算仍是上限)。非汇点中段主保持 true
    /// (SKY-7;中途第 8 轮会先提醒一次,第 16 轮才收尾)。
    pub no_progress_guard: bool,
    /// 集成主分片读取软提醒阈值(SKY-8;None = 关闭)。
    /// 仅 team::run_main 对依赖汇点主填 `Some(TASK_SHARD_READ_NUDGE)`;其余路径 None。
    pub shard_read_nudge_after: Option<usize>,
}

/// 单主 agent 工具自循环的产出。
pub(crate) struct AgentLoopOutput {
    /// 最终正文(空正文收尾重试后的版本)
    pub text: String,
    /// 整轮累计 usage(调用方按此记一次;首轮空正文行由本函数单独落库)
    pub usage: TokenUsage,
    /// 空转守卫截断原因(连续只读/无进展/语义/重复调用);None = 正常收尾。
    /// 空正文收尾重试不会抹掉首轮的截断事实——重试只是补一段收尾正文,工作仍被截断。
    pub guard_stopped: Option<String>,
    /// 预算类收尾(步骤墙钟/token 预算/团队份额)发生时为 true(SKY-8,2026-10-24)。
    /// 与 `guard_stopped` 分开承载:预算收尾的步骤终态要看声明文件的磁盘事实——
    /// 文件已齐可记 done,文件缺失必须记 partial;空正文重试同样不得抹掉首轮事实。
    pub budget_stopped: bool,
}

/// 单主 agent 工具自循环(solo/multi 执行器主体;team 各主 agent 复用):
/// system 组装(内置执行者指令 → 人设 → 世界书 → 提示词注入 → Agent 提示词,
/// 外部段落逐一 untrusted 包裹)→ run_tool_loop(工具按 task_tool_policy 编译 +
/// ToolGate 闸门,恒不等待授权:名单外工具立即拒绝)→ 调用追踪统一出口落 task_llm_calls(成功/空/中断/错误均一行)。
/// 成功返回 (正文, 整轮累计 usage);中断/错误/空内容返回 Err。
/// P0-1:失败/中止循环的 usage_total 记账。
///
/// 成功路径由调用方按返回的 `TokenUsage` 记一次；错误/中断路径没有返回值，
/// 此前只落 `task_llm_calls` 错误行且 usage 传 `None`，导致失败循环的消耗
/// 完全不出现在 `usage_total`（2026-10-21 实测：234 轮循环记成 0 token）。
/// `total_usage` 是整轮累计；空正文重试过时调用行只有增量，本函数补的是整轮，
/// 与「各行 token 求和 == usage_total」的既有口径一致。
fn record_failed_loop_usage(
    svc: &Arc<dyn TaskBackend>,
    call: &AgentLoopCall,
    total_usage: &TokenUsage,
) {
    svc.record_usage(
        &call.task_id,
        call.phase,
        call.step_index,
        &usage_as_output(total_usage),
    );
}

pub(crate) async fn run_agent_loop(
    svc: Arc<dyn TaskBackend>,
    engine: Arc<AgentEngine>,
    call: AgentLoopCall,
    cancel: watch::Receiver<bool>,
) -> Result<AgentLoopOutput, String> {
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
    // 视觉验证纪律段的条件(D4;修复批次并含截图):工具面含任一图像工具才追加
    // (单一判据出处 tool_sets::is_image_tool:视觉三件 ∪ 截图)
    let has_image_tools = allowed
        .iter()
        .any(|n| crate::tools::tool_sets::is_image_tool(n));
    // 写盘优先纪律的条件(TMR-1):本轮实际下发了工作区写工具才追加。用**实际放行名单**
    // 判定(与下发给模型的 defs 同源),策略收窄到只读时不会出现「教模型写文件但没工具」。
    let has_write_tools = allowed
        .iter()
        .any(|n| crate::tools::tool_sets::is_workspace_write_tool(n));
    // system 提示词组装:统一走 TaskBackend::assemble_executor_system_prompt
    // (单一实现,宿主侧 prompt.rs);拼装顺序与 untrusted 包裹纪律同 legacy,
    // 勿在本文件复制实现(WP7)。
    let sys = svc.assemble_executor_system_prompt(
        settings,
        call.executor_id.as_deref(),
        call.character_id.as_deref(),
        &call.goal,
        has_tools,
        has_write_tools,
        has_image_tools,
    );

    let mut messages = vec![
        LlmMessage::plain("system", &sys),
        LlmMessage::plain("user", &call.goal),
    ];

    // 任务侧工具循环的两道限制(提交 3 · D3):单步墙钟预算 + 语义熔断收紧,
    // 与 custom 的工具节点共用同一映射(单一出处,见 task_loop_limits 文档)。
    // PRODCAP-2:本步墙钟预算再按任务级截止期取小——总预算开启时,单步不得超过
    // 「距截止期剩余时间」,到点复用既有「带产出收尾」路径(总预算关 = None,行为不变)。
    let (step_budget, semantic_guard) = super::task_loop_limits(settings);
    let step_budget = super::effective_step_budget(step_budget, call.deadline);
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
        // 任务模式取消轮次上限(2026-10-19):0 = 不限;solo/multi/team/plan/followup
        // 共用本装配点。聊天 AGENT 仍走 settings.max_tool_rounds。
        max_tool_rounds: Some(0),
        tool_choice: ToolChoice::Auto,
        // 任务级连接(A 批 B1):solo 无节点级连接可配,直接用任务绑定的那条
        // (None = 默认连接,与本批之前一致)
        connection_id: call.connection_id.clone(),
        parallel_tool_calls: None,
        step_budget,
        semantic_guard,
        response_format: None,
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
        // 任务侧预算载体(PRODCAP-2):子 agent 派发路径(agentgo/subtask)据此
        // 透传语义熔断三值进子循环;墙钟预算不在其中(父层轮末闸门即上限)。
        // TMR-5:连续只读检测上限(8 次只读调用)随任务侧装配下发;聊天路径
        // `budget` 恒 None,行为不变。followup 与 team 集成主经
        // `readonly_guard=false` 关闭该闸门(终态续跑/总装常需先通读既有产物)。
        budget: Some(ToolBudget {
            semantic_guard,
            readonly_streak_limit: call
                .readonly_guard
                .then_some(super::TASK_READONLY_STREAK_LIMIT),
            // P0-2:team 的 scope 带声明文件时生效；solo/多主未声明时自动跳过。
            // SKY-6/SKY-7:team 的依赖汇点主经 `no_progress_guard=false` 关闭(读上游
            // 分片期间本就不会写自己的交付文件);非汇点中段主保持开启,第 8 轮先提醒、
            // 第 16 轮收尾(步骤墙钟/团队总预算仍是上限)。
            no_progress_limit: call
                .no_progress_guard
                .then_some(super::TASK_NO_PROGRESS_LIMIT),
            // SKY-8:集成主读满阈值后提醒先写入口骨架(只提醒,不熔断)。
            shard_read_nudge_after: call.shard_read_nudge_after,
        }),
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
    // TMR3-2:截断自愈留痕由本函数持有,成功/失败都能补落调用行与 usage
    // (失败路径此前随 Err 丢弃,自愈重发的 token 两头都不记)。
    let mut self_heals = Vec::new();
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
        &mut self_heals,
        &run_id,
        gate,
        // 单次调用超时覆盖(A 批 A1):solo 无节点级配置,恒走宿主既有判定
        None,
        true,
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
    // SKY-6 第 2 项:首轮若被守卫截断,空正文收尾重试也不能抹掉这个事实——
    // 重试只补收尾正文,被截断的工作仍没做完。
    let first_guard_stopped = result.as_ref().ok().and_then(|r| r.guard_stopped.clone());
    // SKY-8:预算收尾同理。首轮因步骤墙钟/token 预算/团队份额到点收尾、随后空正文
    // 重试成功时,`res.budget_stopped` 会是重试轮的值(false);必须保留首轮事实,
    // 否则「预算收尾 + 声明文件缺失」会被误判成 done。
    let first_budget_stopped = result
        .as_ref()
        .ok()
        .map(|r| r.budget_stopped)
        .unwrap_or(false);
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
                &mut self_heals,
                &run_id,
                retry_gate,
                None,
                true,
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
                // P0-1:失败路径同样落 usage_total。首轮空正文行已单独落库但未记
                // usage_total,这里用整轮累计一次补记(first + retry),与调用行求和一致。
                record_failed_loop_usage(&svc, &call, &total_usage);
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
            // PRODCAP-2/TMR3-4:本步被墙钟预算收尾且截止期已过 → 收尾的真实约束来源是
            // 「任务总预算」或「团队总预算」(引擎事件只报被收窄后的本步数字,不复述总预算)。
            // 这里用中性「预算截止期」措辞:调用点不知道截止期来自哪一档,团队侧会另行
            // 补一条「团队总预算用尽」事件说明成因;单步预算自行到点(截止期未过)不补。
            if res.budget_stopped && super::deadline_exhausted(call.deadline) {
                svc.emit_event(
                    TaskEventKind::Status,
                    &call.task_id,
                    None,
                    None,
                    Some("预算截止期用尽:工具循环在预算截止处收尾,已保留本轮产出".into()),
                );
            }
            Ok(AgentLoopOutput {
                text,
                usage: total_usage,
                guard_stopped: first_guard_stopped.or(res.guard_stopped),
                budget_stopped: first_budget_stopped || res.budget_stopped,
            })
        }
        Ok(res) => {
            // 中断(用户 stop):与 legacy 的 connector Err 同款记 error 行。
            // P0-1:行内带「本次增量」usage;usage_total 用整轮累计补记
            // (空正文重试过时,首轮行已单独落库,增量才不会双计)。
            // TMR3-2:中断前发生的截断自愈同样要落行与 usage。
            svc.record_self_heals(
                &call.task_id,
                call.phase,
                call.step_index,
                &model,
                &messages,
                &super::executor::to_self_heals(&res.self_heals),
            );
            let out = usage_as_output(&row_usage);
            svc.record_llm_call(
                &call.task_id,
                call.phase,
                call.step_index,
                &model,
                &messages,
                "(已中断)",
                Some(&out),
                elapsed,
                "error",
            );
            record_failed_loop_usage(&svc, &call, &total_usage);
            if let Err(e) = state_machine.transition(AgentState::Interrupted, &call.session_id) {
                tracing::warn!(error = %e, session = %call.session_id, "状态迁移被拒");
            }
            Err("任务已停止".into())
        }
        Err(e) => {
            // 对外契约是字符串错误(任务追踪列/步骤 result),分类在此落回文案;
            // 分类只服务聊天路径的 SSE 错误终态。
            let msg = e.message().to_string();
            // P0-1:错误循环的已累计 token 必须落账(2026-10-21 实测:234 轮失败循环
            // 被记成 0 token)。行内用增量,usage_total 用整轮累计,保持调用行求和口径。
            // TMR3-2:失败前发生的截断自愈也要补落(此前随 Err 丢弃)。
            svc.record_self_heals(
                &call.task_id,
                call.phase,
                call.step_index,
                &model,
                &messages,
                &super::executor::to_self_heals(&self_heals),
            );
            let out = usage_as_output(&row_usage);
            svc.record_llm_call(
                &call.task_id,
                call.phase,
                call.step_index,
                &model,
                &messages,
                &msg,
                Some(&out),
                elapsed,
                "error",
            );
            record_failed_loop_usage(&svc, &call, &total_usage);
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
            // 任务级总预算(PRODCAP-2):0 = 关时恒 None,行为逐字节不变
            deadline: ctx.deadline,
            readonly_guard: true,
            no_progress_guard: true,
            // solo 不是集成主:无分片读取软提醒。
            shard_read_nudge_after: None,
        };
        let out = run_agent_loop(
            self.svc.clone(),
            self.engine.clone(),
            call,
            ctx.cancel.clone(),
        )
        .await?;
        // usage 落库(批次 4.3b 口径补齐:solo 整轮一行,phase=agent;approve 续跑
        // 自 2026-08 起由 ApprovedPlanExecutor 逐步落库,本执行器仅作 plan 为空时的兜底)
        self.svc
            .record_usage(&ctx.task_id, "agent", None, &usage_as_output(&out.usage));
        // 成功终态:result 文本 + done;守卫截断是「带产出的部分完成」,降 partial
        // 并写明原因(SKY-6 第 2 项),不得让半成品冒充 done。
        let (status, error) = match out.guard_stopped {
            Some(reason) => (
                TaskStatus::Partial,
                Some(format!(
                    "工具循环被守卫截断({reason}),成果可能不完整;可追加指令继续完成"
                )),
            ),
            None => (TaskStatus::Done, None),
        };
        Ok((
            TaskTerminal::Complete {
                result: out.text,
                status,
                error,
            },
            out.usage,
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
