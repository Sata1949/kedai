//! 工作区画像:项目类型探测(2026-09-30 编码能力包 CODE-4)。
//!
//! 用途:任务绑定工作区后,创建表单(选定目录即探测)与任务详情显示「检测到哪些项目类型 +
//! 建议的验证命令」,让用户一眼知道这个任务在什么项目里干活。**只展示,不注入执行者提示词**
//! (Q4=(a) 拍板,见 `docs/计划.md` CODE-4)。
//! (TM-SCOUT-1 起本模块的探测原语被**规划器侦察快照**复用——只读、确定性采集,注入的是
//! 规划调用;「不注入执行者提示词」的口径不变。)
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
///
/// **链接先判**:`is_symlink()` 在 Windows 上对符号链接**与 junction(挂载点)都为真**,
/// 而 junction 同时带目录属性——若先判 `is_dir()` 就会把它当普通子目录扫下去(等于跟随链接)。
/// 故本函数先排除链接,与 `workspace_scan` 的「符号链接不下降」同一口径。
fn read_dir_names(dir: &Path) -> Option<DirNames> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut files = Vec::new();
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        match entry.file_type() {
            // 链接(含 junction)与无法判型的条目:既不当文件也不当子目录,一律不跟随
            Ok(t) if t.is_symlink() => {}
            Ok(t) if t.is_dir() => subdirs.push(name),
            Ok(t) if t.is_file() => files.push(name),
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

// ==================== 规划器侦察快照(TM-SCOUT-1 D1(b)) ====================

/// 快照清单条目上限(根与一层合计);超限截断并留痕。
pub(crate) const SNAPSHOT_MAX_ITEMS: usize = 120;
/// 快照清单段字符上限(含子目录小节头);超限截断并留痕。
pub(crate) const SNAPSHOT_MAX_LISTING_CHARS: usize = 2400;
/// 入口文档摘要的单篇字符上限。
pub(crate) const SNAPSHOT_DOC_MAX_CHARS: usize = 600;
/// 入口文档的读取字节上限(先截读再按字符截段,防超大文件整读;600 字符按 UTF-8 至多 2400 字节)。
const SNAPSHOT_DOC_READ_BYTES: u64 = 4096;
/// 入口文档白名单(根层;不存在 / 非文本 / 读不动 / 是链接一律跳过)。
const SNAPSHOT_ENTRY_DOCS: &[&str] = &["AGENTS.md", "README.md"];

/// 规划器侦察快照(TM-SCOUT-1 D1(b)):工作区根 + 一层的**可见**清单、入口文档摘要、
/// 项目类型行。系统侧确定性采集(排序固定、无时间戳、幂等),实现「无论模型是否主动侦察,
/// 规划器都基于真实文件规划」;目标文件的深读仍由模型经「侦察指引」条件段驱动。
///
/// 形态留痕(不假装扫全):空目录 → `目录:… 当前无可见文件`;清单超限 → `(清单已截断)`;
/// 根不可读 → `不可读(扫描已跳过)`;子目录不可读 → `(不可读)`。只读、零命令(同 `probe` 纪律)。
/// 调用方负责 untrusted 包裹与机器说明句(装配点:`task_service::executor` 的规划调用构建)。
pub fn scout_snapshot(root: &Path) -> String {
    let mut out = String::new();
    match read_dir_names(root) {
        None => out.push_str(&format!("目录:{} 不可读(扫描已跳过)", root.display())),
        Some(names) => {
            if snapshot_visible(&names).is_empty() {
                out.push_str(&format!("目录:{} 当前无可见文件", root.display()));
            } else {
                let (listing, truncated) = snapshot_listing(root, &names);
                out.push_str(&listing);
                if truncated {
                    out.push_str("\n(清单已截断)");
                }
            }
        }
    }
    if let Some(docs) = snapshot_entry_docs(root) {
        out.push_str("\n入口文档摘要:\n");
        out.push_str(&docs);
    }
    out.push_str("\n项目类型: ");
    out.push_str(&snapshot_type_line(root));
    out
}

/// 一个目录里的**可见**条目(文件 + 可扫子目录,目录名带尾斜杠),合并按名排序。
/// 过滤口径与遍历纪律同源:隐藏项(点前缀)与忽略集目录一律不进。
fn snapshot_visible(names: &DirNames) -> Vec<String> {
    let mut items: Vec<(String, bool)> = Vec::new();
    for f in &names.files {
        if !f.starts_with('.') {
            items.push((f.clone(), false));
        }
    }
    for d in &names.subdirs {
        if is_scannable_dir(d) {
            items.push((d.clone(), true));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
        .into_iter()
        .map(|(n, is_dir)| if is_dir { format!("{n}/") } else { n })
        .collect()
}

/// 根 + 一层清单段。返回 (清单文本, 是否截断)。
/// 预算纪律(确定性):根条目 → 各一层子目录小节(排序 + 同 `probe` 的子目录上限),
/// 条目 ≤ [`SNAPSHOT_MAX_ITEMS`]、清单段字符 ≤ [`SNAPSHOT_MAX_LISTING_CHARS`],
/// 任一超限即停止并在文本尾留 `(清单已截断)`(由调用方追加)。
fn snapshot_listing(root: &Path, root_names: &DirNames) -> (String, bool) {
    const LISTING_HEADER: &str = "根与一层可见项(按名排序):";

    /// 清单段预算:条目与字符双上限;超限即置 `truncated` 并拒绝本次计入。
    struct Budget {
        items: usize,
        chars: usize,
        truncated: bool,
    }
    impl Budget {
        /// 计费一行(`is_item` = 该行是否为清单条目;小节头只计字符)。
        fn charge(&mut self, text: &str, is_item: bool) -> bool {
            let cost = text.chars().count() + 1;
            if self.items + usize::from(is_item) > SNAPSHOT_MAX_ITEMS
                || self.chars + cost > SNAPSHOT_MAX_LISTING_CHARS
            {
                self.truncated = true;
                return false;
            }
            self.items += usize::from(is_item);
            self.chars += cost;
            true
        }
    }

    let mut lines: Vec<String> = vec![LISTING_HEADER.to_string()];
    let mut b = Budget {
        items: 0,
        chars: LISTING_HEADER.chars().count(),
        truncated: false,
    };

    // 根:可见条目(文件与目录合并按名排序)
    for entry in snapshot_visible(root_names) {
        let line = format!("- {entry}");
        if !b.charge(&line, true) {
            break;
        }
        lines.push(line);
    }

    // 一层:每个可扫子目录一小节(排序;超过 MAX_SCANNED_DIRS 截断留痕,与 probe 同口径)
    if !b.truncated {
        let mut subdirs: Vec<String> = root_names
            .subdirs
            .iter()
            .filter(|name| is_scannable_dir(name))
            .cloned()
            .collect();
        subdirs.sort();
        if subdirs.len() > MAX_SCANNED_DIRS {
            b.truncated = true;
            subdirs.truncate(MAX_SCANNED_DIRS);
        }
        'sections: for dir in &subdirs {
            match read_dir_names(&subdir_path(root, dir)) {
                None => {
                    let line = format!("{dir}/: (不可读)");
                    if !b.charge(&line, true) {
                        break 'sections;
                    }
                    lines.push(line);
                }
                Some(children) => {
                    let header = format!("{dir}/:");
                    if !b.charge(&header, false) {
                        break 'sections;
                    }
                    lines.push(header);
                    for entry in snapshot_visible(&children) {
                        let line = format!("- {entry}");
                        if !b.charge(&line, true) {
                            break 'sections;
                        }
                        lines.push(line);
                    }
                }
            }
        }
    }

    (lines.join("\n"), b.truncated)
}

/// 入口文档摘要段(白名单 [`SNAPSHOT_ENTRY_DOCS`]):各篇截 [`SNAPSHOT_DOC_MAX_CHARS`] 字符;
/// 不存在 / 非文本 / 读不动 / 是链接一律跳过;无任何命中返回 `None`(不产生空壳小节)。
fn snapshot_entry_docs(root: &Path) -> Option<String> {
    use std::io::Read;
    let mut out = String::new();
    for name in SNAPSHOT_ENTRY_DOCS {
        let path = root.join(name);
        // 链接先判(与 read_dir_names 同纪律:不跟随链接)
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut buf = Vec::new();
        if file
            .take(SNAPSHOT_DOC_READ_BYTES)
            .read_to_end(&mut buf)
            .is_err()
        {
            continue;
        }
        let text: &str = match std::str::from_utf8(&buf) {
            Ok(t) => t,
            // 截读尾巴可能落在多字节字符中间(error_len=None = 不完整):取有效前缀;
            // 其余(文件内部确有非法 UTF-8 字节)= 非文本,整篇跳过。
            Err(e) if e.error_len().is_none() => {
                std::str::from_utf8(&buf[..e.valid_up_to()]).unwrap_or("")
            }
            Err(_) => continue,
        };
        let excerpt: String = text.chars().take(SNAPSHOT_DOC_MAX_CHARS).collect();
        if excerpt.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("- {name}: {excerpt}"));
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 项目类型行(复用 [`probe`] 的检测结果):`类型(证据文件 → 建议命令)` 按表序 `、` 分隔;
/// 无命中 = `未识别`;probe 子目录扫描被截断时留痕(不假装扫全——截断与未识别是两件事)。
fn snapshot_type_line(root: &Path) -> String {
    let profile = probe(root);
    let mut s = if profile.detected.is_empty() {
        "未识别".to_string()
    } else {
        profile
            .detected
            .iter()
            .map(|h| format!("{}({} → {})", h.kind, h.marker, h.suggested_command))
            .collect::<Vec<_>>()
            .join("、")
    };
    if profile.truncated {
        s.push_str("(子目录扫描截断)");
    }
    s
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

    /// 判型顺序钉子:链接分支必须排在目录分支**之前**。
    ///
    /// Windows 上 `is_symlink()` 对符号链接**与 junction 都为真**,而 junction 同时带目录属性
    /// ——顺序反了就会跟随链接扫下去。这条特性在 Linux 上看不出来、也难以在测试里造链接
    /// (需特权;且本模块禁 `Command`,不能 shell 出 `mklink /J`),故用源码顺序断言兜住。
    /// 针带 `t.` 前缀,与文档注释里的裸 `is_symlink()` 区分开。
    #[test]
    fn symlink_arm_precedes_dir_arm() {
        let src = include_str!("workspace_profile.rs");
        let link = src.find("t.is_symlink()").expect("应有链接判型分支");
        let dir = src.find("t.is_dir()").expect("应有目录判型分支");
        assert!(
            link < dir,
            "链接必须先于目录判型(Windows junction 同时带目录属性)"
        );
    }

    // ==================== 规划器侦察快照(TM-SCOUT-1 D1(b)) ====================

    /// 快照全貌:根/一层清单(排序、目录带斜杠、隐藏与忽略集不进)+ 入口文档摘要 +
    /// 项目类型行;同一目录两次调用逐字节相同(确定性/幂等,无时间戳)。
    #[test]
    fn snapshot_lists_root_and_first_level_sorted_deterministically() {
        let ws = TempDataDir::new("snap-full");
        touch(ws.path(), "README.md");
        std::fs::write(ws.path().join("README.md"), "入口").unwrap();
        touch(ws.path(), "package.json");
        touch(ws.path(), "src/vec2.mjs");
        touch(ws.path(), "test/vec2.test.mjs");
        touch(ws.path(), ".hidden");
        touch(ws.path(), ".git/Cargo.toml");
        touch(ws.path(), "node_modules/dep.js");

        let expected = "\
根与一层可见项(按名排序):
- README.md
- package.json
- src/
- test/
src/:
- vec2.mjs
test/:
- vec2.test.mjs
入口文档摘要:
- README.md: 入口
项目类型: node(package.json → npm test)";
        let snap = scout_snapshot(ws.path());
        assert_eq!(snap, expected);
        assert_eq!(
            scout_snapshot(ws.path()),
            snap,
            "快照必须幂等(排序固定、无时间戳)"
        );
    }

    /// 条目上限:超过 [`SNAPSHOT_MAX_ITEMS`] 即截断并留痕,且被截掉的条目确实不在。
    #[test]
    fn snapshot_truncates_at_item_cap_with_note() {
        let ws = TempDataDir::new("snap-items");
        for i in 0..200 {
            touch(ws.path(), &format!("f{i:03}.txt"));
        }
        let snap = scout_snapshot(ws.path());
        assert!(snap.contains("(清单已截断)"), "{snap}");
        let listed = snap.lines().filter(|l| l.starts_with("- ")).count();
        assert_eq!(listed, SNAPSHOT_MAX_ITEMS, "应恰好列到上限条目:{snap}");
        assert!(snap.contains("- f119.txt"), "第 120 条应入列:{snap}");
        assert!(!snap.contains("- f120.txt"), "超限条目不得出现:{snap}");
        assert!(
            !snap.contains("- f199.txt"),
            "被截断的尾部条目不得出现:{snap}"
        );
    }

    /// 字符上限:长名条目在条目数用尽前先触顶 2400 字符,截断留痕且清单段长度有界。
    #[test]
    fn snapshot_truncates_at_char_cap_with_note() {
        let ws = TempDataDir::new("snap-chars");
        for i in 0..120 {
            touch(ws.path(), &format!("{}{i:03}.txt", "x".repeat(28)));
        }
        let snap = scout_snapshot(ws.path());
        assert!(snap.contains("(清单已截断)"), "{snap}");
        let listed = snap.lines().filter(|l| l.starts_with("- ")).count();
        assert!(
            listed < SNAPSHOT_MAX_ITEMS,
            "应先于条目上限被字符上限截住:{listed}"
        );
        let listing_part = &snap[..snap.find("\n(清单已截断)").expect("应有截断注记")];
        assert!(
            listing_part.chars().count() <= SNAPSHOT_MAX_LISTING_CHARS,
            "清单段字符超上限:{listing_part}"
        );
    }

    /// 空目录 / 只有隐藏项 / 根不可读:三种形态各有显式留痕,不假装有内容。
    #[test]
    fn snapshot_empty_and_unreadable_roots_marked() {
        let ws = TempDataDir::new("snap-empty");
        let snap = scout_snapshot(ws.path());
        assert!(snap.starts_with("目录:"), "{snap}");
        assert!(snap.contains("当前无可见文件"), "{snap}");
        assert!(snap.contains("项目类型: 未识别"), "{snap}");
        assert!(
            !snap.contains("入口文档摘要"),
            "无文档不产生空壳小节:{snap}"
        );

        // 只有隐藏项 = 无可见内容(与遍历口径一致)
        let ws2 = TempDataDir::new("snap-hidden");
        touch(ws2.path(), ".env");
        touch(ws2.path(), ".git/config");
        let snap2 = scout_snapshot(ws2.path());
        assert!(snap2.contains("当前无可见文件"), "{snap2}");

        // 根不可读(不存在):显式留痕,不报错
        let missing = ws.path().join("no-such-dir");
        let snap3 = scout_snapshot(&missing);
        assert!(snap3.contains("不可读"), "{snap3}");
        assert!(snap3.contains("项目类型:"), "{snap3}");
    }

    /// 入口文档:白名单外不看、不存在跳过、空/非文本跳过、超长截 600 字符、
    /// 有则按白名单序(AGENTS.md 先于 README.md)。
    #[test]
    fn snapshot_entry_docs_capped_skipped_and_ordered() {
        let ws = TempDataDir::new("snap-docs-cap");
        std::fs::write(ws.path().join("README.md"), "x".repeat(700)).unwrap();
        touch(ws.path(), "GUIDE.md");
        let snap = scout_snapshot(ws.path());
        assert!(snap.contains(&format!(
            "- README.md: {}",
            "x".repeat(SNAPSHOT_DOC_MAX_CHARS)
        )));
        assert!(
            !snap.contains(&"x".repeat(SNAPSHOT_DOC_MAX_CHARS + 1)),
            "摘要不得超 600 字符"
        );
        assert!(!snap.contains("GUIDE.md:"), "白名单外文档不得进摘要:{snap}");
        assert!(!snap.contains("AGENTS.md:"), "不存在的文档跳过:{snap}");

        let ws2 = TempDataDir::new("snap-docs-order");
        std::fs::write(ws2.path().join("README.md"), "入口文档").unwrap();
        std::fs::write(ws2.path().join("AGENTS.md"), "代理约定").unwrap();
        let snap2 = scout_snapshot(ws2.path());
        let agents_at = snap2
            .find("- AGENTS.md: 代理约定")
            .expect("应含 AGENTS.md 摘要");
        let readme_at = snap2
            .find("- README.md: 入口文档")
            .expect("应含 README.md 摘要");
        assert!(agents_at < readme_at, "顺序应按白名单:{snap2}");

        // 非文本(非法 UTF-8 字节)与空白内容:跳过,不留半截摘要
        let ws3 = TempDataDir::new("snap-docs-binary");
        std::fs::write(ws3.path().join("README.md"), [0xff, 0xfe, 0xfd]).unwrap();
        let snap3 = scout_snapshot(ws3.path());
        assert!(!snap3.contains("- README.md: "), "非文本不得进摘要:{snap3}");
        // 但文件本身仍在清单里(清单与摘要两条通路互不影响)
        assert!(snap3.contains("- README.md"), "文件仍应出现在清单:{snap3}");
    }

    /// 项目类型行复用 probe:多类型按表序、子目录扫描截断在「未识别」时同样留痕。
    #[test]
    fn snapshot_type_line_carries_probe_truncation() {
        let ws = TempDataDir::new("snap-type");
        touch(ws.path(), "package.json");
        touch(ws.path(), "server-rs/Cargo.toml");
        let snap = scout_snapshot(ws.path());
        assert!(
            snap.contains(
                "项目类型: rust(server-rs/Cargo.toml → cargo test)、node(package.json → npm test)"
            ),
            "{snap}"
        );

        let ws2 = TempDataDir::new("snap-type-cap");
        for i in 1..=33 {
            std::fs::create_dir_all(ws2.path().join(format!("d{i:02}"))).unwrap();
        }
        touch(ws2.path(), "d33/go.mod");
        let snap2 = scout_snapshot(ws2.path());
        assert!(
            snap2.contains("项目类型: 未识别(子目录扫描截断)"),
            "{snap2}"
        );
        assert!(
            snap2.contains("(清单已截断)"),
            "子目录小节同样超上限留痕:{snap2}"
        );
    }
}
