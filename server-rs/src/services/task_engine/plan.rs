// plan 模式:只规划不执行(零副作用纪律)——产出计划落库后进 planned 待批准态,
// 放弃 = stop;批准(POST /api/tasks/{id}/approve,可携修改后计划)的续跑由本模块
// ApprovedPlanExecutor 承担(2026-08 实测修复:旧实现 approve 强制 solo 只吃 goal,
// 已批准计划仅落库存档,步骤永远 pending、step result 全空,任务却 done)。
// 规划调用复用 legacy 的 plan_task_retry(含解析/截断打捞/分级重试与 planner 追踪落库);
// 续跑执行段复用 solo 的 run_agent_loop 与 legacy 的 summarize_task_retry(语义对齐)。
// 结果契约(提交 2 修订批次 R1):
// - planned 态:计划清单文本(「计划已产出,共 N 步:…」)落 tasks.result,语义 =
//   待批准的计划清单(批准前预览;经 TaskTerminal::AwaitApproval →
//   finalize_terminal → planned_mode_run 写入,只更新 result 列、不动状态);
// - 续跑完成:result = **汇总文本**(批次 R1 曾在此追加「## 最终计划」段——各步
//   名称/状态/result 概要;前端「计划步骤」区已按 task.plan 渲染同一份结构化数据,
//   该段是纯文本副本,提交 2 已移除,C3 裁定「同一份信息不两处存」);
// - 续跑汇总失败:能拼出已完成步骤的产出即 partial + 拼装成果(不再整体丢成果,
//   见 task_core::assemble)。
use super::context::TaskRunContext;
use super::executor::{usage_as_output, ModeExecutor};
use super::solo::{run_agent_loop, AgentLoopCall};
use crate::agents::engine::AgentEngine;
use crate::models::types::{TaskEventKind, TaskStatus, TaskStep, TaskStepStatus, TokenUsage};
use crate::services::task_core::{fallback_terminal, TaskBackend, TaskTerminal};
use futures::future::BoxFuture;
use std::sync::Arc;

/// plan 批准续跑执行器:消费已批准计划(task.plan 非空时由 run_approved 派发)。
/// 逐步骤执行:每步一次 run_agent_loop(步骤 name+goal 为该步指令,整体目标与计划
/// 作上下文;虚拟 session 复用 task:{id}),完成即回写该步 status/result;单步失败
/// 记 error 继续后续步骤(对齐 legacy),取消即中断。全部步骤完成后
/// summarize_task_retry(SUMMARIZER_PROMPT + 空输出分级重试)汇总产出最终 result;
/// 含 error 步骤时终态 partial(对齐 legacy「有产出则 partial」语义)。
pub(crate) struct ApprovedPlanExecutor {
    svc: Arc<dyn TaskBackend>,
    engine: Arc<AgentEngine>,
    /// 已批准计划(approve 处已落库,可为用户修改版)
    plan: Vec<TaskStep>,
}

impl ApprovedPlanExecutor {
    pub(crate) fn new(
        svc: Arc<dyn TaskBackend>,
        engine: Arc<AgentEngine>,
        plan: Vec<TaskStep>,
    ) -> Self {
        ApprovedPlanExecutor { svc, engine, plan }
    }

    async fn run_inner(&self, ctx: TaskRunContext) -> Result<(TaskTerminal, TokenUsage), String> {
        let svc = &self.svc;
        let mut total = TokenUsage::default();
        let mut plan = self.plan.clone();
        let count = plan.len();
        let mut had_error = false;

        // approve 已置 planning,执行段归执行器所有:推进 running
        svc.set_status(&ctx.task_id, TaskStatus::Running);

        // ===== 逐步执行:每步独立 agent 调用,完成即回写该步 =====
        for i in 0..count {
            if *ctx.cancel.borrow() {
                // 取消:剩余 pending 步骤统一置 error「任务已停止」再收尾(与 team 口径对齐)
                fail_pending_steps(svc, &ctx.task_id, &mut plan, "任务已停止");
                return Err("任务已停止".into());
            }
            // 任务总预算用尽(PRODCAP-2):不再启动新步骤——剩余未执行步骤统一标错
            //(与取消同款「不留永远 pending 的步骤」纪律),以已完成部分收尾;
            // 无任何已完成产出时 fallback_terminal 自然回落 Failed(不伪造成果)。
            if super::deadline_exhausted(ctx.deadline) {
                svc.emit_event(
                    TaskEventKind::Status,
                    &ctx.task_id,
                    None,
                    None,
                    Some(format!(
                        "任务总预算用尽:不再启动第 {} 步起的 {} 个步骤,以已完成部分收尾",
                        i + 1,
                        count - i
                    )),
                );
                fail_pending_steps(svc, &ctx.task_id, &mut plan, "任务总预算用尽(未执行)");
                return Ok((
                    fallback_terminal(&plan, "任务总预算用尽,剩余步骤未执行".into()),
                    total,
                ));
            }
            plan[i].status = TaskStepStatus::Running;
            svc.set_plan(&ctx.task_id, &plan);

            // 当前步骤指令在前(该步 name+goal),整体目标与已批准计划作上下文在后。
            // 整体目标文本随每步重复下发是有意的:各步共用同一虚拟 session 但每步都是
            // 独立调用,重复携带上下文可保模型对全局意图的记忆,不轻易精简
            let goal = format!(
                "当前步骤(第 {}/{} 步):{}\n{}\n\n整体目标与已批准计划(执行上下文):\n{}\n\n请完成当前步骤并直接产出该步骤的结果。",
                i + 1,
                count,
                plan[i].name,
                plan[i].goal,
                ctx.goal
            );
            let call = AgentLoopCall {
                task_id: ctx.task_id.clone(),
                session_id: format!("task:{}", ctx.task_id),
                goal,
                settings: ctx.settings.clone(),
                executor_id: ctx.executor_id.clone(),
                character_id: ctx.character_id.clone(),
                phase: "agent",
                step_index: Some(i),
                label: format!("步骤 {}", i + 1),
                connection_id: ctx.connection_id.clone(),
                // 工作区作用域(编码通道批次 1):批准后的逐步执行同样受工作区约束
                scope: ctx.scope.clone(),
                // 任务级总预算(PRODCAP-2):每步墙钟预算按其取小,到点不再启动新步骤
                deadline: ctx.deadline,
            };
            match run_agent_loop(svc.clone(), self.engine.clone(), call, ctx.cancel.clone()).await {
                Ok((text, usage)) => {
                    total.prompt_tokens += usage.prompt_tokens;
                    total.completion_tokens += usage.completion_tokens;
                    total.total_tokens += usage.total_tokens;
                    svc.record_usage(&ctx.task_id, "agent", Some(i), &usage_as_output(&usage));
                    plan[i].status = TaskStepStatus::Done;
                    plan[i].result = text;
                }
                Err(e) => {
                    plan[i].status = TaskStepStatus::Error;
                    plan[i].result = e.clone();
                    svc.set_plan(&ctx.task_id, &plan);
                    if *ctx.cancel.borrow() {
                        // 取消:剩余 pending 步骤统一置 error「任务已停止」再收尾(与 team 口径对齐)
                        fail_pending_steps(svc, &ctx.task_id, &mut plan, "任务已停止");
                        return Err("任务已停止".into());
                    }
                    // 单步失败记 error 继续后续步骤(对齐 legacy 执行段语义)
                    had_error = true;
                    continue;
                }
            }
            svc.set_plan(&ctx.task_id, &plan);
        }

        // ===== 汇总:SUMMARIZER_PROMPT + 空输出分级重试(对齐 legacy 执行段语义) =====
        if *ctx.cancel.borrow() {
            return Err("任务已停止".into());
        }
        let Some(task) = svc.get(&ctx.task_id) else {
            return Err("任务不存在".into());
        };
        // 汇总失败的降级(提交 2 部分成果兜底):能拼出已完成步骤的产出 → partial
        // (成果 + 原因);一步都没成 → Failed。取消时保持 Err,由引擎按 ended 收尾
        // (终态兜底会从库里的 plan 补写已完成步骤的产出)。
        let out = match super::retry::summarize_task_retry(svc.as_ref(), &task, &plan, &ctx.cancel)
            .await
        {
            Ok(o) => o,
            Err(e) if *ctx.cancel.borrow() => return Err(e),
            Err(e) => return Ok((fallback_terminal(&plan, format!("汇总失败:{e}")), total)),
        };
        svc.record_usage(&ctx.task_id, "summary", None, &out);
        total.prompt_tokens += out.prompt_tokens;
        total.completion_tokens += out.completion_tokens;
        total.total_tokens += out.prompt_tokens + out.completion_tokens;
        // 结果契约(提交 2 起):result **只放汇总文本**——planned 态写入的计划清单
        // 文本由 TaskTerminal::Complete → complete_mode_run 以此整体覆盖。
        // 含 error 步骤但成果已产出 → partial(对齐 legacy「有产出则 partial」语义)
        let status = if had_error {
            TaskStatus::Partial
        } else {
            TaskStatus::Done
        };
        Ok((
            TaskTerminal::Complete {
                result: out.text.trim().to_string(),
                status,
                error: None,
            },
            total,
        ))
    }
}

/// 剩余 pending 步骤统一置 error 并落库(与 team 口径对齐:收尾只写任务终态,
/// 步骤态归执行器负责,不留永远 pending 的步骤)。取消与任务总预算用尽共用本条
/// (PRODCAP-2 起 reason 参数化:取消 =「任务已停止」,预算 =「任务总预算用尽(未执行)」)。
/// 本执行器单线程逐步推进,取消检查点不存在 running 态步骤(当前步已在 Err 分支置
/// error),故只需扫 pending。
fn fail_pending_steps(
    svc: &Arc<dyn TaskBackend>,
    task_id: &str,
    plan: &mut [TaskStep],
    reason: &str,
) {
    let mut dirty = false;
    for s in plan.iter_mut() {
        if s.status == TaskStepStatus::Pending {
            s.status = TaskStepStatus::Error;
            s.result = reason.to_string();
            dirty = true;
        }
    }
    if dirty {
        svc.set_plan(task_id, plan);
    }
}

impl ModeExecutor for ApprovedPlanExecutor {
    fn run<'a>(
        &'a self,
        ctx: TaskRunContext,
    ) -> BoxFuture<'a, Result<(TaskTerminal, TokenUsage), String>> {
        Box::pin(async move { self.run_inner(ctx).await })
    }
}

/// plan 执行器:只需任务后端(规划/落库/事件),不经聊天引擎。
pub(crate) struct PlanExecutor {
    svc: Arc<dyn TaskBackend>,
}

impl PlanExecutor {
    pub(crate) fn new(svc: Arc<dyn TaskBackend>) -> Self {
        PlanExecutor { svc }
    }
}

impl ModeExecutor for PlanExecutor {
    fn run<'a>(
        &'a self,
        ctx: TaskRunContext,
    ) -> BoxFuture<'a, Result<(TaskTerminal, TokenUsage), String>> {
        Box::pin(async move {
            // 显式置 planning(run 入口 reset_task 已是 planning,幂等;语义上规划阶段归执行器所有)
            self.svc.set_status(&ctx.task_id, TaskStatus::Planning);
            let (steps, out) = super::retry::plan_task_retry(
                self.svc.as_ref(),
                &ctx.task_id,
                &ctx.goal,
                ctx.character_id.as_deref(),
                &ctx.cancel,
                ctx.scope.clone(),
                // 能力事实 = ToolLoop:本模式批准后的步骤走 run_agent_loop(有工具);
                // 规划器据此把「交付物正文 + 需要时用文件/命令」写进步骤(D1)。
                crate::services::task_core::prompt_consts::StepCapability::ToolLoop,
            )
            .await?;
            // 规划产出后落库前再查一次取消:已停止则不进 planned(终态由收尾写 ended)
            if *ctx.cancel.borrow() {
                return Err("任务已停止".into());
            }
            // usage 落库(批次 4.3b 口径补齐:规划轮 phase=planner;批准后的执行段
            // usage 由 approve 续跑的 ApprovedPlanExecutor 逐步补记,phase=agent/summary)
            self.svc.record_usage(&ctx.task_id, "planner", None, &out);
            // 计划落库 → planned 待批准 → 广播批准请求;全程不执行任何步骤(零副作用)
            self.svc.set_plan(&ctx.task_id, &steps);
            self.svc.set_status(&ctx.task_id, TaskStatus::Planned);
            self.svc.emit_event(
                TaskEventKind::ApprovalRequired,
                &ctx.task_id,
                None,
                Some(TaskStatus::Planned),
                Some("计划已产出,待批准".into()),
            );
            // 批次 R1:本清单文本经 TaskTerminal::AwaitApproval 落 tasks.result
            //(planned 态 result 语义 = 待批准的计划清单,批准前预览用)
            let mut summary = format!("计划已产出,共 {} 步:", steps.len());
            for (i, s) in steps.iter().enumerate() {
                summary.push_str(&format!("\n{}. {}:{}", i + 1, s.name, s.goal));
            }
            let usage = TokenUsage {
                prompt_tokens: out.prompt_tokens,
                completion_tokens: out.completion_tokens,
                total_tokens: out.prompt_tokens + out.completion_tokens,
                ..Default::default()
            };
            Ok((TaskTerminal::AwaitApproval { plan_text: summary }, usage))
        })
    }
}
