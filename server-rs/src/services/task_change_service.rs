// 任务文件变更台账(2026-09-30 批次 4,PRODCAP-4「交付可审计」)。
//
// 回答三个问题:任务**改过哪些文件**、每个文件**改成什么样**(unified diff)、
// 以及**能不能收拾**(按基线回滚单个文件)。
//
// 代际:L2(业务服务)——被 `tools/`(写盘侧)与 `api/`(读取侧)双向引用,
// 与 `services/exec/audit.rs` 同构:记账失败**只 warn**,绝不让用户的写操作判失败。
//
// 与 `undo_service` 的分工(勿合并):那份管**聊天侧**角色文件区的「回退到此处」,
// 键是 session_id、快照含 payload 且限 256KB;这份管**任务侧**工作区,键是 task_id、
// 外随 tasks(id) 级联删除。两者的名单语义也不同(见 `tools/tool_sets.rs::state_anchor_arg` 注释)。
use crate::models::db::{now_iso, Db};
use crate::models::types::TaskFileChangeRecord;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;

/// 基线正文留存上限(字节)。与 `undo_service::MAX_SNAPSHOT_FILE_BYTES` 同档:
/// 同一台机器、同一类「可回退基线」,内存与磁盘的权衡不该有两套口径。
pub const MAX_BASELINE_BYTES: u64 = 256 * 1024;

/// unified diff 的两级预算(批次 4 的诚实边界):
/// - 单侧超过 `DIFF_MAX_LINES` 行 → 不生成逐行 diff,返回「过大」说明( LCS 是 O(n·m),
///   不设上限会让一次详情点击变成分钟级 CPU 占用);
/// - 单侧超过 `DIFF_MAX_BYTES` 字节 → 同上。
const DIFF_MAX_LINES: usize = 2_000;
const DIFF_MAX_BYTES: usize = 2 * 1024 * 1024;

/// 改动前的基线快照(调用方在**落盘前**读好,再传给 `record`)。
pub struct Baseline {
    pub hash: Option<String>,
    pub bytes: u64,
    pub blob: Option<Vec<u8>>,
    pub truncated: bool,
    /// 改动前该路径**是否存在**。缺这个字段就分不清「新建」与「基线读取失败」,
    /// op 会被误记成 create(而哈希、正文两头都对不上,事后无从分辨)。
    pub exists: bool,
}

impl Baseline {
    /// 读文件当前内容作基线。文件不存在 = 新建前的空基线(`hash` 为 None,不是空串哈希)。
    /// 读失败(权限/IO)按「基线不可用」处理:仍然记账,但 diff 与回滚会如实说不可用。
    pub fn capture(path: &Path) -> Self {
        match std::fs::metadata(path) {
            Ok(meta) => {
                let len = meta.len();
                let bytes = match std::fs::read(path) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "文件变更基线读取失败,按「基线不可用」记账");
                        return Baseline {
                            hash: None,
                            bytes: len,
                            blob: None,
                            truncated: true,
                            exists: true,
                        };
                    }
                };
                let hash = Some(hash_hex(&bytes));
                // 超限只留长度与哈希:正文进库会让 256KB×N 行的台账吃掉数据目录
                let (blob, truncated) = if len > MAX_BASELINE_BYTES {
                    (None, true)
                } else {
                    (Some(bytes), false)
                };
                Baseline {
                    hash,
                    bytes: len,
                    blob,
                    truncated,
                    exists: true,
                }
            }
            Err(_) => Baseline {
                hash: None,
                bytes: 0,
                blob: None,
                truncated: false,
                exists: false,
            },
        }
    }
}

/// SHA-256 的十六进制文本。除 `record`/`capture` 自用外,扫描侧
/// (`tools::workspace_scan`)也用它给**驻留正文**算前态哈希——同一实现,不复制。
pub(crate) fn hash_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest.iter() {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// 从任务侧虚拟 session 取**根任务 id**。
///
/// 口径与 `tools/bash.rs` 的审计不同:那份把整个 `ctx.session_id` 去掉前缀就存(它只是
/// 文本备注,长一点无害);这里的 `task_id` 是 `tasks(id)` 的外键,子 agent 的层叠会话
/// (`task:<uuid>:main:3:sub:<uuid>`)必须收敛到第一段,否则外键不匹配、记账静默丢失。
pub fn task_id_of(session_id: &str) -> Option<String> {
    let rest = session_id.strip_prefix("task:")?;
    let head = rest.split(':').next().unwrap_or(rest);
    if head.is_empty() {
        None
    } else {
        Some(head.to_string())
    }
}

/// 记一次文件改动。返回是否落库成功(失败已 warn,调用方不必处理)。
///
/// `after_path` 为 None 表示改动后文件不存在(op=delete)。
pub fn record(
    db: &Arc<Db>,
    task_id: &str,
    rel_path: &str,
    op: &str,
    source: &str,
    before: &Baseline,
    after_path: Option<&Path>,
) -> bool {
    let (after_hash, after_bytes) = match after_path.and_then(|p| std::fs::read(p).ok()) {
        Some(bytes) => (hash_hex(&bytes), bytes.len() as u64),
        // 删除或读不到:正文按空处理(哈希给空串的哈希,不是 NULL —— NULL 只留给「基线未知」)
        None => (hash_hex(b""), 0u64),
    };
    let conn = db.write();
    let result = conn.execute(
        "INSERT INTO task_file_changes \
         (task_id, step_index, path, op, source, before_hash, after_hash, before_bytes, after_bytes, before_blob, truncated, created_at) \
         VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            task_id,
            rel_path,
            op,
            source,
            before.hash,
            after_hash,
            before.bytes as i64,
            after_bytes as i64,
            before.blob,
            before.truncated as i64,
            now_iso(),
        ],
    );
    match result {
        Ok(_) => true,
        Err(e) => {
            // 记账失败不改变写操作的结果(同 exec_audit 口径),但必须留痕:
            // 「台账少一行」与「没改文件」在 UI 上看起来一样,只能靠日志区分。
            tracing::warn!(task_id, path = rel_path, error = %e, "文件变更台账写入失败");
            false
        }
    }
}

/// 记一次改动并广播 `file_changed` 事件(PRODCAP-1 回补 PRODCAP-4 追注)。
///
/// 语义:`record` 成功落行后才发事件(失败已 warn,不假装有变更);`tasks` 为 `None`
/// (聊天路径 / 未注入任务服务)时只记账不发事件。事件的权威数据仍是
/// `GET /api/tasks/{id}/changes`,事件只是「现在去重拉」的信号。
///
/// **句柄来源**:文件工具族经 `ToolDeps.tasks`(弱引用)注入的任务服务,拿得到
/// `TaskService`,故不引入进程级全局。**边界(如实登记)**:`bash` 侧启发式检出
/// (`tools/workspace_scan::persist_plan`)当前**不发**本事件——其调用链(`bash::run`)
/// 不携带任务服务句柄,需句柄穿透(见 `计划.md` PRODCAP-4 追注的后续项);bash 检出的
/// 变更在前端仍由「打开详情 + 进终态各拉一次」覆盖(与 PRODCAP-4 现状一致)。
#[allow(clippy::too_many_arguments)] // 与 record 同参 + 事件句柄;与 persist_event 同款豁免
pub fn record_and_notify(
    db: &Arc<Db>,
    tasks: Option<&crate::services::task_service::TaskService>,
    task_id: &str,
    rel_path: &str,
    op: &str,
    source: &str,
    before: &Baseline,
    after_path: Option<&Path>,
) -> bool {
    let ok = record(db, task_id, rel_path, op, source, before, after_path);
    if ok {
        if let Some(svc) = tasks {
            svc.emit_event(
                crate::models::types::TaskEventKind::FileChanged,
                task_id,
                None,
                None,
                Some(format!("文件变更:{rel_path}({op})")),
            );
        }
    }
    ok
}

/// 记一条**扫描标记**:bash 侧的启发式树扫描单次未能完整检出时调用。
///
/// 为什么单独一张表、而不是混进变更行:台账一行 = 一个**文件改动**,而「扫不全」不是改动;
/// 且「命令改了文件但没扫完」与「命令什么都没改」都可能零行,标记混进改动行会同时
/// 污染清单长度语义与回滚定位(见 `docs/契约.md`「任务文件变更台账」小节)。
/// 失败只 warn:标记是旁路观测,不影响命令结果。
pub fn record_scan_mark(db: &Arc<Db>, task_id: &str, reason: &str) -> bool {
    let conn = db.write();
    let result = conn.execute(
        "INSERT INTO task_scan_marks (task_id, reason, created_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![task_id, reason, now_iso()],
    );
    match result {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(task_id, error = %e, "扫描标记写入失败");
            false
        }
    }
}

/// 某任务**最近一次**扫描未完整检出的原因(None = 从未有过不完整扫描)。
/// 端点把它作为顶层字段下发;前端横幅按它说明是哪一类缺项,不再用固定文案冒充。
pub fn latest_scan_mark(db: &Arc<Db>, task_id: &str) -> Option<String> {
    let conn = db.read().ok()?;
    let mut stmt = conn
        .prepare_cached(
            "SELECT reason FROM task_scan_marks WHERE task_id = ?1 ORDER BY id DESC LIMIT 1",
        )
        .ok()?;
    stmt.query_row(rusqlite::params![task_id], |row| row.get::<_, String>(0))
        .ok()
}

/// 某任务的变更清单(不含基线正文;按发生顺序)。
/// 读侧口径与 `services/exec/audit.rs::list` 一致:取不到连接或查询失败 → warn + 空列表,
/// **不 panic**(阻塞线程 panic 会经 JoinError 放大成 500)。
pub fn list(db: &Arc<Db>, task_id: &str) -> Vec<TaskFileChangeRecord> {
    let conn = match db.read() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(op = "文件变更清单 read", error = %e, "只读连接获取失败,回退空列表");
            return Vec::new();
        }
    };
    let mut stmt = match conn.prepare(
        "SELECT id, task_id, path, op, source, before_hash, after_hash, \
                before_bytes, after_bytes, truncated, created_at, \
                (before_blob IS NOT NULL OR (op = 'create' AND truncated = 0)) \
         FROM task_file_changes WHERE task_id = ?1 ORDER BY id ASC",
    ) {
        Ok(s) => s,
        Err(e) => return crate::services::log_query_failure("文件变更清单 prepare", e),
    };
    let rows = stmt.query_map(rusqlite::params![task_id], |row| {
        Ok(TaskFileChangeRecord {
            id: row.get(0)?,
            task_id: row.get(1)?,
            path: row.get(2)?,
            op: row.get(3)?,
            source: row.get(4)?,
            before_hash: row.get(5)?,
            after_hash: row.get(6)?,
            before_bytes: row.get::<_, i64>(7)? as u64,
            after_bytes: row.get::<_, i64>(8)? as u64,
            truncated: row.get::<_, i64>(9)? != 0,
            created_at: row.get(10)?,
            has_baseline: row.get::<_, i64>(11)? != 0,
        })
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(e) => crate::services::log_query_failure("文件变更清单 query_map", e),
    }
}

/// 某文件在本任务内的**最新一条**变更(含基线正文),供 diff 与回滚定位。
struct LatestChange {
    op: String,
    before_blob: Option<Vec<u8>>,
    truncated: bool,
}

fn latest_for(db: &Arc<Db>, task_id: &str, path: &str) -> Option<LatestChange> {
    let conn = db.read().ok()?;
    let mut stmt = conn
        .prepare_cached(
            "SELECT op, before_blob, truncated FROM task_file_changes \
             WHERE task_id = ?1 AND path = ?2 ORDER BY id DESC LIMIT 1",
        )
        .ok()?;
    stmt.query_row(rusqlite::params![task_id, path], |row| {
        Ok(LatestChange {
            op: row.get(0)?,
            before_blob: row.get(1)?,
            truncated: row.get::<_, i64>(2)? != 0,
        })
    })
    .ok()
}

/// 基线不可用的原因(端点按此返回中文说明,**不返回空 diff**——空 diff 会被读成「没改动」)。
pub enum BaselineState {
    /// create:改动前本就没有该文件,没有可比的基线
    NewFile,
    /// 有基线正文
    Ready(Vec<u8>),
    /// 基线不可用(过大 / 读取失败 / 旧行无正文)
    Unavailable(&'static str),
}

/// 取某文件改动前的基线正文,按状态区分原因。
pub fn baseline_state(db: &Arc<Db>, task_id: &str, path: &str) -> BaselineState {
    let Some(c) = latest_for(db, task_id, path) else {
        return BaselineState::Unavailable("该文件不在本任务的变更台账里");
    };
    match c.op.as_str() {
        "create" => BaselineState::NewFile,
        "delete" => {
            BaselineState::Unavailable("该文件已被删除,基线正文见改动前快照;当前无正文可比对")
        }
        _ => {
            if c.truncated || c.before_blob.is_none() {
                BaselineState::Unavailable("基线不可用(改动前正文超出留存上限或当时读取失败)")
            } else {
                BaselineState::Ready(c.before_blob.unwrap_or_default())
            }
        }
    }
}

/// 回滚结果(端点按变体给中文响应)。
pub enum RollbackOutcome {
    /// 恢复到改动前正文
    Restored(u64),
    /// 该条是 create:回滚 = 删掉这个文件(成功)
    Removed,
    /// 基线不可用:不动手,如实说明(绝不「恢复成空文件」这种伪装修复)
    Unavailable(&'static str),
    /// 落盘失败
    Failed(String),
}

/// 把某个文件回滚到**本任务内它最近一次改动前**的状态。
///
/// `abs_path` 由调用方经 `tools::workspace_guard::safe_workspace_path` 解析(端点侧唯一入口),
/// 本函数不碰路径策略;`rel_path` 是台账里登记的相对路径,回滚后**再记一条 `op=rollback`**
/// ——回滚本身也是一次改动,必须在清单里看得见,不能隐身。
pub fn rollback(db: &Arc<Db>, task_id: &str, rel_path: &str, abs_path: &Path) -> RollbackOutcome {
    match baseline_state(db, task_id, rel_path) {
        BaselineState::Unavailable(reason) => RollbackOutcome::Unavailable(reason),
        BaselineState::NewFile => {
            // **先取快照再删除**(CODE-2):删除之后再 capture 只会拍到「文件不存在」,
            // 于是回滚行没有正文——那意味着「点错一次,文件永久消失」而且再点一次
            // 只会得到「基线不可用」。先 capture 则回滚行带上被删内容,「回滚」本身
            // 也可再被回滚(整任务回滚的逐项可逆由此成立)。
            let before = Baseline::capture(abs_path);
            match std::fs::remove_file(abs_path) {
                Ok(_) => {
                    record(db, task_id, rel_path, "rollback", "rollback", &before, None);
                    RollbackOutcome::Removed
                }
                Err(e) => RollbackOutcome::Failed(format!("删除失败:{e}")),
            }
        }
        BaselineState::Ready(bytes) => {
            let before = Baseline::capture(abs_path);
            match crate::utils::fs_atomic::write_atomic(abs_path, &bytes) {
                Ok(_) => {
                    record(
                        db,
                        task_id,
                        rel_path,
                        "rollback",
                        "rollback",
                        &before,
                        Some(abs_path),
                    );
                    RollbackOutcome::Restored(bytes.len() as u64)
                }
                Err(e) => RollbackOutcome::Failed(format!("恢复写入失败:{e}")),
            }
        }
    }
}

/// 整任务回滚的**单项**结果(CODE-2;端点按此逐项拼响应)。
pub struct BulkRollbackItem {
    pub rel_path: String,
    pub result: BulkRollbackResult,
}

/// 单项结果四态:成功两类 + 跳过(无基线/路径被拒) + 失败(IO)。
pub enum BulkRollbackResult {
    /// 恢复到改动前正文(字节数)
    Restored(u64),
    /// 该路径最新一条是 create:回滚 = 删除(成功)
    Removed,
    /// 无基线 / 路径被闸门拒绝:**跳过该项**,不否决其余(计划 CODE-2 的 Q2=(a))
    Skipped(String),
    /// 落盘失败(恢复写入或删除失败)
    Failed(String),
}

/// 整任务回滚:对本任务台账里的**每个路径**逐项执行与单文件端点完全相同的语义
/// (取该路径最新一行;create → 删除;无基线 → 跳过;每项成功再记 `op=rollback`)。
///
/// 顺序 = 首次出现在台账里的顺序(与清单展示一致);文件之间相互独立——
/// 某一项无基线/失败**不拦住**其余项,由响应逐项报告。
/// 路径解析由调用方注入(端点侧复用 `tools::workspace_guard::safe_workspace_path`
/// 唯一闸门,本层不碰路径策略)。
pub fn rollback_all<F>(db: &Arc<Db>, task_id: &str, resolve: F) -> Vec<BulkRollbackItem>
where
    F: Fn(&str) -> Result<std::path::PathBuf, String>,
{
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for row in list(db, task_id) {
        if !seen.insert(row.path.clone()) {
            continue;
        }
        let result = match resolve(&row.path) {
            Err(e) => BulkRollbackResult::Skipped(format!("路径被工作区闸门拒绝({e});该项跳过")),
            Ok(abs) => match rollback(db, task_id, &row.path, &abs) {
                RollbackOutcome::Restored(bytes) => BulkRollbackResult::Restored(bytes),
                RollbackOutcome::Removed => BulkRollbackResult::Removed,
                RollbackOutcome::Unavailable(reason) => {
                    BulkRollbackResult::Skipped(reason.to_string())
                }
                RollbackOutcome::Failed(e) => BulkRollbackResult::Failed(e),
            },
        };
        items.push(BulkRollbackItem {
            rel_path: row.path,
            result,
        });
    }
    items
}

/// 生成 **git 可 apply** 的单文件 hunk(单个 `@@` 块,上下文各最多 3 行)。CODE-2 patch 导出用。
///
/// 与 `unified_diff` 的差别只在头部计数:那份是给前端**原样展示**的人读 diff,
/// 前缀/后缀上下文不进 `@@` 行数(展示无从校验);而 patch 会被用户交给 `git apply`,
/// 计数错了工具直接拒绝——故本函数严格按 unified diff 语义:起点取「首行上下文」,
/// 行数 = 上下文 + 该侧中段行数(中段含共同行,' ' 两侧都算)。
///
/// 返回 Err = **不进 patch 正文**的两类(由调用方写进头部注释):改动过大 / 无逐行变化。
/// 诚实边界:按行文本生成(行尾统一 LF),不含 git 的 `\ No newline at end of file`
/// 标记处理——无尾换行的文件可能被 `git apply` 拒绝。
pub fn git_hunk(before: &str, after: &str) -> Result<String, String> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    if a.len() > DIFF_MAX_LINES || b.len() > DIFF_MAX_LINES {
        return Err(format!(
            "改动过大,不生成逐行 diff:{} 行 vs {} 行",
            a.len(),
            b.len()
        ));
    }
    if before.len() > DIFF_MAX_BYTES || after.len() > DIFF_MAX_BYTES {
        return Err(format!(
            "改动过大,不生成逐行 diff:{} 字节 vs {} 字节",
            before.len(),
            after.len()
        ));
    }
    let mut head = 0usize;
    while head < a.len() && head < b.len() && a[head] == b[head] {
        head += 1;
    }
    let mut tail = 0usize;
    while tail < a.len().saturating_sub(head)
        && tail < b.len().saturating_sub(head)
        && a[a.len() - 1 - tail] == b[b.len() - 1 - tail]
    {
        tail += 1;
    }
    let am = &a[head..a.len() - tail];
    let bm = &b[head..b.len() - tail];
    if am.is_empty() && bm.is_empty() {
        return Err("无逐行变化(仅行尾差异或空改动)".into());
    }
    let script = lcs_script(am, bm);
    let pre_n = head.min(3);
    let post_n = tail.min(3);
    let before_mid = script.iter().filter(|(m, _)| *m != '+').count();
    let after_mid = script.iter().filter(|(m, _)| *m != '-').count();
    let start = head - pre_n + 1; // 两侧起点相同(首行上下文是公共前缀)
    let mut out = String::new();
    use std::fmt::Write as _;
    let _ = writeln!(
        out,
        "@@ -{start},{} +{start},{} @@",
        pre_n + before_mid + post_n,
        pre_n + after_mid + post_n
    );
    for l in &a[head - pre_n..head] {
        let _ = writeln!(out, " {l}");
    }
    for (mark, line) in &script {
        let _ = writeln!(out, "{mark}{line}");
    }
    for l in &b[b.len() - tail..b.len() - tail + post_n] {
        let _ = writeln!(out, " {l}");
    }
    Ok(out)
}

/// unified diff(自实现,不引 crate:`Cargo.lock` 里没有 diff/similar/imara,
/// 且新增依赖要走双 Cargo.lock 同步门禁)。
///
/// 做法:先削公共首尾行(大文件小改动的主路径,成本 O(n)),中段再做 LCS 回溯生成
/// 有序编辑脚本。中段行数或字节数超预算时**不硬算**,返回说明文本 —— 宁可少给 diff,
/// 不可让一次详情点击把服务 CPU 打满(LCS 到 2000×2000 已是百万级操作)。
pub fn unified_diff(before: &str, after: &str, path: &str) -> String {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    if a.len() > DIFF_MAX_LINES || b.len() > DIFF_MAX_LINES {
        return format!(
            "(改动过大,不生成逐行 diff:{} 行 vs {} 行;请对照清单里的字节数或直接打开文件)",
            a.len(),
            b.len()
        );
    }
    if before.len() > DIFF_MAX_BYTES || after.len() > DIFF_MAX_BYTES {
        return format!(
            "(改动过大,不生成逐行 diff:{} 字节 vs {} 字节)",
            before.len(),
            after.len()
        );
    }
    // 公共前缀 / 公共后缀(tail 不得越过 head,否则中段切负)
    let mut head = 0usize;
    while head < a.len() && head < b.len() && a[head] == b[head] {
        head += 1;
    }
    let mut tail = 0usize;
    while tail < a.len().saturating_sub(head)
        && tail < b.len().saturating_sub(head)
        && a[a.len() - 1 - tail] == b[b.len() - 1 - tail]
    {
        tail += 1;
    }
    let am = &a[head..a.len() - tail];
    let bm = &b[head..b.len() - tail];
    if am.is_empty() && bm.is_empty() {
        return String::from("@@ 无逐行变化(仅行尾差异或空改动)@@\n");
    }
    let script = lcs_script(am, bm);
    let mut out = String::new();
    use std::fmt::Write as _;
    let _ = writeln!(
        out,
        "--- a/{path}\n+++ b/{path}\n@@ -{},{} +{},{} @@",
        head + 1,
        am.len() + tail,
        head + 1,
        bm.len() + tail
    );
    // 前置上下文(最多 3 行,首块之外也给读者一个落点)
    for l in &a[head.saturating_sub(3)..head] {
        let _ = writeln!(out, " {l}");
    }
    for (mark, line) in &script {
        let _ = writeln!(out, "{mark}{line}");
    }
    // 后置上下文:公共后缀的头 3 行
    for l in &b[b.len() - tail..(b.len() - tail + 3).min(b.len())] {
        let _ = writeln!(out, " {l}");
    }
    out
}

/// 中段的最小编辑脚本(有序):`(' ', 共同行)` / `('-', 删除)` / `('+', 新增)`。
/// 等长分支里让删除先于新增(`dp[i+1][j] >= dp[i][j+1]`)——与 git 的呈现习惯一致。
fn lcs_script<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(char, &'a str)> {
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return b.iter().map(|l| ('+', *l)).collect();
    }
    if m == 0 {
        return a.iter().map(|l| ('-', *l)).collect();
    }
    // dp[i][j] = a[i..] 与 b[j..] 的 LCS 长度
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut out: Vec<(char, &'a str)> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(('-', a[i]));
            i += 1;
        } else {
            out.push(('+', b[j]));
            j += 1;
        }
    }
    while i < n {
        out.push(('-', a[i]));
        i += 1;
    }
    while j < m {
        out.push(('+', b[j]));
        j += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_of_takes_root_segment() {
        assert_eq!(task_id_of("task:57cf15f9").as_deref(), Some("57cf15f9"));
        // 子 agent 层叠会话必须收敛到根 id,否则外键不匹配、记账静默丢失
        assert_eq!(
            task_id_of("task:57cf15f9:main:3:sub:abcdef").as_deref(),
            Some("57cf15f9")
        );
        assert_eq!(task_id_of("session-1"), None);
        assert_eq!(task_id_of("task:"), None);
    }

    #[test]
    fn hash_is_stable_and_hex64() {
        let h = hash_hex(b"abc");
        assert_eq!(h.len(), 64);
        assert_eq!(h, hash_hex(b"abc"));
        assert_ne!(h, hash_hex(b"abd"));
    }

    #[test]
    fn unified_diff_marks_added_and_removed_lines() {
        let before = "line1\nline2\nline3\n";
        let after = "line1\ntouched\nline3\n";
        let d = unified_diff(before, after, "src/app.rs");
        assert!(d.contains("-line2"), "{d}");
        assert!(d.contains("+touched"), "{d}");
        assert!(d.contains("--- a/src/app.rs"), "{d}");
        assert!(d.contains("+++ b/src/app.rs"), "{d}");
    }

    /// 大文件小改动:公共首尾先削掉,中段才做 LCS(否则一次点击就是百万级操作)
    #[test]
    fn unified_diff_handles_small_change_in_big_file() {
        let mut before = String::new();
        let mut after = String::new();
        for i in 0..1_500 {
            before.push_str(&format!("l{i}\n"));
            after.push_str(&if i == 700 {
                "CHANGED\n".to_string()
            } else {
                format!("l{i}\n")
            });
        }
        let d = unified_diff(&before, &after, "a.txt");
        assert!(d.contains("-l700"), "{d}");
        assert!(d.contains("+CHANGED"), "{d}");
        assert!(
            d.lines().count() < 40,
            "小改动不应吐出全部行: {}",
            d.lines().count()
        );
    }

    /// 超预算时**明说不生成**,而不是硬算或返回空(空 diff 会被读成「没改动」)
    #[test]
    fn unified_diff_bails_out_on_oversized_input() {
        let before: String = (0..3_000).map(|i| format!("l{i}\n")).collect();
        let after: String = (0..3_000).map(|i| format!("x{i}\n")).collect();
        let d = unified_diff(&before, &after, "big.txt");
        assert!(d.contains("改动过大"), "{d}");
        assert!(!d.contains("+x0"), "{d}");
    }

    #[test]
    fn unified_diff_no_line_change_is_explained() {
        let d = unified_diff("a\nb\n", "a\nb\n", "same.txt");
        assert!(d.contains("无逐行变化"), "{d}");
    }

    /// CODE-2:git_hunk 的头部计数严格按 unified diff 语义(起点含首行上下文、行数含两侧上下文)。
    /// `git apply` 会按计数校验,这份计数错了工具直接拒绝(与「人读 diff」的 `unified_diff` 不同)。
    #[test]
    fn git_hunk_counts_follow_unified_diff_semantics() {
        // 7 行文件改中间一行:head=3, tail=3;起点 = 3-3+1 = 1;行数 = 3(前置)+1(中段)+3(后置) = 7
        let before = "l1\nl2\nl3\nl4\nl5\nl6\nl7\n";
        let after = "l1\nl2\nl3\nX\nl5\nl6\nl7\n";
        let h = git_hunk(before, after).expect("应生成 hunk");
        assert!(h.starts_with("@@ -1,7 +1,7 @@\n"), "{h}");
        assert!(h.contains("-l4\n"), "{h}");
        assert!(h.contains("+X\n"), "{h}");
        assert_eq!(
            h.lines().filter(|l| l.starts_with(' ')).count(),
            6,
            "上下文行 = 前置 3 + 后置 3:{h}"
        );
    }

    /// 改动靠后时起点要按「首行上下文的实际位置」前移(不能拿 head 直接当起点)。
    #[test]
    fn git_hunk_start_point_offsets_by_context() {
        let before: String = (1..=12).map(|i| format!("l{i}\n")).collect();
        let after = before.replace("l8\n", "X8\n");
        let h = git_hunk(&before, &after).expect("应生成 hunk");
        // head=7, pre_n=3 → 起点 5;中段该侧 = 1 行 → -5,7
        assert!(h.starts_with("@@ -5,7 +5,7 @@\n"), "{h}");
    }

    /// 两类「不进 patch 正文」的情形要把原因作为 Err 文本交出(由调用方写进头部注释)。
    #[test]
    fn git_hunk_reports_non_content_cases_into_notes() {
        assert!(git_hunk("same\n", "same\n")
            .unwrap_err()
            .contains("无逐行变化"));
        let big: String = (0..3_000).map(|i| format!("l{i}\n")).collect();
        let big2: String = (0..3_000).map(|i| format!("x{i}\n")).collect();
        assert!(git_hunk(&big, &big2).unwrap_err().contains("改动过大"));
    }
}
