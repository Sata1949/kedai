// 消息构建模块(目录化拆分,纯代码移动,逻辑不变):
//   build.rs  6 层位置拼装 LLM 消息构建、步骤参数派生、步骤提示词与反思回退定位
//   inject.rs @INJECT 精确消息插入、反思建议注入、摘要槽/记忆槽插入
//   trim.rs   上下文裁剪(trim_to_context)与受保护头部长度
// 本文件只做再导出,保证 engine/mod.rs 的 `use self::messages::{...}` 无需改动。
// 可见性说明:子文件中原 pub(super)(= 对 engine 可见)的条目改为
// pub(in crate::agents::engine),可见范围与拆分前完全一致,未放宽。
mod build;
mod inject;
mod trim;

pub(super) use build::{
    build_llm_messages_with_position, retreat_to_generating_step, step_params_for, with_step_prompt,
};
pub(super) use inject::{
    apply_inject_insertions, inject_reflect_advice, insert_memory_slot, insert_summary_slot,
    parse_inject_insertion, InjectAt, InjectInsertion,
};
pub(super) use trim::{trim_to_context, trim_tool_history, TOOL_HISTORY_SUMMARY_PREFIX};
