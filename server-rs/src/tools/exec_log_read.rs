// 完整命令证据的只读入口（TQUAL-18）。不接受磁盘路径，handle 只能在当前任务/会话下解析。
use crate::models::types::{ToolContext, ToolDefinition};
use crate::services::exec::evidence::{self, ExecEvidence, LogStream};
use crate::tools::registry::ToolRegistry;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

pub const TOOL_NAME: &str = "exec_log_read";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    handle: Option<String>,
    #[serde(default = "stdout")]
    stream: LogStream,
    #[serde(default = "first_line")]
    start_line: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn stdout() -> LogStream {
    LogStream::Stdout
}
fn first_line() -> usize {
    1
}
fn default_limit() -> usize {
    100
}

pub fn register(registry: &ToolRegistry, data_dir: std::path::PathBuf) {
    registry.register(ToolDefinition {
        name: TOOL_NAME.into(),
        description: "按 handle 回读本任务完整命令日志的一段（stdout 或 stderr），带原始行号。省略 handle 列出最近 20 次命令，含取消/超时状态。单次最多 200 行、16000 字节；有 next_line 时可继续读取，不必重跑失败命令。".into(),
        parameters: json!({"type":"object","properties":{
            "handle":{"type":"string","description":"bash 返回的 log_handle；省略则列本任务最近命令"},
            "stream":{"type":"string","enum":["stdout","stderr"],"description":"默认 stdout"},
            "start_line":{"type":"integer","minimum":1,"description":"起始行号，从 1 起"},
            "limit":{"type":"integer","minimum":1,"maximum":200,"description":"最多读取行数，默认 100"}
        }}),
    }, Arc::new(move |args, ctx| {
        let data_dir = data_dir.clone();
        Box::pin(async move {
            crate::models::data_activity::spawn_blocking(move || run(&data_dir, &ctx, args))
                .await.map_err(|e| format!("读取命令日志任务失败: {e}"))?
        })
    }));
}

fn run(data_dir: &Path, ctx: &ToolContext, value: serde_json::Value) -> Result<String, String> {
    let args: ReadArgs =
        serde_json::from_value(value).map_err(|e| format!("命令日志读取参数无效: {e}"))?;
    let owner = evidence::owner_from_session(&ctx.session_id);
    match args.handle {
        Some(handle) => {
            let page = ExecEvidence::open(data_dir, &owner, &handle)?.read_range(
                args.stream,
                args.start_line,
                args.limit,
            )?;
            let header = json!({"handle": page.handle, "stream": page.stream, "command":page.execution.command,
                "state": page.execution.state, "exit_code": page.execution.exit_code,
                "start_line": page.start_line, "end_line": page.end_line,
                "next_line": page.next_line, "truncated": page.truncated});
            // 正文不塞 JSON 字符串，避免控制字符转义把 12KB 日志膨胀成 72KB。
            Ok(format!(
                "[exec-log] {header}\n执行说明：{}\n{}",
                page.execution.detail, page.text
            ))
        }
        None => {
            let logs: Vec<_> = evidence::recent(data_dir, &owner)?.into_iter().map(|log| json!({
                "handle": log.handle, "command":log.command, "state": log.state, "exit_code": log.exit_code, "created_at": log.created_at,
            })).collect();
            Ok(json!({"logs":logs}).to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    #[test]
    fn tqual18_log_tool_shares_task_scope_but_not_other_tasks() {
        let dir = TempDataDir::new("tqual18-log-tool");
        let log = ExecEvidence::create(dir.path(), "task:one", "fixture-command").unwrap();
        std::fs::write(log.path(LogStream::Stdout), "first\nsecond\n").unwrap();
        let ctx = ToolContext {
            session_id: "task:one:main:2:sub:three".into(),
            character_id: String::new(),
            agent_depth: 1,
            scope: None,
            budget: None,
        };
        let args = json!({"handle":log.handle,"start_line":2,"limit":1});
        let output = run(dir.path(), &ctx, args.clone()).unwrap();
        assert!(output.contains("2: second"));
        let other = ToolContext {
            session_id: "task:other".into(),
            ..ctx
        };
        assert!(run(dir.path(), &other, args).is_err());
        assert!(!run(dir.path(), &other, json!({}))
            .unwrap()
            .contains(&log.handle));
        assert!(run(dir.path(), &other, json!({"path":"private.log"})).is_err());
    }

    #[test]
    fn tqual18_log_tool_output_budget_includes_json_escaping() {
        let dir = TempDataDir::new("tqual18-log-budget");
        let log = ExecEvidence::create(dir.path(), "task:one", "fixture-command").unwrap();
        std::fs::write(
            log.path(LogStream::Stdout),
            "\u{0001}".repeat(5000) + "\n" + &"x".repeat(5000) + "\n",
        )
        .unwrap();
        let ctx = ToolContext {
            session_id: "task:one".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: None,
            budget: None,
        };
        let output = run(dir.path(), &ctx, json!({"handle":log.handle})).unwrap();
        assert!(output.len() <= 16_000);
        assert!(output.contains("长行截断"));
    }
}
