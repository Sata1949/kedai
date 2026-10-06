// 任务 scratch 产物的清单与回收(PRODCAP-5):任务过程文件可在应用内浏览/下载/回收,
// 不再只靠手动去数据目录旁边的 `task_scratch/` 翻找。
//
// 设计要点:
//   - **清单**:递归列文件(目录不入清单),相对路径 + 大小 + mtime,路径升序;
//     条目数与递归深度双上限,触顶给 `truncated` 标记而非静默截断。
//   - **回收**:只删「超过保留期(`KEDAI_TASK_SCRATCH_KEEP_DAYS`,默认 30,0 = 不自动清)
//     且不归属任何非终态任务」的任务目录。判据两半 = 目录自身 mtime 的年龄 + 任务状态
//     (running/planning/planned 的产物不许清——在跑或待批准的任务还要用)。
//     目录 mtime 而非「递归最新文件时间」:直接子项增删会刷新它,agent 落文件即刷新,
//     足够贴近「最近活动」且零额外遍历成本。
//   - **无任务归属的目录**(任务已删除 / 测试造的目录)同样按年龄清理——scratch 根的
//     语义就是「任务产物的家」,其下一级目录一律按任务目录对待。
//   - 清理由空闲看守的**同一 tick** 顺带执行(不新增常驻任务),与设置页「立即清理」
//     入口共用同一实现(本文的 `cleanup_scratch_root`)。
//
// 本模块的核是自由函数(显式传根 + 活跃判定闭包),便于以临时目录单测;
// TaskService 上的方法只是「取根 + 以任务状态作活跃判定」的薄包装。
use std::path::{Path, PathBuf};

/// 清单条目(api 层转 JSON;mtime 为 None 表示取不到时间戳,如实回 null 不猜)
pub(crate) struct ArtifactEntry {
    pub path: String,
    pub size: u64,
    pub mtime: Option<chrono::DateTime<chrono::Utc>>,
}

/// 单次清单上限(条目数;防病态目录撑爆响应与前端)。触顶时 `truncated = true`。
pub(crate) const ARTIFACT_LIST_LIMIT: usize = 1000;

/// 单次清单的最大递归深度(防深目录树拖死请求)
const MAX_SCAN_DEPTH: usize = 16;

/// 回收结果(api 层转 JSON、看守 tick 记日志共用)
#[derive(Default)]
pub(crate) struct ScratchCleanupReport {
    /// 被删除的目录名(任务 id)
    pub removed: Vec<String>,
    /// 因任务在跑/待批准而跳过的目录名
    pub skipped_active: Vec<String>,
    /// 未超过保留期而保留的目录数
    pub kept_fresh: usize,
    /// 删除失败的目录名(日志已记原因;如实回显,不静默吞)
    pub failed: Vec<String>,
}

/// 任务 id 是否可作目录名(与服务内 `scratch_dir_for` 同一判据:服务端生成的 id 为
/// UUID,此处只做廉价防御——带分隔符/上溯段/冒号的 id 绝不拼进路径)。
fn usable_task_id(task_id: &str) -> bool {
    !task_id.is_empty()
        && !task_id.contains(['/', '\\'])
        && !task_id.contains("..")
        && !task_id.contains(':')
}

/// 已存在的任务 scratch 目录(不创建——对「从未落过产物」的任务不应当把目录建出来)
pub(crate) fn existing_task_dir(scratch_root: &Path, task_id: &str) -> Option<PathBuf> {
    if !usable_task_id(task_id) {
        return None;
    }
    let dir = scratch_root.join(task_id);
    dir.is_dir().then_some(dir)
}

/// 列出某任务 scratch 目录下的文件(递归;相对路径统一 `/` 分隔 —— 清单是跨平台
/// 线格式,Windows 的 `\` 在 JSON 里还要转义)。目录不存在 → 空数组(非错误)。
pub(crate) fn list_artifacts_in(task_root: &Path) -> (Vec<ArtifactEntry>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    collect_artifacts(task_root, task_root, 0, &mut out, &mut truncated);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    (out, truncated)
}

/// 递归收集文件(深度与条目数双重上限;不跟随符号链接——`file_type` 不追链接)
fn collect_artifacts(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<ArtifactEntry>,
    truncated: &mut bool,
) {
    if depth > MAX_SCAN_DEPTH {
        *truncated = true;
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        if out.len() >= ARTIFACT_LIST_LIMIT {
            *truncated = true;
            return;
        }
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        if ft.is_dir() {
            collect_artifacts(root, &path, depth + 1, out, truncated);
        } else if ft.is_file() {
            let Ok(meta) = entry.metadata() else { continue };
            let rel = path.strip_prefix(root).unwrap_or(&path);
            out.push(ArtifactEntry {
                path: rel.to_string_lossy().replace('\\', "/"),
                size: meta.len(),
                mtime: meta
                    .modified()
                    .ok()
                    .map(chrono::DateTime::<chrono::Utc>::from),
            });
        }
    }
}

/// 回收 scratch 根下的一级任务目录。`is_active(name)` = 该目录归属的任务是否在跑/
/// 待批准(false = 可删;无归属任务同样返回 false,按年龄清理)。
/// `keep_days` 必须 > 0(0 = 关的判定在调用方,避免两处各判一次)。
pub(crate) fn cleanup_scratch_root(
    scratch_root: &Path,
    keep_days: u64,
    is_active: impl Fn(&str) -> bool,
) -> ScratchCleanupReport {
    let mut report = ScratchCleanupReport::default();
    let Ok(rd) = std::fs::read_dir(scratch_root) else {
        return report; // 根不存在 = 从未产生过产物
    };
    let cutoff = std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(
        keep_days.saturating_mul(86_400),
    ));
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_active(&name) {
            report.skipped_active.push(name);
            continue;
        }
        match (cutoff, entry.metadata().and_then(|m| m.modified()).ok()) {
            (Some(cutoff), Some(mtime)) if mtime < cutoff => {
                match std::fs::remove_dir_all(entry.path()) {
                    Ok(()) => report.removed.push(name),
                    Err(e) => {
                        tracing::warn!(dir = name.as_str(), error = %e, "任务草稿目录清理失败");
                        report.failed.push(name);
                    }
                }
            }
            // 未超期,或时间戳取不到(权限/时钟异常):保守保留,如实计数
            _ => report.kept_fresh += 1,
        }
    }
    report.removed.sort();
    report.skipped_active.sort();
    report.failed.sort();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    #[test]
    fn list_artifacts_walks_recursively_and_marks_truncation() {
        let dir = TempDataDir::new("scratch-list");
        let root = dir.join("task-1");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        std::fs::write(root.join("sub").join("b.bin"), vec![0u8; 7]).unwrap();

        let (files, truncated) = list_artifacts_in(&root);
        assert!(!truncated);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["a.txt", "sub/b.bin"],
            "相对路径升序且统一 / 分隔"
        );
        assert_eq!(files[0].size, 5);
        assert_eq!(files[1].size, 7);
        assert!(files[0].mtime.is_some(), "mtime 应可读");

        // 空目录 → 空数组(非错误)
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let (files, truncated) = list_artifacts_in(&empty);
        assert!(files.is_empty() && !truncated, "空目录返回空清单");
    }

    #[test]
    fn cleanup_removes_only_aged_inactive_dirs() {
        let dir = TempDataDir::new("scratch-cleanup");
        let root = dir.join("scratch-root");
        std::fs::create_dir_all(&root).unwrap();
        // 两个「过期」目录(手改 mtime 到 40 天前,> 30 天保留期)+ 一个新鲜目录
        // + 一个活跃任务目录
        for name in ["old-1", "old-2"] {
            let p = root.join(name);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("x.txt"), b"x").unwrap();
            set_dir_mtime_days_ago(&p, 40);
        }
        std::fs::create_dir_all(root.join("fresh")).unwrap();
        std::fs::create_dir_all(root.join("active")).unwrap();
        set_dir_mtime_days_ago(&root.join("active"), 40); // 过期但在跑,必须跳过

        let report = cleanup_scratch_root(&root, 30, |name| name == "active");
        assert_eq!(
            report.removed,
            vec!["old-1", "old-2"],
            "只删过期且无归属的目录"
        );
        assert_eq!(report.skipped_active, vec!["active"], "在跑任务目录跳过");
        assert_eq!(report.kept_fresh, 1, "新鲜目录保留");
        assert!(report.failed.is_empty());
        assert!(!root.join("old-1").exists() && !root.join("old-2").exists());
        assert!(root.join("fresh").exists() && root.join("active").exists());

        // 幂等:再跑一次零删除
        let again = cleanup_scratch_root(&root, 30, |name| name == "active");
        assert!(again.removed.is_empty(), "重复清理零命中");
    }

    #[test]
    fn existing_task_dir_rejects_bad_ids_and_missing_dirs() {
        let dir = TempDataDir::new("scratch-existing");
        let root = dir.join("scratch-root");
        std::fs::create_dir_all(root.join("t1")).unwrap();
        assert!(existing_task_dir(&root, "t1").is_some());
        assert!(
            existing_task_dir(&root, "missing").is_none(),
            "不存在不创建"
        );
        assert!(
            existing_task_dir(&root, "../escape").is_none(),
            "上溯 id 拒绝"
        );
        assert!(existing_task_dir(&root, "").is_none());
    }

    /// 把目录自身 mtime 改到 `days` 天前(跨平台:std 无 set_mtime,直接按平台 API)。
    fn set_dir_mtime_days_ago(path: &Path, days: u64) {
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
        let ok = set_mtime(path, past);
        assert!(ok, "测试前置:目录 mtime 改写失败 {}", path.display());
    }

    #[cfg(windows)]
    fn set_mtime(path: &Path, t: std::time::SystemTime) -> bool {
        use std::os::windows::fs::OpenOptionsExt;
        // 目录句柄须 FILE_FLAG_BACKUP_SEMANTICS + FILE_WRITE_ATTRIBUTES
        // (GENERIC_WRITE 打不开目录;access_mode 直设 dwDesiredAccess 绕过 generic 映射)
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let Ok(file) = std::fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
        else {
            return false;
        };
        file.set_modified(t).is_ok()
    }

    #[cfg(unix)]
    fn set_mtime(path: &Path, t: std::time::SystemTime) -> bool {
        // libc 未引入为依赖:用文件句柄的 set_modified(Unix 上 File 对目录有效)
        let Ok(file) = std::fs::File::open(path) else {
            return false;
        };
        file.set_modified(t).is_ok()
    }
}
