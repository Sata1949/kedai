// custom 模式:复用 AgentFlowService 当前启用流程(AgentFlowConfig 步骤序列,
// L2 既有资产)配轻量 step 执行器(批次 4.3b,docs/功能.md 第一节)。
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
use super::context::TaskRunContext;
use super::executor::ModeExecutor;
use super::sink;
use crate::agents::engine::executor::run_tool_loop;
use crate::agents::engine::{AbortFlag, AgentEngine};
use crate::agents::state_machine::StateMachine;
use crate::models::types::{
    GenerationParams, LlmMessage, PlanStep, TaskStatus, TaskStep, TaskStepStatus, TokenUsage,
    ToolChoice, ToolContext,
};
use crate::services::agent_flow_service::{
    effective_max_parallel, flow_label, output_index, resolve_graph, MAX_SUB_FLOW_DEPTH,
};
use crate::services::prompt_kit::untrusted_boundary;
use crate::services::task_core::prompt_consts::{
    CUSTOM_REFLECT_PROMPT, EXECUTOR_PROMPT, TASK_INTERNAL_PLAN_PROMPT,
};
use crate::services::task_core::{TaskBackend, TaskGenOutput, TaskTerminal};
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
    /// 调用追踪在本函数内落(phase 由调用方给出:`step` 或 `subflow.<路径>`;
    /// 对齐 generate_text 的统一出口语义:成功/空/中断/错误均落一行 task_llm_calls)。
    async fn run_step_with_tools(
        &self,
        ctx: &TaskRunContext,
        step_index: usize,
        step: &PlanStep,
        messages: &mut Vec<LlmMessage>,
        whitelist: &[String],
        phase: &str,
    ) -> Result<(String, TokenUsage), String> {
        let settings = &ctx.settings;
        // 任务模式工具策略:先按策略编译候选集(默认拒绝危险工具、剔除元工具),
        // 再与步骤白名单取交——步骤白名单只能收窄,不能突破任务策略放行危险工具。
        let policy = super::tool_policy::compile(
            &settings.task_tool_policy,
            &settings.task_tool_allowlist,
            &self.engine.tool_registry(),
        );
        // Some([]) = 策略全量集;Some(list) = 策略集 ∩ 步骤白名单
        let tools: Vec<_> = if whitelist.is_empty() {
            policy.defs
        } else {
            policy
                .defs
                .into_iter()
                .filter(|d| whitelist.iter().any(|w| w == &d.name))
                .collect()
        };
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
            max_tokens: step.max_tokens.unwrap_or(settings.default_max_tokens),
            stop: None,
            tools,
            max_tool_rounds: Some(settings.max_tool_rounds),
            tool_choice,
            parallel_tool_calls: step.parallel_tool_calls,
        };
        // 会话 id 保留既有形状 `task:{任务 id}:step:{下标}`(入口流程逐字节不变);
        // 子图用 phase 段替换 `step`,避免同一下标在父子两层撞 key(取值仍可用
        // split(':')[1] 反解任务 id)
        let session_id = format!("task:{}:{}:{}", ctx.task_id, phase, step_index + 1);
        let mut state_machine = StateMachine::new(&session_id);
        let run_id = Uuid::new_v4().to_string();
        let (flag, _flag_rx) = AbortFlag::new();
        let tool_ctx = ToolContext {
            session_id: session_id.clone(),
            character_id: String::new(), // custom 不吃角色卡
            agent_depth: 0,
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
        )
        .await;
        drop(tx);
        let _ = drain.await;

        let elapsed = started.elapsed();
        let model = self.engine.model();
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

    /// 节点 user 消息:任务目标 + 上游产出(二维批次 1)。
    ///  - 无上游(源节点/首步):给**本层源消息**——入口流程即任务目标(与旧版首步逐字节
    ///    一致),子图则是「挂载它的那个节点收到的输入消息」(上游产出因此流进子图);
    ///  - 单上游:沿用旧版格式「上一步「X」产出:」——线性兼容流程逐字节不变;
    ///  - 多上游:D3 按父节点**数组下标升序**逐段拼接(顺序确定 → 同一流程两次运行可复现)。
    ///
    ///    上游失败或产出为空时该段留空(不写占位词,与旧版清空 prev_output 的语义一致)。
    fn node_user_message(
        source_message: &str,
        goal: &str,
        steps: &[PlanStep],
        inputs: &[Vec<usize>],
        index: usize,
        outputs: &[String],
    ) -> String {
        match inputs[index].as_slice() {
            [] => source_message.to_string(),
            [parent] => format!(
                "任务目标:\n{}\n\n上一步「{}」产出:\n{}",
                goal, steps[*parent].name, outputs[*parent]
            ),
            many => {
                let segments: Vec<String> = many
                    .iter()
                    .map(|&p| format!("【{}】\n{}", steps[p].name, outputs[p]))
                    .collect();
                format!(
                    "任务目标:\n{}\n\n上游节点产出:\n{}",
                    goal,
                    segments.join("\n\n")
                )
            }
        }
    }

    /// 执行单个节点:挂载了子流程 → 跑子图(二维批次 6b);否则组装消息走纯生成 / 工具循环。
    /// 调用追踪口径不变(phase 由 `g` 给出、step_index = 步骤在**本层**数组中的下标);
    /// 失败与中断都返回 Err,由调度器决定状态与降级。
    /// 档位分支见 `PlanStep::is_strict`(二维批次 6a):严格档恒走单次调用。
    async fn execute_node(
        &self,
        ctx: &TaskRunContext,
        g: &GraphCtx<'_>,
        index: usize,
        step: &PlanStep,
        user: String,
    ) -> Result<NodeOutcome, String> {
        // 子流程节点不自己发起模型调用(与 6a「档位优先于 tools」同一纪律:
        // goal/action/kind/tools/system_prompt/temperature/max_tokens 全被旁路但保留配置)
        if let Some(sub_id) = step.sub_flow_ref() {
            return self.run_sub_flow(ctx, g, index, step, sub_id, user).await;
        }
        let sys = self.build_step_system(step, &ctx.goal);
        let mut messages = vec![
            LlmMessage::plain("system", &sys),
            LlmMessage::plain("user", &user),
        ];
        // 档位优先于 tools 配置(二维批次 6a):严格档 = 单次调用、不下发任何工具——
        // 即使步骤声明了工具也如此(配置保留在流程里,切回宽松档即生效;编辑器已按此提示)。
        match (&step.tools, step.is_strict()) {
            // 无工具步骤,或严格档:纯生成统一出口(generate_text 自带调用追踪落库)
            (None, _) | (Some(_), true) => {
                let settings = &ctx.settings;
                self.svc
                    .generate_text(
                        &ctx.task_id,
                        g.phase,
                        Some(index),
                        messages,
                        Vec::new(),
                        step.max_tokens.unwrap_or(settings.default_max_tokens),
                        step.temperature.unwrap_or(settings.default_temperature),
                        settings.default_top_p,
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
                .run_step_with_tools(ctx, index, step, &mut messages, list, g.phase)
                .await
                .map(|(text, usage)| NodeOutcome {
                    out: super::executor::usage_as_output(&usage),
                    text,
                    degraded: false,
                }),
        }
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
        let cfg = self.svc.flow_by_id(sub_id)?;
        if g.chain.iter().any(|id| id == sub_id) {
            return Err(format!(
                "子流程调用链存在环:「{}」在当前调用链上已出现过,拒绝再次进入",
                flow_label(&cfg)
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
            return Err(format!("子流程「{}」没有启用的步骤", flow_label(&cfg)));
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
            max_parallel: effective_max_parallel(&cfg),
        };
        // 子图不写 plan:plan 是**外层**流程的节点列表,嵌套节点没有对应行(IFW-5)。
        // 进度体现在挂载节点那一行的 running 态与调用追踪的 subflow.<路径> 行
        let outcome = self.run_graph(ctx, &inner, None).await?;
        let draft = select_draft(&steps, output_idx, &outcome.outputs);
        if draft.is_empty() {
            return Err(format!(
                "子流程「{}」未产出任何成果(生成步骤全部失败或为空)",
                flow_label(&cfg)
            ));
        }
        Ok(NodeOutcome {
            text: draft,
            out: super::executor::usage_as_output(&outcome.total),
            degraded: outcome.any_error,
        })
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
                let user = Self::node_user_message(
                    g.source_message,
                    &ctx.goal,
                    steps,
                    g.inputs,
                    i,
                    &outputs,
                );
                let (ctx_ref, step) = (ctx, &steps[i]);
                inflight.push(Box::pin(async move {
                    (i, self.execute_node(ctx_ref, g, i, step, user).await)
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
        let cfg = self.svc.current_flow()?;
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
        let mut plan: Vec<TaskStep> = steps
            .iter()
            .map(|s| TaskStep {
                name: s.name.clone(),
                goal: s.goal.clone(),
                status: TaskStepStatus::Pending,
                result: String::new(),
            })
            .collect();
        svc.set_plan(&ctx.task_id, &plan);

        // 调用链以入口流程 id 起头:子图引用回入口流程会被运行期环守卫拦下
        let chain = vec![cfg.id.clone()];
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
        };
        let outcome = self.run_graph(&ctx, &g, Some(&mut plan)).await?;

        // 成果选拔:成果节点产出优先;为空则回退到末个非空生成产出(见 `select_draft`)
        let draft = select_draft(&steps, output_idx, &outcome.outputs);
        if draft.is_empty() {
            return Err("自定义流程未产出任何成果(生成步骤全部失败或为空)".into());
        }
        // 含 error 节点但成果已产出 → partial(对齐 legacy WP3 语义)
        let status = if outcome.any_error {
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
