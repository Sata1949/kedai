// custom 单节点执行:系统提示装配、严格/宽松档生成、空产出重试与节点级上下文裁剪。
// 由原 `custom.rs` 整段搬来(C-2D,纯搬运)。
use super::context::released_flows;
use super::{CustomExecutor, GraphCtx, NodeInput, NodeOutcome};
use crate::agents::engine::executor::{run_tool_loop, ToolLoopRequest};
use crate::agents::engine::AbortFlag;
use crate::agents::state_machine::StateMachine;
use crate::models::types::{
    GenerationParams, LlmMessage, PlanStep, TokenUsage, ToolBudget, ToolChoice, ToolContext,
};
use crate::services::prompt_kit::{
    select_segments_within_budget, untrusted_boundary, SegmentCost, OMITTED_SEGMENT_MARKER,
};
use crate::services::task_core::prompt_consts::{
    CUSTOM_REFLECT_PROMPT, EXECUTOR_PROMPT, TASK_INTERNAL_PLAN_PROMPT,
};
use crate::services::task_core::TaskGenOutput;
use crate::services::task_engine::context::TaskRunContext;
use crate::services::task_engine::flow_call;
use crate::services::task_engine::sink;
use crate::tools::run_flow as run_flow_tool;
use std::time::Instant;
use uuid::Uuid;

impl CustomExecutor {
    /// 组装步骤 system:内置基础指令(direct 生成=执行者 / direct 非生成=内部规划 /
    /// reflect=内置反思)+ 步骤提示词(用户可编辑 → 宏渲染 + untrusted 包裹)+ 任务目标上下文(untrusted 包裹)。
    /// generates=false 的 direct 步骤用内部规划指令:其语义是「只做分析规划、不产出正文」,
    /// 与步骤自身定位一致(此前复用执行者指令会吐出完整正文,2026-09-10 实测修复)。
    pub(super) fn build_step_system(&self, step: &PlanStep, goal: &str) -> String {
        let mut sys = match step.action.as_str() {
            "reflect" => String::from(CUSTOM_REFLECT_PROMPT),
            _ if step.generates == Some(false) => String::from(TASK_INTERNAL_PLAN_PROMPT),
            _ => String::from(EXECUTOR_PROMPT),
        };
        if let Some(prompt) = &step.system_prompt {
            if !prompt.trim().is_empty() {
                // 步骤提示词为用户可编辑配置:与 agent_system_prompt 同款宏渲染
                //(无角色卡/聊天历史:character=None,角色类占位符置空不泄漏宏原文)
                let rendered = self.svc.render_agent_prompt(prompt, None, "", goal);
                if !rendered.trim().is_empty() {
                    sys.push_str(&format!(
                        "\n\n{}",
                        untrusted_boundary("flow_step", &rendered)
                    ));
                }
            }
        }
        sys.push_str(&format!(
            "\n\n任务目标:\n{}",
            untrusted_boundary("task_goal", goal)
        ));
        sys
    }

    /// 本节点的一次尝试:严格档/无工具走纯生成,宽松档 + 工具走工具循环。
    ///
    /// 抽成独立函数是为 A 批的空产出重试服务——重试必须**重建**消息(见 `execute_node`),
    /// 而两条路径的输入装配(系统提示/节点消息/超时/连接)完全相同,复制一份必然漂移。
    /// `sys`/`user` 是**本层**输入(已过 `apply_step_max_context` 裁剪);
    /// `max_tokens_override` 是重试时的预算翻倍值(None = 用节点/全局配置的原始值)。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_node_attempt(
        &self,
        ctx: &TaskRunContext,
        g: &GraphCtx<'_>,
        index: usize,
        step: &PlanStep,
        sys: &str,
        user: &str,
        node_model: &str,
        max_tokens_override: Option<u32>,
    ) -> Result<NodeOutcome, String> {
        // 工具纪律段(提交 3 · D3-c):与下方档位 match **同一判据**——严格档/无工具步骤
        // 不下发工具,也就不需要「怎么用工具」的纪律。判据与那条 match 一处改动必须同步
        // (放在本函数内而非 build_step_system,正是为了与档位判定同处、不复制档位知识)。
        let sys_with_discipline;
        let sys: &str = if matches!((&step.tools, step.is_strict()), (Some(_), false)) {
            sys_with_discipline = format!(
                "{}\n\n{}",
                sys.trim_end(),
                crate::services::task_core::prompt_consts::EXECUTOR_TOOL_DISCIPLINE
            );
            sys_with_discipline.as_str()
        } else {
            sys
        };
        let mut messages = vec![
            LlmMessage::plain("system", sys),
            LlmMessage::plain("user", user),
        ];
        // 节点级单次调用超时(A 批 A1):None = 宿主缺省看门狗(300s,行为不变)。
        let call_timeout = step
            .call_timeout_secs
            .map(|secs| std::time::Duration::from_secs(secs as u64));
        // 档位优先于 tools 配置(二维批次 6a):严格档 = 单次调用、不下发任何工具——
        // 即使步骤声明了工具也如此(配置保留在流程里,切回宽松档即生效;编辑器已按此提示)。
        match (&step.tools, step.is_strict()) {
            // 无工具步骤,或严格档:纯生成统一出口(generate_text 自带调用追踪落库)
            (None, _) | (Some(_), true) => {
                let settings = &ctx.settings;
                // 输出预算:重试翻倍值优先,其次节点配置,最后全局缺省(A 批 A2)
                let max_tokens = max_tokens_override
                    .unwrap_or_else(|| step.max_tokens.unwrap_or(settings.default_max_tokens));
                self.svc
                    .generate_text_with_timeout(
                        &ctx.task_id,
                        g.phase,
                        Some(index),
                        messages,
                        Vec::new(),
                        max_tokens,
                        step.temperature.unwrap_or(settings.default_temperature),
                        settings.default_top_p,
                        step.connection_ref(),
                        call_timeout,
                        ctx.cancel.clone(),
                    )
                    .await
                    .map(|out| NodeOutcome {
                        text: out.text.trim().to_string(),
                        out,
                        degraded: false,
                    })
            }
            // 宽松档 + 工具步骤:run_tool_loop(白名单自动放行),调用追踪在步骤函数内落。
            // usage → TaskGenOutput 统一走 usage_as_output(批次 B.4 单一出处);
            // 该 out 仅作 record_usage 入参(text 不消费)
            (Some(list), false) => self
                .run_step_with_tools(
                    ctx,
                    g,
                    index,
                    step,
                    &mut messages,
                    list,
                    node_model,
                    max_tokens_override,
                )
                .await
                .map(|(text, usage)| NodeOutcome {
                    out: crate::services::task_engine::executor::usage_as_output(&usage),
                    text,
                    degraded: false,
                }),
        }
    }

    /// 应用节点级上下文上限(二维批次 8):返回**进 prompt 的** node 消息文本。
    ///
    /// 口径(纯选择逻辑在 [`select_segments_within_budget`],此处只做计数与渲染):
    ///  - 留空(`None`)= 不裁剪 → 与批次 8 之前**逐字节一致**(存量流程零行为变化);
    ///  - 恒保留「任务目标」/源消息,从**最旧**的上游产出段起把正文换成
    ///    [`OMITTED_SEGMENT_MARKER`](标签行保留),直到合计落在预算内,并 `warn` 一次;
    ///  - 任务目标单独超预算 → 本节点**明确报错**(不静默截断,口径同 5b「引用失效不静默回退」);
    ///  - **不裁系统提示**:它是本节点的指令与档位语义,裁它等于换了个节点;
    ///  - **不裁工具轮内历史**:那是 `tool_history_keep_rounds` / `tool_history_budget_tokens`
    ///    的职责(两者口径不同),故本函数只作用于**初始输入装配**。
    ///  - 计数用本节点**真实模型**(与调用追踪同源):`node_model` 由连接解析给出。
    pub(super) fn apply_step_max_context(
        &self,
        step: &PlanStep,
        input: &NodeInput,
        node_model: &str,
        default_budget: u32,
    ) -> Result<String, String> {
        // 预算来源(A 批 A4):节点级 `max_context` 优先;缺省回落任务侧设置
        // 「默认节点上下文上限」(0 = 不裁剪)。两者皆空 = 与批次 8 之前逐字节一致。
        let budget = step
            .max_context
            .or_else(|| (default_budget > 0).then_some(default_budget));
        let Some(budget) = budget else {
            return Ok(input.render_full());
        };
        let head_tokens = self.engine.count_tokens(&input.head, node_model);
        let marker_tokens = self.engine.count_tokens(OMITTED_SEGMENT_MARKER, node_model);
        let costs: Vec<SegmentCost> = input
            .segments
            .iter()
            .map(|s| {
                let label = self.engine.count_tokens(&s.label, node_model);
                SegmentCost {
                    kept: label + self.engine.count_tokens(&s.text, node_model),
                    omitted: label + marker_tokens,
                }
            })
            .collect();
        let keep_from = select_segments_within_budget(head_tokens, &costs, budget)
            .map_err(|e| format!("步骤「{}」:{}", step.name, e))?;
        if keep_from > 0 {
            tracing::warn!(
                step = %step.name,
                budget,
                omitted_segments = keep_from,
                "节点输入超出上下文上限:已省略最旧的 {} 个上游产出段(正文换成标记,标签行保留)",
                keep_from
            );
        }
        Ok(input.render_omitted(keep_from))
    }

    /// 带工具步骤:run_tool_loop + 步骤白名单;返回 (正文, 整轮 usage)。
    /// 调用追踪在本函数内落(phase 取自 `g`:入口 `step` / 静态子图 `subflow.<路径>` /
    /// 动态调用层 `call.<路径>`;对齐 generate_text 的统一出口语义:成功/空/中断/错误
    /// 均落一行 task_llm_calls)。
    /// `node_model` = 本节点连接的**真实模型名**(二维批次 5b;调用方已解析,不再恒取全局模型)。
    /// `max_tokens_override` = 空产出重试时的预算翻倍值(A 批 A2;None = 节点/全局配置的原始值)。
    // 参数较多是「追踪落库三件套(phase/step_index/model)」与执行输入各占一位所致,
    // 打包成结构体只会把一个直白的调用点换成一次构造,收益为负。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_step_with_tools(
        &self,
        ctx: &TaskRunContext,
        g: &GraphCtx<'_>,
        step_index: usize,
        step: &PlanStep,
        messages: &mut Vec<LlmMessage>,
        whitelist: &[String],
        node_model: &str,
        max_tokens_override: Option<u32>,
    ) -> Result<(String, TokenUsage), String> {
        let settings = &ctx.settings;
        let phase = g.phase;
        // 任务侧工具循环的两道限制(提交 3 · D3;与 solo 主循环同一映射、单一出处)
        let limits = crate::services::task_engine::task_loop_limits(settings);
        // 任务模式工具策略:先按策略编译候选集(默认拒绝危险工具、剔除元工具),
        // 再与步骤白名单取交——步骤白名单只能收窄,不能突破任务策略放行危险工具。
        let policy = crate::services::task_engine::tool_policy::compile(
            &settings.task_tool_policy,
            &settings.task_tool_allowlist,
            &self.engine.tool_registry(),
            // 工作区工具族随「作用域是否存在」下发(口径同 solo.rs:任务自 D1 起恒有
            // 作用域——工作区或任务 scratch;传 is_some() 是为守住「没有作用域就不给
            // 文件工具」这条不变量)
            ctx.scope.is_some(),
            // 包专属工具随编码能力包开关(口径同 solo.rs)
            settings.task_coding_bundle_enabled,
            // 视觉三件随生效连接的「视觉输入」能力位(口径单一出处;见 tool_policy::vision_gate)
            crate::services::settings_service::vision_enabled(settings),
            // 截图工具随「视觉与截图」总开关(默认关)
            settings.vision_screenshot_enabled,
        );
        // Some([]) = 策略全量集;Some(list) = 策略集 ∩ 步骤白名单
        let mut tools: Vec<_> = if whitelist.is_empty() {
            policy.defs
        } else {
            policy
                .defs
                .into_iter()
                .filter(|d| whitelist.iter().any(|w| w == &d.name))
                .collect()
        };
        // 对比模式(二维批次 7b):本节点额外**释放名单内流程**——工具定义按名单改写描述
        // 后直接进下发列表,于是闸门名单(下行由 tools 派生)自动包含它,不需要第二份判定。
        // 前端节点白名单是自由文本,但策略编译集已剔除元工具,用户手写也拿不到它
        // (可见性由这里单点决定,这是「用户无须知道这个名字」的落实处)。
        let released = g
            .call_state
            .map(|st| released_flows(st.callable(), g.flows))
            .unwrap_or_default();
        if !released.is_empty() {
            tools.push(run_flow_tool::definition_with_description(
                // 预算文案按**本轮设置**写(A 批 A3):描述里承诺的次数必须与闸门判界同源,
                // 否则用户调高设置后会看到「最多可调用 8 次」而实际能调 16 次
                flow_call::run_flow_description(
                    &released,
                    ctx.settings.max_flow_calls_per_task as usize,
                ),
            ));
        }
        // 视觉验证纪律段(2026-10-02 修复批次):本节点工具面含任一图像工具时追加到
        // system——判据以**实际下发名单**为准(视觉三件 ∪ 截图),与 solo 主循环、
        // 聊天路径同源(单一实现 prompt_kit::append_vision_discipline_if_needed);
        // 此前 custom 工具节点只贴工具纪律,漏了视觉验证纪律(截图/看图节点静默无段)。
        if let Some(s0) = messages.first_mut() {
            if s0.role == "system" {
                crate::services::prompt_kit::append_vision_discipline_if_needed(
                    &mut s0.content,
                    &tools,
                );
            }
        }
        // 写盘优先纪律(TMR-1):判据以**实际下发名单**为准(不是节点配置)——策略收窄
        // 或步骤白名单只读时不会出现「教模型写文件但没工具」。与 solo 主循环同源常量。
        if let Some(s0) = messages.first_mut() {
            if s0.role == "system" {
                crate::services::prompt_kit::append_write_first_discipline_if_needed(
                    &mut s0.content,
                    &tools,
                );
            }
        }
        // 闸门名单与下发工具一致:名单外立即拒绝(任务模式无 UI 授权上下文)
        let gate_list: Vec<String> = tools.iter().map(|d| d.name.clone()).collect();
        let gate = crate::agents::engine::executor::ToolGate::listed(&gate_list);
        let tool_choice = match step.tool_choice.as_deref() {
            Some("none") => ToolChoice::None,
            Some("required") => ToolChoice::Required,
            Some("function") => step
                .tool_choice_function
                .clone()
                .map(ToolChoice::Function)
                .unwrap_or_default(),
            _ => ToolChoice::Auto,
        };
        let params = GenerationParams {
            temperature: step.temperature.unwrap_or(settings.default_temperature),
            top_p: settings.default_top_p,
            // 节点级输出上限;空产出重试时被翻倍值覆盖(A 批 A2)
            max_tokens: max_tokens_override
                .unwrap_or_else(|| step.max_tokens.unwrap_or(settings.default_max_tokens)),
            stop: None,
            tools,
            // 任务模式默认不限轮次(0 = 不限);节点作者显式配置的 max_tool_rounds
            // 仍作为该节点的显式预算保留。严格档下本参数无意义(不下发工具)。
            max_tool_rounds: Some(step.max_tool_rounds.unwrap_or(0)),
            tool_choice,
            // 连接:节点级(二维批次 5b)优先,缺省回退**任务级**(A 批 B1);
            // 两者皆空 = 默认连接(run_tool_loop 内层经 engine 单点解析)
            connection_id: step
                .connection_ref()
                .map(str::to_string)
                .or_else(|| ctx.connection_id.clone()),
            parallel_tool_calls: step.parallel_tool_calls,
            // 任务侧工具循环的两道限制(提交 3 · D3):与 solo 主循环共用同一映射
            // (单一出处 task_engine::task_loop_limits,勿在此另算一份)。
            // PRODCAP-2:本节点墙钟预算再按任务级截止期取小(总预算关 = None)
            step_budget: crate::services::task_engine::effective_step_budget(
                limits.0,
                ctx.deadline,
            ),
            semantic_guard: limits.1,
            response_format: None,
        };
        // 会话 id 保留既有形状 `task:{任务 id}:step:{下标}`(入口流程逐字节不变);
        // 子图用 phase 段替换 `step`,避免同一下标在父子两层撞 key(取值仍可用
        // split(':')[1] 反解任务 id)
        let session_id = format!("task:{}:{}:{}", ctx.task_id, phase, step_index + 1);
        // 对比模式:登记本节点的**调用点**(key 与工具处理器手里的 `ToolContext.session_id`
        // 逐字节一致),`run_flow` 被调用时据此回到本任务上下文。守卫(RAII)在整个循环
        // 期间持有,循环结束或本节点 future 被丢弃(取消)都会注销。
        let _call_site = (!released.is_empty())
            .then(|| run_flow_tool::register_call_site(&session_id, self.flow_invoker(ctx, g)));
        let mut state_machine = StateMachine::new(&session_id);
        let run_id = Uuid::new_v4().to_string();
        let (flag, _flag_rx) = AbortFlag::new();
        let tool_ctx = ToolContext {
            session_id: session_id.clone(),
            character_id: String::new(), // custom 不吃角色卡
            agent_depth: 0,
            scope: ctx.scope.clone(),
            // 任务侧预算载体(PRODCAP-2):子 agent 派发路径据此透传语义三值;
            // TMR-5:连续只读检测随任务侧装配下发(聊天/审计路径不填)。
            budget: Some(ToolBudget {
                semantic_guard: limits.1,
                readonly_streak_limit: Some(
                    crate::services::task_engine::TASK_READONLY_STREAK_LIMIT,
                ),
                // custom 节点没有规划器声明文件，P0-2 守卫不启用。
                no_progress_limit: None,
                // custom 节点不是 team 集成主：无分片读取软提醒。
                shard_read_nudge_after: None,
            }),
        };
        // 事件桥(批次 R4 携 phase/step_index):custom 工具步骤的调用追踪口径为
        // phase + 本步骤下标(与下方 record_llm_call 一致;子图为 subflow.<路径>)
        let (tx, drain) = sink::spawn(
            self.svc.clone(),
            ctx.task_id.clone(),
            "主 agent",
            phase,
            Some(step_index),
        );
        let mut total_usage = TokenUsage::default();
        // TMR3-2:截断自愈留痕由调用方持有,失败/中断也能补落(不再随 Err 丢弃)
        let mut self_heals = Vec::new();
        let started = Instant::now();
        // 节点级单次调用超时(A 批 A1):None = 宿主既有 300s 看门狗(行为不变),
        // Some = 覆盖该值(工具循环**逐轮**各按它计,与宿主既有粒度一致)。
        let call_timeout = step
            .call_timeout_secs
            .map(|secs| std::time::Duration::from_secs(secs as u64));
        let result = run_tool_loop(ToolLoopRequest {
            engine: &self.engine,
            state_machine: &mut state_machine,
            agent_session: None,
            session_id: &session_id,
            llm_messages: messages,
            params: &params,
            tool_ctx: &tool_ctx,
            tx: &tx,
            abort: &ctx.cancel,
            flag: &flag,
            total_usage: &mut total_usage,
            self_heals: &mut self_heals,
            run_id: &run_id,
            gate,
            call_timeout,
            emit_tokens: true,
        })
        .await;
        drop(tx);
        let _ = drain.await;

        let elapsed = started.elapsed();
        // 调用追踪的模型列 = 本节点连接的**真实模型**(二维批次 5b):原先恒取全局模型,
        // 节点绑了别的连接时会记错 provider。解析已在 execute_node 做过(同一份结果传入)。
        let model = node_model.to_string();
        match result {
            Ok(res) if !res.interrupted => {
                // 截断自愈留痕落库(问题①):与 solo.rs run_agent_loop 共用单一实现
                // (TaskService::record_self_heals:补落被截断行 + 补 usage)。
                self.svc.record_self_heals(
                    &ctx.task_id,
                    phase,
                    Some(step_index),
                    &model,
                    messages,
                    &crate::services::task_engine::executor::to_self_heals(&res.self_heals),
                );
                let text = res.content.trim().to_string();
                let status = if text.is_empty() { "empty" } else { "ok" };
                let out = TaskGenOutput {
                    text: text.clone(),
                    // 上游 finish_reason 经 run_tool_loop 末轮透出(可观测性问题①)
                    finish_reason: res.finish_reason.clone(),
                    prompt_tokens: total_usage.prompt_tokens,
                    completion_tokens: total_usage.completion_tokens,
                    reasoning_tokens: 0,
                    reasoning_chars: 0,
                    tool_calls: Vec::new(),
                    dropped_tool_calls: 0,
                };
                self.svc.record_llm_call(
                    &ctx.task_id,
                    phase,
                    Some(step_index),
                    &model,
                    messages,
                    &text,
                    Some(&out),
                    elapsed,
                    status,
                );
                Ok((text, total_usage))
            }
            Ok(res) => {
                // TMR3-2:中断前的截断自愈同样落行与 usage;失败路径不再记 0 token。
                self.svc.record_self_heals(
                    &ctx.task_id,
                    phase,
                    Some(step_index),
                    &model,
                    messages,
                    &crate::services::task_engine::executor::to_self_heals(&res.self_heals),
                );
                let out = crate::services::task_engine::executor::usage_as_output(&total_usage);
                self.svc.record_llm_call(
                    &ctx.task_id,
                    phase,
                    Some(step_index),
                    &model,
                    messages,
                    "(已中断)",
                    Some(&out),
                    elapsed,
                    "error",
                );
                self.svc
                    .record_usage(&ctx.task_id, phase, Some(step_index), &out);
                Err("任务已停止".into())
            }
            Err(e) => {
                // 步骤执行器对外契约是字符串错误(步骤 result/追踪列),分类在此落回文案;
                // 分类只服务聊天路径的 SSE 错误终态。
                let msg = e.message().to_string();
                // TMR3-2:失败前发生的截断自愈与已累计 token 都要入账。
                self.svc.record_self_heals(
                    &ctx.task_id,
                    phase,
                    Some(step_index),
                    &model,
                    messages,
                    &crate::services::task_engine::executor::to_self_heals(&self_heals),
                );
                let out = crate::services::task_engine::executor::usage_as_output(&total_usage);
                self.svc.record_llm_call(
                    &ctx.task_id,
                    phase,
                    Some(step_index),
                    &model,
                    messages,
                    &msg,
                    Some(&out),
                    elapsed,
                    "error",
                );
                self.svc
                    .record_usage(&ctx.task_id, phase, Some(step_index), &out);
                Err(msg)
            }
        }
    }
}
