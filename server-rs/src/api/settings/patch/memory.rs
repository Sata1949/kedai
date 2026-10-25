//! 跨会话记忆与剧情推演词条同步(roleplay 写扁平;task 写覆盖层)。
use crate::services::settings_service::RuntimeSettings;

use super::super::UpdateSettingsBody;
use super::apply_mode;
use super::PatchError;

pub(super) fn apply(
    s: &mut RuntimeSettings,
    body: &UpdateSettingsBody,
    is_task: bool,
) -> Result<(), PatchError> {
    // 记忆蒸馏开关(落地项 2)
    if let Some(v) = body.memory_distill_enabled {
        apply_mode!(s, is_task, memory_distill_enabled, v);
    }
    // 剧情推演词条同步两档(RPFLOW-2):角色档默认开、全局档默认关
    if let Some(v) = body.worldbook_sync_character_enabled {
        apply_mode!(s, is_task, worldbook_sync_character_enabled, v);
    }
    if let Some(v) = body.worldbook_sync_global_enabled {
        apply_mode!(s, is_task, worldbook_sync_global_enabled, v);
    }
    // 记忆注入上限(0..=50;0 = 关闭注入)
    if let Some(v) = body.memory_inject_limit {
        if v > 50 {
            return Err(PatchError::new(
                "memory_inject_limit",
                "memory_inject_limit 必须在 0..=50",
            ));
        }
        apply_mode!(s, is_task, memory_inject_limit, v);
    }
    // 记忆槽字符预算(0..=20000;0 = 不限制)
    if let Some(v) = body.memory_inject_char_budget {
        if v > 20_000 {
            return Err(PatchError::new(
                "memory_inject_char_budget",
                "memory_inject_char_budget 必须在 0..=20000",
            ));
        }
        apply_mode!(s, is_task, memory_inject_char_budget, v);
    }
    // 每角色记忆容量上限(0..=10000;0 = 不淘汰)
    if let Some(v) = body.memory_max_entries {
        if v > 10_000 {
            return Err(PatchError::new(
                "memory_max_entries",
                "memory_max_entries 必须在 0..=10000",
            ));
        }
        apply_mode!(s, is_task, memory_max_entries, v);
    }
    Ok(())
}
