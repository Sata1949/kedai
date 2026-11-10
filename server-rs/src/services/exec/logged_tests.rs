#![cfg(test)]

use crate::services::exec::{execute, ExecRequest, ShellTier};
use crate::utils::test_support::TempDataDir;

#[tokio::test]
async fn tqual18_middle_failure_is_in_diagnostic_window() {
    let dir = TempDataDir::new("tqual18-middle");
    let noise = format!("echo {}\n", "ordinary-output-".repeat(8));
    let script = format!(
        "{}{}echo error: MIDDLE_FAILURE_EVIDENCE\n{}{}",
        if cfg!(windows) { "@echo off\n" } else { "" },
        noise.repeat(800),
        noise.repeat(800),
        if cfg!(windows) {
            "exit /b 1\n"
        } else {
            "exit 1\n"
        },
    );
    let filename = if cfg!(windows) {
        "probe.cmd"
    } else {
        "probe.sh"
    };
    std::fs::write(dir.path().join(filename), script).unwrap();
    let command = if cfg!(windows) {
        filename.to_string()
    } else {
        format!("sh {filename}")
    };
    let evidence =
        crate::services::exec::evidence::ExecEvidence::create(dir.path(), "task:middle", &command)
            .unwrap();
    let result = execute(
        ExecRequest {
            command,
            cwd: Some(dir.path().to_string_lossy().into_owned()),
            timeout_ms: Some(20_000),
            evidence: Some(evidence.clone()),
        },
        &[ShellTier::Sandbox],
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, 1);
    let receipt = serde_json::to_value(evidence.metadata().unwrap()).unwrap();
    assert_eq!(
        receipt["command"],
        if cfg!(windows) {
            "probe.cmd"
        } else {
            "sh probe.sh"
        },
        "审计按日志句柄取证时，实际命令必须与退出码一起可读"
    );
    assert!(
        result.stdout.contains("MIDDLE_FAILURE_EVIDENCE"),
        "中段失败必须进入诊断窗口"
    );
    let raw =
        std::fs::read_to_string(evidence.path(crate::services::exec::evidence::LogStream::Stdout))
            .unwrap();
    assert!(raw.len() > 100_000);
    assert!(raw.contains("MIDDLE_FAILURE_EVIDENCE"));
}

#[tokio::test]
async fn tqual18_timeout_retains_emitted_output() {
    use crate::services::exec::evidence::{ExecEvidence, LogState, LogStream};
    let dir = TempDataDir::new("tqual18-timeout");
    let command = if cfg!(windows) {
        "echo STDOUT_BEFORE_TIMEOUT && (echo STDERR_BEFORE_TIMEOUT 1>&2) && ping -n 30 127.0.0.1"
    } else {
        // 保持外壳和 sleep 两个进程；杀外壳后 sleep 仍持有双流写端。
        "echo STDOUT_BEFORE_TIMEOUT; echo STDERR_BEFORE_TIMEOUT >&2; sleep 30; :"
    };
    let evidence = ExecEvidence::create(dir.path(), "task:timeout", command).unwrap();
    let result = execute(
        ExecRequest {
            command: command.into(),
            cwd: None,
            timeout_ms: Some(1_000),
            evidence: Some(evidence.clone()),
        },
        &[ShellTier::Sandbox],
    )
    .await
    .unwrap_err();
    assert!(result.contains("命令超时"), "{result}");
    let meta = evidence.metadata().unwrap();
    assert_eq!(meta.state, LogState::TimedOut, "{result}");
    assert_eq!(meta.exit_code, None);
    for (stream, marker) in [
        (LogStream::Stdout, "STDOUT_BEFORE_TIMEOUT"),
        (LogStream::Stderr, "STDERR_BEFORE_TIMEOUT"),
    ] {
        assert!(result.contains(marker), "超时回执应保留双流证据: {result}");
        assert!(evidence
            .read_range(stream, 1, 10)
            .unwrap()
            .text
            .contains(marker));
    }
    #[cfg(unix)]
    {
        assert!(result.contains("排空超时"), "{result}");
        assert!(meta.detail.contains("[stdout]"), "{meta:?}");
        assert!(meta.detail.contains("[stderr]"), "{meta:?}");
    }
}

#[tokio::test]
async fn tqual18_cancel_keeps_full_log_and_discoverable_state() {
    use crate::services::exec::evidence::{ExecEvidence, LogState, LogStream};
    let dir = TempDataDir::new("tqual18-cancel");
    let evidence = ExecEvidence::create(dir.path(), "task:cancel", "fixture-command").unwrap();
    let request = ExecRequest {
        command: if cfg!(windows) {
            "echo BEFORE_CANCEL && ping -n 30 127.0.0.1".into()
        } else {
            "echo BEFORE_CANCEL; sleep 30".into()
        },
        cwd: None,
        timeout_ms: Some(30_000),
        evidence: Some(evidence.clone()),
    };
    let task = tokio::spawn(execute(request, &[ShellTier::Sandbox]));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if std::fs::read_to_string(evidence.path(LogStream::Stdout))
                .unwrap()
                .contains("BEFORE_CANCEL")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("命令应先产生输出");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(evidence.metadata().unwrap().state, LogState::Cancelled);
    assert!(evidence.cancel_path().is_file());
    assert!(evidence
        .read_range(LogStream::Stdout, 1, 10)
        .unwrap()
        .text
        .contains("BEFORE_CANCEL"));
}

#[tokio::test]
async fn tqual18_denied_tier_does_not_leave_running_evidence() {
    use crate::services::exec::evidence::{ExecEvidence, LogState};
    let dir = TempDataDir::new("tqual18-denied");
    let log = ExecEvidence::create(dir.path(), "task:denied", "fixture-command").unwrap();
    let result = execute(
        ExecRequest {
            command: "echo denied".into(),
            cwd: None,
            timeout_ms: Some(1000),
            evidence: Some(log.clone()),
        },
        &[],
    )
    .await;
    assert!(result.is_err());
    assert_eq!(log.metadata().unwrap().state, LogState::Failed);
}
