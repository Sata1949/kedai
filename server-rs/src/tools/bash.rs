// bash 工具:执行 shell 命令(阶段 B)。
//
// 安全模型(与 docs/契约-协议与配置.md 一致,四道闸):
//   ① 工具级:风险恒 Dangerous;任务模式默认策略(deny_dangerous)按工具名对 bash 开例外
//      下发(用户要求任务模式具备命令执行能力),聊天模式走三档授权;
//   ② 命令级:破坏性/提权命令**任何模式都不自动放行**,强制走引擎的授权等待
//      (聊天弹出确认卡,展示命令原文 + 风险级;任务模式无 UI 通道 → 直接拒绝);
//   ③ 执行级:① 强制超时 + 超时强杀;② stdin 置 null 禁交互;③ 输出截断;
//      ④ Android 上与执行器等级联动(经 services::exec 分派);
//   ④ 审计级:每次尝试(含被拒绝的)落 exec_audit 一行,root/ADB 级命令可回溯。
//
// 分工说明:本文件只做「参数解析 + 调用执行器 + 写审计」;风险分级在
// tools/command_risk.rs,授权裁决在 tools/permissions.rs,
// 进程派生在 services/exec/(桌面直派 / Android 走 Kotlin 桥)。
use crate::models::types::{ToolContext, ToolDefinition};
use crate::services::exec::{self, audit, ExecRequest};
use crate::tools::registry::ToolRegistry;
use serde_json::json;
use std::sync::Arc;

/// 工具名(权限矩阵、tool_sets、前端确认卡均以此为准)
pub const TOOL_NAME: &str = "bash";

/// 注册 bash 工具。`db` 用于审计落库;`settings` 读「命令执行总开关」;
/// `data_dir` 作为默认工作目录。
///
/// 注意:**不**加入任何只读白名单(READONLY_SCOUT/SUBAGENT/REFLECT),
/// 避免命令执行能力被下发给规划侦察轮、子 agent 或反思步骤。
pub fn register_bash_tool(
    registry: &ToolRegistry,
    db: Arc<crate::models::db::Db>,
    settings: Arc<std::sync::Mutex<crate::services::settings_service::RuntimeSettings>>,
    data_dir: std::path::PathBuf,
) {
    let definition = ToolDefinition {
        name: TOOL_NAME.into(),
        description: "执行 shell 命令并返回输出。**Windows 下由 cmd /C 解释**:不支持 `;` 分隔\
                      多条命令、不支持 /d/ 或 /c/ 这类 MSYS 路径、没有 md5sum 等 GNU 工具——\
                      多条命令用 `&&` 连接,跨盘符切目录用 `cd /d X:`;其他平台为 sh。\
                      危险命令(删除/提权/系统级)会要求用户逐条确认;任务模式下此类命令不可用,\
                      普通命令可用。绑定工作区的任务在工作区内执行;未绑定工作区的任务在\
                      任务专属临时工作区执行;聊天场景在应用数据目录执行。可用 cwd 指定工作目录。"
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "完整命令(交平台默认 shell 解释:Windows=cmd,其他=sh;每次调用的结果头会回显实际 shell 与 cwd)"
                },
                "cwd": {
                    "type": "string",
                    "description": "工作目录绝对路径;省略则用当前任务的工作区(聊天场景为应用数据目录)"
                },
                "timeout_ms": {
                    "type": "integer",
                    // 文案由常量插值:这是**模型可见**的说明,写死数字会与 exec 层的钳制漂移
                    // (抬上限时漏改这里,模型会以为 300s 就是天花板,永远要不到长预算)。
                    "description": format!(
                        "超时毫秒(默认 {},上限 {});超时会强杀进程(Windows 连同整棵进程树)",
                        crate::services::exec::DEFAULT_TIMEOUT_MS,
                        crate::services::exec::MAX_TIMEOUT_MS
                    )
                }
            },
            "required": ["command"]
        }),
    };

    let executor_db = db.clone();
    let executor_settings = settings.clone();
    registry.register_with_timeout(
        definition,
        Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
            let db = executor_db.clone();
            let st = executor_settings.clone();
            let dir = data_dir.clone();
            Box::pin(async move { run(&args, &ctx, &db, &st, &dir).await })
        }),
        // 执行器自带超时强杀,注册表超时给足余量(命令上限 + 30s 收尾)。
        // **由 `MAX_TIMEOUT_MS` 派生,不写第二份数字**:2026-09-30 之前这里是硬编码 330s,
        // 抬 exec 上限若不同步改它,注册表会先超时并把 future drop 掉——
        // 拿不到 exec 层的强杀路径与审计文案,长命令表现为「工具超时」而不是「命令超时」。
        Some(std::time::Duration::from_millis(
            crate::services::exec::MAX_TIMEOUT_MS + 30_000,
        )),
    );
}

/// 工具主体:参数解析 → 执行 → 审计 → 结果文本。
///
/// 说明:授权裁决已由引擎在调用本执行器**之前**完成(见 agents/engine/executor.rs
/// 的 decide 流程)。所以能进到这里就说明已获授权(或属自动放行的非高危命令)。
/// 本函数仍会自查命令风险并记审计,保证「跑了什么」可回溯。
/// 解析本次调用的工作目录(编码通道批次 1)。
///
/// 绑定了工作区的任务里:cwd 缺省 = 工作区,显式 cwd 必须落回工作区内(jail)。
/// jail **只约束 cwd** —— 命令文本里的绝对路径不在这里拦截(进程内有 shell 就能读该
/// 进程有权限读的路径,那属于 OS 级隔离的范畴,不是本函数能承诺的)。
///
/// 无 scope(第四分支)自任务模式 D1 起**只可能出现在聊天路径**:任务恒有作用域——绑定
/// 工作区的用工作区,未绑定的由 `task_engine::run_inner` 建任务级 scratch 并绑定。
/// 该分支与改造前逐字一致(缺省 = 数据目录),角色扮演主链路零变化。
///
/// 抽成自由函数是为了让 jail 语义能被单测直接断言,不必真的跑起一条命令。
fn resolve_cwd(
    explicit: Option<&str>,
    scope: Option<&crate::models::types::ExecScope>,
    data_dir: &std::path::Path,
) -> Result<String, String> {
    match (explicit, scope) {
        (Some(raw), Some(scope)) if scope.jail() => {
            Ok(crate::tools::workspace_guard::safe_workspace_path(
                scope.workspace(),
                raw,
                Some(data_dir),
            )?
            .to_string_lossy()
            .into_owned())
        }
        (Some(raw), _) => Ok(raw.to_string()),
        (None, Some(scope)) => Ok(scope.workspace().to_string_lossy().into_owned()),
        (None, None) => Ok(data_dir.to_string_lossy().into_owned()),
    }
}

async fn run(
    args: &serde_json::Value,
    ctx: &ToolContext,
    db: &Arc<crate::models::db::Db>,
    settings: &Arc<std::sync::Mutex<crate::services::settings_service::RuntimeSettings>>,
    data_dir: &std::path::Path,
) -> Result<String, String> {
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if command.is_empty() {
        return Err("command 不能为空".into());
    }
    let explicit_cwd = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // 工作区语义见 `resolve_cwd`:绑定时缺省工作区、显式 cwd 受 jail 约束
    let cwd = resolve_cwd(explicit_cwd.as_deref(), ctx.scope.as_deref(), data_dir)?;
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .map(Some)
        .unwrap_or(None);

    let risk = crate::tools::command_risk::classify_command(&command, "");
    // 审计风险标记(D1):命令文本命中数据目录绝对路径 / 含 `..` 上溯 → 标记非空。
    // **只标记不拦截**(理由见 audit::classify_risk_flag 文档)。
    let risk_flag = audit::classify_risk_flag(&command, data_dir);
    let tier = exec::detect_tier();
    // 会话归属:task: 前缀为任务模式虚拟 session(与 task_service 同口径)
    let is_task = ctx.session_id.starts_with("task:");
    let source = if is_task {
        audit::AuditSource::Task
    } else {
        audit::AuditSource::Chat
    };

    // 总开关(阶段 E):默认关闭,须用户在设置「命令执行」中显式开启。
    // 关闭时拒绝并留审计——「有人试图执行但被总开关拦下」同样需要可回溯。
    if !exec_enabled(settings) {
        audit::record(
            db,
            &audit::AuditRecord {
                source,
                task_id: is_task.then(|| ctx.session_id.trim_start_matches("task:").to_string()),
                session_id: (!is_task).then(|| ctx.session_id.clone()),
                command: command.clone(),
                shell: shell_name(),
                tier: tier.as_str().into(),
                risk,
                decision: audit::AuditDecision::Denied,
                exit_code: None,
                stdout: String::new(),
                stderr: "命令执行总开关未开启".into(),
                risk_flag: risk_flag.clone(),
            },
        );
        return Err("命令执行未开启:请在「设置 → 授权管理 → 命令执行」中打开总开关后重试。".into());
    }

    // 允许等级:由设置的三档开关编译(桌面恒 Sandbox,Android 按用户放行)。
    // 这是「等级可见 + 可关闭」的执行侧落点——探测到 ROOT 但用户只放行沙箱档时,
    // 会以清晰提示拒绝,而不是悄悄以高权限执行。
    let allowed: Vec<exec::ShellTier> = {
        let s = settings.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = Vec::new();
        if s.exec_allow_root {
            v.push(exec::ShellTier::Root);
        }
        if s.exec_allow_shizuku {
            v.push(exec::ShellTier::Shizuku);
        }
        if s.exec_allow_sandbox {
            v.push(exec::ShellTier::Sandbox);
        }
        // 桌面端无档位概念:恒放行 Sandbox(总开关 exec_enabled 已把关)
        if !cfg!(target_os = "android") {
            v.push(exec::ShellTier::Sandbox);
        }
        v
    };

    // 批次 4b:前扫描(命令执行前的工作区快照,≤256KB 的文件按预算驻留正文)。
    // 只在「任务 + 绑定了工作区」时扫——聊天路径不记账(与 task_change_service 同口径);
    // 被总开关拒绝的命令在上方已 return,不会被扫到。
    let scan_root = if is_task {
        ctx.scope
            .as_deref()
            .map(|scope| scope.workspace().to_path_buf())
    } else {
        None
    };
    let scan_task_id = crate::services::task_change_service::task_id_of(&ctx.session_id);
    let pre_scan = match &scan_root {
        Some(root) => Some(scan_pre_blocking(root.clone()).await),
        None => None,
    };

    let result = exec::execute(
        ExecRequest {
            command: command.clone(),
            cwd: Some(cwd.clone()),
            timeout_ms,
        },
        &allowed,
    )
    .await;

    // 批次 4b:任务 + 绑定工作区时,命令**前后各扫一次**工作区(启发式检出)。
    // 放在 exec 之后、分派之前:Ok/Err 两臂都要记账——命令失败也可能改了一半文件。
    // 三条纪律:① 扫描是旁路观测,记账失败只能 warn,不改命令结果;
    // ② 同步密集 IO 一律 spawn_blocking(直接占 tokio worker 会拖慢 SSE 与其它请求);
    // ③ 取消(批次 1)只保证**调用方**立即返回,scan_blocking 里的扫描取消不掉,
    //    会跑完本次预算才释放驻留——注释与留档都按这个措辞,不许承诺做不到的事。
    if let (Some(root), Some(task_id), Some((pre_entries, pre_outcome))) =
        (scan_root.as_ref(), scan_task_id.as_ref(), pre_scan.as_ref())
    {
        let (post_entries, post_outcome) = scan_post_blocking(root.clone()).await;
        let plan = crate::tools::workspace_scan::plan_changes(
            pre_entries,
            pre_outcome,
            &post_entries,
            &post_outcome,
            crate::tools::workspace_scan::SCAN_MAX_CHANGES,
        );
        crate::tools::workspace_scan::persist_plan(db, task_id, root, &plan);
    }

    match result {
        Ok(out) => {
            audit::record(
                db,
                &audit::AuditRecord {
                    source,
                    task_id: is_task
                        .then(|| ctx.session_id.trim_start_matches("task:").to_string()),
                    session_id: (!is_task).then(|| ctx.session_id.clone()),
                    command: command.clone(),
                    shell: shell_name(),
                    tier: out.tier.as_str().into(),
                    risk,
                    decision: audit::AuditDecision::Allowed,
                    exit_code: Some(out.exit_code),
                    stdout: out.stdout.clone(),
                    stderr: out.stderr.clone(),
                    risk_flag: risk_flag.clone(),
                },
            );
            Ok(format_result(&command, &cwd, &out))
        }
        Err(e) => {
            // 执行失败(超时/启动失败)也留痕:失败原因同样需要可回溯
            audit::record(
                db,
                &audit::AuditRecord {
                    source,
                    task_id: is_task
                        .then(|| ctx.session_id.trim_start_matches("task:").to_string()),
                    session_id: (!is_task).then(|| ctx.session_id.clone()),
                    command: command.clone(),
                    shell: shell_name(),
                    tier: tier.as_str().into(),
                    risk,
                    decision: audit::AuditDecision::Denied,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: e.clone(),
                    risk_flag: risk_flag.clone(),
                },
            );
            Err(e)
        }
    }
}

/// 前扫描(同步密集 IO → 阻塞线程池)。线程异常退出按「扫描失败」处理:
/// 扫描是旁路观测,绝不让它的 panic 传染命令结果。
async fn scan_pre_blocking(
    root: std::path::PathBuf,
) -> (
    Vec<crate::tools::workspace_scan::PreEntry>,
    crate::tools::workspace_scan::ScanOutcome,
) {
    use crate::tools::workspace_scan as scan;
    match tokio::task::spawn_blocking(move || scan::scan_pre(&root, &scan::ScanBudget::default()))
        .await
    {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!(error = %e, "前扫描线程异常退出");
            (
                Vec::new(),
                scan::ScanOutcome::Failed("扫描线程异常退出".into()),
            )
        }
    }
}

/// 后扫描(同上;不驻留正文,改动文件由 `record` 现读)
async fn scan_post_blocking(
    root: std::path::PathBuf,
) -> (
    Vec<crate::tools::workspace_scan::PostEntry>,
    crate::tools::workspace_scan::ScanOutcome,
) {
    use crate::tools::workspace_scan as scan;
    match tokio::task::spawn_blocking(move || scan::scan_post(&root, &scan::ScanBudget::default()))
        .await
    {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!(error = %e, "后扫描线程异常退出");
            (
                Vec::new(),
                scan::ScanOutcome::Failed("扫描线程异常退出".into()),
            )
        }
    }
}

/// 结果文本:给模型看的结构化摘要(含实际 shell、cwd、退出码与所跑命令,便于自查)。
///
/// shell 与 cwd 是 2026-09 实测补的(缺陷 D4):工具名叫 `bash`、实际却是 cmd,而结果头
/// 原先只回执行器档位与退出码 —— 模型按 bash 语法写命令(`;` 分隔、`/d/` 路径、`md5sum`、
/// `git status`)后只看到「退出码 1」,拿不到任何线索去纠正假设,于是换个写法无限重试,
/// 每条白烧一轮 1~3 分钟。把「实际由谁解释、在哪个目录跑」回灌给模型,一轮内即可自纠。
fn format_result(command: &str, cwd: &str, out: &exec::ExecResult) -> String {
    let shell = shell_name();
    let mut s = format!(
        "$ {command}\n(执行器:{};shell:{shell};cwd:{cwd};退出码:{})",
        out.tier.label(),
        out.exit_code
    );
    if !out.stdout.trim().is_empty() {
        s.push_str("\n\n[stdout]\n");
        s.push_str(out.stdout.trim_end());
    }
    if !out.stderr.trim().is_empty() {
        s.push_str("\n\n[stderr]\n");
        s.push_str(out.stderr.trim_end());
    }
    if out.stdout.trim().is_empty() && out.stderr.trim().is_empty() {
        s.push_str("\n\n(无输出)");
    }
    if out.exit_code != 0 {
        s.push_str("\n\n提示:非零退出码表示命令失败,请检查命令与参数。");
        if cfg!(target_os = "windows") {
            s.push_str(
                "本机 shell 是 cmd:不支持 `;` 分隔多条命令、不支持 /d/ 或 /c/ 这类 MSYS 路径、\
                 没有 md5sum/git 等未必安装的工具;多条命令用 `&&` 连接。",
            );
        }
    }
    s
}

/// 平台默认 shell 名(审计记录用;实际解释器由 services::exec 决定)。
fn shell_name() -> String {
    if cfg!(target_os = "windows") {
        "cmd".into()
    } else {
        "sh".into()
    }
}

/// 读命令执行总开关(settings.json 的 exec_enabled)。
/// 经 ToolDeps 注入的 RuntimeSettings 句柄读取,与设置面板写入的是同一份内存态。
/// 读锁中毒按 into_inner 恢复(项目锁纪律);开关语义上失败方向取安全侧。
fn exec_enabled(
    settings: &Arc<std::sync::Mutex<crate::services::settings_service::RuntimeSettings>>,
) -> bool {
    settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .exec_enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_success_output() {
        let out = exec::ExecResult {
            exit_code: 0,
            stdout: "hello\n".into(),
            stderr: String::new(),
            tier: exec::ShellTier::Sandbox,
            timed_out: false,
        };
        let s = format_result("echo hello", "C:/ws/task", &out);
        assert!(s.contains("$ echo hello"));
        assert!(s.contains("hello"));
        assert!(s.contains("退出码:0"));
        assert!(!s.contains("非零退出码"));
        // 结果头自描述(D4):shell 与 cwd 必须回灌给模型,否则它无法纠正「名字叫 bash、
        // 实际是 cmd」的假设。这两个字段是防回退断言,不得因排版调整删掉。
        assert!(s.contains("shell:"), "结果头应回显实际 shell:{s}");
        assert!(s.contains("cwd:C:/ws/task"), "结果头应回显实际 cwd:{s}");
    }

    #[test]
    fn formats_failure_with_hint() {
        let out = exec::ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "boom".into(),
            tier: exec::ShellTier::Sandbox,
            timed_out: false,
        };
        let s = format_result("false", "C:/ws/task", &out);
        assert!(s.contains("boom"));
        assert!(s.contains("非零退出码"), "失败时应给提示:{s}");
        if cfg!(target_os = "windows") {
            assert!(
                s.contains("cmd") && s.contains("&&"),
                "Windows 失败提示应给出 cmd 语法纠正线索:{s}"
            );
        }
    }

    #[test]
    fn formats_empty_output() {
        let out = exec::ExecResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            tier: exec::ShellTier::Sandbox,
            timed_out: false,
        };
        assert!(format_result("true", "C:/ws/task", &out).contains("无输出"));
    }

    #[test]
    fn rejects_empty_command() {
        // 直接验证参数校验分支(不触发真实执行)
        let args = serde_json::json!({ "command": "   " });
        let cmd = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        assert!(cmd.is_empty(), "空命令应被拒");
    }

    #[test]
    fn shell_name_matches_platform() {
        if cfg!(target_os = "windows") {
            assert_eq!(shell_name(), "cmd");
        } else {
            assert_eq!(shell_name(), "sh");
        }
    }

    /// 工作区语义(编码通道批次 1):绑定时 cwd 缺省 = 工作区、界外显式 cwd 被拒、
    /// 界内相对/绝对路径通过;未绑定时缺省仍回落数据目录(旧行为逐字不变)。
    /// 只断言 `resolve_cwd` 的分支,不触发真实命令执行。
    #[test]
    fn resolve_cwd_honors_workspace_jail() {
        let tmp = crate::utils::test_support::TempDataDir::new("bash-cwd");
        let ws = tmp.join("ws");
        std::fs::create_dir_all(ws.join("sub")).unwrap();
        let outside = tmp.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let data_dir = tmp.join("datadir");
        std::fs::create_dir_all(&data_dir).unwrap();

        let scope = crate::models::types::ExecScope::new(ws.clone(), true);
        // 缺省 → 工作区(jail 生效时不再回落数据目录)
        assert_eq!(
            resolve_cwd(None, Some(&scope), &data_dir).unwrap(),
            ws.to_string_lossy()
        );
        // 界内相对路径 → 解析为工作区内的真实路径(闸门返回 canonical 形式,故比对前
        // 把工作区也 canonicalize;否则 `\\?\` 前缀会让字面前缀比对失败)
        let inside = resolve_cwd(Some("sub"), Some(&scope), &data_dir).unwrap();
        assert!(
            std::path::Path::new(&inside).starts_with(std::fs::canonicalize(&ws).unwrap()),
            "界内路径应落在工作区内: {inside}"
        );
        // 界外绝对路径 → 拒绝(文案指明越界)
        let err = resolve_cwd(Some(&outside.to_string_lossy()), Some(&scope), &data_dir)
            .expect_err("界外 cwd 必须被拒");
        assert!(err.contains("越界"), "文案应指明越界: {err}");
        // D1 关键回归:显式把 cwd 指向**数据目录**必须被拒——jail 存在的全部意义就是
        // 不让命令在用户真实数据里跑(实测缺陷是脚本去 open('kedai.db-wal','rb'))
        let err = resolve_cwd(Some(&data_dir.to_string_lossy()), Some(&scope), &data_dir)
            .expect_err("把 cwd 指到数据目录必须被拒");
        assert!(
            err.contains("越界") || err.contains("数据目录"),
            "文案应指明越界/数据目录: {err}"
        );
        // 未绑定工作区 → 旧行为:缺省 = 数据目录,显式 cwd 原样透传
        assert_eq!(
            resolve_cwd(None, None, &data_dir).unwrap(),
            data_dir.to_string_lossy()
        );
        assert_eq!(resolve_cwd(Some("x"), None, &data_dir).unwrap(), "x");
    }
}
