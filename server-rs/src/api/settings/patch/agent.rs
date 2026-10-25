//! Agent 提示词、搜索/反思注入位置与流程调用闸。
use crate::services::agent_flow_service::{
    MAX_FLOW_CALLS_PER_TASK_LIMIT, MAX_FLOW_CALL_DEPTH_LIMIT, MAX_STEP_MAX_CONTEXT,
    MIN_FLOW_CALLS_PER_TASK, MIN_FLOW_CALL_DEPTH, MIN_STEP_MAX_CONTEXT,
};
use crate::services::settings_service::{
    RoleplayPromptConfig, RuntimeSettings, TaskPromptConfig, DEFAULT_SEARCH_ENDPOINT,
};

use super::super::UpdateSettingsBody;
use super::apply_mode;
use super::PatchError;

pub(super) fn apply(
    s: &mut RuntimeSettings,
    body: &UpdateSettingsBody,
    is_task: bool,
) -> Result<(), PatchError> {
    // Agent 系统提示词:允许清空(空 = 用内置默认)。
    // 类型级模式隔离(WP7):roleplay 写 RoleplayPromptConfig 扁平字段,
    // task 写 Option<TaskPromptConfig> 覆盖层;两分支类型不同源,不走通用 apply! 宏。
    if let Some(v) = &body.agent_system_prompt {
        if is_task {
            s.task.agent_system_prompt = Some(TaskPromptConfig(v.clone()));
        } else {
            s.agent_system_prompt = RoleplayPromptConfig(v.clone());
        }
    }
    // 搜索端点:允许清空(空 = 用默认 DDG 端点)。
    // TM-SET-3:直写扁平(全局字段)——search 工具运行期读全局快照,模式覆盖
    // 永不生效;此前 task 模式写入覆盖层是「改了不生效」的静默死写。
    if let Some(v) = &body.search_endpoint {
        let t = v.trim().to_string();
        let endpoint = if t.is_empty() {
            DEFAULT_SEARCH_ENDPOINT.to_string()
        } else {
            t
        };
        s.search_endpoint = endpoint;
    }
    // mvu 变量注入位置:仅接受 system / user_tail
    if let Some(v) = &body.mvu_vars_position {
        let t = v.trim().to_string();
        if t != "system" && t != "user_tail" {
            return Err(PatchError::new(
                "mvu_vars_position",
                "mvu_vars_position 仅支持 system / user_tail",
            ));
        }
        apply_mode!(s, is_task, mvu_vars_position, t);
    }
    // 反思提示词:允许清空(空 = 回退机械规则检查)
    if let Some(v) = &body.reflect_prompt {
        apply_mode!(s, is_task, reflect_prompt, v.clone());
    }
    // 预设尾部提示词:允许清空(空 = 禁用位置0 预设尾部)
    if let Some(v) = &body.preset_tail_prompt {
        apply_mode!(s, is_task, preset_tail_prompt, v.clone());
    }
    // 预设尾部注入角色:仅接受 user / assistant
    if let Some(v) = &body.preset_tail_role {
        let t = v.trim().to_string();
        if t != "user" && t != "assistant" {
            return Err(PatchError::new(
                "preset_tail_role",
                "preset_tail_role 仅支持 user / assistant",
            ));
        }
        apply_mode!(s, is_task, preset_tail_role, t);
    }
    // 反思失败建议提示词:允许清空(空 = 禁用)
    if let Some(v) = &body.reflect_advice_prompt {
        apply_mode!(s, is_task, reflect_advice_prompt, v.clone());
    }
    // 反思建议注入角色:仅接受 user / assistant
    if let Some(v) = &body.reflect_advice_role {
        let t = v.trim().to_string();
        if t != "user" && t != "assistant" {
            return Err(PatchError::new(
                "reflect_advice_role",
                "reflect_advice_role 仅支持 user / assistant",
            ));
        }
        apply_mode!(s, is_task, reflect_advice_role, t);
    }
    // 工具循环轮次上限:仅接受 1..=200(防止误填 0 或超大值打爆模型请求)
    if let Some(v) = body.max_tool_rounds {
        if !(1..=200).contains(&v) {
            return Err(PatchError::new(
                "max_tool_rounds",
                "max_tool_rounds 必须在 1..=200",
            ));
        }
        apply_mode!(s, is_task, max_tool_rounds, v);
    }
    // 流程调用闸(A 批 A3):动态调用深度与每任务调用次数。
    // 上限刻意保守——两条都是**成本**闸,调大等于允许更长的模型自主链。
    if let Some(v) = body.max_flow_call_depth {
        if !(MIN_FLOW_CALL_DEPTH..=MAX_FLOW_CALL_DEPTH_LIMIT).contains(&v) {
            return Err(PatchError::new(
                "max_flow_call_depth",
                "max_flow_call_depth 必须在 1..=5",
            ));
        }
        apply_mode!(s, is_task, max_flow_call_depth, v);
    }
    if let Some(v) = body.max_flow_calls_per_task {
        if !(MIN_FLOW_CALLS_PER_TASK..=MAX_FLOW_CALLS_PER_TASK_LIMIT).contains(&v) {
            return Err(PatchError::new(
                "max_flow_calls_per_task",
                "max_flow_calls_per_task 必须在 1..=64",
            ));
        }
        apply_mode!(s, is_task, max_flow_calls_per_task, v);
    }
    // 节点默认上下文上限(A 批 A4):0 = 不裁剪;非 0 时与节点级字段同口径
    // (下限 256 的理由见 `agent_flow_service::MIN_STEP_MAX_CONTEXT`)。
    if let Some(v) = body.default_node_max_context {
        let ok = v == 0 || (MIN_STEP_MAX_CONTEXT..=MAX_STEP_MAX_CONTEXT).contains(&v);
        if !ok {
            return Err(PatchError::new(
                "default_node_max_context",
                "default_node_max_context 必须为 0(不裁剪)或 256..=1048576",
            ));
        }
        apply_mode!(s, is_task, default_node_max_context, v);
    }
    Ok(())
}
