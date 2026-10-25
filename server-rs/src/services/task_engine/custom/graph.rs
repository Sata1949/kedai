// custom 节点图:节点输入装配、单节点调度(含子流程分流)与并发调度循环。
// 由原 `custom.rs` 整段搬来(C-2D,纯搬运)。
use super::{
    CustomExecutor, GraphCtx, GraphOutcome, NodeFuture, NodeInput, NodeOutcome, NodeSegment,
};
use crate::models::types::{PlanStep, TaskEventKind, TaskStep, TaskStepStatus, TokenUsage};
use crate::services::task_core::TaskGenOutput;
use crate::services::task_engine::context::TaskRunContext;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use std::collections::BTreeSet;

impl CustomExecutor {
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
    pub(super) async fn execute_node(
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
                            dropped_tool_calls: 0,
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
                    crate::services::task_engine::retry::RETRY_MAX_TOKENS_CAP,
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
            tokio::time::sleep(crate::services::task_engine::retry::EMPTY_RETRY_BACKOFF).await;
            if *ctx.cancel.borrow() {
                return res;
            }
        }
    }

    /// 执行一层图(调度语义见 `run_graph_inner`)。
    ///
    /// 返回**装箱** future:入口流程与静态子图共用同一调度器,于是存在
    /// 「节点 future → execute_node → run_sub_flow → run_graph」的递归 async 调用。
    /// 不装箱时 rustc 无法为这个自指的递归 future 推导 `Send`(节点 future 装在
    /// `FuturesUnordered` 里并发跑,必须 Send);装箱把递归边界擦除成
    /// `dyn Future + Send`,类型即可有限展开。
    pub(super) fn run_graph<'a>(
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
        // 任务总预算跳过标记(PRODCAP-2):截止期已过且仍有可启动节点未被启动
        let mut budget_skipped = false;
        let mut total = TokenUsage::default();

        while finished < node_count {
            // 补满在飞槽位(上限 = 本层流程的 max_parallel_nodes,默认 2:并行成倍消耗 token)
            while inflight.len() < g.max_parallel {
                let Some(&i) = ready.iter().next() else { break };
                // 任务总预算用尽(PRODCAP-2):不再启动新节点——在飞节点各自由被
                // 截止期收窄的步骤预算收口(不 drop:硬切断会把在飞调用留给「中断」语义)
                if crate::services::task_engine::deadline_exhausted(ctx.deadline) {
                    budget_skipped = true;
                    break;
                }
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
                            // 文案走单一出处(提交 3 · D6,补上此前唯一缺 finish_reason 的点):
                            // 本处拿不到节点实际用的输出上限(在 run_node_attempt 内算),
                            // 传 None 省略该子句,不报一个可能不准的数。
                            p[i].result =
                                crate::services::task_core::prompt_consts::empty_output_error(
                                    &format!("步骤「{}」", step.name),
                                    &out,
                                    None,
                                );
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
        // 任务总预算用尽:未启动的节点统一标错(与取消同款「不留永远 pending 的步骤」
        // 纪律),并计入 any_error——整条链未走完,任务不得报 done(如实降级 partial)。
        if budget_skipped {
            svc.emit_event(
                TaskEventKind::Status,
                &ctx.task_id,
                None,
                None,
                Some("任务总预算用尽:部分节点未启动,以已完成部分收尾".into()),
            );
            any_error = true;
            // 最后一次使用 plan(后续不再借用),直接取所有权避免冗余重借
            if let Some(p) = plan {
                for s in p.iter_mut() {
                    if s.status == TaskStepStatus::Pending {
                        s.status = TaskStepStatus::Error;
                        s.result = "任务总预算用尽(未执行)".into();
                    }
                }
                svc.set_plan(&ctx.task_id, p);
            }
        }
        Ok(GraphOutcome {
            outputs,
            total,
            any_error,
        })
    }
}
