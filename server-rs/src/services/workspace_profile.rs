//! 工作区画像:项目类型探测(2026-09-30 编码能力包 CODE-4)。
//!
//! 用途:任务绑定工作区后,创建表单(选定目录即探测)与任务详情显示「检测到哪些项目类型 +
//! 建议的验证命令」,让用户一眼知道这个任务在什么项目里干活。**只展示,不注入执行者提示词**
//! (Q4=(a) 拍板,见 `docs/计划.md` CODE-4)。
//!
//! 三条硬边界(改动前先读):
//!   ① **零命令执行**:只用目录读取与文件存在性判断,绝不拉起进程。回看走试
//!      `no_command_execution_in_source`——它读本文件源码做字面断言,以后顺手加进来即红。
//!   ② **浅层**:根(0 层)+ 直接子目录(1 层),不进递归。**只探根会对 monorepo 给出错误的
//!      验证命令**——本仓自身即反例:根只有 `package.json`,而 `Cargo.toml` 在 `server-rs/` 下,
//!      只探根会报「Node / `npm test`」而漏掉 `cargo test`。错误的画像比没有画像更坏。
//!      全树递归则是另一个量级(见 `tools/workspace_scan.rs` 的三档预算纪律),出包。
//!   ③ **诚实留痕**:子目录数超上限时 `truncated = true`,不假装扫全——与 `workspace_scan`
//!      「不可检测与没有变更是两件事」同一条纪律。
//!
//! 建议命令是**常量表文案**,不解析 manifest(首版口径:仅文件存在性,见 `docs/计划.md`
//! CODE-4 的「建议命令的来源」)。UI 上按「建议」呈现,不宣称已核实可用。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 扫描的子目录数上限(不含根);超限 `truncated = true`。
///
/// 取 32 是「够覆盖常见 monorepo 且单次探测仍是毫秒级」的折中——每个目录一次 `read_dir`,
/// 超限即截断并留痕,不做更深的补偿扫描。
const MAX_SCANNED_DIRS: usize = 32;

/// 一条「项目类型标记」:命中即认定该类型,并给出建议的验证命令。
struct ProjectMark {
    /// 类型标识(线格式;前端按它做展示名映射)
    kind: &'static str,
    /// 精确文件名;以 `*.` 开头表示按扩展名匹配
    marker: &'static str,
    /// 建议的验证命令(常量表文案,**不是**实测结论)
    command: &'static str,
}

/// 标记表:**顺序即输出顺序**(与命中深度无关,故断言可按表序写死)。
///
/// 同一类型多条标记时**首个命中者胜**(如 python 的 `pyproject.toml` 优先于 `requirements.txt`),
/// 故同一类型在一次探测里最多产出一条 hint——monorepo 里三个 crate 只报一条 rust,
/// 避免界面出现三条同样的 chip。
const PROJECT_MARKS: &[ProjectMark] = &[
    ProjectMark {
        kind: "rust",
        marker: "Cargo.toml",
        command: "cargo test",
    },
    ProjectMark {
        kind: "node",
        marker: "package.json",
        command: "npm test",
    },
    ProjectMark {
        kind: "python",
        marker: "pyproject.toml",
        command: "pytest",
    },
    ProjectMark {
        kind: "python",
        marker: "requirements.txt",
        command: "pytest",
    },
    ProjectMark {
        kind: "python",
        marker: "setup.py",
        command: "pytest",
    },
    ProjectMark {
        kind: "go",
        marker: "go.mod",
        command: "go test ./...",
    },
    ProjectMark {
        kind: "java",
        marker: "pom.xml",
        command: "mvn -q test",
    },
    ProjectMark {
        kind: "java",
        marker: "build.gradle",
        command: "gradle test",
    },
    ProjectMark {
        kind: "java",
        marker: "build.gradle.kts",
        command: "gradle test",
    },
    ProjectMark {
        kind: "dotnet",
        marker: "*.csproj",
        command: "dotnet test",
    },
    ProjectMark {
        kind: "dotnet",
        marker: "*.sln",
        command: "dotnet test",
    },
];

/// 单条探测结果:命中的类型、证据文件(相对工作区根的路径)与建议命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectTypeHint {
    /// 类型标识(常量表取值:rust / node / python / go / java / dotnet)
    pub kind: String,
    /// 证据文件的**相对路径**(根层即文件名,子目录层形如 `server-rs/Cargo.toml`)
    pub marker: String,
    /// 建议的验证命令
    pub suggested_command: String,
    /// 命中所在的层级:0 = 工作区根,1 = 直接子目录
    pub depth: u8,
}

/// 工作区画像:一次探测的全部结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProfile {
    /// 被探测的工作区根(**原样回显**调用方给的冻结路径,含 Windows 的 `\\?\` 前缀)
    pub root: String,
    /// 命中项,按 [`PROJECT_MARKS`] 表序排列(表序决定类型顺序,与命中深度无关)
    pub detected: Vec<ProjectTypeHint>,
    /// 实际扫描过的**子目录**数(不含根;根恒扫一次)
    pub scanned_dirs: u32,
    /// 是否因 [`MAX_SCANNED_DIRS`] 上限截断了子目录扫描
    pub truncated: bool,
}

/// 探测一个工作区根(调用方须已 canonicalize,见 `api/workspace.rs` 的 `validate_workspace`)。
///
/// 读不动目录一律按「没命中」处理、不报错:调用方已校验过它是目录,
/// 走到这里仍读不动(权限/句柄)属于环境问题,不该让任务详情整体失败——
/// 这与 `workspace_scan` 的「子目录不可读静默跳过」同口径。
pub fn probe(root: &Path) -> WorkspaceProfile {
    // 根:一次 read_dir(顺带拿到子目录清单);读不动就按「没命中」走
    let root_names = read_dir_names(root);
    let empty: Vec<String> = Vec::new();
    let root_files = root_names
        .as_ref()
        .map(|d| d.files.as_slice())
        .unwrap_or(empty.as_slice());

    // 子目录:过滤(隐藏 / 忽略集)→ 排序(确定性)→ 截断(留痕)
    let mut subdirs: Vec<String> = root_names
        .as_ref()
        .map(|d| {
            d.subdirs
                .iter()
                .filter(|name| is_scannable_dir(name))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    subdirs.sort();
    let truncated = subdirs.len() > MAX_SCANNED_DIRS;
    subdirs.truncate(MAX_SCANNED_DIRS);

    // 子目录内容:只对**通过过滤且未被截断**的目录各读一次
    let scanned: Vec<(&str, Vec<String>)> = subdirs
        .iter()
        .filter_map(|name| {
            let names = read_dir_names(&subdir_path(root, name))?;
            Some((name.as_str(), names.files))
        })
        .collect();
    let scanned_dirs = scanned.len() as u32;

    // 按表序判定,每类**首个命中者胜**(先根后子目录;子目录已排序,结果确定)
    let mut detected: Vec<ProjectTypeHint> = Vec::new();
    let mut seen_kinds: Vec<&str> = Vec::new();
    for mark in PROJECT_MARKS {
        if seen_kinds.contains(&mark.kind) {
            continue;
        }
        if let Some(file) = match_marker(root_files, mark.marker) {
            detected.push(hint(mark, file, 0));
            seen_kinds.push(mark.kind);
            continue;
        }
        for (dir, files) in &scanned {
            if let Some(file) = match_marker(files, mark.marker) {
                detected.push(hint(mark, format!("{dir}/{file}"), 1));
                seen_kinds.push(mark.kind);
                break;
            }
        }
    }

    WorkspaceProfile {
        root: root.to_string_lossy().into_owned(),
        detected,
        scanned_dirs,
        truncated,
    }
}

/// 组装一条命中(marker 为**展示用相对路径**,分隔符统一 `/`,与平台无关)。
fn hint(mark: &ProjectMark, marker: String, depth: u8) -> ProjectTypeHint {
    ProjectTypeHint {
        kind: mark.kind.into(),
        marker,
        suggested_command: mark.command.into(),
        depth,
    }
}

// ==================== 内部:目录名采集 ====================

/// 一个目录里采到的名字集合(只读一遍 `read_dir`,文件与子目录分列)。
struct DirNames {
    files: Vec<String>,
    subdirs: Vec<String>,
}

/// 采集目录内容。不可读返回 `None`(调用方按「没命中」处理)。
fn read_dir_names(dir: &Path) -> Option<DirNames> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut files = Vec::new();
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        match entry.file_type() {
            Ok(t) if t.is_dir() => subdirs.push(name),
            Ok(t) if t.is_file() => files.push(name),
            // 符号链接与无法判型的条目:既不当文件也不当子目录(不跟随,与
            // `workspace_scan` 的「符号链接不下降」同口径)
            _ => {}
        }
    }
    Some(DirNames { files, subdirs })
}

/// 名称集合里是否有条目命中该 marker;命中的返回**实际文件名**(扩展名匹配时用得上)。
fn match_marker(files: &[String], marker: &str) -> Option<String> {
    if let Some(ext) = marker.strip_prefix("*.") {
        let suffix = format!(".{ext}");
        return files
            .iter()
            .find(|f| f.len() > suffix.len() && f.to_lowercase().ends_with(&suffix.to_lowercase()))
            .cloned();
    }
    files.iter().find(|f| f.as_str() == marker).cloned()
}

/// 该子目录是否值得扫描:跳过隐藏目录与遍历忽略集。
fn is_scannable_dir(name: &str) -> bool {
    !name.starts_with('.') && !crate::tools::EXCLUDED_SEGMENTS.contains(&name)
}

/// 子目录是否可扫(用于 `read_dir` 出来的路径 → 名字)。
fn subdir_path(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    /// 在工作区里放一个空文件(自动建父目录)。
    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, "").unwrap();
    }

    #[test]
    fn empty_dir_detects_nothing() {
        let ws = TempDataDir::new("ws-empty");
        let p = probe(ws.path());
        assert!(p.detected.is_empty(), "空目录不该有命中:{:?}", p.detected);
        assert_eq!(p.scanned_dirs, 0);
        assert!(!p.truncated);
        assert_eq!(p.root, ws.path().to_string_lossy());
    }

    #[test]
    fn root_manifest_hits_depth_zero() {
        let ws = TempDataDir::new("ws-root");
        touch(ws.path(), "Cargo.toml");
        let p = probe(ws.path());
        assert_eq!(
            p.detected,
            vec![ProjectTypeHint {
                kind: "rust".into(),
                marker: "Cargo.toml".into(),
                suggested_command: "cargo test".into(),
                depth: 0,
            }]
        );
    }

    /// 本仓自身的形态:根 `package.json` + 子目录 `Cargo.toml` → 两项并存,
    /// 顺序按常量表(rust 先于 node),子目录项带相对路径。
    #[test]
    fn monorepo_root_and_subdir_both_detected_in_table_order() {
        let ws = TempDataDir::new("ws-monorepo");
        touch(ws.path(), "package.json");
        touch(ws.path(), "server-rs/Cargo.toml");
        let p = probe(ws.path());
        assert_eq!(
            p.detected
                .iter()
                .map(|h| (h.kind.as_str(), h.marker.as_str(), h.depth))
                .collect::<Vec<_>>(),
            vec![
                ("rust", "server-rs/Cargo.toml", 1),
                ("node", "package.json", 0)
            ],
            "表序应为 rust 先;子目录项 marker 是相对路径"
        );
        assert_eq!(p.scanned_dirs, 1);
    }

    /// 忽略集与隐藏目录不下降:`node_modules/package.json` 与 `.git/Cargo.toml` 都不算命中。
    #[test]
    fn ignored_and_hidden_subdirs_are_not_scanned() {
        let ws = TempDataDir::new("ws-ignored");
        touch(ws.path(), "node_modules/package.json");
        touch(ws.path(), ".git/Cargo.toml");
        touch(ws.path(), "target/package.json");
        let p = probe(ws.path());
        assert!(
            p.detected.is_empty(),
            "忽略集/隐藏目录不该被扫:{:?}",
            p.detected
        );
        assert_eq!(p.scanned_dirs, 0, "被跳过的目录不计入已扫描数");
    }

    /// 子目录数超上限:截断留痕(`truncated`),且**被截掉的那个目录确实没扫**。
    #[test]
    fn scanned_dirs_capped_and_truncated_flagged() {
        let ws = TempDataDir::new("ws-cap");
        for i in 1..=33 {
            std::fs::create_dir_all(ws.path().join(format!("d{i:02}"))).unwrap();
        }
        // 排序后 d33 落在上限之外 → 其中的 go.mod 不该被看到
        touch(ws.path(), "d33/go.mod");
        touch(ws.path(), "d01/package.json");
        let p = probe(ws.path());
        assert!(p.truncated, "超上限必须留痕");
        assert_eq!(p.scanned_dirs, MAX_SCANNED_DIRS as u32);
        let kinds: Vec<&str> = p.detected.iter().map(|h| h.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["node"],
            "d01 命中、d33 被截断未扫:{:?}",
            p.detected
        );
    }

    /// 扩展名标记:`*.csproj` 按扩展名命中,marker 回**实际文件名**。
    #[test]
    fn extension_marker_matches_and_reports_real_file_name() {
        let ws = TempDataDir::new("ws-dotnet");
        touch(ws.path(), "src/App.csproj");
        let p = probe(ws.path());
        assert_eq!(
            p.detected,
            vec![ProjectTypeHint {
                kind: "dotnet".into(),
                marker: "src/App.csproj".into(),
                suggested_command: "dotnet test".into(),
                depth: 1,
            }]
        );
    }

    /// 同类型多标记:`pyproject.toml` 与 `requirements.txt` 并存时只出一条(表序靠前者胜)。
    #[test]
    fn same_kind_yields_single_hint_first_marker_wins() {
        let ws = TempDataDir::new("ws-python");
        touch(ws.path(), "requirements.txt");
        touch(ws.path(), "pyproject.toml");
        let p = probe(ws.path());
        assert_eq!(p.detected.len(), 1, "同一类型只出一条:{:?}", p.detected);
        assert_eq!(p.detected[0].marker, "pyproject.toml");
    }

    /// 零命令执行钉子:本模块源码不得出现拉起进程的调用。
    /// 判定针由片段**拼**出来(而非字面量):本文件即被测文本,写死字面量的断言会命中自己。
    #[test]
    fn no_command_execution_in_source() {
        let src = include_str!("workspace_profile.rs");
        let needles = [
            format!("{}::Command", ["std", "process"].join("::")),
            ["Command", "new"].join("::"),
        ];
        for needle in &needles {
            assert!(
                !src.contains(needle.as_str()),
                "工作区画像不得执行命令,但源码出现了 {needle}"
            );
        }
    }
}
