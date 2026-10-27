//! GET/PUT `/api/settings` 的响应投影(API Key 只下发掩码与布尔)。
use serde_json::{json, Value};

use crate::services::settings_service::RuntimeSettings;

/// 序列化运行期设置(API Key 脱敏)。
/// 字段数已达 serde_json::json! 宏的递归展开上限,故拆成两段构建再合并
/// (比给整个 crate 提高 recursion_limit 影响面更小)。
pub(super) fn settings_json(s: &RuntimeSettings) -> Value {
    let mut v = json!({
        "openai_base_url": s.openai_base_url,
        "api_key_masked": s.masked_api_key(),
        "has_api_key": !s.openai_api_key.is_empty(),
        "model": s.model,
        "default_temperature": s.default_temperature,
        "default_top_p": s.default_top_p,
        "default_max_tokens": s.default_max_tokens,
        "max_context_tokens": s.max_context_tokens,
        "agent_system_prompt": s.agent_system_prompt,
        "search_endpoint": s.search_endpoint,
        "mvu_vars_position": s.mvu_vars_position,
        "reflect_prompt": s.reflect_prompt,
        "preset_tail_prompt": s.preset_tail_prompt,
        "preset_tail_role": s.preset_tail_role,
        "reflect_advice_prompt": s.reflect_advice_prompt,
        "reflect_advice_role": s.reflect_advice_role,
        "bypass_mode": s.bypass_mode,
        "authorization_mode": s.authorization_mode.as_str(),
        "bypass_blacklist": s.bypass_blacklist,
        "tool_authorization_timeout_secs": s.tool_authorization_timeout_secs,
        "task_tool_policy": s.task_tool_policy,
        "task_tool_allowlist": s.task_tool_allowlist,
        "max_tool_rounds": s.max_tool_rounds,
        "max_flow_call_depth": s.max_flow_call_depth,
        "max_flow_calls_per_task": s.max_flow_calls_per_task,
        "default_node_max_context": s.default_node_max_context,
        "render_html": s.render_html,
        "compaction_mode": s.compaction_mode,
        "compaction_threshold": s.compaction_threshold,
        "compaction_keep_recent": s.compaction_keep_recent,
        "compaction_snip_bytes": s.compaction_snip_bytes,
        "llm_request_log": s.llm_request_log,
        "memory_distill_enabled": s.memory_distill_enabled,
        "memory_inject_limit": s.memory_inject_limit,
        "memory_inject_char_budget": s.memory_inject_char_budget,
        "memory_max_entries": s.memory_max_entries,
        "worldbook_sync_character_enabled": s.worldbook_sync_character_enabled,
        "worldbook_sync_global_enabled": s.worldbook_sync_global_enabled,
    });
    let rest = json!({
        "embedding_enabled": s.embedding_enabled,
        "embedding_base_url": s.embedding_base_url,
        "embedding_api_key_masked": s.masked_embedding_api_key(),
        "has_embedding_api_key": !s.embedding_api_key.is_empty(),
        "embedding_model": s.embedding_model,
        "embedding_dim": s.embedding_dim,
        "skill_progressive_disclosure": s.skill_progressive_disclosure,
        "undo_enabled": s.undo_enabled,
        "subagent_max_depth": s.subagent_max_depth,
        "subagent_max_concurrency": s.subagent_max_concurrency,
        "subagent_result_max_chars": s.subagent_result_max_chars,
        "mcp_enabled": s.mcp_enabled,
        "mcp_servers": s.mcp_servers,
        "exec_enabled": s.exec_enabled,
        "exec_allow_root": s.exec_allow_root,
        "exec_allow_shizuku": s.exec_allow_shizuku,
        "exec_allow_sandbox": s.exec_allow_sandbox,
        // 视觉与截图总开关(视觉能力包 D5;全局扁平)
        "vision_screenshot_enabled": s.vision_screenshot_enabled,
        "task_coding_bundle_enabled": s.task_coding_bundle_enabled,
        // 文学能力包两开关(LIT-1;角色扮演侧为纯扁平、任务侧为扁平+覆盖层)
        "literary_bundle_enabled": s.literary_bundle_enabled,
        "task_literary_bundle_enabled": s.task_literary_bundle_enabled,
        // 文学包选择型字段(LIT-6/LIT-7;快照是内部状态,不投影给前端)
        "literary_style_preset": s.literary_style_preset,
        "literary_recommend_preset": s.literary_recommend_preset,
        "task_default_connection_id": s.task_default_connection_id,
        "tool_history_keep_rounds": s.tool_history_keep_rounds,
        "tool_history_budget_tokens": s.tool_history_budget_tokens,
        "session_token_budget": s.session_token_budget,
        "session_budget_action": s.session_budget_action,
        "loop_guard_semantic_window": s.loop_guard_semantic_window,
        "loop_guard_semantic_min_calls": s.loop_guard_semantic_min_calls,
        "loop_guard_semantic_max_distinct": s.loop_guard_semantic_max_distinct,
        // 任务侧两道闸(提交 3 · D3/D7):扁平字段,任务侧消费(步骤墙钟预算 / 空闲看守)
        "task_step_budget_secs": s.task_step_budget_secs,
        // 任务级总预算(PRODCAP-2):扁平字段,任务侧消费(0 = 关)
        "task_total_budget_secs": s.task_total_budget_secs,
        // 团队任务总预算(TMR3-4;TPROF-1 起默认 0 = 不限),仅 team 执行器消费
        "task_team_budget_secs": s.task_team_budget_secs,
        "task_idle_timeout_secs": s.task_idle_timeout_secs,
        // HB-7 接线:变量两步生成的独立模型/温度档(此前可落盘但无 API 通路)
        "mvu_model": s.mvu_model,
        "mvu_temperature": s.mvu_temperature,
    });
    if let (Some(dst), Some(src)) = (v.as_object_mut(), rest.as_object()) {
        for (k, val) in src {
            dst.insert(k.clone(), val.clone());
        }
    }
    // 多套连接:独立第三段构建,避免继续加深 json! 宏的递归展开;
    // 只下发掩码与布尔,明文 Key 永不出现在响应里。
    let connections = Value::Array(
        s.connections
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "connector_type": p.connector_type,
                    "base_url": p.base_url,
                    "model": p.model,
                    "api_style": p.api_style,
                    "enabled": p.enabled,
                    // 模型能力位(2026-10-02 视觉能力包 D1;五项均为连接级声明)
                    "supports_vision": p.supports_vision,
                    "supports_structured_output": p.supports_structured_output,
                    "supports_prefix_completion": p.supports_prefix_completion,
                    "supports_mid_conversation_system": p.supports_mid_conversation_system,
                    "image_auto_split": p.image_auto_split,
                    "api_key_masked": p.masked_api_key(),
                    "has_api_key": p.has_api_key(),
                })
            })
            .collect(),
    );
    if let Some(dst) = v.as_object_mut() {
        dst.insert("connections".to_string(), connections);
        dst.insert(
            "active_connection_id".to_string(),
            json!(s.active_connection_id),
        );
    }
    v
}
