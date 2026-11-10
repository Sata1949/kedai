#![cfg(test)]
use crate::services::exec::evidence::*;
use crate::utils::test_support::TempDataDir;

#[test]
fn tqual18_log_range_is_scoped_bounded_and_numbered() {
    let dir = TempDataDir::new("tqual18-ranges");
    let log = ExecEvidence::create(dir.path(), "task:a", "fixture-command").unwrap();
    std::fs::write(log.path(LogStream::Stdout), "一\n二\n三\n四\n").unwrap();
    let page = log.read_range(LogStream::Stdout, 2, 2).unwrap();
    assert_eq!(page.text, "2: 二\n3: 三\n");
    assert_eq!(page.next_line, Some(4));
    assert!(ExecEvidence::open(dir.path(), "task:b", &log.handle).is_err());
    for bad in [
        "../secret",
        "C:\\secret",
        "00000000-0000-0000-0000-000000000000/../x",
    ] {
        assert!(ExecEvidence::open(dir.path(), "task:a", bad).is_err());
    }
    assert!(log.read_range(LogStream::Stdout, 0, 1).is_err());
    assert!(log
        .read_range(LogStream::Stdout, 1, MAX_READ_LINES + 1)
        .is_err());
}

#[test]
fn tqual18_long_lines_do_not_bypass_output_budget() {
    let dir = TempDataDir::new("tqual18-long-lines");
    let log = ExecEvidence::create(dir.path(), "task:a", "fixture-command").unwrap();
    let data = format!("{}\n末行\n", "字".repeat(100_000));
    std::fs::write(log.path(LogStream::Stderr), &data).unwrap();
    let page = log.read_range(LogStream::Stderr, 1, 2).unwrap();
    assert!(page.truncated);
    assert!(page.text.len() <= MAX_READ_BYTES);
    assert!(page.text.contains("2: 末行"));
    assert_eq!(
        std::fs::read_to_string(log.path(LogStream::Stderr)).unwrap(),
        data
    );
}

#[test]
fn tqual18_cancel_guard_keeps_discoverable_handle_without_paths() {
    let dir = TempDataDir::new("tqual18-cancel-state");
    let log = ExecEvidence::create(dir.path(), "task:a", "fixture-command").unwrap();
    drop(crate::services::exec::evidence::EvidenceRun(Some(
        log.clone(),
    )));
    assert!(log.cancel_path().is_file());
    let logs = recent(dir.path(), "task:a").unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].state, LogState::Cancelled);
    let json = serde_json::to_string(&logs).unwrap();
    assert!(!json.contains(&dir.path().to_string_lossy().to_string()));
    assert!(recent(dir.path(), "task:b").unwrap().is_empty());
}

#[test]
fn tqual18_cancel_marker_failure_is_explicit_in_persisted_state() {
    let dir = TempDataDir::new("tqual18-cancel-failure");
    let log = ExecEvidence::create(dir.path(), "task:a", "fixture-command").unwrap();
    std::fs::create_dir(log.cancel_path()).unwrap();
    let error = log.cancel().unwrap_err();
    assert!(error.contains("无法保证 Android 命令已终止"));
    let meta = log.metadata().unwrap();
    assert_eq!(meta.state, LogState::Failed);
    assert_eq!(meta.detail, error);
}

#[test]
fn tqual18_diagnostics_do_not_match_successful_error_case_names() {
    use crate::services::exec::evidence::failure_hint;
    assert!(!failure_hint("test handles_error_case ... ok"));
    assert!(!failure_hint("test result: ok. 200 passed; 0 failed"));
    assert!(failure_hint("error[E0308]: wrong type"));
    assert!(failure_hint("test worker ... FAILED"));
    assert!(failure_hint("thread 'x' panicked at src/main.rs:2"));
}

#[tokio::test]
async fn tqual1718_successful_long_log_returns_small_numbered_edges_and_keeps_raw_file() {
    use crate::services::exec::{execute, ExecRequest, ShellTier};
    let dir = TempDataDir::new("tqual1718-success-preview");
    let log = ExecEvidence::create(dir.path(), "task:long-success", "fixture-command").unwrap();
    let script = format!(
        "{}echo BEGIN_SUCCESS_81\n{}echo END_SUCCESS_81\n{}",
        if cfg!(windows) { "@echo off\n" } else { "" },
        format!("echo {}\n", "ordinary-success-data ".repeat(5)).repeat(1500),
        if cfg!(windows) {
            "exit /b 0\n"
        } else {
            "exit 0\n"
        }
    );
    let name = if cfg!(windows) {
        "success.cmd"
    } else {
        "success.sh"
    };
    std::fs::write(dir.join(name), script).unwrap();
    let result = execute(
        ExecRequest {
            command: if cfg!(windows) {
                name.into()
            } else {
                format!("sh {name}")
            },
            cwd: Some(dir.path().to_string_lossy().into_owned()),
            timeout_ms: Some(20_000),
            evidence: Some(log.clone()),
        },
        &[ShellTier::Sandbox],
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(result.truncated);
    assert!(
        result.stdout.len() < 3000,
        "成功长日志不得继续回灌32K预览：{}",
        result.stdout.len()
    );
    assert!(result.stdout.contains("1: BEGIN_SUCCESS_81"));
    assert!(result.stdout.contains("1502: END_SUCCESS_81"));
    let raw = std::fs::read_to_string(log.path(LogStream::Stdout)).unwrap();
    assert!(raw.len() > 100_000 && raw.contains("END_SUCCESS_81"));
    assert_eq!(result.log_handle.as_deref(), Some(log.handle.as_str()));
}

#[tokio::test]
async fn tqual1718_small_command_output_stays_byte_for_byte_original() {
    use crate::services::exec::{execute, ExecRequest, ShellTier};
    let dir = TempDataDir::new("tqual1718-small-preview");
    let log = ExecEvidence::create(dir.path(), "task:small", "fixture-command").unwrap();
    let result = execute(
        ExecRequest {
            command: "echo SHORT_SUCCESS_81".into(),
            cwd: None,
            timeout_ms: Some(10_000),
            evidence: Some(log.clone()),
        },
        &[ShellTier::Sandbox],
    )
    .await
    .unwrap();
    assert!(!result.truncated);
    assert_eq!(
        result.stdout,
        std::fs::read_to_string(log.path(LogStream::Stdout)).unwrap()
    );
    assert_eq!(result.stdout.trim(), "SHORT_SUCCESS_81");
}

#[test]
fn tqual1718_archived_diagnostics_recognize_existing_log_line_numbers() {
    let excerpt = diagnostic_excerpt("[诊断线索窗口]\n2400: 准备完成\n2401: ERROR ACCEPTANCE_KEEP_81: expected 1, got 0\n2402: 结束");
    assert!(excerpt.contains("2401: ERROR ACCEPTANCE_KEEP_81"));
    assert!(diagnostic_excerpt(
        "1: test handles_error_case ... ok\n2: test result: ok. 200 passed; 0 failed"
    )
    .is_empty());
}
