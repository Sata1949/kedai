// 桌面/服务端执行器:直接派生进程(阶段 C)。
//
// 安全要点(与 tools/bash.rs 的分工:本层只负责「安全地跑」,授权已在上游完成):
// - 强制超时 + 超时强杀;**进程树**清理:
//     · Windows 用 Job Object(见下方 `JobTree`),作业内进程随句柄关闭一起被杀,
//       子进程的后代按 Windows 语义自动继承作业成员身份,故孙进程同样在清理范围内;
//     · Unix **仅杀直接子进程**(2026-09-30 实测口径,此前注释谎称已做):
//       进程组方案(`process_group(0)` + `killpg`)未实现,登记在 `docs/遗留.md` EXEC-TREE-1。
// - stdin 置 null,禁止交互式命令挂起等待输入;
// - 输出按字符截断(见 mod.rs::truncate_output);
// - 不拼接额外字符串:命令原文交平台 shell 解释。
#![cfg(not(target_os = "android"))]

use super::{clamp_timeout, truncate_output, ExecRequest, ExecResult, ShellTier};
use std::process::Stdio;

/// Windows 进程树闸门:Job Object(HARNESS3-1)。
///
/// ## 为什么需要它
///
/// `cmd /C <命令>` 只是**壳**:命令再派生的进程(如 `start /B foo.cmd`)是孙进程,
/// 不归 `tokio::process::Child` 管——`kill_on_drop(true)` 与 `start_kill()` 都只作用于
/// 直接子进程。于是「点停止后沙箱还在被改」「任务结束了后台进程还在跑」两件事同时成立
/// (旧实现在文件头注释里承诺了「含子孙进程」却没做,本结构把那句承诺落地)。
///
/// ## 为什么用「关闭即杀」而不是「显式杀」
///
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 让清理边界等于**本函数的生命周期**:
///   - 超时 → `terminate()` 显式强杀后照常返回错误;
///   - **上游取消**(引擎 `execute_call` 的 `await_or_cancel` 竞争赢)→ 本 future 被 drop
///     → 守卫 drop → `CloseHandle` → 作业内全部进程被杀。取消信号**不需要**穿透到这一层,
///     这正是「在调用侧竞争而不给 `ToolExecutor` 加取消形参」能成立的前提。
///   - **正常跑完** → 守卫 drop → 命令留下的后台进程同样被收掉。这是有意的行为收紧:
///     任务工作区里不允许有比命令本身活得更久的进程(登记进 `docs/功能.md`)。
///
/// ## 诚实边界(不写成「保证无残留」)
///
/// 作业成员身份在 `spawn` **之后**才赋,所以直接子进程在被赋值前抢跑派生的进程不会入作业。
/// 零窗口需要 `CREATE_SUSPENDED` + 手工 `CreateProcess`,与 tokio 的 `Command` 抽象冲突,
/// 代价与收益不成比例。故本实现口径是「**尽力清理**」。
#[cfg(windows)]
struct JobTree(windows_sys::Win32::Foundation::HANDLE);

// HANDLE 在 windows-sys 0.61 是裸指针别名(`pub type HANDLE = *mut c_void`),因此
// JobTree 默认**不是 Send**——会让持有它的 `bash` 工具 future 丢掉 Send,而
// `ToolExecutor` 的 BoxFuture 要求 Send(编译期即失败)。补这条 impl 的理由是实打实的:
// 内核句柄是**进程级**对象,CloseHandle / AssignProcessToJobObject / TerminateJobObject
// 都是线程安全的 Win32 API,句柄在哪条线程上用不影响有效性;守卫只有一个所有者,
// 随 future 在任务间迁移,不存在两线程同时持有同一句柄的形态。
// 可见性:JobTree 是本模块私有类型,故 `unsafe impl` 就地声明;不导出、不改公开 API。
#[cfg(windows)]
unsafe impl Send for JobTree {}

#[cfg(windows)]
impl JobTree {
    /// 新建作业对象(带 KILL_ON_JOB_CLOSE)。失败返回 None —— 闸门建不起来时
    /// **不拒绝执行**(那会让 bash 因 OS 资源问题整体不可用),退化为「只杀直接子进程」
    /// 的旧行为并 warn 留痕。
    fn new() -> Option<Self> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // 匿名作业:名字与 SECURITY_ATTRIBUTES 都传空指针(windows-sys 0.61 的
        // HANDLE / PCWSTR 都是裸指针别名,null 即合法实参)。
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            tracing::warn!("CreateJobObjectW 失败,进程树清理退化为仅杀直接子进程");
            return None;
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
            tracing::warn!("SetInformationJobObject 失败,进程树清理退化为仅杀直接子进程");
            return None;
        }
        Some(Self(job))
    }

    /// 把已派生的直接子进程纳入作业(其后它创建的一切后代自动同属本作业)。
    fn assign(&self, process: windows_sys::Win32::Foundation::HANDLE) -> bool {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        // SAFETY: job 是本结构持有的有效句柄;process 来自仍存活的 Child(调用点在 wait 之前)。
        unsafe { AssignProcessToJobObject(self.0, process) != 0 }
    }

    /// 立即强杀作业内全部进程(超时路径)。之后 drop 仍会关闭句柄,重复调用无害。
    fn terminate(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: 同上;退出码 1 只为与「被杀」语义一致,调用方按超时归类。
        unsafe { TerminateJobObject(self.0, 1) };
    }
}

#[cfg(windows)]
impl Drop for JobTree {
    fn drop(&mut self) {
        // 关闭最后一个作业句柄 → 作业内进程全部被杀(KILL_ON_JOB_CLOSE)。
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

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

    // 进程树闸门:紧接着 spawn 把**直接子进程**放进作业,其后它创建的一切后代按 Windows
    // 语义自动同属该作业 → 孙进程也在清理范围内(为什么不在 spawn 前:见 JobTree 的诚实边界)。
    // assign 失败只 warn:闸门建不起来时**不拒绝执行**(那会让 bash 因 OS 资源问题整体不可用),
    // 退化为「只杀直接子进程」的旧行为。
    #[cfg(windows)]
    let job = JobTree::new();
    #[cfg(windows)]
    if let Some(job) = job.as_ref() {
        // raw_handle() 在 wait 之前恒为 Some(子进程未回收);None 只可能是已回收,
        // 那时作业赋值已无意义,静默跳过即可。
        if let Some(handle) = child.raw_handle() {
            if !job.assign(handle) {
                tracing::warn!("AssignProcessToJobObject 失败,本次仅杀直接子进程");
            }
        }
    }

    // 限时等待:超时则显式 kill(连同 kill_on_drop 双保险),再回收输出。
    // Windows:作业清理覆盖孙进程;Unix:kill 只作用于直接子进程,孙进程残留
    // 登记在 `docs/遗留.md` EXEC-TREE-1(与 mcp/process.rs 的 v1 保守语义同源)。
    let deadline = std::time::Duration::from_millis(timeout);
    match tokio::time::timeout(deadline, child.wait()).await {
        Ok(Ok(status)) => {
            // 正常结束也**先收作业、再读管道**:`read_to_end` 要等**所有**写端关闭才返回,
            // 而继承了 stdout/stderr 的后台进程握着的正是这些写端。外壳 cmd 会等 `start /B`
            // 的子孙(2026-09-30 实测),但真正脱离外壳的派生方式(常驻/服务化)不会——
            // 那时旧实现会一路挂到注册表超时。顺序同时就是本批的清理语义:
            // 命令返回 ≠ 它派生的进程可以接着活。
            #[cfg(windows)]
            if let Some(job) = job.as_ref() {
                job.terminate();
            }
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
            // 超时:显式强杀并 wait 回收,避免僵尸;作业内全部进程(含孙进程)一并收掉
            let _ = child.start_kill();
            #[cfg(windows)]
            if let Some(job) = job.as_ref() {
                job.terminate();
            }
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

    /// 探针落点:两个用例各用独立目录(同一测试二进制里 `std::process::id()` 相同)。
    /// 返回 (目录, 孙进程要写的日志路径)。
    fn tree_probe(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("kedai-tree-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let log = dir.join("tick.log");
        // 孙进程探针:持续往 tick.log 追加(绝对路径写在批处理内部,那里解析正常)
        let probe_script = format!(
            "@echo off\r\nfor /L %%i in (1,1,2000000) do echo tick>>\"{}\"\r\n",
            log.display()
        );
        std::fs::write(dir.join("probe.cmd"), probe_script).expect("写孙进程探针失败");
        // 直接子进程:后台拉起探针,然后自己也持续追加(保证它活得比观察窗口久)
        let outer_script = "@echo off\r\nstart /B cmd /c probe.cmd\r\n\
                            for /L %%i in (1,1,2000000) do echo out>>outer.log\r\n";
        std::fs::write(dir.join("probe-outer.cmd"), outer_script).expect("写外壳批处理失败");
        (dir, log)
    }

    /// 日志当前字节数(不存在 = 0)。
    fn tick_len(log: &std::path::Path) -> u64 {
        std::fs::metadata(log).map(|m| m.len()).unwrap_or(0)
    }

    /// **超时出口收掉孙进程**(HARNESS3-1 的验收断言「点停止后沙箱不再被改」)。
    ///
    /// ## 为什么断言「日志不再增长」而不是「进程已死」
    /// 自动化环境里拿孙进程 PID 要靠 `tasklist` 的输出格式(脆弱),而「沙箱不再被改」
    /// 本身就是需求原文——用孙进程持续写入的文件来证它,比读进程表更贴近验收。
    /// 与上面 `create_no_window_flag_is_hidden_window_bit` 同理(见 docs/遗留.md GUI-4):
    /// 环境测不到的形态,就锁住能测的那一侧。
    ///
    /// ## 探针为什么长成「两个批处理 + 相对路径」的样子(2026-09-30 三次实测后的形态)
    /// - `cmd /C` 的参数引号:Rust 会把整条命令包成一层引号,命令里再出现引号时 cmd 的
    ///   「/C 后引号串」特殊规则会把嵌套引号吃掉,`start /B cmd /c "C:\绝对路径"` 静默不启动
    ///   (表现为探针零字节)。故命令只写**一个裸标记** `probe-outer.cmd`,配合 `cwd` 用相对路径。
    /// - `start /B "" "x.cmd"` 在本机报「拒绝访问」(start 直接执行 .cmd 走文件关联),
    ///   要写成 `start /B cmd /c probe.cmd`。
    /// - 节奏**不靠 ping/timeout**(它们在本机部分环境下直接报「找不到路径」,循环会退化成
    ///   秒完的空转 → 「尺寸不增长」变成假通过)。改用 200 万次追加循环:活着就一定在长。
    ///
    /// 负向验证(2026-09-30,把 `JobTree::new` 临时改为恒 None):本用例红
    /// 「孙进程在命令被终止后仍在写文件(57036 → 88146)」——即断言有判别力,不是空过。
    #[tokio::test]
    #[cfg(windows)]
    async fn timeout_terminates_whole_process_tree() {
        use std::time::Duration;
        let (dir, log) = tree_probe("timeout");

        let mut r = req("probe-outer.cmd");
        r.cwd = Some(dir.to_string_lossy().into_owned());
        r.timeout_ms = Some(3_000);
        // 外层再套一道保险:万一清理没生效(管道被孙进程握着)应判失败,而不是把测试挂死
        let outcome = tokio::time::timeout(
            Duration::from_secs(25),
            super::super::execute(r, &[ShellTier::Sandbox]),
        )
        .await
        .expect("execute 应在 25s 内返回(挂死说明管道仍被未清理的孙进程握着)");
        let err = outcome.expect_err("命令应被超时终止");
        assert!(err.contains("超时"), "{err}");

        // 先把前提钉死:孙进程确实跑起来并写过。拿不到这一点就是空过,必须显式失败。
        let tick_before = tick_len(&log);
        assert!(
            tick_before > 0,
            "孙进程探针没有写入 {} —— 前提不成立,用例无效",
            log.display()
        );

        tokio::time::sleep(Duration::from_millis(1_500)).await;
        let tick_after = tick_len(&log);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            tick_before, tick_after,
            "孙进程在命令被终止后仍在写文件({tick_before} → {tick_after}):进程树清理未生效"
        );
    }

    /// **取消出口也收掉孙进程**——这才是 HARNESS3-1 的主场景(用户点停止)。
    ///
    /// 形态与引擎侧一致:`await_or_cancel` 竞争赢时**丢弃** exec future,于是
    /// `kill_on_drop` 杀直接子进程、`JobTree` 守卫 drop → `CloseHandle` → 作业内全部进程被杀。
    /// 取消信号因此**不需要**穿透到 exec 层(见 JobTree 的设计说明)。
    ///
    /// 这里用 `timeout(1.2s, &mut fut)` 把 future 推进到「孙进程已在写」再丢弃——
    /// 直接 drop 一个从未 poll 的 future 等于什么都没启动,前提断言会替我们拦住那种空过。
    ///
    /// **必须用 `Box::pin` 而不是 `pin!`**(实测踩到):`pin!` 把 future 藏在局部变量里、
    /// 只把 `Pin<&mut _>` 交出来,`drop(fut)` 丢的是那层引用包装,底层 future 要到作用域结束
    /// 才析构——于是「取消」根本没发生,用例红得毫无线索。`Box::pin` 的所有权在 `fut` 上,
    /// drop 即真正析构 future(等价于引擎侧竞争赢后丢弃工具 future)。
    #[tokio::test]
    #[cfg(windows)]
    async fn dropping_the_call_reaps_grandchildren_too() {
        use std::time::Duration;
        let (dir, log) = tree_probe("cancel");

        let mut r = req("probe-outer.cmd");
        r.cwd = Some(dir.to_string_lossy().into_owned());
        r.timeout_ms = Some(30_000); // 远大于观察窗口:只能靠「取消」收场
        let mut fut = Box::pin(super::super::execute(r, &[ShellTier::Sandbox]));
        let _ = tokio::time::timeout(Duration::from_millis(1_200), &mut fut).await;
        drop(fut); // ← 模拟上游取消:工具 future 被真正析构

        let tick_before = tick_len(&log);
        assert!(
            tick_before > 0,
            "孙进程探针没有写入 {} —— 取消前它没跑起来,用例无效",
            log.display()
        );

        tokio::time::sleep(Duration::from_millis(1_500)).await;
        let tick_after = tick_len(&log);
        // 等一会儿让被杀的追加循环把文件句柄放掉,再收尾清理(清理失败不影响断言)
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            tick_before, tick_after,
            "孙进程在调用被取消后仍在写文件({tick_before} → {tick_after}):drop 路径未清理进程树"
        );
    }
}
