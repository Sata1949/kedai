// 任务详情 / token 累计的响应线格式（C-2F-4）。
//
// 此前由 `tasks.rs` 的 `json!` 手工拼装：字段名或可选性漂移只能靠集成用例偶然发现。
// 收进结构体后，线格式由 `tools/check-contract.mjs` 与前端 `TaskDetail` /
// `TaskUsageTotal` 双向对拍（未登记不校验）。字段名与 `json!` 时期逐字一致。
use crate::models::types::{TaskMessageRecord, TaskRecord, TaskSubtaskRecord};
use crate::services::agent_flow_service::FlowSnapshot;
use crate::services::task_engine::delivery::TaskDeliverySummary;
use crate::services::workspace_profile::WorkspaceProfile;
use serde::Serialize;

/// 任务 / 全局 token 累计（三个字段均为 SQL SUM 结果，未发生时 0）。
#[derive(Serialize)]
pub struct TaskUsageTotal {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub reasoning_tokens: i64,
}

impl TaskUsageTotal {
    pub fn from_counts(prompt_tokens: i64, completion_tokens: i64, reasoning_tokens: i64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            reasoning_tokens,
        }
    }
}

/// `GET /api/tasks/{id}` 的响应体：任务、子任务、指令历史、流程快照、工作区画像、交付摘要。
/// 后四项为 `None` 时如实序列化为 `null`（前端按 optional/null 容错，语义不变）。
#[derive(Serialize)]
pub struct TaskDetailResponse {
    pub task: TaskRecord,
    pub subtasks: Vec<TaskSubtaskRecord>,
    pub usage_total: TaskUsageTotal,
    pub messages: Vec<TaskMessageRecord>,
    pub flow_snapshot: Option<FlowSnapshot>,
    pub workspace_profile: Option<WorkspaceProfile>,
    pub delivery: Option<TaskDeliverySummary>,
}

impl TaskDetailResponse {
    /// 详情 handler 的装载点；把字段装配收在这里，tasks.rs 只留一次调用。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task: TaskRecord,
        subtasks: Vec<TaskSubtaskRecord>,
        usage: (i64, i64, i64),
        messages: Vec<TaskMessageRecord>,
        flow_snapshot: Option<FlowSnapshot>,
        workspace_profile: Option<WorkspaceProfile>,
        delivery: Option<TaskDeliverySummary>,
    ) -> Self {
        let (prompt_tokens, completion_tokens, reasoning_tokens) = usage;
        Self {
            task,
            subtasks,
            usage_total: TaskUsageTotal::from_counts(
                prompt_tokens,
                completion_tokens,
                reasoning_tokens,
            ),
            messages,
            flow_snapshot,
            workspace_profile,
            delivery,
        }
    }
}

/// `GET /api/tasks/usage/total` 的响应体：全局累计单独一层信封。
#[derive(Serialize)]
pub struct TaskUsageTotalResponse {
    pub usage_total: TaskUsageTotal,
}

impl TaskUsageTotalResponse {
    pub fn from_counts(prompt_tokens: i64, completion_tokens: i64, reasoning_tokens: i64) -> Self {
        Self {
            usage_total: TaskUsageTotal::from_counts(
                prompt_tokens,
                completion_tokens,
                reasoning_tokens,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局累计的线格式冻结：外层 `usage_total` + 三个 snake_case 字段。
    #[test]
    fn usage_total_response_wire_shape_is_frozen() {
        let value = serde_json::to_value(TaskUsageTotalResponse {
            usage_total: TaskUsageTotal {
                prompt_tokens: 11,
                completion_tokens: 22,
                reasoning_tokens: 33,
            },
        })
        .expect("TaskUsageTotalResponse 必须可序列化");
        assert_eq!(
            value,
            serde_json::json!({
                "usage_total": {
                    "prompt_tokens": 11,
                    "completion_tokens": 22,
                    "reasoning_tokens": 33,
                }
            })
        );
    }
}
