// custom 模式:复用**任务绑定的流程快照**(二维批次 5a;未绑定任务取当时的当前流程,
// 与 5a 之前一致)配轻量 step 执行器(批次 4.3b,docs/功能.md 第一节)。
// 语义:逐 step 顺序执行,上一步输出作为下一步输入;只吃 steps+goal,
// 不依赖角色卡/聊天历史(角色类占位符渲染为空,{{char}} 等宏原文不泄漏)。
// 工具:step.tools=None 走 generate_text 纯生成;Some([]) = 按 task_tool_policy 编译的
// 全部工具,Some(list) = 与步骤白名单取交(只能收窄);均经 run_tool_loop 的 ToolGate
// 闸门,恒不等待授权(任务模式名单外工具立即拒绝)。
// 反思步骤(action=reflect)按契约不携带用户 system_prompt,统一用内置
// CUSTOM_REFLECT_PROMPT;首版不做 reflect 回退循环(判定结论作为文本流向下一步)。
//
// 二维批次 1(图执行):流程步骤可声明 `inputs`(上游节点 id),执行按下
// `agent_flow_service::resolve_graph` 给出的**拓扑序**推进,节点 user 消息由
// `node_user_message` 按上游产出合成。全部步骤 `inputs` 为空 = 线性兼容模式:
// 拓扑序等于数组顺序、单上游格式与旧版逐字节一致,存量一维流程零迁移。
// 批次 2 起同一批就绪节点走并行调度(上限 max_parallel_nodes)。
//
// 二维批次 6b(静态子图):节点可挂载 `sub_flow_id` 指向库内另一流程,该节点不自己
// 发起模型调用,而是把被引用流程当作**子图**跑一遍,子图成果即本节点产出。调度器因此
// 抽成 `run_graph`(入口流程与子图共用同一份实现,含并行与 plan 单写者纪律)。
// 记账口径(D6):子图节点的调用行 phase = `subflow.<父节点下标链>`,step_index 为
// 节点在**本层**的下标——phase 不含冒号(前端流式缓冲 key 以第一个冒号切分)。
//
// 二维批次 7b(对比模式):任务可带一份**流程名单**(`tasks.flow_ids`),名单内流程作为
// 单一内置工具 `run_flow` 释放给根流程内的**宽松节点**,模型在工具循环里自主调用、
// 取回成果(根流程照常执行)。可调用集 = 名单 ∩ 冻结闭包 − 根流程;被调流程的节点
// 与入口流程走**同一条** `execute_node` 路径(权限因此不可能因为「它是被调流程」而放宽)。
// 记账口径:动态调用层的顶层 phase = `call.d<序号>`(嵌套为 `call.d1.d2`),其内部静态
// 子图仍是 `subflow.<路径>`。三道闸(环/动态深度/每任务预算)见 `flow_call.rs`,全部在
// 任何模型调用之前判定——被拒的调用不产生任何调用行。
use super::context::TaskRunContext;
use super::executor::ModeExecutor;
use super::flow_call::{self, FlowCallState};
use super::sink;
use crate::agents::engine::executor::run_tool_loop;
use crate::agents::engine::{AbortFlag, AgentEngine};
use crate::agents::state_machine::StateMachine;
use crate::models::types::{
    GenerationParams, LlmMessage, PlanStep, TaskStatus, TaskStep, TaskStepStatus, TokenUsage,
    ToolChoice, ToolContext,
};
use crate::services::agent_flow_service::{
    effective_max_parallel, flow_label, output_index, resolve_graph, AgentFlowConfig,
    MAX_SUB_FLOW_DEPTH,
};
use crate::services::prompt_kit::{
    select_segments_within_budget, untrusted_boundary, SegmentCost, OMITTED_SEGMENT_MARKER,
};
use crate::services::task_core::prompt_consts::{
    CUSTOM_REFLECT_PROMPT, EXECUTOR_PROMPT, TASK_INTERNAL_PLAN_PROMPT,
};
use crate::services::task_core::{TaskBackend, TaskGenOutput, TaskTerminal};
use crate::tools::run_flow as run_flow_tool;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

/// custom 执行器:任务后端(流程库/落库/追踪)+ 聊天引擎(带工具步骤的工具循环)。
pub(crate) struct CustomExecutor {
    svc: Arc<dyn TaskBackend>,
    engine: Arc<AgentEngine>,
}

/// 单个节点的执行结果(二维批次 6b 审查修正:把「本节点是否降级完成」也带回来)。
///
/// 为什么需要 `degraded` 而不只看 `Err`:挂载子流程的节点即使**子图内部**有节点失败,
/// 只要子图整体仍选得出成果就会返回 `Ok`。那种情况必须让本层汇总知道「这里降级了」,
/// 否则任务会以 `done` 收尾(而非 `partial`),与「同层普通节点失败 → partial」的既有
/// 语义不自洽。降级只影响状态与展示文案,`text` 保持纯成果(不污染下游提示词)。
struct NodeOutcome {
    /// 节点产出(下游与成果选拔用)
    text: String,
    /// 记账用的 usage(子流程节点只进本层合计,见 `run_graph_inner` 的 `record_usage` 分支)
    out: TaskGenOutput,
    /// 本节点是否「降级完成」(仅子图场景会为真:子图内有失败/空产出节点)
    degraded: bool,
}

/// 节点输入的分段形态(二维批次 8):`head` 恒保留,`segments` 是上游产出段(**最旧在前**,
/// 与 `PlanStep.inputs` 下标升序一致)——裁剪从下标 0 开始)。
///
/// 分段存在的意义:节点级 `max_context` 要按「上游产出段」取舍,而 `node_user_message`
/// 在批次 8 之前是**直接拼串**,段结构一旦拍平就再也分不出来(只能整条截断,可能把
/// 任务目标一起切掉)。渲染仍由 `render_omitted(0)` 产出与批次 8 之前**逐字节相同**的消息。
struct NodeInput {
    /// 恒保留的头部:无上游时即源消息(任务目标),有上游时是「任务目标:<goal>」
    head: String,
    /// 上游产出段(按 `inputs` 下标升序;空 = 源节点)
    segments: Vec<NodeSegment>,
}

/// 上游产出段:标签行 + 正文。省略时**标签行保留**、正文换成 [`OMITTED_SEGMENT_MARKER`],
/// 让模型与读提示词的人都看得出「这里原本有内容」,而不是静默少一段。
struct NodeSegment {
    label: String,
    text: String,
}

impl NodeInput {
    /// 完整渲染(不省略任何段)= 批次 8 之前 `node_user_message` 的输出。
    fn render_full(&self) -> String {
        self.render_omitted(0)
    }

    /// 保留下标 `[keep_from, len)` 的段;`[0, keep_from)` 的正文换成省略标记。
    fn render_omitted(&self, keep_from: usize) -> String {
        let body: Vec<String> = self
            .segments
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let text = if i < keep_from {
                    OMITTED_SEGMENT_MARKER
                } else {
                    s.text.as_str()
                };
                format!("{}\n{}", s.label, text)
            })
            .collect();
        match body.len() {
            0 => self.head.clone(),
            1 => format!("{}\n\n{}", self.head, body[0]),
            _ => format!("{}\n\n上游节点产出:\n{}", self.head, body.join("\n\n")),
        }
    }
}

/// 在飞节点的执行 future:节点在流程数组中的下标 + 执行结果
/// (Err 为该节点的中文错误文本,由调度器落回 plan 行)
type NodeFuture<'a> = BoxFuture<'a, (usize, Result<NodeOutcome, String>)>;

/// 一层图的执行上下文(入口流程与静态子图共用;二维批次 6b 抽取调度器时的参数束)。
struct GraphCtx<'a> {
    /// 本层节点(下标对齐)
    steps: &'a [PlanStep],
    /// 本层有效上游(下标对齐;由 `effective_inputs` 解析)
    inputs: &'a [Vec<usize>],
    /// 本层**源节点**(无上游)的 user 消息:入口流程是任务目标,子图是挂载它的
    /// 那个节点收到的输入消息(上游产出因此照常流进子图,子图节点不是孤岛)
    source_message: &'a str,
    /// 本层节点调用的记账 phase(`step` = 入口流程;`subflow.<路径>` = 子图节点)
    phase: &'a str,
    /// 本层节点在调用链中的**下标路径**(入口为空串;子图为其父节点下标链)
    path: &'a str,
    /// 已进入的子图层数(入口 0;每次进入子图 +1)
    depth: usize,
    /// 子流程 id 调用链(运行期环守卫,与保存期 `validate_sub_flows` 同判定)
    chain: &'a [String],
    /// 本层并行上限(各层取**自己流程**的 max_parallel_nodes)
    max_parallel: usize,
    /// 本轮可用的流程集合 = **任务流程快照的闭包**(二维批次 5a):子图解析只在这里查,
    /// 故跑起来的编排与冻结的那一份完全一致,不受流程库后续改动影响。
    /// 二维批次 7b 起用 `Arc` 承载:动态调用的工具处理器要在**任意时刻**(节点栈早已
    /// 退出)重跑被调流程,只能捕获 owned 句柄;Arc 让「同一份冻结闭包」在调度器与被调
    /// 流程之间共享而无需深拷。
    flows: &'a Arc<Vec<AgentFlowConfig>>,
    /// 对比模式的可调用状态(二维批次 7b;入口层创建,静态子图层原样下传同一份 Arc)。
    /// None = 强制模式(未给 `flow_ids`,或名单在本轮闭包内一个都取不到)
    call_state: Option<&'a Arc<FlowCallState>>,
    /// 已进入的**动态调用**层数(入口 0;静态子图不改变它——动态深度与静态子图深度
    /// 是两条独立轴,各自封顶、各自报错文案)
    call_depth: usize,
}

/// 一层图跑完的汇总。
struct GraphOutcome {
    /// 各节点产出(下标对齐 steps;失败或空产出留空串,下游据此拿到空段)
    outputs: Vec<String>,
    /// 本层所有调用的 usage 合计
    total: TokenUsage,
    /// 是否出现过失败/空产出节点(成果已产出 → 任务 partial)
    any_error: bool,
}

/// 本层节点调用的记账 phase:入口流程 = `step`,子图 = `subflow.<父节点下标链>`。
/// `path` 为空即入口流程。
fn phase_for_path(path: &str) -> String {
    if path.is_empty() {
        "step".to_string()
    } else {
        format!("subflow.{}", path)
    }
}

/// 进入子图后的路径:入口层(空路径)记父节点下标,深层在其后追加。
fn child_path(path: &str, index: usize) -> String {
    if path.is_empty() {
        index.to_string()
    } else {
        format!("{}.{}", path, index)
    }
}

/// 可调用集 → 流程引用(二维批次 7b;生成 `run_flow` 描述与「本节点是否要释放工具」用)。
/// 顺序恒为可调用集顺序(= 名单声明顺序);解析不到的 id 跳过——可调用集本就是从**同一份**
/// 冻结闭包里筛出来的,这里的跳过只是防御,不是常规路径。
fn released_flows<'a>(
    callable: &[String],
    flows: &'a [AgentFlowConfig],
) -> Vec<&'a AgentFlowConfig> {
    callable
        .iter()
        .filter_map(|id| flows.iter().find(|f| &f.id == id))
        .collect()
}

/// 成果选拔(入口流程与子图共用,口径自二维批次 1 起未变):成果节点产出优先;
/// 为空则按**数组下标降序**回退到上一个非空生成产出——线性流程语义就是「末个有效
/// 生成步」,且该兜底与执行顺序无关(并行下成果节点失败时不会退化成「谁先跑完算谁」)。
fn select_draft(steps: &[PlanStep], output_idx: Option<usize>, outputs: &[String]) -> String {
    output_idx
        .map(|i| outputs[i].clone())
        .filter(|text| !text.is_empty())
        .or_else(|| {
            (0..steps.len())
                .rev()
                .find(|&i| {
                    steps[i].action == "direct"
                        && steps[i].generates == Some(true)
                        && !outputs[i].is_empty()
                })
                .map(|i| outputs[i].clone())
        })
        .unwrap_or_default()
}

impl CustomExecutor {
    pub(crate) fn new(svc: Arc<dyn TaskBackend>, engine: Arc<AgentEngine>) -> Self {
        CustomExecutor { svc, engine }
    }

    /// 组装步骤 system:内置基础指令(direct 生成=执行者 / direct 非生成=内部规划 /
    /// reflect=内置反思)+ 步骤提示词(用户可编辑 → 宏渲染 + untrusted 包裹)+ 任务目标上下文(untrusted 包裹)。
    /// generates=false 的 direct 步骤用内部规划指令:其语义是「只做分析规划、不产出正文」,
    /// 与步骤自身定位一致(此前复用执行者指令会吐出完整正文,2026-09-10 实测修复)。
    fn build_step_system(&self, step: &PlanStep, goal: &str) -> String {
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

    /// 带工具步骤:run_tool_loop + 步骤白名单;返回 (正文, 整轮 usage)。
    /// 调用追踪在本函数内落(phase 取自 `g`:入口 `step` / 静态子图 `subflow.<路径>` /
    /// 动态调用层 `call.<路径>`;对齐 generate_text 的统一出口语义:成功/空/中断/错误
    /// 均落一行 task_llm_calls)。
    /// `node_model` = 本节点连接的**真实模型名**(二维批次 5b;调用方已解析,不再恒取全局模型)。
    /// `max_tokens_override` = 空产出重试时的预算翻倍值(A 批 A2;None = 节点/全局配置的原始值)。
    // 参数较多是「追踪落库三件套(phase/step_index/model)」与执行输入各占一位所致,
    // 打包成结构体只会把一个直白的调用点换成一次构造,收益为负。
    #[allow(clippy::too_many_arguments)]
    async fn run_step_with_tools(
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
        // 任务模式工具策略:先按策略编译候选集(默认拒绝危险工具、剔除元工具),
        // 再与步骤白名单取交——步骤白名单只能收窄,不能突破任务策略放行危险工具。
        let policy = super::tool_policy::compile(
            &settings.task_tool_policy,
            &settings.task_tool_allowlist,
            &self.engine.tool_registry(),
            // 工作区工具族只在绑定了工作区的任务里下发
            ctx.scope.is_some(),
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
            // 节点级轮次上限(二维批次 5b):缺省沿用全局;严格档下本参数无意义(不下发工具),
            // 与「严格档保留工具配置」同一纪律——配置保留、切换档位即生效。
            max_tool_rounds: Some(step.max_tool_rounds.unwrap_or(settings.max_tool_rounds)),
            tool_choice,
            // 连接:节点级(二维批次 5b)优先,缺省回退**任务级**(A 批 B1);
            // 两者皆空 = 默认连接(run_tool_loop 内层经 engine 单点解析)
            connection_id: step
                .connection_ref()
                .map(str::to_string)
                .or_else(|| ctx.connection_id.clone()),
            parallel_tool_calls: step.parallel_tool_calls,
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
        let started = Instant::now();
        // 节点级单次调用超时(A 批 A1):None = 宿主既有 300s 看门狗(行为不变),
        // Some = 覆盖该值(工具循环**逐轮**各按它计,与宿主既有粒度一致)。
        let call_timeout = step
            .call_timeout_secs
            .map(|secs| std::time::Duration::from_secs(secs as u64));
        let result = run_tool_loop(
            &self.engine,
            &mut state_machine,
            None,
            &session_id,
            messages,
            &params,
            &tool_ctx,
            &tx,
            &ctx.cancel,
            &flag,
            &mut total_usage,
            &run_id,
            gate,
            call_timeout,
        )
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
                    &super::executor::to_self_heals(&res.self_heals),
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
            Ok(_) => {
                self.svc.record_llm_call(
                    &ctx.task_id,
                    phase,
                    Some(step_index),
                    &model,
                    messages,
                    "(已中断)",
                    None,
                    elapsed,
                    "error",
                );
                Err("任务已停止".into())
            }
            Err(e) => {
                // 步骤执行器对外契约是字符串错误(步骤 result/追踪列),分类在此落回文案;
                // 分类只服务聊天路径的 SSE 错误终态。
                let msg = e.message().to_string();
                self.svc.record_llm_call(
                    &ctx.task_id,
                    phase,
                    Some(step_index),
                    &model,
                    messages,
                    &msg,
                    None,
                    elapsed,
                    "error",
                );
                Err(msg)
            }
        }
    }

    /// 节点输入:任务目标 + 上游产出(二维批次 1;批次 8 起返回**分段**形态,由
    /// [`NodeInput::render_full`] 渲染出与批次 8 之前逐字节一致的文本)。
    ///  - 无上游(源节点/首步):给**本层源消息**——入口流程即任务目标(与旧版首步逐字节
    ///    一致),子图则是「挂载它的那个节点收到的输入消息」(上游产出因此流进子图);
    ///  - 单上游:沿用旧版格式「上一步「X」产出:」——线性兼容流程逐字节不变;
    ///  - 多上游:D3 按父节点**数组下标升序**逐段拼接(顺序确定 → 同一流程两次运行可复现)。
    ///
    ///    上游失败或产出为空时该段留空(不写占位词,与旧版清空 prev_output 的语义一致)。
    ///
    ///    分段还承载批次 8 的裁剪语义:段数组**最旧在前**,`max_context` 从下标 0 起省略。
    fn node_input(
        source_message: &str,
        goal: &str,
        steps: &[PlanStep],
        inputs: &[Vec<usize>],
        index: usize,
        outputs: &[String],
    ) -> NodeInput {
        match inputs[index].as_slice() {
            [] => NodeInput {
                head: source_message.to_string(),
                segments: Vec::new(),
            },
            [parent] => NodeInput {
                head: format!("任务目标:\n{}", goal),
                segments: vec![NodeSegment {
                    label: format!("上一步「{}」产出:", steps[*parent].name),
                    text: outputs[*parent].clone(),
                }],
            },
            many => NodeInput {
                head: format!("任务目标:\n{}", goal),
                segments: many
                    .iter()
                    .map(|&p| NodeSegment {
                        label: format!("【{}】", steps[p].name),
                        text: outputs[p].clone(),
                    })
                    .collect(),
            },
        }
    }

    /// 执行单个节点:挂载了子流程 → 跑子图(二维批次 6b);否则组装消息走纯生成 / 工具循环。
    /// 调用追踪口径不变(phase 由 `g` 给出、step_index = 步骤在**本层**数组中的下标);
    /// 失败与中断都返回 Err,由调度器决定状态与降级。
    /// 档位分支见 `PlanStep::is_strict`(二维批次 6a):严格档恒走单次调用。
    /// 节点级上下文上限(二维批次 8)在这里消费——它是**唯一**消费点(见 `apply_step_max_context`)。
    /// 节点级超时 / 空产出重试(A 批 A1/A2)同在这里消费(见 `run_node_attempt` 与本函数的重试循环)。
    async fn execute_node(
        &self,
        ctx: &TaskRunContext,
        g: &GraphCtx<'_>,
        index: usize,
        step: &PlanStep,
        input: NodeInput,
    ) -> Result<NodeOutcome, String> {
        // 子流程节点不自己发起模型调用(与 6a「档位优先于 tools」同一纪律:
        // goal/action/kind/tools/system_prompt/temperature/max_tokens 全被旁路但保留配置)。
        // 批次 8 的 `max_context` 同属被旁路的执行参数(子图各节点各自裁剪),故这里用
        // `render_full()` 把**未裁剪**的输入交出去,行为与批次 8 之前逐字节一致。
        // A 批的 `call_timeout_secs` / `max_retries` 同属被旁路的执行参数:子图各节点
        // 各自的配置生效,本节点这两项配置保留。
        if let Some(sub_id) = step.sub_flow_ref() {
            return self
                .run_sub_flow(ctx, g, index, step, sub_id, input.render_full())
                .await;
        }
        // 节点级连接(二维批次 5b):**先解析一次**——引用的连接被删除/停用即本节点失败
        // (文案点名连接,不静默回退默认连接;口径同 5a「有 flow_id 却无快照」)。
        // 解析结果同时给出本节点的真实模型,供纯生成与工具循环两条路径的调用追踪使用。
        // 挂载子流程的节点在上一行已返回:connection_id 对它**旁路**(子图各节点各自解析),
        // 故这里不会因一个被旁路的失效引用而误伤挂载节点。
        // 生效连接(A 批 B1):节点级优先,缺省回退**任务级**;两者皆空 = 默认连接。
        // 先解析一次即得本节点真实模型——必须用「生效连接」而不是只看节点字段,否则
        // 任务级连接的场景下调用追踪会记成默认连接的模型(记什么用什么)。
        // 引用失效在此**立即**失败(不静默回退),与 5b 口径一致。
        let effective_connection = step
            .connection_ref()
            .map(str::to_string)
            .or_else(|| ctx.connection_id.clone());
        let (_, node_model) = self
            .engine
            .resolve_connector(effective_connection.as_deref())
            .await
            .map_err(|e| format!("步骤「{}」:{}", step.name, e))?;
        let user = self.apply_step_max_context(
            step,
            &input,
            &node_model,
            ctx.settings.default_node_max_context,
        )?;
        let sys = self.build_step_system(step, &ctx.goal);
        // 空产出重试(A 批 A2):`max_retries` = **额外**尝试上限,总尝试 = 1 + n;
        // 未配即 1 次(与本批之前逐字节一致)。**只重试「产出为空」**:硬错误(连接失效/
        // 超时/上游报错)立即返回——那类失败重试只会把成本翻倍地耗在同一个坏引用上,
        // 且会与 `call_timeout_secs` 的语义互相掩盖。每次尝试都用**同一份**输入重建消息,
        // 不在上一轮的 messages 上续写(重试 = 重跑本节点,不是接着聊)。
        let max_attempts = step.max_retries.map(|n| n as usize).unwrap_or(0) + 1;
        let mut attempt = 0usize;
        // 重试时的输出预算覆盖(见下方翻倍逻辑):None = 用节点/全局配置的原始值
        let mut budget_override: Option<u32> = None;
        // 失败尝试的 token 携带量(A 批 A2 的记账收口):中间尝试各自落了一行
        // `task_llm_calls`,而节点级 usage 只由**最终结果**携带(调度器按 `out` 记一行)
        // ——两处口径不一致就会破「各行求和 == usage_total」不变量(WF-17 预告的记账风险)。
        // 故把失败尝试的 token 归并进最终结果;若最终是硬错误(Err 不带 usage),
        // 则显式补记一行,不让已花掉的 token 只留在明细里。
        let mut carried_prompt = 0i64;
        let mut carried_completion = 0i64;
        loop {
            attempt += 1;
            let mut res = self
                .run_node_attempt(
                    ctx,
                    g,
                    index,
                    step,
                    &sys,
                    &user,
                    &node_model,
                    budget_override,
                )
                .await;
            let retryable = matches!(&res, Ok(out) if out.text.trim().is_empty());
            if !retryable || attempt >= max_attempts {
                if let Ok(out) = &mut res {
                    out.out.prompt_tokens += carried_prompt;
                    out.out.completion_tokens += carried_completion;
                } else if carried_prompt > 0 || carried_completion > 0 {
                    self.svc.record_usage(
                        &ctx.task_id,
                        g.phase,
                        Some(index),
                        &TaskGenOutput {
                            text: String::new(),
                            finish_reason: None,
                            prompt_tokens: carried_prompt,
                            completion_tokens: carried_completion,
                            reasoning_tokens: 0,
                            reasoning_chars: 0,
                            tool_calls: Vec::new(),
                        },
                    );
                }
                return res;
            }
            if let Ok(out) = &res {
                carried_prompt += out.out.prompt_tokens;
                carried_completion += out.out.completion_tokens;
            }
            if *ctx.cancel.borrow() {
                return res;
            }
            // 重试前按 legacy/plan 同一算法**翻倍输出预算**(空产出最常见的成因是推理吃光
            // 预算,只重试不加预算等于把同一次失败重放一遍);翻倍不出(已到封顶)即同预算。
            let current = budget_override
                .unwrap_or_else(|| step.max_tokens.unwrap_or(ctx.settings.default_max_tokens));
            budget_override = Some(
                crate::utils::retry::doubled_heal_budget(
                    current,
                    super::retry::RETRY_MAX_TOKENS_CAP,
                )
                .unwrap_or(current),
            );
            tracing::warn!(
                task_id = %ctx.task_id,
                step = %step.name,
                attempt,
                max_attempts,
                max_tokens = budget_override.unwrap_or(current),
                "节点产出为空,按节点级重试配置再试一次(输出预算已翻倍)"
            );
            // 退避间隔复用 legacy/plan 侧同一常量(单一出处,不另立数字)
            tokio::time::sleep(super::retry::EMPTY_RETRY_BACKOFF).await;
            if *ctx.cancel.borrow() {
                return res;
            }
        }
    }

    /// 本节点的一次尝试:严格档/无工具走纯生成,宽松档 + 工具走工具循环。
    ///
    /// 抽成独立函数是为 A 批的空产出重试服务——重试必须**重建**消息(见 `execute_node`),
    /// 而两条路径的输入装配(系统提示/节点消息/超时/连接)完全相同,复制一份必然漂移。
    /// `sys`/`user` 是**本层**输入(已过 `apply_step_max_context` 裁剪);
    /// `max_tokens_override` 是重试时的预算翻倍值(None = 用节点/全局配置的原始值)。
    #[allow(clippy::too_many_arguments)]
    async fn run_node_attempt(
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
                    out: super::executor::usage_as_output(&usage),
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
    fn apply_step_max_context(
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

    /// 跑一个静态子图(二维批次 6b):被引用流程当作独立的一层图执行,
    /// **子图成果即本节点产出**(下游拿到的就是子图成果,与普通节点无差别)。
    ///
    /// 守卫(保存期 `validate_sub_flows` 的运行期兜底——防外部手改配置/导入的脏数据):
    ///  - 环:被引用流程已在当前调用链上 → 拒绝(A→B→A 会在运行期无限递归);
    ///  - 深度:再嵌一层会超过 [`MAX_SUB_FLOW_DEPTH`] → 拒绝(每层把该节点的调用
    ///    次数乘上子图节点数,深度是成本闸门);
    ///  - 结构:启用步骤非空、图可解析(图内环由 `resolve_graph` 拦下)。
    ///
    /// 记账:子图节点各自落调用行 + usage 行(phase = `subflow.<路径>`),本节点**不落**;
    /// 返回的 usage 只进 GraphOutcome 汇总(供完成日志),由调度器识别子流程节点跳过
    /// `record_usage`——否则同一批 token 会被父子两层各记一次,破坏
    /// 「`/calls` 各行求和 == 详情 usage_total」不变量。
    ///
    /// 降级(二维批次 6b 审查修正):子图内有失败/空产出节点、但成果仍选得出时,本节点
    /// **成功完成**——`degraded = true` 把信号带回本层,由调度器置任务 `partial`
    /// 并给 plan 行加标注(否则任务会以 `done` 收尾,与同层普通节点失败即 `partial`
    /// 的既有语义不自洽)。
    async fn run_sub_flow(
        &self,
        ctx: &TaskRunContext,
        g: &GraphCtx<'_>,
        index: usize,
        step: &PlanStep,
        sub_id: &str,
        incoming: String,
    ) -> Result<NodeOutcome, String> {
        let cfg = g
            .flows
            .iter()
            .find(|f| f.id == sub_id)
            .ok_or_else(|| {
                format!(
                    "子流程不在本任务的流程快照内:{}（快照为创建/开跑时冻结的编排，请检查流程库或任务快照）",
                    sub_id
                )
            })?;
        if g.chain.iter().any(|id| id == sub_id) {
            return Err(format!(
                "子流程调用链存在环:「{}」在当前调用链上已出现过,拒绝再次进入",
                flow_label(cfg)
            ));
        }
        if g.depth + 1 > MAX_SUB_FLOW_DEPTH {
            return Err(format!(
                "子流程嵌套超过 {} 层:步骤「{}」再嵌一层会超出上限",
                MAX_SUB_FLOW_DEPTH, step.name
            ));
        }
        let steps: Vec<PlanStep> = cfg.steps.iter().filter(|s| s.enabled).cloned().collect();
        if steps.is_empty() {
            return Err(format!("子流程「{}」没有启用的步骤", flow_label(cfg)));
        }
        let graph = resolve_graph(&steps)?;
        let output_idx = output_index(&steps, &graph.inputs);
        let path = child_path(g.path, index);
        let phase = phase_for_path(&path);
        let mut chain: Vec<String> = g.chain.to_vec();
        chain.push(sub_id.to_string());
        let inner = GraphCtx {
            steps: &steps,
            inputs: &graph.inputs,
            source_message: &incoming,
            phase: &phase,
            path: &path,
            depth: g.depth + 1,
            chain: &chain,
            max_parallel: effective_max_parallel(cfg),
            // 子图沿用**同一份**任务快照闭包(嵌套子流程也必须在冻结域内解析)
            flows: g.flows,
            // 对比模式状态与动态层数原样下传:静态子图的节点同样可以动态调用名单内流程,
            // 且它的环守卫看到的是**同一条**调用链(静态与动态共用,不存在两条判断分叉)
            call_state: g.call_state,
            call_depth: g.call_depth,
        };
        // 子图不写 plan:plan 是**外层**流程的节点列表,嵌套节点没有对应行(IFW-5)。
        // 进度体现在挂载节点那一行的 running 态与调用追踪的 subflow.<路径> 行
        let outcome = self.run_graph(ctx, &inner, None).await?;
        let draft = select_draft(&steps, output_idx, &outcome.outputs);
        if draft.is_empty() {
            return Err(format!(
                "子流程「{}」未产出任何成果(生成步骤全部失败或为空)",
                flow_label(cfg)
            ));
        }
        Ok(NodeOutcome {
            text: draft,
            out: super::executor::usage_as_output(&outcome.total),
            degraded: outcome.any_error,
        })
    }

    /// 构造本节点的 `run_flow` **调用点**(二维批次 7b)。
    ///
    /// 闭包捕获的全是 owned 副本(上下文 / 冻结闭包 Arc / 状态 Arc / 调用链 / 路径):
    /// 工具处理器由全局注册表在**任意时刻**调用,那时本节点的栈早已退出,任何借用都不成立。
    /// 任务上下文与闭包都用 Arc/Clone 共享而非深拷——一次节点工具循环只构造一次。
    fn flow_invoker(&self, ctx: &TaskRunContext, g: &GraphCtx<'_>) -> run_flow_tool::FlowInvoker {
        let svc = self.svc.clone();
        let engine = self.engine.clone();
        let run_ctx = ctx.clone();
        let flows = g.flows.clone();
        let state = g.call_state.cloned();
        let chain = g.chain.to_vec();
        let call_depth = g.call_depth;
        let parent_path = g.path.to_string();
        Arc::new(move |flow: String, input: String| {
            let (svc, engine) = (svc.clone(), engine.clone());
            let (run_ctx, flows, state) = (run_ctx.clone(), flows.clone(), state.clone());
            let (chain, parent_path) = (chain.clone(), parent_path.clone());
            Box::pin(async move {
                let Some(state) = state else {
                    return Err("本次任务未启用对比模式(没有可调用的流程名单)".to_string());
                };
                CustomExecutor::new(svc, engine)
                    .run_called_flow(
                        &run_ctx,
                        &flows,
                        &state,
                        &chain,
                        call_depth,
                        &parent_path,
                        &flow,
                        &input,
                    )
                    .await
            })
        })
    }

    /// 跑一套**被动态调用**的流程(二维批次 7b):名单内流程当作独立的一层图执行,
    /// 其成果即本次工具调用的结果(与静态子图「子图成果即本节点产出」同型)。
    ///
    /// 守卫顺序 = 拒绝代价从低到高,**全部在任何模型调用之前**:
    ///   ① 名单内解析(名单外 → 拒绝,连闭包都不查:这是「不烧钱」的关键一步);
    ///   ② 环 + 动态嵌套深度(见 `flow_call::check_call_guards`);
    ///   ③ 每任务调用预算(`charge`,兼作 `d<序号>`)。
    ///
    /// 记账:被调流程的顶层 phase = `call.<路径>`(路径含 `d<序号>`),它内部若再挂静态
    /// 子图则为 `subflow.<路径>`;逐节点各记一行调用 + 一行 usage,调用方节点自有 usage
    /// **不含**这些 token(工具调用不计入宿主节点的 usage 合计,与工具循环内其他工具同口径)。
    ///
    /// 降级:被调流程内有失败/空产出但成果仍选得出时,本节点成功返回、结果文本保持纯成果
    /// (不污染模型的后续推理),只把任务整体置 `partial`([`FlowCallState::mark_degraded`])。
    ///
    /// 静态深度**重新起算**:动态调用与静态子图是两条独立的成本轴(口径见
    /// `agent_flow_service::MAX_FLOW_CALL_DEPTH` 的说明),被调流程自己那套子图嵌套
    /// 从 0 层数起,否则「从子图深处调用」会让同一份流程在两种入口下行为不一致。
    #[allow(clippy::too_many_arguments)]
    async fn run_called_flow(
        &self,
        ctx: &TaskRunContext,
        flows: &Arc<Vec<AgentFlowConfig>>,
        state: &Arc<FlowCallState>,
        chain: &[String],
        call_depth: usize,
        parent_path: &str,
        flow_key: &str,
        input: &str,
    ) -> Result<String, String> {
        let cfg = flow_call::resolve_flow_ref(flows, state.callable(), flow_key)?;
        flow_call::check_call_guards(
            chain,
            call_depth,
            cfg,
            // 深度上限按本轮设置(A 批 A3;执行期改设置不影响本轮)
            ctx.settings.max_flow_call_depth as usize,
        )?;
        let n = state.charge()?;
        let steps: Vec<PlanStep> = cfg.steps.iter().filter(|s| s.enabled).cloned().collect();
        if steps.is_empty() {
            return Err(format!("流程「{}」没有启用的步骤", flow_label(cfg)));
        }
        let graph = resolve_graph(&steps)?;
        let output_idx = output_index(&steps, &graph.inputs);
        let path = flow_call::dynamic_path(parent_path, n);
        let phase = format!("call.{path}");
        let mut inner_chain: Vec<String> = chain.to_vec();
        inner_chain.push(cfg.id.clone());
        // 输入:模型给了 input 就用它,否则用任务目标(与静态子图「挂载节点收到的输入
        // 消息」同源语义——被调流程不是孤岛,但它的输入由调用方决定)
        let source = if input.trim().is_empty() {
            ctx.goal.clone()
        } else {
            input.to_string()
        };
        let inner = GraphCtx {
            steps: &steps,
            inputs: &graph.inputs,
            source_message: &source,
            phase: &phase,
            path: &path,
            depth: 0,
            chain: &inner_chain,
            max_parallel: effective_max_parallel(cfg),
            flows,
            call_state: Some(state),
            call_depth: call_depth + 1,
        };
        // 被调流程不写 plan:plan 是**入口流程**的节点列表,动态层的节点没有对应行
        // (与静态子图同口径,IFW-5)
        let outcome = self.run_graph(ctx, &inner, None).await?;
        let draft = select_draft(&steps, output_idx, &outcome.outputs);
        if draft.is_empty() {
            return Err(format!(
                "流程「{}」未产出任何成果(生成步骤全部失败或为空)",
                flow_label(cfg)
            ));
        }
        if outcome.any_error {
            state.mark_degraded();
        }
        Ok(draft)
    }

    /// 执行一层图(调度语义见 `run_graph_inner`)。
    ///
    /// 返回**装箱** future:入口流程与静态子图共用同一调度器,于是存在
    /// 「节点 future → execute_node → run_sub_flow → run_graph」的递归 async 调用。
    /// 不装箱时 rustc 无法为这个自指的递归 future 推导 `Send`(节点 future 装在
    /// `FuturesUnordered` 里并发跑,必须 Send);装箱把递归边界擦除成
    /// `dyn Future + Send`,类型即可有限展开。
    fn run_graph<'a>(
        &'a self,
        ctx: &'a TaskRunContext,
        g: &'a GraphCtx<'a>,
        plan: Option<&'a mut Vec<TaskStep>>,
    ) -> BoxFuture<'a, Result<GraphOutcome, String>> {
        Box::pin(self.run_graph_inner(ctx, g, plan))
    }

    /// 一层图的调度循环:就绪判定 = 上游计数 + 后继表,就绪节点按下标升序取
    /// (线性流程顺序与旧版一致)。
    ///
    /// plan 写入纪律(IFW-4):**调度器是 plan 的唯一写者**,节点执行体只回传文本、
    /// 不持有 plan 快照。多节点并发完成时因此不存在「各自读旧快照再整列覆写」的丢更新
    /// (那是把 plan 交给各节点自行读-改-写才会有的缺陷)。这条纪律由
    /// tests/task_custom_graph.rs 的并发用例锁定:全部节点终态必须都落回 plan。
    /// 子图传 `plan = None`(不写 plan),其余调度语义与入口流程完全一致。
    ///
    /// 取消语义与抽取前一致:发现取消即丢弃在飞 future 并返回 Err(「任务已停止」),
    /// 由调用方(入口 run_inner 或上层节点)收口。
    async fn run_graph_inner(
        &self,
        ctx: &TaskRunContext,
        g: &GraphCtx<'_>,
        mut plan: Option<&mut Vec<TaskStep>>,
    ) -> Result<GraphOutcome, String> {
        let steps = g.steps;
        let svc = &self.svc;
        let node_count = steps.len();
        let mut missing_parents: Vec<usize> = g.inputs.iter().map(Vec::len).collect();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); node_count];
        for (i, parents) in g.inputs.iter().enumerate() {
            for &p in parents {
                children[p].push(i);
            }
        }
        let mut ready: BTreeSet<usize> = (0..node_count)
            .filter(|&i| missing_parents[i] == 0)
            .collect();
        // 在飞节点集合:节点 future 借用 &self / 流程数据 / ctx,故用 FuturesUnordered
        // 而非 JoinSet(后者要求 'static 捕获,会把执行器与流程数据逼成 Arc);
        // 两者并发语义等价(都是等待 LLM 的异步 I/O),且不引入新依赖。
        let mut inflight: FuturesUnordered<NodeFuture<'_>> = FuturesUnordered::new();
        let mut finished = 0usize;
        // 各节点产出(下标对齐 steps):失败或空产出留空串,下游据此拿到空段
        let mut outputs: Vec<String> = vec![String::new(); node_count];
        let mut any_error = false;
        let mut total = TokenUsage::default();

        while finished < node_count {
            // 补满在飞槽位(上限 = 本层流程的 max_parallel_nodes,默认 2:并行成倍消耗 token)
            while inflight.len() < g.max_parallel {
                let Some(&i) = ready.iter().next() else { break };
                ready.remove(&i);
                if *ctx.cancel.borrow() {
                    // 取消:丢弃在飞 future 即中止分支(与串行口径一致:在飞行保持 running),
                    // 随后统一由「任务已停止」收口
                    drop(inflight);
                    return Err("任务已停止".into());
                }
                if let Some(p) = plan.as_deref_mut() {
                    p[i].status = TaskStepStatus::Running;
                    svc.set_plan(&ctx.task_id, p);
                }
                let input =
                    Self::node_input(g.source_message, &ctx.goal, steps, g.inputs, i, &outputs);
                let (ctx_ref, step) = (ctx, &steps[i]);
                inflight.push(Box::pin(async move {
                    (i, self.execute_node(ctx_ref, g, i, step, input).await)
                }));
            }
            let Some((i, result)) = inflight.next().await else {
                break;
            };
            finished += 1;
            let step = &steps[i];
            match result {
                Ok(NodeOutcome {
                    text,
                    out,
                    degraded,
                }) => {
                    total.prompt_tokens += out.prompt_tokens;
                    total.completion_tokens += out.completion_tokens;
                    total.total_tokens += out.prompt_tokens + out.completion_tokens;
                    // usage 落库:逐节点一行(phase 同调用行,对齐 legacy 按调用计口径)。
                    // 子流程节点除外——子图各节点已经在自己的那一层记过账了,
                    // 再记一次汇总行会破坏「各行求和 == usage_total」不变量。
                    if !step.is_sub_flow() {
                        svc.record_usage(&ctx.task_id, g.phase, Some(i), &out);
                    }
                    // 子图降级(子图内有失败/空产出节点,但成果仍选得出)同样算 any_error:
                    // 本节点虽成功,整条链的质量已降级,任务不该报 done(见 NodeOutcome)
                    any_error |= degraded;
                    if text.is_empty() {
                        any_error = true;
                        if let Some(p) = plan.as_deref_mut() {
                            p[i].status = TaskStepStatus::Error;
                            p[i].result = format!("步骤「{}」返回空内容", step.name);
                        }
                    } else {
                        if let Some(p) = plan.as_deref_mut() {
                            p[i].status = TaskStepStatus::Done;
                            // 标注只改**展示**(p[i].result);outputs[i] 仍是纯成果,
                            // 免得「(子图内有失败节点)」混进下游提示词
                            if degraded {
                                p[i].result = format!("(子图内有失败节点)\n{text}");
                            } else if step.action == "direct"
                                && step.generates == Some(false)
                                && !step.is_sub_flow()
                            {
                                // generates=false 的内部规划步骤:产出仅供后续步骤参考,
                                // 加标注区分于面向用户的成果(不参与成果选拔)。
                                // 挂载子流程的节点除外——它的产出是子图成果(正文级),
                                // 标注会反着说(子图成果被写成「内部规划」)。
                                p[i].result = format!("(内部规划)\n{text}");
                            } else {
                                p[i].result = text.clone();
                            }
                        }
                        outputs[i] = text;
                    }
                }
                Err(e) => {
                    if *ctx.cancel.borrow() {
                        drop(inflight);
                        return Err("任务已停止".into());
                    }
                    any_error = true;
                    if let Some(p) = plan.as_deref_mut() {
                        p[i].status = TaskStepStatus::Error;
                        p[i].result = e;
                    }
                }
            }
            if let Some(p) = plan.as_deref_mut() {
                svc.set_plan(&ctx.task_id, p);
            }
            // 释放后继:上游全部完成(无论成败)即就绪——下游照常执行并拿到空段,
            // 与串行版「上游失败即清空产出、后续步骤继续」的语义一致
            for &child in &children[i] {
                missing_parents[child] -= 1;
                if missing_parents[child] == 0 {
                    ready.insert(child);
                }
            }
        }
        Ok(GraphOutcome {
            outputs,
            total,
            any_error,
        })
    }

    async fn run_inner(&self, ctx: TaskRunContext) -> Result<(TaskTerminal, TokenUsage), String> {
        // 本轮要跑的流程 = 任务流程快照(二维批次 5a):绑定任务用它创建时冻结的那份
        // (改流程/换当前流程都不影响),未绑定任务按当时的当前流程解析。
        // 解析与校验收在宿主侧单一出处(`TaskService::resolve_task_flow`),此处不复制规则。
        let task = self.svc.get(&ctx.task_id);
        let snapshot = self.svc.resolve_task_flow(task.as_ref())?;
        // 未绑定任务:把本轮**实际用到**的编排记进任务行(徽标与追溯的数据源)。
        // 重跑会按新的当前流程覆写——未绑定语义就是「跟随当前流程」,快照只作记录。
        if task.as_ref().is_some_and(|t| t.flow_id.is_none()) {
            self.svc.set_flow_snapshot(&ctx.task_id, &snapshot);
        }
        let cfg = snapshot
            .root()
            .ok_or_else(|| format!("流程快照缺少入口流程:{}", snapshot.root_id))?
            .clone();
        let steps: Vec<PlanStep> = cfg.steps.iter().filter(|s| s.enabled).cloned().collect();
        if steps.is_empty() {
            return Err("当前 Agent 流程没有启用的步骤".into());
        }
        // 图语义(二维批次 1):输入集/拓扑序/成果节点三件事都由共享原语回答——
        // 与保存期校验(validate_flow)、聊天侧线性化(make_custom_plan)同一出处,
        // 此处不复制实现(否则「什么算合法图」会出现多个版本)。
        let graph = resolve_graph(&steps)?;
        let output_idx = output_index(&steps, &graph.inputs);
        let svc = &self.svc;
        svc.set_status(&ctx.task_id, TaskStatus::Running);

        // 进度模型 = plan 步骤(前端 custom 渲染:plan 步骤 + 状态徽标)。
        // 展示顺序恒为流程数组顺序(用户编排顺序),执行顺序另由 graph.order 决定。
        // 挂载子流程的节点仍占**一行**(子图内部节点不进 plan,见 run_sub_flow)。
        //
        // node_id = 流程节点 id(`PlanStep.id`):plan 行由**过滤后的启用步骤**构造,
        // 故 plan 下标 ≠ 流程数组下标——前端要问「这一行是哪个节点」只能靠它
        // (遗留.md IFW-5)。其余模式不填,序列化时整键省略。
        let mut plan: Vec<TaskStep> = steps
            .iter()
            .map(|s| TaskStep {
                name: s.name.clone(),
                goal: s.goal.clone(),
                status: TaskStepStatus::Pending,
                result: String::new(),
                node_id: Some(s.id.clone()),
            })
            .collect();
        svc.set_plan(&ctx.task_id, &plan);

        // 调用链以入口流程 id 起头:子图引用回入口流程会被运行期环守卫拦下
        let chain = vec![cfg.id.clone()];
        // 对比模式(二维批次 7b):可调用集 = 名单 ∩ 冻结闭包 − 根流程。
        // 名单为空 = 强制模式(None);名单非空但一份都取不到(成员已从库里消失且不在
        // 冻结域内)→ 本轮按强制模式跑并告警——「根流程照常执行」是主语义,
        // 不该因为名单失效让整个任务跑不起来。
        let flow_ids = task
            .as_ref()
            .and_then(|t| t.flow_ids.clone())
            .unwrap_or_default();
        let call_state = if flow_ids.is_empty() {
            None
        } else {
            let callable = flow_call::callable_ids(&flow_ids, &snapshot.flows, &cfg.id);
            if callable.is_empty() {
                tracing::warn!(
                    task_id = ctx.task_id,
                    "对比模式名单在本轮流程快照内一个都取不到,本轮按强制模式执行"
                );
                None
            } else {
                Some(Arc::new(FlowCallState::new(
                    callable,
                    // 每任务调用次数上限按本轮设置(A 批 A3)
                    ctx.settings.max_flow_calls_per_task as usize,
                )))
            }
        };
        // 冻结闭包改由 Arc 承载(动态调用的工具处理器要在节点栈退出后重跑被调流程,
        // 只能捕获 owned 句柄;Arc 让调度器与被调流程共享同一份闭包而不深拷)
        let flows = Arc::new(snapshot.flows);
        let g = GraphCtx {
            steps: &steps,
            inputs: &graph.inputs,
            source_message: &ctx.goal,
            phase: "step",
            path: "",
            depth: 0,
            chain: &chain,
            // 并行上限(二维批次 2):流程级配置,缺省 2、1 = 完全串行(批次 1 行为)
            max_parallel: effective_max_parallel(&cfg),
            // 子图解析域 = 本轮快照闭包(二维批次 5a)
            flows: &flows,
            call_state: call_state.as_ref(),
            call_depth: 0,
        };
        let outcome = self.run_graph(&ctx, &g, Some(&mut plan)).await?;

        // 成果选拔:成果节点产出优先;为空则回退到末个非空生成产出(见 `select_draft`)
        let draft = select_draft(&steps, output_idx, &outcome.outputs);
        if draft.is_empty() {
            return Err("自定义流程未产出任何成果(生成步骤全部失败或为空)".into());
        }
        // 含 error 节点但成果已产出 → partial(对齐 legacy WP3 语义)。
        // 对比模式再加一条:被调流程内有失败/空产出(degraded)同算降级——与本层静态
        // 子图的 degraded 同一口径(那条链的质量已经降级,不该报 done)。
        let degraded = call_state.as_ref().is_some_and(|s| s.degraded());
        let status = if outcome.any_error || degraded {
            TaskStatus::Partial
        } else {
            TaskStatus::Done
        };
        Ok((
            TaskTerminal::Complete {
                result: draft,
                status,
                error: None,
            },
            outcome.total,
        ))
    }
}

impl ModeExecutor for CustomExecutor {
    fn run<'a>(
        &'a self,
        ctx: TaskRunContext,
    ) -> BoxFuture<'a, Result<(TaskTerminal, TokenUsage), String>> {
        Box::pin(async move { self.run_inner(ctx).await })
    }
}
