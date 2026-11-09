//! 桌面壳与图形启动器共用的进程所有权边界（仅 std，无第三方依赖）。
//! TCP 端点确定候选；完整镜像路径确定安装归属；已打开的句柄确定进程身份。

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// 只取能够服务目标地址的 TCP 监听者。监听行的远端端口为 0，不依赖本地化状态文案。
pub fn parse_netstat_listeners(text: &str, target: SocketAddr) -> Vec<u32> {
    let mut pids = Vec::new();
    for line in text.lines() {
        let columns: Vec<_> = line.split_whitespace().collect();
        if columns.len() != 5 || !matches!(columns[0], "TCP" | "TCPv6") {
            continue;
        }
        let (Ok(local), Ok(remote)) = (
            columns[1].parse::<SocketAddr>(),
            columns[2].parse::<SocketAddr>(),
        ) else {
            continue;
        };
        let address_matches = local.ip() == target.ip()
            || (local.ip().is_unspecified() && local.is_ipv4() == target.is_ipv4());
        if local.port() != target.port() || !address_matches || remote.port() != 0 {
            continue;
        }
        if let Ok(pid) = columns[4].parse::<u32>() {
            if !pids.contains(&pid) {
                pids.push(pid);
            }
        }
    }
    pids
}

pub fn listening_pids(target: SocketAddr) -> io::Result<Vec<u32>> {
    let mut command = std::process::Command::new("netstat");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.arg("-ano").output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            "读取 TCP 监听进程失败：netstat 未成功退出",
        ));
    }
    Ok(parse_netstat_listeners(
        &String::from_utf8_lossy(&output.stdout),
        target,
    ))
}

pub fn listener_owned_by(target: SocketAddr, pid: u32) -> io::Result<bool> {
    Ok(listening_pids(target)? == [pid])
}

/// 路径先由文件系统规范化；比较时仅消除 Windows 的大小写与扩展路径前缀差异。
pub fn same_path(left: &Path, right: &Path) -> io::Result<bool> {
    let left = left.canonicalize()?;
    let right = right.canonicalize()?;
    #[cfg(windows)]
    {
        Ok(left
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy()))
    }
    #[cfg(not(windows))]
    {
        Ok(left == right)
    }
}

/// 可信范围只来自当前 exe 的安装位置，不采信进程名、响应体或工作目录。
pub fn trusted_images(executable: &Path) -> Vec<PathBuf> {
    let mut images = vec![executable.to_path_buf()];
    if let Some(directory) = executable.parent() {
        images.push(directory.join("kedai-server.exe"));
    }
    if let Some(root) = executable
        .ancestors()
        .find(|path| path.join("web").is_dir() && path.join("server-rs").is_dir())
    {
        for relative in [
            "dist/Kedai-portable/Kedai.exe",
            "dist/kedai-server.exe",
            "server-rs/target/debug/kedai-server.exe",
            "server-rs/target/release/kedai-server.exe",
            "src-tauri/target/debug/kedai-desktop.exe",
            "src-tauri/target/release/kedai-desktop.exe",
            "src-tauri/target/debug/kedai-portable.exe",
            "src-tauri/target/release/kedai-portable.exe",
            "src-tauri/target/release/kedai-server.exe",
        ] {
            images.push(root.join(relative));
        }
    }
    images
}

pub fn trusted_image(image: &Path, allowed: &[PathBuf]) -> bool {
    allowed
        .iter()
        .any(|path| same_path(image, path).unwrap_or(false))
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn QueryFullProcessImageNameW(
            handle: *mut c_void,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn TerminateProcess(handle: *mut c_void, exit_code: u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    /// 持有内核句柄直到完成核验与终止，后续不再按 PID 重新打开目标。
    pub struct VerifiedProcess {
        handle: *mut c_void,
        pub pid: u32,
    }

    // SAFETY:句柄只由本对象关闭；Windows 进程句柄允许跨线程使用，方法均借用 self。
    unsafe impl Send for VerifiedProcess {}

    impl Drop for VerifiedProcess {
        fn drop(&mut self) {
            // SAFETY:handle 由成功的 OpenProcess 创建且只在此处关闭一次。
            unsafe { CloseHandle(self.handle) };
        }
    }

    impl VerifiedProcess {
        pub fn open(pid: u32, allowed: &[PathBuf]) -> io::Result<Self> {
            // PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE。
            // SAFETY:纯标量参数；返回的非空进程句柄交由本对象持有。
            let handle = unsafe { OpenProcess(0x1000 | 0x0001, 0, pid) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let process = Self { handle, pid };
            let mut name = vec![0u16; 32768];
            let mut size = name.len() as u32;
            // SAFETY:name 提供 size 个可写 UTF-16 单元，handle 在调用期间有效。
            if unsafe { QueryFullProcessImageNameW(handle, 0, name.as_mut_ptr(), &mut size) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let image = PathBuf::from(std::ffi::OsString::from_wide(&name[..size as usize]));
            if !trusted_image(&image, allowed) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("拒绝接管非本安装目录的进程：{}", image.display()),
                ));
            }
            Ok(process)
        }

        pub fn terminate(self) -> io::Result<()> {
            // SAFETY:终止经过完整路径核验且句柄仍被持有的同一个进程；不再按 PID 寻址。
            if unsafe { TerminateProcess(self.handle, 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
pub use windows::VerifiedProcess;

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint() -> SocketAddr {
        "127.0.0.1:3001".parse().unwrap()
    }

    #[test]
    fn tcp_ownership_excludes_udp_other_addresses_and_connections() {
        let rows = [
            "UDP 127.0.0.1:3001 *:* 111",
            "TCP 192.168.1.2:3001 0.0.0.0:0 LISTENING 222",
            "TCP 127.0.0.1:3001 127.0.0.1:40000 ESTABLISHED 333",
            "TCP 127.0.0.1:13001 0.0.0.0:0 LISTENING 444",
            "TCP 127.0.0.1:3001 0.0.0.0:0 侦听 555",
        ]
        .join("\n");
        assert_eq!(parse_netstat_listeners(&rows, endpoint()), vec![555]);
    }

    #[test]
    fn tcp_ownership_accepts_wildcard_and_deduplicates_pid() {
        let rows =
            "TCP 0.0.0.0:3001 0.0.0.0:0 LISTENING 555\nTCP 127.0.0.1:3001 0.0.0.0:0 侦听 555";
        assert_eq!(parse_netstat_listeners(rows, endpoint()), vec![555]);
    }

    #[test]
    fn tcp_ownership_keeps_ipv6_separate_from_ipv4() {
        let rows =
            "TCPv6 [::1]:3001 [::]:0 LISTENING 666\nTCP 127.0.0.1:3001 0.0.0.0:0 LISTENING 555";
        assert_eq!(
            parse_netstat_listeners(rows, "[::1]:3001".parse().unwrap()),
            vec![666]
        );
    }

    #[test]
    fn tcp_ownership_ignores_malformed_input() {
        assert!(parse_netstat_listeners("活动连接\nTCP 127.0.0.1:3001", endpoint()).is_empty());
    }

    #[test]
    fn trusted_identity_requires_the_full_existing_image_path() {
        let executable = std::env::current_exe().unwrap();
        assert!(trusted_image(
            &executable,
            std::slice::from_ref(&executable)
        ));
        let foreign = executable
            .parent()
            .unwrap()
            .join("other-install")
            .join(executable.file_name().unwrap());
        assert!(!trusted_image(&executable, &[foreign]));
    }

    #[test]
    fn data_directory_identity_is_canonical() {
        let directory = std::env::current_dir().unwrap();
        assert!(same_path(&directory, &directory.join(".")).unwrap());
        assert!(!same_path(&directory, directory.parent().unwrap()).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn kernel_handle_verifies_current_process_without_terminating_it() {
        let executable = std::env::current_exe().unwrap();
        let verified = VerifiedProcess::open(std::process::id(), &[executable]).unwrap();
        assert_eq!(verified.pid, std::process::id());
        drop(verified);
        assert!(VerifiedProcess::open(std::process::id(), &[]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn verified_handle_terminates_only_its_owned_child_after_full_path_check() {
        use std::os::windows::process::CommandExt;
        use std::process::{Child, Command, Stdio};

        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                // 关闭输入管道也会让 set /p 退出；断言失败时仍回收本测试创建的进程。
                drop(self.0.stdin.take());
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let image = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("cmd.exe");
        let spawn_owned = || {
            OwnedChild(
                Command::new(&image)
                    // set /p 是 cmd 内置命令，只等待本测试持有的输入管道，不创建孙进程。
                    .args(["/D", "/Q", "/C", "set", "/p", "KEDAI_TEST_HOLD="])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(0x0800_0000)
                    .spawn()
                    .unwrap(),
            )
        };
        let mut target = spawn_owned();
        let mut control = spawn_owned();
        assert!(target.0.try_wait().unwrap().is_none());
        assert!(control.0.try_wait().unwrap().is_none());

        // 文件名仍为 cmd.exe，只有完整安装路径不同；不得仅凭 basename 放行。
        let wrong_image = image
            .parent()
            .unwrap()
            .join("other-install")
            .join("cmd.exe");
        let rejected = VerifiedProcess::open(target.0.id(), &[wrong_image]);
        assert!(matches!(rejected, Err(error) if error.kind() == io::ErrorKind::PermissionDenied));
        assert!(target.0.try_wait().unwrap().is_none());
        assert!(control.0.try_wait().unwrap().is_none());

        let verified = VerifiedProcess::open(target.0.id(), &[image]).unwrap();
        assert_eq!(verified.pid, target.0.id());
        verified.terminate().unwrap();
        assert!(target.0.wait().unwrap().success());
        assert!(control.0.try_wait().unwrap().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn live_tcp_ownership_matches_only_its_actual_process() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        assert!(listener_owned_by(endpoint, std::process::id()).unwrap());
        assert!(!listener_owned_by(endpoint, std::process::id() + 1).unwrap());
    }
}
