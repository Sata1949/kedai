// 任务工具待授权的两条瞬态事件（2026-11-06 三档授权；从 events.rs 拆出）。
//
// 单独成文件的理由：events.rs 已接近 1000 行阈值，而这两条事件的生命周期与落库型
// 事件完全不同——**只广播、不落库**（重放旧的待授权会让用户对已超时的调用点头），
// 断线恢复走 `GET /api/tasks/{id}/tool-authorizations` 的 pending 快照。
use crate::models::types::{
    SseEvent, TaskEventKind, TaskToolAuthorization, TaskToolAuthorizationDecision,
};
use crate::services::task_service::TaskService;

impl TaskService {
    /// 广播一条「工具待确认」任务事件（瞬态，不落库）。
    pub(crate) fn emit_tool_authorization_required(
        &self,
        task_id: &str,
        auth: TaskToolAuthorization,
    ) {
        let detail = format!("工具 {} 等待确认:{}", auth.name, auth.reason);
        self.broadcast_event(SseEvent::Task {
            task_id: task_id.to_string(),
            kind: Some(TaskEventKind::ToolAuthorizationRequired),
            title: None,
            status: None,
            detail: Some(detail.chars().take(200).collect()),
            finish_reason: None,
            phase: None,
            step_index: None,
            seq: None,
            at: None,
            trace: None,
            tool_authorization: Some(Box::new(auth)),
            tool_authorization_decision: None,
        });
    }

    /// 广播一条「待确认已收束」任务事件（瞬态，不落库）。
    pub(crate) fn emit_tool_authorization_resolved(
        &self,
        task_id: &str,
        decision: TaskToolAuthorizationDecision,
    ) {
        let detail = format!("工具 {} 授权结果:{}", decision.name, decision.decision);
        self.broadcast_event(SseEvent::Task {
            task_id: task_id.to_string(),
            kind: Some(TaskEventKind::ToolAuthorizationResolved),
            title: None,
            status: None,
            detail: Some(detail.chars().take(200).collect()),
            finish_reason: None,
            phase: None,
            step_index: None,
            seq: None,
            at: None,
            trace: None,
            tool_authorization: None,
            tool_authorization_decision: Some(Box::new(decision)),
        });
    }
}
