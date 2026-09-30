// 工作区树扫描(2026-09-30 批次 4b,PRODCAP-4「交付可审计」)。
//
// 用途:bash 侧命令**执行前后各扫一次**工作区,把变化的文件交给
// `task_change_service` 记账(source=bash)。这是**启发式**检出——
// 命令做了什么只有命令自己知道,树扫描是事后推断,因此:
//   ① 三档预算(条目/时间/驻留字节)任一打满都必须**显式留痕**(`task_scan_marks`),
//      「不可检测」与「没有变更」在用户面前是两件事;
//   ② 前扫描不完整就**不做差集**——把「快照里没有」当成「新建」会把已有文件误记成 create;
//   ③ 后扫描截断就**不许判删除**——没走到的文件当成「磁盘上没有」会产出**假删除**。
//
// 驻留正文(D6=(a),2026-09-30 拍板):≤256KB 的文件在**前扫描**顺带读正文,命令跑完后
// 才能算出「改动前」的 diff 与回滚基线;超过留存上限的文件不读、不逐行记,只留扫描级标记
// (理由:after_hash NOT NULL 在语义上要求读完正文,拿空串哈希冒充「太大没读」会永久污染判等)。
//
// 遍历语义与 `agent_tools_fs::walk_files` 同源(见 `walk_tree`,2026-09-30 起为**唯一实现**):
// 目录名命中忽略集即不下降、符号链接不跟随、目录内子项排序后遍历、不可读目录静默跳过。
// 差异只有一处:`walk_tree` 每条目**只取一次元数据**(单 lstat),比原先的三次元数据更快
// (实测本仓 1258 条目:75ms vs 116ms,见 `docs/功能-变更史.md` 批次 4b 段)。
use crate::services::task_change_service::{Baseline, MAX_BASELINE_BYTES};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 遍历时**任一路径段**命中即跳过的目录名(版本库/构建产物/依赖/索引/数据目录)。
/// 这些目录要么体积巨大、要么内容不属于「工作区源码」的语义范围。
/// (2026-09-30 批次 4b 自 `agent_tools_fs` 迁入:遍历实现收拢到本模块,清单随之同源;
/// `fs_glob`/`fs_grep` 经 `walk_files` 继续使用同一份。)
pub(crate) const EXCLUDED_SEGMENTS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    ".kedai-index",
    "data",
];

/// 工作区内的展示用相对路径(正斜杠分隔);不在工作区内时退回绝对路径
pub(crate) fn display_rel(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

/// 条目数上限(实测:冷盘每条约 90~190µs,2 万条约 2.5~3s)
pub(crate) const SCAN_MAX_ENTRIES: u64 = 20_000;
/// 时间上限(含驻留读;按**冷盘**定,热盘快 2~4 倍,按热盘定会低估)
pub(crate) const SCAN_MAX_TIME: Duration = Duration::from_secs(10);
/// 驻留正文预算(≈2× 实测最大 16.1MB;冷读 ≤0.8s,也是内存闸门)
pub(crate) const SCAN_MAX_RETAIN_BYTES: u64 = 32 * 1024 * 1024;

/// 扫描专用忽略集(名称级):`EXCLUDED_SEGMENTS` **之外**追加的构建/依赖/缓存目录。
///
/// 为什么必须加:排除表原先只认 `.git`/`target`/`node_modules`/`dist`,而真实 Python/Node
/// 工作区的 `venv`/`site-packages`/`build` 一个都不在表里——实测本机 ComfyUI-master
/// (51689 条目)与 kohya_ss-master(40441 条目)会让**每条命令**的扫描吃掉 14~20s;
/// 加上本表后同样的树降到 78~165ms(同一份实测见 `docs/功能-变更史.md`)。
///
/// 这是**声明边界**,不是失败:落在这些目录里的改动不会被扫描、也不记 `undected`
/// (与 `fs_glob`/`fs_grep` 跳过 `EXCLUDED_SEGMENTS` 同一体例,契约见 `docs/契约.md`)。
pub(crate) const SCAN_IGNORED_SEGMENTS: &[&str] = &[
    ".venv",
    "venv",
    "site-packages",
    "__pycache__",
    ".gradle",
    "build",
    "out",
    "vendor",
    "Library",
    "Pods",
    ".next",
    ".mypy_cache",
    ".pytest_cache",
];

/// 三档预算(先到者生效)
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanBudget {
    pub max_entries: u64,
    pub max_time: Duration,
    pub max_retain_bytes: u64,
}

impl Default for ScanBudget {
    fn default() -> Self {
        Self {
            max_entries: SCAN_MAX_ENTRIES,
            max_time: SCAN_MAX_TIME,
            max_retain_bytes: SCAN_MAX_RETAIN_BYTES,
        }
    }
}

/// 前扫描的一个文件条目(正文按预算驻留)
pub(crate) struct PreEntry {
    pub rel: String,
    pub mtime: SystemTime,
    pub size: u64,
    pub body: Option<Arc<Vec<u8>>>,
}

/// 后扫描的一个文件条目(不驻留正文,改动文件由 `record` 现读)
pub(crate) struct PostEntry {
    pub rel: String,
    pub mtime: SystemTime,
    pub size: u64,
}

/// 扫描结果三态(诚实边界做成类型,不靠注释;三态**不含**「驻留降级」——
/// 那落在行级 `truncated`/`has_baseline`,由 `plan_changes` 表达)
pub(crate) enum ScanOutcome {
    Complete,
    Truncated { scanned: u64 },
    Failed(String),
}

/// 检出到的一次文件改动
pub(crate) struct DetectedChange {
    pub rel: String,
    /// create / modify / delete
    pub op: &'static str,
    /// 改动后文件是否存在(delete 为 false → `record` 的 after_path 传 None)
    pub after_existed: bool,
    pub before: Baseline,
}

/// 一次命令的检出台账(不含落库)
pub(crate) struct ChangePlan {
    pub changes: Vec<DetectedChange>,
    /// 检出不可用/不完整的中文原因(调用方写一条 `task_scan_marks`);None = 完整且可用
    pub incomplete_reason: Option<String>,
}

/// 遍历预算(条目数 + 时间)。驻留字节**不在这里**:它是前扫描专有的事,
/// 由 `scan_pre` 的访问闭包自己按预算决定「读不读正文」。
#[derive(Debug, Clone, Copy)]
pub(crate) struct WalkBudget {
    pub max_entries: u64,
    pub max_time: Duration,
}

/// 遍历结果(entries 为已访问条目数;root_error 只报根目录不可读,
/// 子目录不可读按既有语义静默跳过)
pub(crate) struct WalkResult {
    pub entries: u64,
    pub stop: Option<ScanStop>,
    pub root_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanStop {
    Entries,
    Time,
}

/// **唯一的工作区树遍历实现**(`fs_glob`/`fs_grep` 经 `walk_files` 与扫描共用)。
///
/// 语义与原 `agent_tools_fs::walk_files` 逐字一致——目录名命中 `ignore` 即不下降、
/// 符号链接不跟随、目录内子项排序后遍历、子目录不可读静默跳过——差异只有每条目
/// **一次 `symlink_metadata`**(原实现为 symlink_metadata + is_dir + is_file 三次)。
/// 无预算(`budget = None`)时行为与旧实现等价。
pub(crate) fn walk_tree(
    root: &Path,
    ignore: &[&str],
    budget: Option<WalkBudget>,
    visit: &mut impl FnMut(&Path, &std::fs::Metadata),
) -> WalkResult {
    let mut entries = 0u64;
    let started = Instant::now();
    if let Err(e) = std::fs::read_dir(root) {
        return WalkResult {
            entries: 0,
            stop: None,
            root_error: Some(e.to_string()),
        };
    }
    let stop = walk_children(root, ignore, budget, started, &mut entries, visit);
    WalkResult {
        entries,
        stop,
        root_error: None,
    }
}

fn walk_children(
    dir: &Path,
    ignore: &[&str],
    budget: Option<WalkBudget>,
    started: Instant,
    entries: &mut u64,
    visit: &mut impl FnMut(&Path, &std::fs::Metadata),
) -> Option<ScanStop> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return None;
    };
    let mut children: Vec<PathBuf> = read.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    children.sort();
    for path in children {
        if let Some(budget) = budget {
            // entries 只计**已扫描**的条目:预算用尽即停,scanned 不会报成 max+1
            if *entries >= budget.max_entries {
                return Some(ScanStop::Entries);
            }
            if started.elapsed() >= budget.max_time {
                return Some(ScanStop::Time);
            }
        }
        *entries += 1;
        let Ok(md) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let file_type = md.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if ignore.contains(&name.as_str()) {
                continue;
            }
            if let Some(stop) = walk_children(&path, ignore, budget, started, entries, visit) {
                return Some(stop);
            }
        } else if file_type.is_file() {
            visit(&path, &md);
        }
    }
    None
}

/// 扫描的忽略集 = 工具族排除表 ∪ 扫描专用忽略集(单一出处,不写第二份常量)
fn scan_ignore_union() -> Vec<&'static str> {
    let mut union: Vec<&'static str> = EXCLUDED_SEGMENTS.to_vec();
    union.extend_from_slice(SCAN_IGNORED_SEGMENTS);
    union
}

fn outcome_of(result: WalkResult) -> ScanOutcome {
    if let Some(err) = result.root_error {
        return ScanOutcome::Failed(err);
    }
    match result.stop {
        None => ScanOutcome::Complete,
        Some(_) => ScanOutcome::Truncated {
            scanned: result.entries,
        },
    }
}

/// 前扫描:快照 + 按预算驻留正文
pub(crate) fn scan_pre(root: &Path, budget: &ScanBudget) -> (Vec<PreEntry>, ScanOutcome) {
    let ignore = scan_ignore_union();
    let mut snapshot: Vec<PreEntry> = Vec::new();
    let mut retained: u64 = 0;
    let mut visit = |path: &Path, md: &std::fs::Metadata| {
        let size = md.len();
        // 双条件:不超单文件上限 **且** 不超驻留总预算。第二个条件先撞上不算失败——
        // 后续条目照常记 mtime+size,只是没有正文(行级 truncated,见文件头)
        let body = if size <= MAX_BASELINE_BYTES && retained + size <= budget.max_retain_bytes {
            match std::fs::read(path) {
                Ok(bytes) => {
                    retained += bytes.len() as u64;
                    Some(Arc::new(bytes))
                }
                // 读不到正文:按「有长度没正文」降级,不让读失败中断扫描
                Err(_) => None,
            }
        } else {
            None
        };
        snapshot.push(PreEntry {
            rel: display_rel(root, path),
            mtime: md.modified().unwrap_or(UNIX_EPOCH),
            size,
            body,
        });
    };
    let result = walk_tree(
        root,
        &ignore,
        Some(WalkBudget {
            max_entries: budget.max_entries,
            max_time: budget.max_time,
        }),
        &mut visit,
    );
    snapshot.sort_by(|a, b| a.rel.cmp(&b.rel));
    (snapshot, outcome_of(result))
}

/// 后扫描:快照(不读正文)
pub(crate) fn scan_post(root: &Path, budget: &ScanBudget) -> (Vec<PostEntry>, ScanOutcome) {
    let ignore = scan_ignore_union();
    let mut snapshot: Vec<PostEntry> = Vec::new();
    let mut visit = |path: &Path, md: &std::fs::Metadata| {
        snapshot.push(PostEntry {
            rel: display_rel(root, path),
            mtime: md.modified().unwrap_or(UNIX_EPOCH),
            size: md.len(),
        });
    };
    let result = walk_tree(
        root,
        &ignore,
        Some(WalkBudget {
            max_entries: budget.max_entries,
            max_time: budget.max_time,
        }),
        &mut visit,
    );
    snapshot.sort_by(|a, b| a.rel.cmp(&b.rel));
    (snapshot, outcome_of(result))
}

/// 一次命令的改动**行数**上限(暂定值,无实测分布支撑——按「行数 × 单行 ≤256KB
/// 不得超驻留预算」倒推;落地后按真实分布复核)。超出即只记前 N 条并留痕。
pub(crate) const SCAN_MAX_CHANGES: usize = 500;

/// 差集:前/后快照 → 改动清单(纯逻辑,不碰盘;不完整语义见文件头 ②③)
pub(crate) fn plan_changes(
    pre: &[PreEntry],
    pre_outcome: &ScanOutcome,
    post: &[PostEntry],
    post_outcome: &ScanOutcome,
    max_changes: usize,
) -> ChangePlan {
    // ② 前扫描不完整 → 不做差集:把「快照里没有」当成「新建」会把已有文件误记成 create
    if let ScanOutcome::Failed(err) = pre_outcome {
        return ChangePlan {
            changes: Vec::new(),
            incomplete_reason: Some(format!(
                "前扫描失败(工作区不可读:{err}),本次命令的改动未检出"
            )),
        };
    }
    if let ScanOutcome::Truncated { scanned } = pre_outcome {
        return ChangePlan {
            changes: Vec::new(),
            incomplete_reason: Some(format!(
                "前扫描未完成(扫到 {scanned} 项即撞预算),本次命令的改动未检出"
            )),
        };
    }
    if let ScanOutcome::Failed(err) = post_outcome {
        return ChangePlan {
            changes: Vec::new(),
            incomplete_reason: Some(format!(
                "后扫描失败(工作区不可读:{err}),本次命令的改动检出不可信"
            )),
        };
    }

    let mut reasons: Vec<String> = Vec::new();
    // ③ 后扫描截断 → 不判删除(没走到的文件当成「磁盘上没有」会产出假删除)
    let post_truncated = matches!(post_outcome, ScanOutcome::Truncated { .. });
    if let ScanOutcome::Truncated { scanned } = post_outcome {
        reasons.push(format!(
            "后扫描未完成(扫到 {scanned} 项即撞预算),已省略删除类改动"
        ));
    }

    let pre_index: HashMap<&str, &PreEntry> = pre.iter().map(|e| (e.rel.as_str(), e)).collect();
    let post_index: HashMap<&str, &PostEntry> = post.iter().map(|e| (e.rel.as_str(), e)).collect();

    let mut changes: Vec<DetectedChange> = Vec::new();
    let mut oversized: usize = 0;
    for p in post {
        match pre_index.get(p.rel.as_str()) {
            Some(before) => {
                if before.mtime == p.mtime && before.size == p.size {
                    continue; // 未动
                }
                // 改动后超留存上限:不逐行记(after_hash NOT NULL 要求读完正文,
                // 拿空串哈希冒充「太大没读」会永久污染判等),改为扫描级标记
                if p.size > MAX_BASELINE_BYTES {
                    oversized += 1;
                    continue;
                }
                changes.push(DetectedChange {
                    rel: p.rel.clone(),
                    op: "modify",
                    after_existed: true,
                    before: baseline_from(before),
                });
            }
            None => {
                if p.size > MAX_BASELINE_BYTES {
                    oversized += 1;
                    continue;
                }
                changes.push(DetectedChange {
                    rel: p.rel.clone(),
                    op: "create",
                    after_existed: true,
                    before: Baseline {
                        hash: None,
                        bytes: 0,
                        blob: None,
                        truncated: false,
                        exists: false,
                    },
                });
            }
        }
    }
    if !post_truncated {
        for e in pre {
            if post_index.contains_key(e.rel.as_str()) {
                continue;
            }
            changes.push(DetectedChange {
                rel: e.rel.clone(),
                op: "delete",
                after_existed: false,
                before: baseline_from(e),
            });
        }
    }
    if oversized > 0 {
        reasons.push(format!(
            "有 {oversized} 个超过留存上限({}KB)的改动文件未逐项记账",
            MAX_BASELINE_BYTES / 1024
        ));
    }
    changes.sort_by(|a, b| a.rel.cmp(&b.rel));
    if changes.len() > max_changes {
        let dropped = changes.len() - max_changes;
        changes.truncate(max_changes);
        reasons.push(format!(
            "改动文件数超过单命令记账上限({max_changes} 项),已省略 {dropped} 项"
        ));
    }
    ChangePlan {
        changes,
        incomplete_reason: if reasons.is_empty() {
            None
        } else {
            Some(reasons.join("；"))
        },
    }
}

/// 把检出结果落库:逐条记账(source=bash)+ 不完整时写一条扫描标记。
///
/// 记账失败一律只 warn(与 `task_change_service::record` 同口径):扫描是**旁路观测**,
/// 绝不能让它把用户命令判成失败。上层(`bash.rs` 的 run)在 Ok/Err 两臂都调用本函数。
pub(crate) fn persist_plan(
    db: &Arc<crate::models::db::Db>,
    task_id: &str,
    root: &Path,
    plan: &ChangePlan,
) {
    for change in &plan.changes {
        let abs = root.join(&change.rel);
        let after = if change.after_existed {
            Some(abs.as_path())
        } else {
            None
        };
        crate::services::task_change_service::record(
            db,
            task_id,
            &change.rel,
            change.op,
            "bash",
            &change.before,
            after,
        );
    }
    if let Some(reason) = &plan.incomplete_reason {
        crate::services::task_change_service::record_scan_mark(db, task_id, reason);
    }
}

/// 由前扫描条目构造「改动前」基线:有正文 → 可 diff 可回滚;
/// 没正文(超上限/驻留预算打满/读失败)→ `truncated=true`(行级降级,不是 undected)
fn baseline_from(e: &PreEntry) -> Baseline {
    Baseline {
        hash: e
            .body
            .as_deref()
            .map(|b| crate::services::task_change_service::hash_hex(b)),
        bytes: e.size,
        blob: e.body.as_ref().map(|b| (**b).clone()),
        truncated: e.body.is_none(),
        exists: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn pre_entry(rel: &str, size: u64, secs: u64, body: Option<&[u8]>) -> PreEntry {
        PreEntry {
            rel: rel.into(),
            mtime: at(secs),
            size,
            body: body.map(|b| Arc::new(b.to_vec())),
        }
    }

    fn post_entry(rel: &str, size: u64, secs: u64) -> PostEntry {
        PostEntry {
            rel: rel.into(),
            mtime: at(secs),
            size,
        }
    }

    /// 差集分类:新建 / 修改 / 删除;未动的文件不出现
    #[test]
    fn plan_classifies_create_modify_delete() {
        let pre = vec![
            pre_entry("a.rs", 10, 1, Some(b"aaaaaaaaaa")),
            pre_entry("b.rs", 20, 1, Some(b"bbbbbbbbbbbbbbbbbbbb")),
            pre_entry("d.rs", 4, 1, Some(b"dddd")),
        ];
        let post = vec![
            post_entry("a.rs", 10, 1), // 未动(mtime/size 都没变)
            post_entry("b.rs", 25, 2), // 修改
            post_entry("c.rs", 5, 2),  // 新建
        ];
        let plan = plan_changes(
            &pre,
            &ScanOutcome::Complete,
            &post,
            &ScanOutcome::Complete,
            SCAN_MAX_CHANGES,
        );
        assert!(plan.incomplete_reason.is_none(), "完整扫描不该有标记");
        let ops: Vec<(&str, &str)> = plan
            .changes
            .iter()
            .map(|c| (c.rel.as_str(), c.op))
            .collect();
        assert_eq!(
            ops,
            vec![("b.rs", "modify"), ("c.rs", "create"), ("d.rs", "delete")],
            "应按 rel 稳定排序且只含三类改动"
        );

        let modify = plan.changes.iter().find(|c| c.rel == "b.rs").unwrap();
        assert_eq!(modify.before.bytes, 20, "修改的基线取**改动前**长度");
        assert!(
            modify.before.blob.is_some(),
            "有驻留正文的修改应能回滚/出 diff"
        );
        assert!(modify.after_existed);

        let create = plan.changes.iter().find(|c| c.rel == "c.rs").unwrap();
        assert!(create.before.hash.is_none() && create.before.blob.is_none());
        assert!(!create.before.exists, "新建的前态是不存在");

        let del = plan.changes.iter().find(|c| c.rel == "d.rs").unwrap();
        assert!(!del.after_existed, "删除的后态是不存在");
        assert_eq!(del.before.bytes, 4);
    }

    /// 前扫描不完整 → **不做差集**:宁可报「未检出」,不得把已有文件误记成 create
    #[test]
    fn plan_refuses_detection_when_pre_incomplete() {
        let post = vec![post_entry("existing.rs", 9, 2)];
        for outcome in [
            ScanOutcome::Truncated { scanned: 3 },
            ScanOutcome::Failed("根不可读".into()),
        ] {
            let plan = plan_changes(
                &[],
                &outcome,
                &post,
                &ScanOutcome::Complete,
                SCAN_MAX_CHANGES,
            );
            assert!(
                plan.changes.is_empty(),
                "前扫描不完整时绝不产出 create 假行:{:?}",
                plan.changes.len()
            );
            let reason = plan.incomplete_reason.expect("必须留下原因");
            assert!(reason.contains("前扫描"), "原因要写明是哪一侧:{reason}");
        }
    }

    /// 后扫描截断 → 只认「看见的」新建/修改,**不许判删除**(假删除比漏记更坏)
    #[test]
    fn plan_skips_deletes_when_post_truncated() {
        let pre = vec![
            pre_entry("seen.rs", 10, 1, Some(b"aaaaaaaaaa")),
            pre_entry("unvisited.rs", 10, 1, Some(b"uuuuuuuuuu")),
        ];
        let post = vec![post_entry("seen.rs", 12, 2)];
        let plan = plan_changes(
            &pre,
            &ScanOutcome::Complete,
            &post,
            &ScanOutcome::Truncated { scanned: 1 },
            SCAN_MAX_CHANGES,
        );
        let rels: Vec<&str> = plan.changes.iter().map(|c| c.rel.as_str()).collect();
        assert_eq!(rels, vec!["seen.rs"], "截断时不得产出删除行");
        assert!(plan.changes[0].op == "modify");
        let reason = plan.incomplete_reason.expect("截断必须留痕");
        assert!(reason.contains("后扫描"), "{reason}");
    }

    /// 改动后超过留存上限的文件**不逐行记**(不读正文就没有 after_hash),
    /// 但要计入扫描级标记——「不可检测」与「没有变更」不许混为一谈
    #[test]
    fn plan_marks_oversized_post_file_without_row() {
        let pre = vec![pre_entry("big.bin", 10, 1, Some(b"0123456789"))];
        let post = vec![post_entry("big.bin", MAX_BASELINE_BYTES + 1, 2)];
        let plan = plan_changes(
            &pre,
            &ScanOutcome::Complete,
            &post,
            &ScanOutcome::Complete,
            SCAN_MAX_CHANGES,
        );
        assert!(
            plan.changes.is_empty(),
            "超上限的改动不逐行记:{}",
            plan.changes.len()
        );
        let reason = plan.incomplete_reason.expect("必须留下原因");
        assert!(
            reason.contains("超过留存上限") && reason.contains('1'),
            "原因要说明是什么、有几项:{reason}"
        );
    }

    /// 驻留预算打满 ≠ 检出缺项:条目一条不少,只是没了正文(行级降级)
    #[test]
    fn scan_drops_bodies_beyond_retention_budget_but_keeps_entries() {
        let dir = TempDataDir::new("scan-retain");
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        let each = 100u64;
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join("ws").join(name), vec![b'x'; each as usize]).unwrap();
        }
        let budget = ScanBudget {
            max_retain_bytes: 150,
            ..Default::default()
        };
        let (snapshot, outcome) = scan_pre(&dir.join("ws"), &budget);
        assert!(
            matches!(outcome, ScanOutcome::Complete),
            "条目数未超,应是完整扫描"
        );
        assert_eq!(snapshot.len(), 3, "条目一条不少");
        let a = snapshot.iter().find(|e| e.rel == "a.txt").unwrap();
        let b = snapshot.iter().find(|e| e.rel == "b.txt").unwrap();
        assert!(a.body.is_some(), "预算内的文件应有正文");
        assert!(
            b.body.is_none() && b.size == each,
            "预算打满后只丢正文、长度照记(行级 truncated 的依据)"
        );
    }

    /// 忽略集与条目预算:venv/build 类目录不下降;撞条目上限即 Truncated
    #[test]
    fn scan_respects_ignore_set_and_entry_budget() {
        let dir = TempDataDir::new("scan-ignore");
        let ws = dir.join("ws");
        for (rel, content) in [
            ("src/a.rs", "a"),
            ("venv/lib/b.py", "b"),
            ("build/c.txt", "c"),
            (".git/config", "d"),
            ("node_modules/pkg/e.js", "e"),
        ] {
            let p = ws.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
        let (snapshot, outcome) = scan_pre(&ws, &ScanBudget::default());
        assert!(matches!(outcome, ScanOutcome::Complete));
        let rels: Vec<&str> = snapshot.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, vec!["src/a.rs"], "只该看到 src/a.rs:{rels:?}");

        let tight = ScanBudget {
            max_entries: 1,
            ..Default::default()
        };
        let (_, outcome) = scan_pre(&ws, &tight);
        match outcome {
            ScanOutcome::Truncated { scanned } => assert_eq!(scanned, 1),
            _ => panic!("撞条目上限应是 Truncated"),
        }
    }

    /// 时间预算已过期 → 立刻 Truncated(判定机制的存在性钉子)
    #[test]
    fn scan_stops_when_time_budget_already_expired() {
        let dir = TempDataDir::new("scan-time");
        std::fs::write(dir.join("x.txt"), "x").unwrap();
        let budget = ScanBudget {
            max_time: Duration::ZERO,
            ..Default::default()
        };
        let (_, outcome) = scan_pre(&dir, &budget);
        assert!(
            matches!(outcome, ScanOutcome::Truncated { .. }),
            "时间预算为零应直接截断"
        );
    }

    /// 单命令记账行数上限:超限只记前 N 条(按 rel 序,确定性)+ 留痕
    #[test]
    fn plan_truncates_change_rows_at_cap() {
        let post = vec![
            post_entry("a.txt", 1, 1),
            post_entry("b.txt", 1, 1),
            post_entry("c.txt", 1, 1),
        ];
        let plan = plan_changes(
            &[],
            &ScanOutcome::Complete,
            &post,
            &ScanOutcome::Complete,
            2,
        );
        let rels: Vec<&str> = plan.changes.iter().map(|c| c.rel.as_str()).collect();
        assert_eq!(rels, vec!["a.txt", "b.txt"], "只记前 N 条");
        let reason = plan.incomplete_reason.expect("超上限必须留痕");
        assert!(
            reason.contains("上限") && reason.contains('1'),
            "原因要写清上限与省略项数:{reason}"
        );
    }
}
