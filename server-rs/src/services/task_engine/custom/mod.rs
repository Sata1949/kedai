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

// C-2D(2026-10-25):本模块由单文件拆为目录模块(纯搬运,行为零变化)——mod.rs 只留
// 执行器骨架、共享类型与 `run_inner` 收尾;step(单节点生成/工具循环/上下文裁剪)、
// graph(节点图调度)、subflow(静态子图与动态流程调用)按职责分文件,context 放
// 纯路径/成果选拔辅助。外部路径 `task_engine::custom::CustomExecutor` 零改动。

mod context;
mod graph;
mod step;
mod subflow;

use self::context::select_draft;
use super::context::TaskRunContext;
use super::executor::ModeExecutor;
use super::flow_call::{self, FlowCallState};
use crate::agents::engine::AgentEngine;
use crate::models::types::{PlanStep, TaskStatus, TaskStep, TaskStepStatus, TokenUsage};
use crate::services::agent_flow_service::{
    effective_max_parallel, output_index, resolve_graph, AgentFlowConfig,
};
use crate::services::prompt_kit::OMITTED_SEGMENT_MARKER;
use crate::services::task_core::{fallback_terminal, TaskBackend, TaskGenOutput, TaskTerminal};
use futures::future::BoxFuture;
use std::sync::Arc;

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

impl CustomExecutor {
    pub(crate) fn new(svc: Arc<dyn TaskBackend>, engine: Arc<AgentEngine>) -> Self {
        CustomExecutor { svc, engine }
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
                files: Vec::new(),
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
        // 运行失败的降级(提交 2 部分成果兜底):取消保持 Err(引擎按 ended 收尾,终态
        // 兜底会从库里的 plan 补写已完成节点产出);其余失败能拼出已完成节点产出即 partial。
        // 用量口径:Err 分支拿不到 outcome.total,此处返回默认值——权威用量在
        // task_usage / task_llm_calls(每个节点各自 record_usage),该返回值只进引擎完成日志。
        let outcome = match self.run_graph(&ctx, &g, Some(&mut plan)).await {
            Ok(o) => o,
            Err(e) if *ctx.cancel.borrow() => return Err(e),
            Err(e) => return Ok((fallback_terminal(&plan, e), TokenUsage::default())),
        };

        // 成果选拔:成果节点产出优先;为空则回退到末个非空生成产出(见 `select_draft`)
        let draft = select_draft(&steps, output_idx, &outcome.outputs);
        if draft.is_empty() {
            // 无选拔结果时 plan 里仍可能有已完成节点产出(如生成步全空但工具步已跑),
            // 能拼出成果就走 partial,一步都没有才 Failed
            return Ok((
                fallback_terminal(
                    &plan,
                    "自定义流程未产出任何成果(生成步骤全部失败或为空)".into(),
                ),
                outcome.total,
            ));
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
