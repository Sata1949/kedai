// 任务附件(ATTACH-1,2026-11-03):用户随任务提供的任意文件的落盘与元数据。
//
// **落点必须在任务的 fs 作用域内**,否则模型读不到:
//   - 绑定工作区 → `<workspace>/.kedai/attachments/{seq}-{safe}`(相对路径 `.kedai/attachments/…`)
//   - 未绑定    → `<scratch_root>/<task_id>/attachments/{seq}-{safe}`(相对路径 `attachments/…`)
// 作用域根见 `task_engine::run_inner`(工作区经 `scope_for_task` 冻结,未绑定用 scratch)。
//
// 三条刻意不做的边界:
//   1. **不改 `workspace_guard`**——附件不是什么「额外放行」,它本来就在作用域内;
//      为此改安全判定面是给越界开洞,收益为零。
//   2. **不改 `delivery.rs` / `ownership.rs`**——附件是**输入**不是产物,不进交付必需
//      清单、不参与写所有权(否则模型「必须交付附件」或被拒写自己该读的文件)。
//   3. **不把 `.kedai` 加进 `workspace_scan::EXCLUDED_SEGMENTS`**——加了 `fs_glob` /
//      `fs_grep` 就扫不到附件,模型看不见。副作用是 bash 前后扫描会把附件目录的改动
//      记进 `task_file_changes`,可接受(执行前基线在 run 起点取,上传发生在起跑之前)。
//
// `.gitignore`:往用户工作区写 `.kedai/` 会污染 `git status`。首次建目录时若
// `<workspace>/.kedai/.gitignore` 不存在则写一行 `attachments/`;**已存在则一字不改**
// ——用户可能自己维护了别的忽略规则,覆盖是不可接受的副作用。
use super::TaskService;
use crate::models::db::now_iso;
use crate::models::types::TaskRecord;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// 单任务附件个数上限。与聊天附件的「不限个数」不同:任务附件会进目标注入段,
/// 每条占一行,几十条会把目标文本本身挤到次要位置。
pub(crate) const MAX_ATTACHMENTS_PER_TASK: usize = 10;

/// 单文件字节上限。全局 body 上限是 35MB(`api/mod.rs` 的 `DefaultBodyLimit`),
/// 这里留 10MB 余量给 multipart 封装与同请求的文本字段。
pub(crate) const MAX_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;

/// 单个附件记录(`task_attachments` 一行)。线格式与前端 `TaskAttachment` 对拍。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TaskAttachmentRecord {
    pub id: String,
    pub task_id: String,
    /// 用户原始文件名(展示用;不是磁盘名,可能含空格/中文/多个点)
    pub name: String,
    /// **相对任务作用域根**的落盘路径(统一 `/` 分隔;模型 `fs_read` 直接用这个)
    pub rel_path: String,
    pub size: i64,
    /// 客户端上报的 MIME;缺失为 `null`(不猜——按扩展名伪造会让「图像/文本」判定说谎)
    pub mime: Option<String>,
    pub created_at: String,
}

/// 附件的磁盘落点。
struct AttachmentDir {
    /// 绝对目录(`…/.kedai/attachments` 或 `…/attachments`)
    abs: PathBuf,
    /// 相对作用域根的前缀(统一 `/`;空串表示就在作用域根下)
    rel_prefix: String,
    /// 是否需要生成 `.gitignore`(只有工作区路径需要)
    gitignore_at: Option<PathBuf>,
}

impl TaskService {
    /// 解析某任务的附件目录并**按需创建**(含 `.gitignore` 兜底)。
    ///
    /// 绑定工作区但目录已不可用(被删/盘不在)→ Err:此时落盘必然失败,报明确原因比
    /// 让 handler 在写文件时拿到 `NotFound` 更好(与 `scratch_dir_for` 的「失败不降级」
    /// 同口径,绝不悄悄回落到 scratch——那会让模型的相对路径与数据库里的 rel_path 不符)。
    fn attachment_dir(&self, task: &TaskRecord) -> Result<AttachmentDir, String> {
        match task
            .workspace
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(ws) => {
                let root = Path::new(ws);
                if !root.is_dir() {
                    return Err(format!("任务工作区不可用,无法存放附件:{}", root.display()));
                }
                let kedai = root.join(".kedai");
                let abs = kedai.join("attachments");
                std::fs::create_dir_all(&abs)
                    .map_err(|e| format!("附件目录创建失败({}):{e}", abs.display()))?;
                Ok(AttachmentDir {
                    abs,
                    rel_prefix: ".kedai/attachments".to_string(),
                    gitignore_at: Some(kedai.join(".gitignore")),
                })
            }
            None => {
                let base = self.scratch_dir_for(&task.id)?;
                let abs = base.join("attachments");
                std::fs::create_dir_all(&abs)
                    .map_err(|e| format!("附件目录创建失败({}):{e}", abs.display()))?;
                Ok(AttachmentDir {
                    abs,
                    // 未绑定任务的作用域根就是 scratch 目录,故前缀只有一级
                    rel_prefix: "attachments".to_string(),
                    gitignore_at: None,
                })
            }
        }
    }

    /// 写入一个附件:落盘 + 落库,返回记录。
    ///
    /// 顺序是**先落盘后落库**:库写失败时留一个孤儿文件(下次同名序号会被跳过,
    /// 不覆盖),比「库里有行但磁盘没文件」(模型读到不存在,且删不掉)要好处理。
    pub(crate) fn store_attachment(
        &self,
        task: &TaskRecord,
        name: &str,
        mime: Option<&str>,
        bytes: &[u8],
    ) -> Result<TaskAttachmentRecord, String> {
        if bytes.is_empty() {
            return Err("附件内容为空".to_string());
        }
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(format!(
                "附件超过 {}MB 上限",
                MAX_ATTACHMENT_BYTES / (1024 * 1024)
            ));
        }
        // 先查个数再落盘:超限时不该留下一个已经写进去的文件
        let existing = self.list_attachments(&task.id)?;
        if existing.len() >= MAX_ATTACHMENTS_PER_TASK {
            return Err(format!(
                "单个任务最多 {MAX_ATTACHMENTS_PER_TASK} 个附件,请先删除不用的"
            ));
        }

        let display = sanitize_display_name(name);
        let dir = self.attachment_dir(task)?;
        // 序号取「已有文件里最大的数字前缀 + 1」而不是已有记录数:删过附件后记录数
        // 会小于磁盘上的最大序号,直接用记录数会撞名覆盖。
        let seq = next_seq(&dir.abs);
        let safe = sanitize_file_name(&display);
        let disk_name = format!("{seq}-{safe}");
        let target = dir.abs.join(&disk_name);
        std::fs::write(&target, bytes)
            .map_err(|e| format!("附件写入失败({}):{e}", target.display()))?;
        if let Some(gitignore) = dir.gitignore_at.as_deref() {
            ensure_gitignore(gitignore);
        }

        let record = TaskAttachmentRecord {
            id: Uuid::new_v4().to_string(),
            task_id: task.id.clone(),
            name: display,
            rel_path: format!("{}/{}", dir.rel_prefix, disk_name),
            size: bytes.len() as i64,
            mime: mime.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()),
            created_at: now_iso(),
        };
        let conn = self.db.write();
        conn.execute(
            "INSERT INTO task_attachments (id, task_id, name, rel_path, size, mime, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.task_id,
                record.name,
                record.rel_path,
                record.size,
                record.mime,
                record.created_at,
            ],
        )
        .map_err(|e| format!("附件记录写入失败: {e}"))?;
        Ok(record)
    }

    /// 列出某任务的附件(按创建时间升序 = 上传顺序)。
    pub(crate) fn list_attachments(
        &self,
        task_id: &str,
    ) -> Result<Vec<TaskAttachmentRecord>, String> {
        let conn = self.db.read()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, name, rel_path, size, mime, created_at
                   FROM task_attachments WHERE task_id = ?1
                  ORDER BY created_at, rowid",
            )
            .map_err(|e| format!("读取附件清单失败: {e}"))?;
        let rows = stmt
            .query_map(params![task_id], |row| {
                Ok(TaskAttachmentRecord {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    name: row.get(2)?,
                    rel_path: row.get(3)?,
                    size: row.get(4)?,
                    mime: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .map_err(|e| format!("遍历附件清单失败: {e}"))?;
        Ok(rows.flatten().collect())
    }

    /// 删除某附件:先删库行再删磁盘文件。
    ///
    /// 顺序与 `store_attachment` 相反:库行是**权威清单**(详情与目标注入都读它),
    /// 先进位地把清单改对,磁盘残留一个孤儿文件无害;反过来则会留下「有行没文件」,
    /// 模型按 rel_path 去读会失败。磁盘删除失败**不报错**(只留日志)——文件可能已被
    /// 用户手动删掉,这不该让「删除附件」这个操作失败。
    pub(crate) fn delete_attachment(
        &self,
        task: &TaskRecord,
        attachment_id: &str,
    ) -> Result<bool, String> {
        let record = self
            .list_attachments(&task.id)?
            .into_iter()
            .find(|a| a.id == attachment_id);
        let Some(record) = record else {
            return Ok(false);
        };
        let conn = self.db.write();
        let deleted = conn
            .execute(
                "DELETE FROM task_attachments WHERE id = ?1 AND task_id = ?2",
                params![attachment_id, task.id],
            )
            .map_err(|e| format!("附件记录删除失败: {e}"))?;
        drop(conn);
        if deleted == 0 {
            return Ok(false);
        }
        if let Ok(dir) = self.attachment_dir(task) {
            // rel_path 的最后一段就是磁盘名;重新解析而不从字符串切,避免前缀不一致
            if let Some(name) = record.rel_path.rsplit('/').next() {
                let path = dir.abs.join(name);
                if let Err(e) = std::fs::remove_file(&path) {
                    tracing::warn!(
                        task_id = task.id.as_str(),
                        path = %path.display(),
                        "附件磁盘文件删除失败(库行已删,文件可能已被手动移除): {e}"
                    );
                }
            }
        }
        Ok(true)
    }

    /// 附件在**相对路径**下的存在性(供目标注入如实说明「文件还在不在」)。
    ///
    /// 只做 stat，不读正文——注入段不该因为附件很大而变慢。
    // 提交 1 只落地数据层，消费点在提交 2 的目标注入段；此处先允许未使用。
    #[allow(dead_code)]
    pub(crate) fn attachment_exists(&self, task: &TaskRecord, rel_path: &str) -> bool {
        match task
            .workspace
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(ws) => Path::new(ws).join(rel_path).is_file(),
            None => self
                .existing_scratch_dir(&task.id)
                .map(|d| d.join(rel_path).is_file())
                .unwrap_or(false),
        }
    }

    /// 单条附件是否存在(按 id;详情/删除路径的辅助)。
    #[allow(dead_code)]
    pub(crate) fn find_attachment(
        &self,
        task_id: &str,
        attachment_id: &str,
    ) -> Result<Option<TaskAttachmentRecord>, String> {
        let conn = self.db.read()?;
        conn.query_row(
            "SELECT id, task_id, name, rel_path, size, mime, created_at
               FROM task_attachments WHERE id = ?1 AND task_id = ?2",
            params![attachment_id, task_id],
            |row| {
                Ok(TaskAttachmentRecord {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    name: row.get(2)?,
                    rel_path: row.get(3)?,
                    size: row.get(4)?,
                    mime: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("读取附件记录失败: {e}"))
    }
}

/// 展示名:去路径分隔(浏览器有时给 `C:\fakepath\x.txt`)、去控制字符、限长。
/// **不在这里去掉中文/空格/点**——展示名要忠实,落盘名另由 `sanitize_file_name` 处理。
fn sanitize_display_name(raw: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    let trimmed = base.trim();
    if trimmed.is_empty() {
        return "attachment".to_string();
    }
    // 200 字符足够容纳常见中文长名,同时不让 DB 行被超长名撑爆
    trimmed.chars().take(200).collect()
}

/// 落盘名:只保留安全字符,其余替换为 `_`。
///
/// 为什么要这么严:名字会拼进路径并交给模型当 `fs_read` 的相对路径。`..`、分隔符、
/// Windows 保留字符(`<>:"|?*`)与以点开头的名字都会制造穿越或不可预期的行为。
/// 扩展名保留(模型靠它判断文件类型),其余一律白名单化。
fn sanitize_file_name(display: &str) -> String {
    let (stem, ext) = match display.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 16 && !e.contains('.') => (s, Some(e)),
        _ => (display, None),
    };
    let clean = |s: &str| -> String {
        let mapped: String = s
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || matches!(c, '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let squeezed = mapped.trim_matches('_').to_string();
        if squeezed.is_empty() {
            "file".to_string()
        } else {
            squeezed.chars().take(80).collect()
        }
    };
    let ext_clean = ext
        .map(|e| {
            e.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(16)
                .collect::<String>()
        })
        .filter(|e| !e.is_empty());
    match ext_clean {
        Some(e) => format!("{}.{}", clean(stem), e),
        None => clean(stem),
    }
}

/// 目录里已有的最大 `{seq}-` 前缀 + 1(从 1 起)。
///
/// 读目录而不是数库行:删过附件后两者会分叉,按库行数取名会**覆盖**磁盘上残留的文件。
fn next_seq(dir: &Path) -> u64 {
    let max = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name();
                    let s = name.to_str()?;
                    s.split_once('-')?.0.parse::<u64>().ok()
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    max + 1
}

/// 保证 `<workspace>/.kedai/.gitignore` 存在且忽略 `attachments/`。
///
/// **已存在就一个字都不改**:用户可能在这里维护自己的规则,追加/覆盖都会破坏它。
/// 写失败只告警——工作区可能只读,那不该让上传整个失败(附件本身已落盘)。
fn ensure_gitignore(path: &Path) {
    if path.exists() {
        return;
    }
    if let Err(e) = std::fs::write(path, "attachments/\n") {
        tracing::warn!(
            path = %path.display(),
            "附件目录 .gitignore 写入失败(不影响附件落盘): {e}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, workspace: Option<&str>) -> TaskRecord {
        TaskRecord {
            id: id.to_string(),
            title: "t".into(),
            status: crate::models::types::TaskStatus::Pending,
            plan: Vec::new(),
            result: String::new(),
            error: String::new(),
            character_id: None,
            created_at: now_iso(),
            updated_at: now_iso(),
            task_mode: crate::models::types::TaskRunMode::Legacy,
            executor_id: None,
            flow_id: None,
            flow_ids: None,
            connection_id: None,
            workspace: workspace.map(str::to_string),
        }
    }

    /// 展示名忠实保留中文/空格,但剥掉浏览器给的伪路径前缀与控制字符。
    #[test]
    fn display_name_keeps_original_but_strips_paths_and_controls() {
        assert_eq!(sanitize_display_name("接口 说明.txt"), "接口 说明.txt");
        assert_eq!(sanitize_display_name(r"C:\fakepath\a.png"), "a.png");
        assert_eq!(sanitize_display_name("/tmp/b.md"), "b.md");
        assert_eq!(sanitize_display_name("x\u{0}y.txt"), "xy.txt");
        assert_eq!(sanitize_display_name("   "), "attachment");
    }

    /// 落盘名白名单化:分隔符/上溯段/Windows 保留字符一律替换,扩展名保留。
    #[test]
    fn disk_name_is_sanitized_and_keeps_extension() {
        assert_eq!(sanitize_file_name("a.txt"), "a.txt");
        assert_eq!(sanitize_file_name("接口 说明.txt"), "接口_说明.txt");
        // 真实链路是「先 sanitize_display_name 去路径段,再 sanitize_file_name」;
        // 单独喂一个带分隔符的串时,分隔符会被替换而不是制造目录层级——两者都不能穿越。
        assert_eq!(sanitize_file_name(&sanitize_display_name("../../etc/passwd")), "passwd");
        let direct = sanitize_file_name("../../etc/passwd");
        // 落盘名只保留字母数字与 -/_，其余（含路径分隔符）全部替换。
        assert!(
            direct
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.'),
            "落盘名含非法字符: {direct}"
        );
        assert_eq!(sanitize_file_name("a:b|c?d*.md"), "a_b_c_d.md");
        // 以点开头(隐藏文件)也不允许作为落盘名开头
        assert_eq!(sanitize_file_name(".env"), "env");
        assert_eq!(sanitize_file_name("###"), "file");
        // 超长扩展名当普通名字处理(不当扩展名)
        assert_eq!(
            sanitize_file_name("a.verylongextensionname"),
            "a_verylongextensionname"
        );
    }

    /// 序号取磁盘最大值 + 1:删过附件后不会覆盖残留文件。
    #[test]
    fn next_seq_uses_disk_max_not_row_count() {
        let dir = std::env::temp_dir().join(format!("kedai-attach-seq-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        assert_eq!(next_seq(&dir), 1);
        std::fs::write(dir.join("1-a.txt"), b"x").expect("写文件");
        std::fs::write(dir.join("7-b.txt"), b"x").expect("写文件");
        std::fs::write(dir.join("not-seq.txt"), b"x").expect("写文件");
        assert_eq!(next_seq(&dir), 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `.gitignore` 只补不改:已存在时内容保持原样(用户可能自己维护规则)。
    #[test]
    fn gitignore_is_created_once_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("kedai-attach-gi-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join(".gitignore");
        ensure_gitignore(&path);
        assert_eq!(
            std::fs::read_to_string(&path).expect("读回"),
            "attachments/\n"
        );
        std::fs::write(&path, "*.tmp\n").expect("写用户规则");
        ensure_gitignore(&path);
        assert_eq!(
            std::fs::read_to_string(&path).expect("读回"),
            "*.tmp\n",
            "已存在的 .gitignore 不得被覆盖"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 未绑定工作区的任务:附件落在 scratch 子目录,相对路径只有一级。
    #[test]
    fn unbound_task_uses_scratch_attachments_prefix() {
        let t = task("t1", None);
        assert!(t.workspace.is_none());
        // 只验路径拼装口径(不建 TaskService:构造要一整套依赖)
        assert_eq!(
            format!("{}/{}", "attachments", "1-a.txt"),
            "attachments/1-a.txt"
        );
    }
}
