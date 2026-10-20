// 任务工作区文件所有权(TMR-4,2026-10-20)。
//
// 背景:team 模式的多个主 agent 共享同一工作区并行执行,却没有文件级约束——
// 两个主同时写同一文件是 last-writer-wins,且一个主可能覆盖另一个主正在写的分片
// (docs/plans/TASK-MODE-ROUNDS-AUDIT.md 的 R-4)。
//
// 本模块只提供**纯数据结构 + 纯函数**,不碰文件系统、不依赖上层:
//   - `normalize_workspace_rel`:把规划器声明与运行期路径压成同一把键;
//   - `WriteOwnership`:规划器声明表(不可变)+ 运行期首写者认领表(互斥)。
// 文件系统路径解析仍由 `tools/workspace_guard::safe_workspace_path` 负责;本模块
// 拿到的已经是工作区内路径,只做「相对键」归一化,不承担越界防护。
//
// 归属(arch-layers.json):models 属 L1(数据契约层),零上层依赖。
use std::collections::HashMap;
use std::sync::Mutex;

/// 单个声明路径的长度上限(规划器输出不可信,防超长键撑爆内存/日志)。
pub const MAX_DECLARED_PATH_CHARS: usize = 256;

/// 把规划器声明的工作区相对路径压成统一键。
///
/// 规则(与运行期 `ExecScope::reserve_write` 共用,单一出处):
///   - 去两端空白;空串 / 含 NUL / 含 `:` / 绝对路径 / `..` 段 → None;
///   - 分隔符统一 `/`;丢弃 `.` 与空段;含 `*` / `?` 的通配声明 → None(只支持精确路径);
///   - 长度超过 [`MAX_DECLARED_PATH_CHARS`] → None;
///   - Windows 下 ASCII 小写化(NTFS 大小写不敏感),其他平台保留原样。
///
/// 为什么拒绝 `..`:声明只用于所有权键与展示,不用于落盘;但若放任 `..`,
/// 同一个文件可能被两个键描述(`a/../b` 与 `b`),所有权就形同虚设。
pub fn normalize_workspace_rel(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.contains('\0') {
        return None;
    }
    let unified = raw.replace('\\', "/");
    // 绝对路径:/foo、//server/share、C:/foo、C:foo 一律拒绝
    if unified.starts_with('/') {
        return None;
    }
    let bytes = unified.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in unified.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return None,
            _ if seg.contains(':') || seg.contains('*') || seg.contains('?') => return None,
            _ => parts.push(seg),
        }
    }
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join("/");
    if joined.chars().count() > MAX_DECLARED_PATH_CHARS {
        return None;
    }
    #[cfg(windows)]
    let joined = joined.to_ascii_lowercase();
    Some(joined)
}

/// 规划器声明 + 运行期首写者认领。
///
/// 不变量:
///   - `declared` 构造后不可变,同一路径被多个 owner 声明时**首声明胜出**(调用方
///     负责先把声明清洗成「每个路径只出现一次」,本结构对重复项静默保留首条);
///   - `claims` 是唯一可变状态,`reserve` 在同一把锁内完成「校验 + 认领」;
///   - 未声明路径由首个写入者认领;认领后其他 owner 一律拒绝。
///
/// 为什么认领发生在写入之前:两个主同时通过「未声明」检查后再各自落盘,仍会互相覆盖;
/// 先认领让竞争收敛为「谁先调用 reserve 谁拥有」。写失败不回滚认领——失败方向是
/// 「该文件仍归本主」,不会造成跨主覆盖。
pub struct WriteOwnership {
    /// 规范化相对路径 -> 规划器指定的 owner
    declared: HashMap<String, String>,
    /// 规范化相对路径 -> 首个写入者 owner
    claims: Mutex<HashMap<String, String>>,
}

impl WriteOwnership {
    /// 以「(owner, 声明路径)」列表构造。非法路径(见 [`normalize_workspace_rel`])跳过;
    /// 同一路径重复出现时保留首条(调用方应在此之前做跨主冲突告警)。
    pub fn new<I>(declarations: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut declared = HashMap::new();
        for (owner, raw) in declarations {
            let Some(key) = normalize_workspace_rel(&raw) else {
                continue;
            };
            declared.entry(key).or_insert(owner);
        }
        WriteOwnership {
            declared,
            claims: Mutex::new(HashMap::new()),
        }
    }

    /// 原子「检查 + 认领」。成功返回 Ok;冲突返回**占用该路径的 owner 名**。
    ///
    /// 判定顺序:
    ///   1. 规划器把路径声明给别的 owner → 直接拒绝(即使还没人写);
    ///   2. 已被别的 owner 认领 → 拒绝;
    ///   3. 否则写入认领表(同 owner 重复写幂等放行)。
    pub fn reserve(&self, owner: &str, key: &str) -> Result<(), String> {
        if let Some(declared_owner) = self.declared.get(key) {
            if declared_owner != owner {
                return Err(declared_owner.clone());
            }
        }
        let mut claims = self.claims.lock().unwrap_or_else(|e| e.into_inner());
        match claims.get(key) {
            Some(claim_owner) if claim_owner != owner => Err(claim_owner.clone()),
            _ => {
                claims.insert(key.to_string(), owner.to_string());
                Ok(())
            }
        }
    }

    /// 只读诊断:该路径是否被声明、归谁(测试与日志用)。
    pub fn declared_owner(&self, key: &str) -> Option<&str> {
        self.declared.get(key).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rejects_absolute_parent_and_glob() {
        for bad in [
            "",
            "   ",
            "/etc/passwd",
            "\\\\server\\share\\a.txt",
            "C:/ws/a.txt",
            "C:a.txt",
            "a/../../b.txt",
            "src/*.js",
            "src/a?.js",
            "src/a\0b",
            "src/a:b.txt",
        ] {
            assert!(
                normalize_workspace_rel(bad).is_none(),
                "应拒绝非法声明路径: {bad:?}"
            );
        }
    }

    #[test]
    fn normalize_collapses_separators_and_dots() {
        assert_eq!(
            normalize_workspace_rel("  ./src//parts\\a.js  ").as_deref(),
            Some("src/parts/a.js"),
            "分隔符/点段应归一"
        );
        assert_eq!(
            normalize_workspace_rel("src/").as_deref(),
            Some("src"),
            "尾斜杠不应留下空段"
        );
        assert_eq!(
            normalize_workspace_rel("src/中文文件.md").as_deref(),
            Some("src/中文文件.md"),
            "Unicode 路径应保留"
        );
    }

    #[cfg(windows)]
    #[test]
    fn normalize_is_case_insensitive_on_windows() {
        assert_eq!(
            normalize_workspace_rel("SRC/Parts/A.JS").as_deref(),
            Some("src/parts/a.js")
        );
    }

    #[test]
    fn declared_owner_rejects_other_writer_even_before_claim() {
        let o = WriteOwnership::new(vec![("主 Agent-1".to_string(), "src/app.js".to_string())]);
        assert_eq!(
            o.reserve("主 Agent-2", "src/app.js"),
            Err("主 Agent-1".to_string()),
            "声明给别的主的路径必须直接拒绝"
        );
        // 声明归属者可以写,并完成认领
        assert_eq!(o.reserve("主 Agent-1", "src/app.js"), Ok(()));
        // 再换人写仍拒绝
        assert_eq!(
            o.reserve("主 Agent-2", "src/app.js"),
            Err("主 Agent-1".to_string())
        );
    }

    #[test]
    fn undeclared_first_writer_wins() {
        let o = WriteOwnership::new(Vec::<(String, String)>::new());
        assert_eq!(o.reserve("主 Agent-1", "notes.txt"), Ok(()));
        assert_eq!(
            o.reserve("主 Agent-2", "notes.txt"),
            Err("主 Agent-1".to_string()),
            "未声明路径由首写者认领"
        );
        // 同一 owner 重复写放行
        assert_eq!(o.reserve("主 Agent-1", "notes.txt"), Ok(()));
    }

    #[test]
    fn duplicate_declarations_keep_first_owner() {
        let o = WriteOwnership::new(vec![
            ("主 Agent-1".to_string(), "src/app.js".to_string()),
            ("主 Agent-2".to_string(), "src/app.js".to_string()),
        ]);
        assert_eq!(
            o.declared_owner("src/app.js"),
            Some("主 Agent-1"),
            "重复声明保留首条"
        );
    }

    #[test]
    fn invalid_declarations_are_dropped_without_panicking() {
        let o = WriteOwnership::new(vec![
            ("主 Agent-1".to_string(), "../escape.txt".to_string()),
            ("主 Agent-1".to_string(), "src/ok.js".to_string()),
        ]);
        assert!(o.declared_owner("../escape.txt").is_none());
        assert_eq!(o.declared_owner("src/ok.js"), Some("主 Agent-1"));
    }
}
