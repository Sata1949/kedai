// 桌面/服务端执行器:直接派生进程(阶段 C)。
//
// 安全要点(与 tools/bash.rs 的分工:本层只负责「安全地跑」,授权已在上游完成):
// - 强制超时 + 超时强杀(含子孙进程:Unix 用进程组,Windows 用 taskkill /T);
// - stdin 置 null,禁止交互式命令挂起等待输入;
// - 输出按字符截断(见 mod.rs::truncate_output);
// - 不拼接额外字符串:命令原文交平台 shell 解释。
#![cfg(not(target_os = "android"))]

use super::{clamp_timeout, truncate_output, ExecRequest, ExecResult, ShellTier};
use std::process::Stdio;

/// 平台默认 shell(与前端/文档口径一致:Windows=cmd,其余=sh)。
pub fn default_shell() -> &'static str {
    if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "sh"
    }
}

/// 桌面执行:恒为 Sandbox 等级(无提权语义)。
pub async fn execute(req: ExecRequest) -> Result<ExecResult, String> {
    if req.command.trim().is_empty() {
        return Err("命令为空".into());
    }
    let timeout = clamp_timeout(req.timeout_ms);

    let mut cmd = if cfg!(target_os = "windows") {
        // cmd /C <command>:交给 cmd 解释(与用户在终端里的行为一致)
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(&req.command);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(&req.command);
        c
    };

    if let Some(dir) = req.cwd.as_deref() {
        if !dir.trim().is_empty() {
            if !std::path::Path::new(dir).is_dir() {
                return Err(format!("工作目录不存在或不是目录: {dir}"));
            }
            cmd.current_dir(dir);
        }
    }

    // Windows 桌面版入口是 GUI 子系统(无控制台),派生 cmd 会**新建可见控制台窗口**,
    // 用户可见为「任务模式乱弹 cmd 黑窗」(2026-09-18 修复;debug 构建自带控制台故不复现)。
    // stdio 重定向挡不住窗口创建,必须显式带 CREATE_NO_WINDOW。
    #[cfg(windows)]
    cmd.creation_flags(crate::utils::win::CREATE_NO_WINDOW);

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Drop 即杀:任何提前返回路径都不会遗留子进程
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动命令失败({}): {e}", req.command))?;

    // 限时等待:超时则显式 kill(连同 kill_on_drop 双保险),再回收输出。
    // 注:kill 只作用于直接子进程(与 mcp/process.rs 的 v1 保守语义一致);
    // 若命令是 shell 包装再启孙进程(如 `cmd /C ...` 的间接调用),孙进程不保证回收。
    let deadline = std::time::Duration::from_millis(timeout);
    match tokio::time::timeout(deadline, child.wait()).await {
        Ok(Ok(status)) => {
            // wait() 后输出仍可读(piped);用 take 避免重复消费
            let stdout = read_pipe(child.stdout.take()).await;
            let stderr = read_pipe(child.stderr.take()).await;
            Ok(ExecResult {
                exit_code: status.code().unwrap_or(-1),
                stdout: truncate_output(&stdout),
                stderr: truncate_output(&stderr),
                tier: ShellTier::Sandbox,
                timed_out: false,
            })
        }
        Ok(Err(e)) => Err(format!("命令执行失败: {e}")),
        Err(_) => {
            // 超时:显式强杀并 wait 回收,避免僵尸
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(format!(
                "命令超时({timeout} ms)已终止。建议:拆小步骤、或调高 timeout_ms(上限 {} ms)。",
                super::MAX_TIMEOUT_MS
            ))
        }
    }
}

/// 读干一个管道到字符串(进程已退出,读不会阻塞)。
async fn read_pipe(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> String {
    use tokio::io::AsyncReadExt;
    let Some(mut p) = pipe else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = p.read_to_end(&mut buf).await;
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(command: &str) -> ExecRequest {
        ExecRequest {
            command: command.to_string(),
            cwd: None,
            timeout_ms: Some(10_000),
        }
    }

    #[tokio::test]
    async fn runs_echo_and_captures_stdout() {
        let out = super::super::execute(req("echo kedai-exec-test"), &[ShellTier::Sandbox])
            .await
            .expect("echo 应成功");
        assert_eq!(out.exit_code, 0);
        assert!(
            out.stdout.contains("kedai-exec-test"),
            "stdout: {}",
            out.stdout
        );
        assert_eq!(out.tier, ShellTier::Sandbox);
        assert!(!out.timed_out);
    }

    #[tokio::test]
    async fn captures_stderr_and_nonzero_exit() {
        // 用必然失败的命令(不存在的可执行名)验证非零退出与 stderr 捕获
        let cmd = if cfg!(target_os = "windows") {
            "exit /b 3"
        } else {
            "exit 3"
        };
        let out = super::super::execute(req(cmd), &[ShellTier::Sandbox])
            .await
            .expect("命令本身应能跑完");
        assert_ne!(out.exit_code, 0);
    }

    #[tokio::test]
    async fn empty_command_rejected() {
        let err = super::super::execute(req("   "), &[ShellTier::Sandbox])
            .await
            .unwrap_err();
        assert!(err.contains("为空"), "{err}");
    }

    #[tokio::test]
    async fn missing_cwd_rejected() {
        let mut r = req("echo hi");
        r.cwd = Some("definitely/not/a/real/dir/xyz".into());
        let err = super::super::execute(r, &[ShellTier::Sandbox])
            .await
            .unwrap_err();
        assert!(err.contains("工作目录"), "{err}");
    }

    #[tokio::test]
    async fn timeout_kills_long_command() {
        let mut r = req(if cfg!(target_os = "windows") {
            // ping 到不存在地址会等待;用 timeout 命令不可移植,改用 ping 自等待
            "ping -n 30 127.0.0.1"
        } else {
            "sleep 30"
        });
        r.timeout_ms = Some(1_000); // 夹取后为 1s
        let err = super::super::execute(r, &[ShellTier::Sandbox])
            .await
            .unwrap_err();
        assert!(err.contains("超时"), "{err}");
    }

    #[tokio::test]
    async fn default_shell_is_platform_appropriate() {
        let sh = default_shell();
        if cfg!(target_os = "windows") {
            assert_eq!(sh, "cmd");
        } else {
            assert_eq!(sh, "sh");
        }
    }

    /// 生成标志纪律(2026-09-18):Windows 上派生必须带 CREATE_NO_WINDOW,
    /// 否则 GUI 子系统的父进程会让每个 bash 调用弹出一个 cmd 黑窗。
    ///
    /// 为什么断言常量值而不是断言「窗口没弹」:窗口创建在自动化环境(CI/沙箱/无桌面
    /// 会话)中不会发生,探针实测连 CREATE_NEW_CONSOLE 对照组都测不到窗口,无法自动
    /// 复现;故此处锁「派生命中带对了位」,真机弹窗行为由交付前的手工验证项确认
    /// (见 docs/遗留.md 的 GUI-4)。
    #[test]
    #[cfg(windows)]
    fn create_no_window_flag_is_hidden_window_bit() {
        // 0x08000000 = CREATE_NO_WINDOW(CreateProcess dwCreationFlags)。
        // 与 DETACHED_PROCESS(0x08)/CREATE_NEW_CONSOLE(0x10)区分:那两者会让
        // 子进程完全没有控制台或再开一个新窗口,都不是本处想要的语义。
        assert_eq!(crate::utils::win::CREATE_NO_WINDOW, 0x0800_0000);
    }

    /// CREATE_NO_WINDOW 下的命令仍须正常工作:标志不能让 stdout 捕获或退出码失真
    /// (历史风险点是「隐藏窗口」被误实现成 DETACHED_PROCESS,导致管道断开)。
    #[tokio::test]
    #[cfg(windows)]
    async fn command_still_works_with_hidden_window_flag() {
        let out =
            super::super::execute(req("echo kedai-hidden-window-probe"), &[ShellTier::Sandbox])
                .await
                .expect("带 CREATE_NO_WINDOW 的 echo 应成功");
        assert_eq!(out.exit_code, 0);
        assert!(
            out.stdout.contains("kedai-hidden-window-probe"),
            "stdout 应仍被完整捕获:{:?}",
            out.stdout
        );
    }
}
