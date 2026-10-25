//! 字段级三态测试:合法值生效 / 非法值按 Reject 策略返回字段名 / 缺省不覆盖。
//!
//! 这里直接调用纯函数 `apply`,不经过 HTTP 与磁盘:验证的是补丁语义本身;
//! 「整批不落库」的事务边界仍由 `settings_connector` 集成测试覆盖。
use serde_json::{json, Value};

use super::{apply, PatchError};
use crate::config::test_config;
use crate::services::settings_service::{AppMode, RuntimeSettings};

fn settings() -> RuntimeSettings {
    RuntimeSettings::from_config(&test_config())
}

fn body(value: Value) -> super::super::UpdateSettingsBody {
    serde_json::from_value(value).expect("测试 patch 应能反序列化")
}

fn snapshot(s: &RuntimeSettings) -> Value {
    serde_json::to_value(s).expect("设置应可序列化")
}

fn apply_err(patch: Value, mode: AppMode) -> PatchError {
    let mut s = settings();
    apply(&mut s, &body(patch), mode).expect_err("该字段应被判为非法")
}

#[test]
fn roleplay_patch_writes_flat_fields() {
    let mut s = settings();
    let out = apply(
        &mut s,
        &body(json!({
            "default_temperature": 0.5,
            "max_tool_rounds": 7,
            "memory_inject_limit": 0,
            "undo_enabled": false,
        })),
        AppMode::Roleplay,
    )
    .expect("合法 patch 应通过");

    assert!(out.cleared_key_ids.is_empty());
    assert_eq!(s.default_temperature, 0.5);
    assert_eq!(s.max_tool_rounds, 7);
    assert_eq!(s.memory_inject_limit, 0);
    assert!(!s.undo_enabled);
    // roleplay 写入不得在 task 覆盖层留下影子值
    assert!(s.task.default_temperature.is_none());
    assert!(s.task.max_tool_rounds.is_none());
    assert!(s.task.memory_inject_limit.is_none());
}

#[test]
fn task_patch_writes_overlay_and_keeps_flat_values() {
    let mut s = settings();
    let flat_temperature = s.default_temperature;
    let flat_prompt = s.agent_system_prompt.0.clone();
    let flat_policy = s.task_tool_policy.clone();

    apply(
        &mut s,
        &body(json!({
            "default_temperature": 0.4,
            "agent_system_prompt": "任务专用提示词",
            "task_tool_policy": "allowlist",
            "exec_enabled": true,
        })),
        AppMode::Task,
    )
    .expect("合法 patch 应通过");

    assert_eq!(s.task.default_temperature, Some(0.4));
    assert_eq!(s.default_temperature, flat_temperature);
    assert_eq!(
        s.task
            .agent_system_prompt
            .as_ref()
            .map(|value| value.0.as_str()),
        Some("任务专用提示词")
    );
    assert_eq!(s.agent_system_prompt.0, flat_prompt);
    assert_eq!(s.task.task_tool_policy.as_deref(), Some("allowlist"));
    assert_eq!(s.task_tool_policy, flat_policy);
    // 全局能力开关即使从 task 模式写也直写扁平(不是覆盖层)
    assert!(s.exec_enabled);
}

#[test]
fn invalid_values_are_rejected_with_field_name() {
    let cases = [
        (json!({ "default_temperature": 2.5 }), "default_temperature"),
        (json!({ "default_top_p": -0.1 }), "default_top_p"),
        (json!({ "max_context_tokens": 1024 }), "max_context_tokens"),
        (json!({ "mvu_vars_position": "top" }), "mvu_vars_position"),
        (json!({ "preset_tail_role": "system" }), "preset_tail_role"),
        (
            json!({ "reflect_advice_role": "system" }),
            "reflect_advice_role",
        ),
        (json!({ "max_tool_rounds": 0 }), "max_tool_rounds"),
        (
            json!({ "authorization_mode": "open" }),
            "authorization_mode",
        ),
        (
            json!({ "session_budget_action": "ignore" }),
            "session_budget_action",
        ),
        (json!({ "embedding_dim": 15 }), "embedding_dim"),
        // C-2B 收口的 6 个「静默忽略」字段:现在必须是 400 + 点名字段
        (json!({ "compaction_mode": "always" }), "compaction_mode"),
        (
            json!({ "compaction_keep_recent": 1 }),
            "compaction_keep_recent",
        ),
        (
            json!({ "compaction_snip_bytes": 1_048_577 }),
            "compaction_snip_bytes",
        ),
        (json!({ "memory_inject_limit": 51 }), "memory_inject_limit"),
        (
            json!({ "memory_inject_char_budget": 20_001 }),
            "memory_inject_char_budget",
        ),
        (
            json!({ "memory_max_entries": 10_001 }),
            "memory_max_entries",
        ),
        (
            json!({ "task_idle_timeout_secs": 1 }),
            "task_idle_timeout_secs",
        ),
    ];

    for (patch, field) in cases {
        let err = apply_err(patch, AppMode::Roleplay);
        assert_eq!(err.field, field, "错误必须点名具体字段");
        assert!(
            err.message.contains(field),
            "用户文案应包含字段名:{field} -> {}",
            err.message
        );
    }
}

#[test]
fn empty_patch_leaves_settings_untouched() {
    for mode in [AppMode::Roleplay, AppMode::Task] {
        let mut s = settings();
        let before = snapshot(&s);
        let out = apply(&mut s, &body(json!({})), mode).expect("空 patch 应通过");
        assert!(out.cleared_key_ids.is_empty());
        assert_eq!(snapshot(&s), before, "缺省字段不得覆盖既有值");
    }
}

#[test]
fn connection_patch_collects_explicit_clear_ids() {
    let mut s = settings();
    let first = s.connections[0].clone();
    let mut second = first.clone();
    second.id = "second".to_string();
    second.name = "第二套".to_string();
    second.api_key = "old-second".to_string();
    s.connections.push(second);

    let out = apply(
        &mut s,
        &body(json!({
            "connections": [
                { "id": first.id, "clear_api_key": true },
                { "id": "second", "api_key": "new-second" }
            ]
        })),
        AppMode::Roleplay,
    )
    .expect("合法连接数组应通过");

    assert_eq!(out.cleared_key_ids, vec![first.id.clone()]);
    let saved_first = s
        .connections
        .iter()
        .find(|p| p.id == first.id)
        .expect("第一套连接仍在");
    assert!(saved_first.api_key.is_empty(), "显式清空应生效");
    let saved_second = s
        .connections
        .iter()
        .find(|p| p.id == "second")
        .expect("第二套连接仍在");
    assert_eq!(saved_second.api_key, "new-second");
}

#[test]
fn task_default_connection_must_exist_and_be_enabled() {
    let mut s = settings();
    let missing = apply(
        &mut s,
        &body(json!({ "task_default_connection_id": "does-not-exist" })),
        AppMode::Task,
    )
    .expect_err("不存在的连接应被拒绝");
    assert_eq!(missing.field, "task_default_connection_id");

    let id = s.connections[0].id.clone();
    s.connections[0].enabled = false;
    let disabled = apply(
        &mut s,
        &body(json!({ "task_default_connection_id": id })),
        AppMode::Task,
    )
    .expect_err("已停用的连接应被拒绝");
    assert_eq!(disabled.field, "task_default_connection_id");
}
