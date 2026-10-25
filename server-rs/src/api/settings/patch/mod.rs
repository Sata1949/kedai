//! `PUT /api/settings` 的字段补丁:按域拆分,handler 只负责加锁/克隆/落盘/替换内存。
//!
//! 域顺序是**语义依赖顺序**(不是字段在旧函数里的出现顺序):
//! 连接(含投影归一化)→ 生成参数 → Agent → 工具/权限 → 记忆 → 任务 → 能力包。
//! `bundles` 必须最后:文学推荐档会写生成/压缩三项,开关翻转后的默认词重物化也要
//! 在本次请求的所有字段应用完之后判定。
use crate::services::settings_service::{AppMode, RuntimeSettings};

use super::UpdateSettingsBody;

mod agent;
mod bundles;
mod connection;
mod generation;
mod memory;
mod task;
mod tools;

#[cfg(test)]
mod tests;

/// 字段级校验失败:带字段名(给测试/日志定位)与用户可读文案(直接回 400)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PatchError {
    pub(crate) field: &'static str,
    pub(crate) message: String,
}

impl PatchError {
    pub(crate) fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// 补丁的副作用回执:连接数组路径显式清空密钥时收集的 profile id,
/// 交给 `save_with_cleared_connection_keys` 绕开磁盘密文保真兜底。
#[derive(Debug, Default)]
pub(crate) struct PatchOutcome {
    pub(crate) cleared_key_ids: Vec<String>,
}

/// 按模式写入:task 模式写 `task` 覆盖层,roleplay 写扁平字段。
///
/// 旧实现是 `update_settings` 函数体内的局部宏,单测无法直接覆盖;提到模块级后
/// 各域共用同一份写入语义。**只有** `for_mode` 会读取的字段才允许走本宏——
/// 全局字段(exec/mcp/子代理/工具历史等)必须直写扁平,否则是静默死写。
macro_rules! apply_mode {
    ($s:expr, $is_task:expr, $field:ident, $value:expr) => {{
        let v = $value;
        if $is_task {
            $s.task.$field = Some(v);
        } else {
            $s.$field = v;
        }
    }};
}

pub(crate) use apply_mode;

/// 按固定域顺序应用整批 patch。任一字段非法即返回 `Err`,调用方丢弃 candidate
/// (不落盘、不替换内存),因此「整批原子」由调用方的事务边界保证。
pub(crate) fn apply(
    s: &mut RuntimeSettings,
    body: &UpdateSettingsBody,
    mode: AppMode,
) -> Result<PatchOutcome, PatchError> {
    let is_task = mode == AppMode::Task;
    let cleared_key_ids = connection::apply(s, body)?;
    generation::apply(s, body, is_task)?;
    agent::apply(s, body, is_task)?;
    tools::apply(s, body, is_task)?;
    memory::apply(s, body, is_task)?;
    task::apply(s, body, is_task)?;
    bundles::apply(s, body, is_task)?;
    Ok(PatchOutcome { cleared_key_ids })
}
