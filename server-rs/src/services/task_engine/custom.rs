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
use crate::services::agent_flow_service::{effective_max_parallel, output_index, resolve_graph};
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

/// 在飞节点的执行 future:节点在流程数组中的下标 + 执行结果
/// (Err 为该节点的中文错误文本,由调度器落回 plan 行)
type NodeFuture<'a> = BoxFuture<'a, (usize, Result<(String, TaskGenOutput), String>)>;

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
    /// 调用追踪在本函数内落(phase=step;对齐 generate_text 的统一出口语义:
    /// 成功/空/中断/错误均落一行 task_llm_calls)。
    async fn run_step_with_tools(
        &self,
        ctx: &TaskRunContext,
        step_index: usize,
        step: &PlanStep,
        messages: &mut Vec<LlmMessage>,
        whitelist: &[String],
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
        let session_id = format!("task:{}:step:{}", ctx.task_id, step_index + 1);
        let mut state_machine = StateMachine::new(&session_id);
        let run_id = Uuid::new_v4().to_string();
        let (flag, _flag_rx) = AbortFlag::new();
        let tool_ctx = ToolContext {
            session_id: session_id.clone(),
            character_id: String::new(), // custom 不吃角色卡
            agent_depth: 0,
        };
        // 事件桥(批次 R4 携 phase/step_index):custom 工具步骤的调用追踪口径为
        // phase=step + 本步骤下标(与下方 record_llm_call 一致)
        let (tx, drain) = sink::spawn(
            self.svc.clone(),
            ctx.task_id.clone(),
            "主 agent",
            "step",
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
                    "step",
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
                    "step",
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
                    "step",
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
                    "step",
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
    ///  - 无上游(源节点/首步):只给任务目标(与旧版首步逐字节一致);
    ///  - 单上游:沿用旧版格式「上一步「X」产出:」——线性兼容流程逐字节不变;
    ///  - 多上游:D3 按父节点**数组下标升序**逐段拼接(顺序确定 → 同一流程两次运行可复现)。
    ///
    ///    上游失败或产出为空时该段留空(不写占位词,与旧版清空 prev_output 的语义一致)。
    fn node_user_message(
        goal: &str,
        steps: &[PlanStep],
        inputs: &[Vec<usize>],
        index: usize,
        outputs: &[String],
    ) -> String {
        match inputs[index].as_slice() {
            [] => goal.to_string(),
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

    /// 执行单个节点(组装消息 → 纯生成或工具循环),返回 (正文, usage DTO)。
    /// 调用追踪/usage 口径不变(phase=step,step_index=步骤在流程数组中的下标);
    /// 失败与中断都返回 Err,由调用方决定状态与降级。
    /// 档位分支见 `PlanStep::is_strict`(二维批次 6a):严格档恒走单次调用。
    async fn execute_node(
        &self,
        ctx: &TaskRunContext,
        index: usize,
        step: &PlanStep,
        user: String,
    ) -> Result<(String, TaskGenOutput), String> {
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
                        "step",
                        Some(index),
                        messages,
                        Vec::new(),
                        step.max_tokens.unwrap_or(settings.default_max_tokens),
                        step.temperature.unwrap_or(settings.default_temperature),
                        settings.default_top_p,
                        ctx.cancel.clone(),
                    )
                    .await
                    .map(|out| {
                        let text = out.text.trim().to_string();
                        (text, out)
                    })
            }
            // 宽松档 + 工具步骤:run_tool_loop(白名单自动放行),调用追踪在步骤函数内落。
            // usage → TaskGenOutput 统一走 usage_as_output(批次 B.4 单一出处);
            // 该 out 仅作 record_usage 入参(text 不消费)
            (Some(list), false) => self
                .run_step_with_tools(ctx, index, step, &mut messages, list)
                .await
                .map(|(text, usage)| {
                    let out = super::executor::usage_as_output(&usage);
                    (text, out)
                }),
        }
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
        // 并行上限(二维批次 2):流程级配置,缺省 2、1 = 完全串行(批次 1 行为)
        let max_parallel = effective_max_parallel(&cfg);
        let svc = &self.svc;
        svc.set_status(&ctx.task_id, TaskStatus::Running);

        // 进度模型 = plan 步骤(前端 custom 渲染:plan 步骤 + 状态徽标)。
        // 展示顺序恒为流程数组顺序(用户编排顺序),执行顺序另由 graph.order 决定。
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

        // 各节点产出(下标对齐 steps):失败或空产出留空串,下游据此拿到空段
        let mut outputs: Vec<String> = vec![String::new(); steps.len()];
        let mut any_error = false;
        let mut total = TokenUsage::default();

        // ===== 并行调度(二维批次 2)=====
        // 就绪判定:上游计数 + 后继表;就绪节点按下标升序取(线性流程顺序与旧版一致)。
        //
        // plan 写入纪律(IFW-4):**调度器是 plan 的唯一写者**,节点执行体只回传文本、
        // 不持有 plan 快照。多节点并发完成时因此不存在「各自读旧快照再整列覆写」的丢更新
        // (那是把 plan 交给各节点自行读-改-写才会有的缺陷)。这条纪律由
        // tests/task_custom_graph.rs 的并发用例锁定:全部节点终态必须都落回 plan。
        let node_count = steps.len();
        let mut missing_parents: Vec<usize> = graph.inputs.iter().map(Vec::len).collect();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); node_count];
        for (i, parents) in graph.inputs.iter().enumerate() {
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

        while finished < node_count {
            // 补满在飞槽位(上限 = 流程级 max_parallel_nodes,默认 2:并行成倍消耗 token)
            while inflight.len() < max_parallel {
                let Some(&i) = ready.iter().next() else { break };
                ready.remove(&i);
                if *ctx.cancel.borrow() {
                    // 取消:丢弃在飞 future 即中止分支(与串行口径一致:在飞行保持 running),
                    // 随后统一由「任务已停止」收口
                    drop(inflight);
                    return Err("任务已停止".into());
                }
                plan[i].status = TaskStepStatus::Running;
                svc.set_plan(&ctx.task_id, &plan);
                let user = Self::node_user_message(&ctx.goal, &steps, &graph.inputs, i, &outputs);
                let (ctx_ref, step) = (&ctx, &steps[i]);
                inflight.push(Box::pin(async move {
                    (i, self.execute_node(ctx_ref, i, step, user).await)
                }));
            }
            let Some((i, result)) = inflight.next().await else {
                break;
            };
            finished += 1;
            let step = &steps[i];
            match result {
                Ok((text, out)) => {
                    total.prompt_tokens += out.prompt_tokens;
                    total.completion_tokens += out.completion_tokens;
                    total.total_tokens += out.prompt_tokens + out.completion_tokens;
                    // usage 落库:逐步骤一行(phase=step,对齐 legacy 按调用计口径)
                    svc.record_usage(&ctx.task_id, "step", Some(i), &out);
                    if text.is_empty() {
                        any_error = true;
                        plan[i].status = TaskStepStatus::Error;
                        plan[i].result = format!("步骤「{}」返回空内容", step.name);
                    } else {
                        plan[i].status = TaskStepStatus::Done;
                        // generates=false 的内部规划步骤:产出仅供后续步骤参考,
                        // 加标注区分于面向用户的成果(不参与成果选拔)。
                        if step.action == "direct" && step.generates == Some(false) {
                            plan[i].result = format!("(内部规划)\n{text}");
                        } else {
                            plan[i].result = text.clone();
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
                    plan[i].status = TaskStepStatus::Error;
                    plan[i].result = e;
                }
            }
            svc.set_plan(&ctx.task_id, &plan);
            // 释放后继:上游全部完成(无论成败)即就绪——下游照常执行并拿到空段,
            // 与串行版「上游失败即清空产出、后续步骤继续」的语义一致
            for &child in &children[i] {
                missing_parents[child] -= 1;
                if missing_parents[child] == 0 {
                    ready.insert(child);
                }
            }
        }

        // 成果选拔:成果节点产出优先;为空则按**数组下标降序**回退到上一个非空
        // 生成产出——存量线性流程语义就是「末个有效生成步」,且该兜底与执行顺序无关
        // (并行下成果节点失败时不会退化成「谁先跑完算谁」,结果仍可复现)。
        let draft = output_idx
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
            .unwrap_or_default();
        if draft.is_empty() {
            return Err("自定义流程未产出任何成果(生成步骤全部失败或为空)".into());
        }
        // 含 error 步骤但成果已产出 → partial(对齐 legacy WP3 语义)
        let status = if any_error {
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
            total,
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
