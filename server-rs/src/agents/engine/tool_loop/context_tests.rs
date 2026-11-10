#![cfg(test)]
use super::*;
use crate::models::types::{ImageRef, ToolCallArgs, ToolDefinition};
use crate::services::token_service::TokenService;

const SUMMARY: &str = r#"{"decisions":["保留 a.rs 原接口并使用单一数据源"],"verified_facts":["已读取 a.rs 与 b.rs"],"unresolved":["cargo test 仍有断言失败"],"next_actions":["修复失败并复测"]}"#;

fn context_at_threshold(messages: &[LlmMessage], ts: &mut TokenService) -> TaskContext {
    let total = ts.count_message_tokens(messages, "gpt-4o")
        + messages
            .iter()
            .map(|m| m.images.len() as i64 * 4096)
            .sum::<i64>();
    TaskContext::new("gpt-4o".into(), (total * 100 / 85) as u32, &[], ts)
}

fn round(id: &str, name: &str, arguments: &str, content: &str) -> Vec<LlmMessage> {
    let mut assistant = LlmMessage::plain("assistant", "采用单一数据源，修改后再运行原验收命令。");
    assistant.tool_calls = Some(vec![ToolCallArgs {
        id: id.into(),
        name: name.into(),
        arguments: arguments.into(),
    }]);
    let mut tool = LlmMessage::plain("tool", content);
    tool.tool_call_id = Some(id.into());
    vec![assistant, tool]
}

fn history() -> Vec<LlmMessage> {
    let mut messages = vec![
        LlmMessage::plain("system", "只允许修改工作区。"),
        LlmMessage::plain("user", "原始目标：修复 a.rs，保留已有接口并验证错误分支。"),
    ];
    messages.extend(round(
        "a",
        "fs_read",
        r#"{"path":"a.rs"}"#,
        &"old source ".repeat(1400),
    ));
    messages.extend(round(
        "b",
        "bash",
        r#"{"command":"cargo test"}"#,
        r#"{"error":"FAILED_ASSERT_492: expected 2, got 3"}"#,
    ));
    messages.extend(round(
        "c",
        "fs_read",
        r#"{"path":"b.rs"}"#,
        &"more source ".repeat(700),
    ));
    messages.extend(round(
        "latest",
        "fs_read",
        r#"{"path":"a.rs"}"#,
        "最近完整工具结果",
    ));
    messages
}

#[test]
fn task_context_compacts_at_threshold_instead_of_rewriting_every_round() {
    let messages = history();
    let mut ts = TokenService::new();
    let total = ts.count_message_tokens(&messages, "gpt-4o");
    let context = TaskContext::new("gpt-4o".into(), (total * 100 / 85) as u32, &[], &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap();
    assert!(
        stage.is_some(),
        "达到 80% 阈值应准备阶段压缩，不能继续全量回灌"
    );
}

#[test]
fn task_context_low_water_history_is_byte_for_byte_unchanged() {
    let messages = history();
    let before = serde_json::to_string(&messages).unwrap();
    let mut ts = TokenService::new();
    let context = TaskContext::new("gpt-4o".into(), 100_000, &[], &mut ts);
    assert!(context.prepare(&messages, &mut ts).unwrap().is_none());
    assert_eq!(before, serde_json::to_string(&messages).unwrap());
}

#[test]
fn task_context_rejects_irreducible_goal_overflow() {
    let messages = vec![
        LlmMessage::plain("system", "系统规则"),
        LlmMessage::plain("user", &"immutable goal ".repeat(1000)),
    ];
    let mut ts = TokenService::new();
    let context = TaskContext::new("gpt-4o".into(), 100, &[], &mut ts);
    assert!(
        context.prepare(&messages, &mut ts).is_err(),
        "原始目标超预算必须明确失败，不能截断目标或继续发送"
    );
}

#[test]
fn task_context_counts_tool_schema_in_input_budget() {
    let messages = vec![
        LlmMessage::plain("system", "系统规则"),
        LlmMessage::plain("user", "目标"),
    ];
    let tools = vec![ToolDefinition {
        name: "large_schema".into(),
        description: "description ".repeat(2000),
        parameters: serde_json::json!({"type":"object"}),
    }];
    let mut ts = TokenService::new();
    let context = TaskContext::new("gpt-4o".into(), 100, &tools, &mut ts);
    assert!(
        context.prepare(&messages, &mut ts).is_err(),
        "工具定义不能漏计"
    );
}

#[test]
fn task_context_preserves_goal_decisions_errors_files_and_latest_pair() {
    let messages = history();
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap().unwrap();
    let compacted = context.finish(stage, SUMMARY, &mut ts).unwrap();
    assert_eq!(
        serde_json::to_value(&compacted[..2]).unwrap(),
        serde_json::to_value(&messages[..2]).unwrap()
    );
    let checkpoint = &compacted[2].content;
    for required in [
        "单一数据源",
        "FAILED_ASSERT_492",
        "cargo test",
        "a.rs",
        "b.rs",
    ] {
        assert!(checkpoint.contains(required), "阶段状态遗失：{required}");
    }
    assert_eq!(compacted.len(), 5);
    assert_eq!(compacted[3].tool_calls.as_ref().unwrap()[0].id, "latest");
    assert_eq!(compacted[4].tool_call_id.as_deref(), Some("latest"));
    assert_eq!(
        serde_json::to_value(&compacted[3..]).unwrap(),
        serde_json::to_value(&messages[messages.len() - 2..]).unwrap()
    );
    assert!(
        context.prepare(&compacted, &mut ts).unwrap().is_none(),
        "刚完成阶段替换不能下一轮再次压缩"
    );
}

#[test]
fn task_context_invalid_summary_never_mutates_original_history() {
    let messages = history();
    let before = serde_json::to_string(&messages).unwrap();
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap().unwrap();
    assert!(context.finish(stage, "已压缩，继续执行", &mut ts).is_err());
    assert_eq!(before, serde_json::to_string(&messages).unwrap());
}

#[test]
fn task_context_requires_complete_tool_id_pairing() {
    let mut messages = history();
    messages.last_mut().unwrap().tool_call_id = Some("wrong-id".into());
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let error = context.prepare(&messages, &mut ts).err().unwrap();
    assert!(error.contains("配对不完整"), "{error}");
}

#[test]
fn task_context_keeps_all_calls_in_the_latest_parallel_round() {
    let mut messages = history();
    let last = messages.len() - 2;
    messages[last]
        .tool_calls
        .as_mut()
        .unwrap()
        .push(ToolCallArgs {
            id: "latest-two".into(),
            name: "fs_read".into(),
            arguments: r#"{"path":"b.rs"}"#.into(),
        });
    let mut second = LlmMessage::plain("tool", "第二个并行读取结果");
    second.tool_call_id = Some("latest-two".into());
    messages.push(second);
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap().unwrap();
    let compacted = context.finish(stage, SUMMARY, &mut ts).unwrap();
    assert_eq!(
        serde_json::to_value(&compacted[3..]).unwrap(),
        serde_json::to_value(&messages[last..]).unwrap()
    );
}

#[test]
fn task_context_old_images_become_persistent_refs_latest_image_stays() {
    let image = |id: &str| ImageRef {
        id: id.into(),
        name: format!("{id}.png"),
        mime: "image/png".into(),
        data_url: format!("data:image/png;base64,{id}"),
        label: None,
    };
    let mut messages = history();
    messages[3].images.push(image("old-image"));
    messages
        .last_mut()
        .unwrap()
        .images
        .push(image("recent-image"));
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap().unwrap();
    assert!(
        !stage
            .request
            .iter()
            .any(|m| m.content.contains("data:image")),
        "摘要器只接收已持久化引用，不假装看过旧图"
    );
    let compacted = context.finish(stage, SUMMARY, &mut ts).unwrap();
    assert!(compacted[2].content.contains("old-image"));
    assert_eq!(compacted.last().unwrap().images[0].id, "recent-image");
    assert_eq!(compacted.iter().map(|m| m.images.len()).sum::<usize>(), 1);
}

#[test]
fn task_context_carries_durable_evidence_across_multiple_stages() {
    let messages = history();
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let first = context.prepare(&messages, &mut ts).unwrap().unwrap();
    let mut compacted = context.finish(first, SUMMARY, &mut ts).unwrap();
    compacted.extend(round(
        "new-a",
        "fs_read",
        r#"{"path":"new.rs"}"#,
        &"new source ".repeat(1800),
    ));
    compacted.extend(round(
        "new-b",
        "fs_read",
        r#"{"path":"last.rs"}"#,
        "最后一轮",
    ));
    let second_context = context_at_threshold(&compacted, &mut ts);
    let second = second_context
        .prepare(&compacted, &mut ts)
        .unwrap()
        .unwrap();
    let result = second_context.finish(second, SUMMARY, &mut ts).unwrap();
    assert_eq!(
        result
            .iter()
            .filter(|m| m.content.starts_with("[任务阶段上下文]"))
            .count(),
        1
    );
    for required in ["FAILED_ASSERT_492", "a.rs", "b.rs", "new.rs", "单一数据源"] {
        assert!(
            result[2].content.contains(required),
            "跨阶段遗失证据：{required}"
        );
    }
}

#[test]
fn task_context_oversized_summary_fails_without_dropping_evidence() {
    let messages = history();
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap().unwrap();
    let response = serde_json::json!({"decisions":["huge summary ".repeat(5000)],"verified_facts":[],"unresolved":[],"next_actions":[]}).to_string();
    assert!(context
        .finish(stage, &response, &mut ts)
        .err()
        .unwrap()
        .contains("未腾出"));
}

#[test]
fn task_context_summary_preserves_selected_connection_and_output_budget() {
    let params = GenerationParams {
        temperature: 0.7,
        top_p: 1.0,
        max_tokens: 12_000,
        stop: None,
        tools: vec![ToolDefinition {
            name: "fs_read".into(),
            description: "读取".into(),
            parameters: serde_json::json!({"type":"object"}),
        }],
        max_tool_rounds: Some(0),
        tool_choice: ToolChoice::Auto,
        connection_id: Some("selected-connection".into()),
        parallel_tool_calls: None,
        step_budget: None,
        semantic_guard: None,
        response_format: None,
    };
    let summary = summary_params(&params, true);
    assert_eq!(
        summary.connection_id.as_deref(),
        Some("selected-connection")
    );
    assert_eq!(summary.max_tokens, 12_000, "输入预算不能再扣输出额度");
    assert!(summary.tools.is_empty());
    assert!(matches!(summary.tool_choice, ToolChoice::None));
    assert!(matches!(
        summary.response_format,
        Some(ResponseFormat::Json)
    ));
}

#[test]
fn task_context_budget_uses_nonzero_input_caps() {
    use crate::agents::engine::executor::task_input_token_budget;
    assert_eq!(task_input_token_budget(8192, 4096), 4096);
    assert_eq!(task_input_token_budget(8192, 0), 8192);
    assert_eq!(task_input_token_budget(0, 4096), 4096);
    assert_eq!(task_input_token_budget(0, 0), 0);
}

fn recorded_usage() -> TokenUsage {
    TokenUsage {
        prompt_tokens: 31,
        completion_tokens: 9,
        total_tokens: 40,
        prompt_cache_hit_tokens: 11,
        ..TokenUsage::default()
    }
}

#[tokio::test]
async fn task_context_timeout_preserves_usage_already_received_from_stream() {
    let (_sender, abort) = watch::channel(false);
    let mut live_usage = TokenUsage::default();
    let mut total_usage = TokenUsage::default();
    let call = async {
        live_usage = recorded_usage();
        std::future::pending::<Result<ExecutorResult, EngineError>>().await
    };
    let outcome = await_summary_call(call, Some(Duration::from_millis(5)), &abort).await;
    let error = account_summary_call(outcome, &live_usage, &mut total_usage)
        .err()
        .unwrap();
    assert_eq!(error.error_code(), "request_timeout");
    assert_eq!(total_usage.prompt_tokens, 31);
    assert_eq!(total_usage.completion_tokens, 9);
    assert_eq!(total_usage.total_tokens, 40);
    assert_eq!(total_usage.prompt_cache_hit_tokens, 11);
}

#[tokio::test]
async fn task_context_cancel_preserves_usage_already_received_from_stream() {
    let (sender, abort) = watch::channel(false);
    let cancel = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        sender.send(true).unwrap();
    });
    let mut live_usage = TokenUsage::default();
    let mut total_usage = TokenUsage::default();
    let call = async {
        live_usage = recorded_usage();
        std::future::pending::<Result<ExecutorResult, EngineError>>().await
    };
    let outcome = await_summary_call(call, None, &abort).await;
    let error = account_summary_call(outcome, &live_usage, &mut total_usage)
        .err()
        .unwrap();
    assert!(matches!(error, EngineError::Interrupted));
    assert_eq!(total_usage.total_tokens, 40);
    cancel.await.unwrap();
}

#[test]
fn task_context_success_counts_returned_usage_once_not_live_plus_returned() {
    let live_usage = recorded_usage();
    let mut total_usage = TokenUsage {
        total_tokens: 5,
        ..TokenUsage::default()
    };
    let result = ExecutorResult {
        content: SUMMARY.into(),
        usage: recorded_usage(),
        interrupted: false,
        tool_calls: Vec::new(),
        reasoning: String::new(),
        finish_reason: Some("stop".into()),
        self_heals: Vec::new(),
        budget_stopped: false,
        stop: None,
    };
    account_summary_call(Ok(result), &live_usage, &mut total_usage).unwrap();
    assert_eq!(total_usage.total_tokens, 45);
    assert_eq!(total_usage.prompt_tokens, 31);
}

/// real-1 的成功复测会产出 32K 预览；已持久化的正文不能随每个阶段再复制一份。
#[test]
fn tqual1718_successful_persisted_logs_archive_as_handles_without_raising_budget() {
    use crate::services::exec::evidence::{ExecEvidence, LogState, LogStream};
    use crate::utils::test_support::TempDataDir;
    let dir = TempDataDir::new("phase-success-logs");
    let mut messages = vec![
        LlmMessage::plain("system", "系统不变"),
        LlmMessage::plain("user", "修复计数器并用 node check.cjs 验收。"),
    ];
    let payload = "LONG_SUCCESS_BODY_81: completed 1234567890\n".repeat(780);
    let mut handles = Vec::new();
    for n in 0..3 {
        let log = ExecEvidence::create(dir.path(), "task:phase", "fixture-command").unwrap();
        std::fs::write(log.path(LogStream::Stdout), &payload).unwrap();
        log.finish(LogState::Completed, Some(0), "").unwrap();
        let output = format!(
            "$ node check.cjs\n[stdout]\n{payload}\n[exec-meta] {}",
            serde_json::json!({
                "exit_code":0,"timed_out":false,"truncated":true,"log_handle":log.handle
            })
        );
        messages.extend(round(
            &format!("success-{n}"),
            "bash",
            r#"{"command":"node check.cjs"}"#,
            &serde_json::to_string(&output).unwrap(),
        ));
        handles.push(log.handle);
    }
    messages.extend(round("padding", "fs_read", r#"{"path":"counter.mjs"}"#, ""));
    let padding = messages.len() - 1;
    messages.extend(round(
        "latest",
        "fs_read",
        r#"{"path":"check.cjs"}"#,
        "最后的完整读取结果",
    ));
    let mut ts = TokenService::new();
    while ts.count_message_tokens(&messages, "gpt-4o") < 28_000 {
        messages[padding]
            .content
            .push_str(&"source fact ".repeat(100));
    }
    let context = TaskContext::new("gpt-4o".into(), 32_768, &[], &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap().unwrap();
    assert!(
        !stage.request[1].content.contains("LONG_SUCCESS_BODY_81"),
        "摘要请求本身也应使用可回读证据，而非再传32K正文"
    );
    let compacted = context.finish(stage, SUMMARY, &mut ts).unwrap();
    let checkpoint = &compacted[2].content;
    assert!(
        checkpoint.len() < 5000,
        "成功日志引用应有界：{}",
        checkpoint.len()
    );
    assert!(!checkpoint.contains("LONG_SUCCESS_BODY_81"));
    for handle in handles {
        assert!(checkpoint.contains(&handle), "命令日志句柄必须保留");
    }
    assert!(checkpoint.contains("node check.cjs") && checkpoint.contains("exec_log_read"));
    assert_eq!(
        serde_json::to_value(&compacted[3..]).unwrap(),
        serde_json::to_value(&messages[messages.len() - 2..]).unwrap()
    );
}

#[test]
fn tqual1718_checkpoint_keeps_decisions_not_repeated_assistant_and_instruction_bodies() {
    let mut messages = history();
    messages[2].content = format!(
        "KEEP_DYNAMIC_RULE：不改变原接口。{}",
        "ASSISTANT_FULL_COPY ".repeat(900)
    );
    messages.insert(
        4,
        LlmMessage::plain(
            "user",
            &format!(
                "KEEP_DYNAMIC_RULE：不改变原接口。{}",
                "INSTRUCTION_FULL_COPY ".repeat(900)
            ),
        ),
    );
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap().unwrap();
    let summary = r#"{"decisions":["KEEP_DYNAMIC_RULE：不改变原接口"],"verified_facts":[],"unresolved":["FAILED_ASSERT_492 尚未修复"],"next_actions":["继续修复"]}"#;
    let compacted = context.finish(stage, summary, &mut ts).unwrap();
    let checkpoint = &compacted[2].content;
    assert!(checkpoint.contains("KEEP_DYNAMIC_RULE"));
    assert!(checkpoint.contains("FAILED_ASSERT_492"));
    assert!(
        !checkpoint.contains("ASSISTANT_FULL_COPY")
            && !checkpoint.contains("INSTRUCTION_FULL_COPY"),
        "历史全文不能绕过结构化摘要永久累积"
    );
}

#[test]
fn tqual1718_host_verification_evidence_cannot_be_erased_by_summary() {
    let mut messages = history();
    for status in ["Failed", "Unavailable", "Passed"] {
        let facts = crate::services::prompt_kit::untrusted_boundary_task(
            "独立验收的外部程序输出",
            &format!("状态：{status}\nHOST_FAILURE_81：按钮点击未改变计数\n已执行检查：点击后断言"),
        );
        let mut message = LlmMessage::plain(
            "user",
            &format!("[独立验收证据]\n原始要求（不得削减）：实现计数器。\n{facts}"),
        );
        message.images.push(ImageRef {
            id: format!("proof-{status}.png"),
            name: "失败截图".into(),
            mime: "image/png".into(),
            data_url: String::new(),
            label: None,
        });
        messages.insert(4, message);
    }
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap().unwrap();
    let misleading = r#"{"decisions":[],"verified_facts":["模型自称已全部通过"],"unresolved":[],"next_actions":[]}"#;
    let compacted = context.finish(stage, misleading, &mut ts).unwrap();
    for fact in [
        "状态：Failed",
        "状态：Unavailable",
        "状态：Passed",
        "HOST_FAILURE_81",
        "proof-Failed.png",
        "proof-Unavailable.png",
        "proof-Passed.png",
        "FAILED_ASSERT_492",
    ] {
        assert!(
            compacted[2].content.contains(fact),
            "摘要不得删掉宿主/工具失败证据：{fact}"
        );
    }
}

#[test]
fn tqual1718_logged_failure_preserves_exit_timeout_handle_and_numbered_window() {
    let mut messages = history();
    let handle = uuid::Uuid::new_v4().to_string();
    let body = format!("[stdout]\n[诊断线索窗口]\n2399: 准备完成\n2400: 模块初始化\n2401: ERROR ACCEPTANCE_KEEP_81: expected 1, got 0\n2402: 结束\n[exec-meta] {}",
        serde_json::json!({"exit_code":1,"timed_out":false,"truncated":true,"log_handle":handle}));
    let extra = round(
        "logged-fail",
        "bash",
        r#"{"command":"node check.cjs"}"#,
        &serde_json::to_string(&body).unwrap(),
    );
    messages.splice(messages.len() - 2..messages.len() - 2, extra);
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap().unwrap();
    let compacted = context.finish(stage, SUMMARY, &mut ts).unwrap();
    let facts = &compacted[2].content;
    for required in [
        &handle,
        "ACCEPTANCE_KEEP_81",
        "2401:",
        "node check.cjs",
        "exec_log_read",
        "FAILED_ASSERT_492",
    ] {
        assert!(facts.contains(required), "已知失败事实不得丢失：{required}");
    }
    let checkpoint: serde_json::Value =
        serde_json::from_str(facts.strip_prefix("[任务阶段上下文]\n").unwrap()).unwrap();
    let command_fact = checkpoint["evidence"]["tool_facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|fact| fact.as_str())
        .find(|fact| fact.contains(&handle))
        .unwrap();
    let meta_line = command_fact
        .lines()
        .find_map(|line| line.strip_prefix("[exec-meta] "))
        .unwrap();
    let meta: serde_json::Value = serde_json::from_str(meta_line).unwrap();
    assert_eq!(meta["exit_code"], 1);
    assert_eq!(meta["timed_out"], false);
}

#[test]
fn tqual1718_reused_call_ids_do_not_convert_file_reads_into_command_logs() {
    let mut messages = history();
    let content = format!(
        "SOURCE_FILE_LITERAL_81\n[exec-meta] {}",
        serde_json::json!({
            "exit_code":0,"timed_out":false,"truncated":true,"log_handle":uuid::Uuid::new_v4().to_string()
        })
    );
    // 工具 id 只需在当前调用组中对应；前轮 bash 与本轮 fs_read 可以复用同一个 id。
    messages.splice(
        messages.len() - 2..messages.len() - 2,
        round(
            "b",
            "fs_read",
            r#"{"path":"captured-log.txt"}"#,
            &serde_json::to_string(&content).unwrap(),
        ),
    );
    let mut ts = TokenService::new();
    let context = context_at_threshold(&messages, &mut ts);
    let stage = context.prepare(&messages, &mut ts).unwrap().unwrap();
    assert!(
        stage.request[1].content.contains("SOURCE_FILE_LITERAL_81"),
        "读取的文件正文不能冒充前轮命令证据而被替换"
    );
}
