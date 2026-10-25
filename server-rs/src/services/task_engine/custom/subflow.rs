// custom 子流程:静态子图展开与对比模式的动态流程调用(三层守卫 + 记账路径)。
// 由原 `custom.rs` 整段搬来(C-2D,纯搬运)。
use super::context::{child_path, phase_for_path, select_draft};
use super::{CustomExecutor, GraphCtx, NodeOutcome};
use crate::models::types::PlanStep;
use crate::services::agent_flow_service::{
    effective_max_parallel, flow_label, output_index, resolve_graph, AgentFlowConfig,
    MAX_SUB_FLOW_DEPTH,
};
use crate::services::task_engine::context::TaskRunContext;
use crate::services::task_engine::flow_call::{self, FlowCallState};
use crate::tools::run_flow as run_flow_tool;
use std::sync::Arc;

impl CustomExecutor {
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
    pub(super) async fn run_sub_flow(
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
            out: crate::services::task_engine::executor::usage_as_output(&outcome.total),
            degraded: outcome.any_error,
        })
    }

    /// 构造本节点的 `run_flow` **调用点**(二维批次 7b)。
    ///
    /// 闭包捕获的全是 owned 副本(上下文 / 冻结闭包 Arc / 状态 Arc / 调用链 / 路径):
    /// 工具处理器由全局注册表在**任意时刻**调用,那时本节点的栈早已退出,任何借用都不成立。
    /// 任务上下文与闭包都用 Arc/Clone 共享而非深拷——一次节点工具循环只构造一次。
    pub(super) fn flow_invoker(
        &self,
        ctx: &TaskRunContext,
        g: &GraphCtx<'_>,
    ) -> run_flow_tool::FlowInvoker {
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
}
