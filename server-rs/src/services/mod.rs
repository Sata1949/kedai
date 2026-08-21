// 业务服务层
pub mod agent_flow_service;
pub mod audio_service;
pub mod agent_session_service;
pub mod agent_subtask_service;
pub mod cache_diagnostics;
pub mod character_service;
pub mod contract_changelog_service;
pub mod kaleido_state_service;
pub mod memory_service;
pub mod prompt_inject_service;
pub mod quick_reply_service;
pub mod runtime_prompt_service;
pub mod secret_store;
pub mod session_service;
pub mod settings_service;
pub mod skill_service;
pub mod task_service;
pub mod token_service;
pub mod user_script_service;
pub mod variable_apply;
pub mod world_book_service;

/// DB 列表查询失败兜底(2026-08 裸 unwrap 审计):记 warn 并回退空列表。
/// 列表函数本就按 filter_map 丢弃坏行的 best-effort 语义工作,prepare/参数绑定失败
/// (schema 级异常)同样不回传 panic——阻塞线程 panic 会经 JoinError 放大为 500。
pub(crate) fn log_query_failure<T>(op: &str, e: rusqlite::Error) -> Vec<T> {
    crate::utils::logger::warn(
        "DB 列表查询失败,回退空列表",
        &[
            ("op", serde_json::json!(op)),
            ("error", serde_json::json!(e.to_string())),
        ],
    );
    Vec::new()
}
