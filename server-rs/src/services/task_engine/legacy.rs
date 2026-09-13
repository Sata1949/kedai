// legacy 任务执行器:原 `task_service/executor.rs::run_task_background` 的机械搬迁,
// 上 `ModeExecutor` 缝以统一派发点(批次 M3)。**行为逐字节不变**是唯一目标:
// - 三段式:规划(plan_task_retry)→ 逐步执行(步骤无工具,generate_step_retry)→ 汇总(summarize_task_retry);
// - 子任务落 `task_subtasks` 表(非六模式的 agent_subtasks 内存覆盖层);
// - phase 用 plan / step / summary(与 solo 的 agent 单段不同);
// - 收尾语义与六模式不同,故走 `OnSuccess::SelfFinalized`:本执行器自行调用
//   `complete_mode_run` / `finalize_mode_run`,引擎不再插手。
//
// 为何让执行器自行收尾:legacy 在步骤循环中途失败时会立即 finalize_run 收尾,
// 成功时自行 set_result + 落 assistant 消息;而六模式契约是「返回 TaskOutcome、
// 引擎统一收尾」。两者混用会重复写入终态,故由 OnSuccess::SelfFinalized 明确划界。
// 本执行器**永不返回 Err**(错误按原逻辑自行消化),否则引擎的 Err 分支会二次收尾。
use super::context::TaskRunContext;
use super::executor::{ModeExecutor, TaskOutcome};
use crate::models::types::{TaskStatus, TaskStepStatus, TaskSubtaskStatus, TokenUsage};
use crate::services::task_service::executor::{
    generate_step_retry, plan_task_retry, summarize_task_retry,
};
use crate::services::task_service::TaskService;
use futures::future::BoxFuture;
use std::sync::Arc;

pub(crate) struct LegacyExecutor {
    svc: Arc<TaskService>,
}

impl LegacyExecutor {
    pub(crate) fn new(svc: Arc<TaskService>) -> Self {
        LegacyExecutor { svc }
    }
}

/// 累计各阶段 TaskGenOutput 的 token 用量(仅用于引擎完成日志展示;
/// 权威用量走 record_usage → task_usage 与 task_llm_calls)。
fn accumulate(acc: &mut TokenUsage, out: &crate::services::task_service::TaskGenOutput) {
    acc.prompt_tokens += out.prompt_tokens;
    acc.completion_tokens += out.completion_tokens;
    acc.total_tokens += out.prompt_tokens + out.completion_tokens;
}

impl ModeExecutor for LegacyExecutor {
    fn run<'a>(&'a self, ctx: TaskRunContext) -> BoxFuture<'a, Result<TaskOutcome, String>> {
        Box::pin(async move {
            let svc = &self.svc;
            let task_id = ctx.task_id.as_str();
            let token = ctx.token;
            let cancel = ctx.cancel;
            let mut usage = TokenUsage::default();

            let Some(task) = svc.get(task_id) else {
                // 任务已不存在:不改终态,仅清理取消条目(finalize_run 的 error=None
                // 且非取消分支本就不写库,语义等价)
                svc.finalize_mode_run(task_id, token, false, None);
                return Ok(TaskOutcome::self_finalized(usage));
            };

            // 1) 规划(带解析与分级重试:截断/空输出翻倍 max_tokens,最多 PLAN_MAX_ATTEMPTS 次)
            let (plan, plan_out) = match plan_task_retry(
                svc,
                task_id,
                &task.title,
                task.character_id.as_deref(),
                &cancel,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    svc.finalize_mode_run(task_id, token, *cancel.borrow(), Some(&e));
                    return Ok(TaskOutcome::self_finalized(usage));
                }
            };
            accumulate(&mut usage, &plan_out);
            svc.record_usage(task_id, "plan", None, &plan_out);
            if *cancel.borrow() {
                svc.finalize_mode_run(task_id, token, true, None);
                return Ok(TaskOutcome::self_finalized(usage));
            }
            let _ = svc.set_plan(task_id, &plan);
            let _ = svc.set_status(task_id, TaskStatus::Running);

            // 2) 逐步执行
            let mut final_plan = plan.clone();
            for (i, step) in plan.iter().enumerate() {
                if *cancel.borrow() {
                    svc.finalize_mode_run(task_id, token, true, None);
                    return Ok(TaskOutcome::self_finalized(usage));
                }
                final_plan[i].status = TaskStepStatus::Running;
                let _ = svc.set_plan(task_id, &final_plan);

                let subtask_id = match svc.create_subtask(task_id, &step.name, &step.goal) {
                    Ok(id) => id,
                    Err(e) => {
                        final_plan[i].status = TaskStepStatus::Error;
                        final_plan[i].result = e.clone();
                        let _ = svc.set_plan(task_id, &final_plan);
                        continue;
                    }
                };

                // 生成步骤:空输出按 finish_reason 分级重试一次(length 加倍 max_tokens,否则调温 0.7),
                // 仍空标 error 且文案带 finish_reason(诊断推理耗尽 vs 内容过滤等不同成因)。
                let gen = generate_step_retry(svc, &task, step, Some(i), &cancel).await;
                match gen {
                    Ok(out) => {
                        accumulate(&mut usage, &out);
                        let text = out.text.trim().to_string();
                        final_plan[i].status = TaskStepStatus::Done;
                        final_plan[i].result = text.clone();
                        svc.set_subtask_status(
                            &subtask_id,
                            TaskSubtaskStatus::Done,
                            Some(&text),
                            None,
                        );
                        svc.record_usage(task_id, "step", Some(i), &out);
                    }
                    Err(e) => {
                        if *cancel.borrow() {
                            svc.finalize_mode_run(task_id, token, true, None);
                            return Ok(TaskOutcome::self_finalized(usage));
                        }
                        final_plan[i].status = TaskStepStatus::Error;
                        final_plan[i].result = e.clone();
                        svc.set_subtask_status(
                            &subtask_id,
                            TaskSubtaskStatus::Error,
                            None,
                            Some(&e),
                        );
                    }
                }
                let _ = svc.set_plan(task_id, &final_plan);
            }

            // 3) 汇总:空输出同样按 finish_reason 分级重试一次。
            if *cancel.borrow() {
                svc.finalize_mode_run(task_id, token, true, None);
                return Ok(TaskOutcome::self_finalized(usage));
            }
            match summarize_task_retry(svc, &task, &final_plan, &cancel).await {
                Ok(out) => {
                    accumulate(&mut usage, &out);
                    svc.record_usage(task_id, "summary", None, &out);
                    // 正常完成:旧执行在重跑后让位,不覆盖新任务状态。
                    // 含 error 步骤时终态为「部分完成」(成果仍产出,但需向用户标示有步骤失败)。
                    // complete_mode_run 与旧 set_result + add_task_message 逐句等价
                    // (is_current_run 守门 + 写 result + 非空落 assistant/result 消息 + 清理取消条目)。
                    let has_error = final_plan.iter().any(|s| s.status == TaskStepStatus::Error);
                    let status = if has_error {
                        TaskStatus::Partial
                    } else {
                        TaskStatus::Done
                    };
                    svc.complete_mode_run(task_id, token, out.text.trim(), status, None);
                }
                Err(e) => {
                    svc.finalize_mode_run(task_id, token, *cancel.borrow(), Some(&e));
                }
            }

            Ok(TaskOutcome::self_finalized(usage))
        })
    }
}
