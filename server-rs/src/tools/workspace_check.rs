// 任务「工作区内」档的 bash 命令边界检查（2026-11-06 三档授权）。
//
// ## 它是什么 / 不是什么
//
// 这是**产品边界**，不是 OS 级沙箱：Windows 上没有成熟的工作区隔离原语，本模块只做
// 「能静态证明在工作区内就放行，越界或无法证明就转逐步确认」。它不承诺拦住所有
// 越界写入——`node script.js`、`cargo test` 等会写用户缓存的命令仍会放行；这些是
// 明确的已知边界，写在 `docs/契约-协议与配置.md` 与 `docs/遗留.md`。
//
// ## 判定顺序
//
// 1. cwd 不在工作区内 → `Outside`（cwd 本身由 bash 的 jail 先判，这里再兜一层）；
// 2. 命令含内联解释器（`node -e` / `python -c` / `powershell -Command` / `cmd /c`…）
//    → `Unknown`（脚本内的写 API 无法静态解析，fail-closed 转确认）；
// 3. `extract_write_targets` 抽出的写目标：含变量/通配符/命令替换 → `Unknown`；
//    可解析但落在工作区外 → `Outside`；全部在工作区内 → 继续；
// 4. 命令中的绝对路径（盘符 / UNC / 前导 `/` / `~`）：落在工作区外 → `Outside`；
// 5. `cd` / `pushd` 的 `..` 或绝对路径逃逸 → `Outside`；
// 6. 其余 → `Inside`。
//
// 判定为 `Outside`/`Unknown` 时，权限层返回 `workspace_escape` 的「需确认」决策，
// 由任务逐步确认通道弹卡；用户确认后照常执行（这是有意保留的逃生口）。
use crate::tools::shell_write_targets::extract_write_targets;
use std::path::{Component, Path, PathBuf};

/// 命令相对工作区的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceVerdict {
    /// 可静态证明命令的路径副作用都在工作区内。
    Inside,
    /// 明确越界（cwd/写目标/绝对路径/`cd` 逃逸）。
    Outside(String),
    /// 无法静态证明（变量/通配符/内联解释器），按产品契约转确认。
    Unknown(String),
}

impl WorkspaceVerdict {
    /// 面向确认卡的中文理由。
    pub fn reason(&self) -> String {
        match self {
            Self::Inside => "工作区内命令自动放行".into(),
            Self::Outside(why) => format!("命令超出工作区:{why}"),
            Self::Unknown(why) => format!("无法确认命令只在工作区内执行:{why}"),
        }
    }
}

/// 判定一条 bash 命令是否可视为「工作区内」。
///
/// `cwd` 为模型显式给出的 cwd（相对/绝对）；None/空 = 任务工作区缺省。
/// `workspace` 为任务冻结的工作区绝对路径（`ExecScope::workspace`）。
pub fn check_command(command: &str, cwd: Option<&str>, workspace: &Path) -> WorkspaceVerdict {
    let command = command.trim();
    if command.is_empty() {
        return WorkspaceVerdict::Unknown("命令为空".into());
    }
    // 1) cwd 先判：显式 cwd 越界直接 Outside；相对 cwd 按工作区解析。
    let cwd_path = resolve_cwd(cwd, workspace);
    if !is_within(&cwd_path, workspace) {
        return WorkspaceVerdict::Outside(format!("工作目录 {} 不在工作区内", cwd_path.display()));
    }
    // 2) 内联解释器：脚本内的写 API 无法静态解析，fail-closed 转确认。
    if let Some(interp) = inline_interpreter(command) {
        return WorkspaceVerdict::Unknown(format!("内联解释器 {interp} 的写操作无法静态判定"));
    }
    // 3) 写目标：变量 → Unknown；可解析越界 → Outside。
    for target in extract_write_targets(command) {
        if target.trim().is_empty() {
            continue;
        }
        if is_ignorable_target(&target) {
            continue;
        }
        let resolved = resolve_target(&target, &cwd_path);
        if !is_within(&resolved, workspace) {
            return WorkspaceVerdict::Outside(format!(
                "写目标 {target} 解析为 {}，不在工作区内",
                resolved.display()
            ));
        }
    }
    // 4) 变量展开：只有落在**写类命令参数**或**重定向目标**里的变量才不可证明
    //    （`%TEMP%\*.*` 就是典型的区外落点）。纯读取的变量（`echo $HOME`、`$PATH`）
    //    不制造确认噪音——它不改变工作区外任何东西。
    if let Some(found) = variable_write_target(command) {
        return WorkspaceVerdict::Unknown(format!("{found} 含变量/命令替换，展开后落点未知"));
    }
    // 5) 绝对路径（读/执行目标同样按「越界转确认」处理）。
    if let Some(found) = first_absolute_path_outside(command, &cwd_path, workspace) {
        return WorkspaceVerdict::Outside(found);
    }
    // 6) cd/pushd 逃逸：`cd ..` / `cd C:\outside` / `pushd ..`。
    if let Some(found) = cd_escape(command, &cwd_path, workspace) {
        return WorkspaceVerdict::Outside(found);
    }
    WorkspaceVerdict::Inside
}

fn resolve_cwd(cwd: Option<&str>, workspace: &Path) -> PathBuf {
    let Some(raw) = cwd.map(str::trim).filter(|s| !s.is_empty()) else {
        return workspace.to_path_buf();
    };
    resolve_target(raw, workspace)
}

/// 把目标解析成绝对路径：绝对路径原样，相对路径按 cwd 拼接，最后词法归一。
fn resolve_target(raw: &str, base: &Path) -> PathBuf {
    let path = Path::new(raw);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    lexical_normalize(&joined)
}

/// 词法归一（不碰磁盘）：处理 `.` / `..`，保留盘符与根。
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // 盘根/根目录之上的 `..` 保留字面量，让后续包含判定拒绝
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Windows 下大小写不敏感；其它平台按字节比较。
fn is_within(candidate: &Path, root: &Path) -> bool {
    let candidate = lexical_normalize(candidate);
    let root = lexical_normalize(root);
    #[cfg(windows)]
    {
        let c = candidate.to_string_lossy().to_lowercase();
        let r = root.to_string_lossy().to_lowercase();
        c == r || c.starts_with(&format!("{r}\\")) || c.starts_with(&format!("{r}/"))
    }
    #[cfg(not(windows))]
    {
        candidate == root || candidate.starts_with(&root)
    }
}

/// 写目标里出现变量/通配符/命令替换/`~` 时不可静态解析。
fn contains_unresolvable(raw: &str) -> bool {
    raw.contains('%')
        || raw.contains('$')
        || raw.contains('`')
        || raw.contains('*')
        || raw.contains('?')
        || raw.starts_with('~')
}

/// 写类命令名（参数里出现变量时无法证明落点）。
///
/// 口径刻意保守：只列会**改变文件系统**的常见命令。`echo $HOME` / `cat $PATH`
/// 这类纯读取不在此列——它们出不了工作区外的东西，不该制造确认噪音。
const WRITE_COMMANDS: &[&str] = &[
    "rm", "del", "erase", "rmdir", "rd", "mkdir", "md", "cp", "copy", "mv", "move", "ren",
    "rename", "tee", "dd", "sed", "install", "xcopy", "robocopy", "touch", "truncate", "curl",
    "wget",
];

/// 命令中「写类参数/重定向目标」里的变量、命令替换。
fn variable_write_target(command: &str) -> Option<String> {
    let segments = command.split(['&', '|', ';', '\n']);
    for seg in segments {
        let words = shell_words(seg);
        // 重定向目标：`> $OUT` / `>> %TEMP%\x`
        let mut prev_redirect = false;
        for w in &words {
            if w == ">" || w == ">>" || w == "&>" {
                prev_redirect = true;
                continue;
            }
            if prev_redirect {
                if contains_var(w) {
                    return Some(w.clone());
                }
                prev_redirect = false;
            }
        }
        // 写类命令的参数：`del %TEMP%\*.*`
        let is_write = words
            .first()
            .map(|w| WRITE_COMMANDS.iter().any(|c| w.eq_ignore_ascii_case(c)))
            .unwrap_or(false);
        if is_write {
            if let Some(w) = words.iter().find(|w| contains_var(w)) {
                return Some((*w).clone());
            }
        }
    }
    None
}

fn contains_var(raw: &str) -> bool {
    raw.contains('$') || raw.contains('%') || raw.contains('`')
}

/// `nul` / `/dev/null` / `con` 等设备名不是文件目标。
fn is_ignorable_target(raw: &str) -> bool {
    let lower = raw.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "nul" | "con" | "/dev/null" | "\\\\.\\nul" | "$null"
    )
}

/// 内联解释器检测：这些命令的脚本体可执行任意写操作，静态解析不到。
fn inline_interpreter(command: &str) -> Option<&'static str> {
    let lower = command.to_ascii_lowercase();
    // 只认「解释器 + 内联参数」的组合，避免把 `node script.js` 误判成 Unknown
    // （脚本文件本身在工作区内，边界与 cargo test 同级）。
    const PATTERNS: &[(&str, &str)] = &[
        ("node", " -e"),
        ("node", " --eval"),
        ("python", " -c"),
        ("python3", " -c"),
        ("py", " -c"),
        ("powershell", " -command"),
        ("powershell", " -c "),
        ("pwsh", " -command"),
        ("pwsh", " -c "),
        ("cmd", " /c"),
        ("cmd", " /k"),
        ("bash", " -c"),
        ("sh", " -c"),
    ];
    for (interp, flag) in PATTERNS {
        if lower.contains(&format!("{interp}{flag}")) {
            return Some(interp);
        }
    }
    None
}

/// 扫描命令中的绝对路径；返回第一个越界理由。
fn first_absolute_path_outside(command: &str, cwd: &Path, workspace: &Path) -> Option<String> {
    for token in shell_words(command) {
        let cleaned = token.trim_matches(|c| matches!(c, '"' | '\'' | '(' | ')' | ',' | ';'));
        if cleaned.is_empty() || contains_unresolvable(cleaned) {
            // 含变量/通配符的 token 已由写目标分支处理；读目标不在这里制造噪音。
            continue;
        }
        if is_absolute_like(cleaned) {
            let resolved = resolve_target(cleaned, cwd);
            if !is_within(&resolved, workspace) {
                return Some(format!(
                    "路径 {cleaned} 解析为 {}，不在工作区内",
                    resolved.display()
                ));
            }
        }
    }
    None
}

fn is_absolute_like(token: &str) -> bool {
    let bytes = token.as_bytes();
    // 盘符盘根：C:\ / C:/
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    // UNC：\\server\share 或以两个反斜杠开头
    if token.starts_with("\\\\") {
        return true;
    }
    // 前导 /（Git Bash 风格）/ ~
    token.starts_with('/') || token.starts_with('~')
}

/// `cd` / `pushd` 的逃逸检测。
fn cd_escape(command: &str, cwd: &Path, workspace: &Path) -> Option<String> {
    // 直接在原命令上按分隔符切段，保留大小写无关的 cd 判断。
    let segments = command.split(['&', '|', ';', '\n']);
    for seg in segments {
        let words: Vec<&str> = seg.split_whitespace().collect();
        let Some(pos) = words
            .iter()
            .position(|w| w.eq_ignore_ascii_case("cd") || w.eq_ignore_ascii_case("pushd"))
        else {
            continue;
        };
        let Some(target) = words.get(pos + 1).copied() else {
            continue;
        };
        let target = target.trim_matches(|c| matches!(c, '"' | '\''));
        if target.is_empty() || contains_unresolvable(target) {
            return Some(format!("cd/pushd 目标 {target} 无法静态判定"));
        }
        let resolved = resolve_target(target, cwd);
        if !is_within(&resolved, workspace) {
            return Some(format!(
                "cd/pushd {target} 解析为 {}，不在工作区内",
                resolved.display()
            ));
        }
    }
    None
}

/// 简易 shell 分词（只为绝对路径扫描服务；不追求完整语法）。
fn shell_words(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                c if c.is_whitespace() || matches!(c, '&' | '|' | ';' | '>' | '<' | '=') => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                _ => cur.push(c),
            },
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\work\repo")
        } else {
            PathBuf::from("/work/repo")
        }
    }

    fn rel(parts: &str) -> String {
        ws().join(parts).to_string_lossy().to_string()
    }

    #[test]
    fn plain_commands_are_inside() {
        assert_eq!(
            check_command("ls -la", None, &ws()),
            WorkspaceVerdict::Inside
        );
        assert_eq!(
            check_command("cargo test --workspace", None, &ws()),
            WorkspaceVerdict::Inside
        );
        assert_eq!(
            check_command("echo $HOME", None, &ws()),
            WorkspaceVerdict::Inside,
            "无写目标的变量读取不应制造确认噪音"
        );
    }

    #[test]
    fn write_inside_workspace_is_allowed() {
        let cmd = format!("rm -rf {}", rel("target"));
        assert_eq!(check_command(&cmd, None, &ws()), WorkspaceVerdict::Inside);
        let cmd = format!("echo hi > {}", rel("out.txt"));
        assert_eq!(check_command(&cmd, None, &ws()), WorkspaceVerdict::Inside);
    }

    #[test]
    fn write_outside_workspace_is_outside() {
        let outside = if cfg!(windows) {
            r"C:\Windows\Temp\x.txt".to_string()
        } else {
            "/tmp/x.txt".to_string()
        };
        let cmd = format!("echo hi > {outside}");
        assert!(matches!(
            check_command(&cmd, None, &ws()),
            WorkspaceVerdict::Outside(_)
        ));
    }

    #[test]
    fn parent_escape_is_outside() {
        assert!(matches!(
            check_command("cd .. && rm -rf foo", None, &ws()),
            WorkspaceVerdict::Outside(_)
        ));
        assert!(matches!(
            check_command("rm -rf ../foo", None, &ws()),
            WorkspaceVerdict::Outside(_)
        ));
    }

    #[test]
    fn unparseable_targets_are_unknown() {
        // 变量:展开后才知目标,静态无法判定
        assert!(matches!(
            check_command("del %TEMP%\\*.txt", None, &ws()),
            WorkspaceVerdict::Unknown(_)
        ));
        assert!(matches!(
            check_command("rm -rf $DIR", None, &ws()),
            WorkspaceVerdict::Unknown(_)
        ));
        // 内联解释器:脚本内的写 API 无法静态解析
        assert!(matches!(
            check_command("node -e \"require('fs').rmSync('x')\"", None, &ws()),
            WorkspaceVerdict::Unknown(_)
        ));
        assert!(matches!(
            check_command("powershell -Command Remove-Item x", None, &ws()),
            WorkspaceVerdict::Unknown(_)
        ));
    }

    /// 相对通配符由 shell 在当前目录展开,而 cwd 已被 jail 在工作区 → 仍算区内。
    #[test]
    fn relative_wildcard_stays_inside() {
        assert_eq!(
            check_command("del *.txt", None, &ws()),
            WorkspaceVerdict::Inside
        );
        assert_eq!(
            check_command("rm -rf target/*", None, &ws()),
            WorkspaceVerdict::Inside
        );
    }

    #[test]
    fn absolute_read_outside_is_outside() {
        let outside = if cfg!(windows) {
            r"C:\Windows\win.ini"
        } else {
            "/etc/passwd"
        };
        assert!(matches!(
            check_command(&format!("type {outside}"), None, &ws()),
            WorkspaceVerdict::Outside(_)
        ));
    }

    #[test]
    fn explicit_cwd_outside_is_outside() {
        let outside = if cfg!(windows) { r"C:\Windows" } else { "/etc" };
        assert!(matches!(
            check_command("dir", Some(outside), &ws()),
            WorkspaceVerdict::Outside(_)
        ));
    }

    #[test]
    fn nested_inside_path_is_allowed() {
        let cmd = format!("cat {}", rel("src/main.rs"));
        assert_eq!(check_command(&cmd, None, &ws()), WorkspaceVerdict::Inside);
    }
}
