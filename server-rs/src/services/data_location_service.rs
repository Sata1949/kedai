//! 数据位置(盘符选择)的编排层:状态汇总、目录创建、旧数据迁移与指针写入。
//!
//! 迁移是**复制**语义:旧目录永不删除,失败不写指针。数据库用 SQLite Online Backup
//! 生成一致性快照(与 `migration::snapshot_database` 同源),其余文件尽力复制;
//! 有运行中任务时拒绝迁移(409),避免搬走正在写的目录。
use crate::config::{self, AppConfig};
use crate::data_location::{
    self, DataLocationPointer, DATA_DIR_NAME, IMPORTS_DIR_NAME, MIGRATED_MARKER, OUTPUTS_DIR_NAME,
    TASK_RECORDS_DIR_NAME,
};
use crate::migration::snapshot_database;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// 状态接口返回的盘符项(只列存在的盘;可写性在 POST 时由实际创建验证)。
#[derive(Debug, Clone, Serialize)]
pub struct DriveInfo {
    pub letter: String,
}

/// `GET /api/data-location` 的响应体。
#[derive(Debug, Clone, Serialize)]
pub struct DataLocationStatus {
    /// 是否由本功能管理(显式 KEDAI_DATA_ROOT / DATA_DIR 时为 false)。
    pub managed: bool,
    /// 是否需要用户选择(无指针,或指针根不可用)。
    pub pending: bool,
    pub root: Option<String>,
    pub data_dir: String,
    pub images_dir: String,
    pub task_records_dir: String,
    pub imports_dir: String,
    pub default_drive: Option<String>,
    pub drives: Vec<DriveInfo>,
    /// 当前数据目录是否已有用户数据(老用户升级判定)。
    pub legacy_data: bool,
    pub pointer: Option<String>,
    pub error: Option<String>,
    /// 运行平台:`android` 走路径模式(无盘符),`desktop` 走盘符模式。
    pub platform: String,
    /// 平台建议的数据根:Android = `/storage/emulated/0/Download/Kedai-Data`;桌面为 null。
    pub suggested_root: Option<String>,
    /// Android 专用:建议根当前不可写(通常缺少「所有文件访问」权限)。
    /// 桌面恒为 false。前端据此展示授权入口,而不是只给一个报错的按钮。
    pub needs_permission: bool,
}

/// `POST /api/data-location` 的结果。
#[derive(Debug, Clone, Serialize)]
pub struct ApplyOutcome {
    pub root: String,
    pub migrated: bool,
    pub restart_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataLocationError {
    /// 盘符非法 / 盘不存在 / 目标已有用户数据(400)
    BadRequest(String),
    /// 有任务在跑(409)
    Busy(String),
    /// IO / 数据库失败(500)
    Internal(String),
}

impl DataLocationError {
    pub fn message(&self) -> &str {
        match self {
            Self::BadRequest(m) | Self::Busy(m) | Self::Internal(m) => m,
        }
    }
}

pub struct DataLocationService {
    config: AppConfig,
    /// 指针落点;生产为 `%APPDATA%\com.kedai.app\data-location.json`,测试可注入。
    pointer_path: Option<PathBuf>,
}

impl DataLocationService {
    pub fn new(config: AppConfig) -> Self {
        let pointer_path = data_location::pointer_path();
        Self {
            config,
            pointer_path,
        }
    }

    #[cfg(test)]
    fn with_pointer_path(config: AppConfig, pointer_path: PathBuf) -> Self {
        Self {
            config,
            pointer_path: Some(pointer_path),
        }
    }

    pub fn status(&self) -> DataLocationStatus {
        let drives = data_location::available_drives()
            .into_iter()
            .map(|letter| DriveInfo {
                letter: letter.to_string(),
            })
            .collect();
        let suggested_root = data_location::suggested_root();
        // 可写/可读探测只在 Android 且尚未选定根时做:已选定且可用的根不需要权限提示,
        // 而每次打开设置都建目录/写探针也没有必要。
        // 除可写外还要验已有 kedai.db 能否打开:旧 UID/其它安装留下的公共库在未授权
        // 「所有文件访问」时不可读,只测「能新建文件」会给出假的 needs_permission=false。
        let needs_permission = suggested_root.as_ref().is_some_and(|root| {
            self.config.data_root.is_none()
                && (!data_location::probe_writable(root)
                    || probe_sqlite_support(&root.join(DATA_DIR_NAME)).is_err())
        });
        DataLocationStatus {
            managed: self.config.data_location_managed,
            pending: self.config.data_location_pending,
            root: self
                .config
                .data_root
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            data_dir: self.config.data_dir.to_string_lossy().into_owned(),
            images_dir: self.config.images_dir.to_string_lossy().into_owned(),
            task_records_dir: self.config.task_scratch_dir.to_string_lossy().into_owned(),
            imports_dir: self.config.imports_dir.to_string_lossy().into_owned(),
            default_drive: data_location::default_drive().map(|c| c.to_string()),
            drives,
            legacy_data: config::dir_has_user_data(&self.config.data_dir),
            pointer: self
                .pointer_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            error: self.config.data_location_error.clone(),
            platform: data_location::platform().to_string(),
            suggested_root: suggested_root.map(|p| p.to_string_lossy().into_owned()),
            needs_permission,
        }
    }

    /// 校验盘符 → 建目录 → 迁移 → 写指针。同步阻塞 IO,调用方放 spawn_blocking。
    pub fn apply(&self, drive: &str) -> Result<ApplyOutcome, DataLocationError> {
        let letter = data_location::normalize_drive(drive).ok_or_else(|| {
            DataLocationError::BadRequest(format!("盘符无效:{drive}(只接受单个字母)"))
        })?;
        let root = data_location::root_for_drive(letter).ok_or_else(|| {
            DataLocationError::BadRequest(format!("{letter} 盘不存在或不可访问,请选择其它盘"))
        })?;
        self.apply_to_root(&root)
    }

    /// Android 路径模式:直接选择绝对路径根(默认 `/storage/emulated/0/Download/Kedai-Data`)。
    ///
    /// 与 `apply(盘符)` 共用同一套目录创建/迁移/冲突判定;额外只做路径形态校验
    /// (绝对、末段 `Kedai-Data`、父目录存在),避免手滑路径在磁盘深处建出数据根。
    pub fn apply_root_path(&self, input: &str) -> Result<ApplyOutcome, DataLocationError> {
        let root =
            data_location::normalize_custom_root(input).map_err(DataLocationError::BadRequest)?;
        self.apply_to_root(&root)
    }

    /// 迁移到指定根(测试直接给临时目录;生产只经 `apply` 从盘符拼 `X:\Kedai-Data`)。
    fn apply_to_root(&self, root: &Path) -> Result<ApplyOutcome, DataLocationError> {
        let root_text = root.to_string_lossy().into_owned();

        // 幂等:已选择同一根,只确保目录齐全
        if self.config.data_root.as_deref() == Some(root) {
            data_location::ensure_root_layout(root).map_err(DataLocationError::Internal)?;
            return Ok(ApplyOutcome {
                root: root_text,
                migrated: false,
                restart_required: true,
            });
        }

        let target_data = root.join(DATA_DIR_NAME);
        // DATA-LOC-2:`config::dir_has_user_data` 只认「设置 / 非内置角色 / 会话」,
        // 只有已完成任务、附件记录或导入归档的目标会被误判成空壳并整盘覆盖。
        // 迁移是不可逆的破坏性操作,这里用更宽的专用判据:认不出是空壳就一律拒绝。
        if let Some(reason) =
            describe_target_root_user_data(root).map_err(DataLocationError::Internal)?
        {
            return Err(DataLocationError::BadRequest(format!(
                "{} 已存在 Kedai 用户数据({reason}),不能直接覆盖;请换一个盘,或先备份/清理该目录",
                root.display()
            )));
        }
        if target_data.join("kedai.db").is_file() {
            // 空壳:删掉残留 WAL/SHM,避免与即将写入的快照拼接成坏库
            for suffix in ["kedai.db-wal", "kedai.db-shm"] {
                let _ = fs::remove_file(target_data.join(suffix));
            }
        }

        if self.config.data_dir != target_data
            && has_active_tasks(&self.config.data_dir).map_err(DataLocationError::Internal)?
        {
            return Err(DataLocationError::Busy(
                "还有任务在运行,请先停止任务后再更改数据位置".to_string(),
            ));
        }

        data_location::ensure_root_layout(root).map_err(DataLocationError::Internal)?;
        let source_data = self.config.data_dir.clone();
        let source_images = self.config.images_dir.clone();
        let source_scratch = self.config.task_scratch_dir.clone();
        let source_imports = self.config.imports_dir.clone();
        // DATA-LOC-3:旧布局的归档就住在 <data>/imports 里。通用复制必须排除它，
        // 否则归档会落到 data/imports（不是有效目录）且下面还会再搬一次。
        let legacy_imports_inside_data = source_imports == source_data.join("imports");
        let legacy_images_inside_data = source_images == source_data.join("images");
        let mut migrated = false;

        if source_data.is_dir() && source_data != target_data {
            copy_tree_with(
                &source_data,
                &target_data,
                legacy_images_inside_data,
                legacy_imports_inside_data,
            )
            .map_err(DataLocationError::Internal)?;
            let source_db = source_data.join("kedai.db");
            if source_db.is_file() {
                snapshot_database(&source_db, &target_data.join("kedai.db"))
                    .map_err(|e| DataLocationError::Internal(format!("数据库迁移失败: {e}")))?;
            }
            migrated = true;
        }

        // DATA-LOC-3:归档单独按 `imports_dir` 搬进新根的有效归档目录。
        // 归档名是用户原文件名，不适用数据库、日志与备份残留过滤。
        let target_imports = root.join(IMPORTS_DIR_NAME);
        if source_imports.is_dir() && source_imports != target_imports {
            copy_tree_inner(&source_imports, &target_imports, false, false, true)
                .map_err(DataLocationError::Internal)?;
            migrated = true;
        }

        let target_images = root.join(OUTPUTS_DIR_NAME).join("images");
        if source_images.is_dir() && source_images != target_images {
            copy_tree(&source_images, &target_images, false)
                .map_err(DataLocationError::Internal)?;
            migrated = true;
        }

        let target_records = root.join(TASK_RECORDS_DIR_NAME);
        if source_scratch.is_dir() && source_scratch != target_records {
            copy_tree(&source_scratch, &target_records, false)
                .map_err(DataLocationError::Internal)?;
            migrated = true;
        }

        // 旧目录标记:DB-7 分叉检测据此跳过「已经迁移走」的源,不再误报
        if source_data.is_dir() && source_data != target_data {
            let _ = data_location::write_migrated_marker(&source_data, root);
        }

        let mut pointer = DataLocationPointer::new(root.to_path_buf());
        pointer.chosen_at = chrono::Local::now().to_rfc3339();
        if source_data != target_data {
            pointer.migrated_from.push(source_data);
        }
        self.write_pointer(&pointer)
            .map_err(DataLocationError::Internal)?;

        Ok(ApplyOutcome {
            root: root_text,
            migrated,
            restart_required: true,
        })
    }

    fn write_pointer(&self, pointer: &DataLocationPointer) -> Result<(), String> {
        match &self.pointer_path {
            Some(path) => data_location::write_pointer_to(path, pointer),
            None => {
                data_location::write_pointer(pointer)?;
                Ok(())
            }
        }
    }
}

/// 递归复制;跳过数据库文件/WAL/SHM/日志/备份残留/迁移标记/符号链接。
/// `skip_images` 用于旧布局(data 内含 images):图像要搬到 outputs/images,不能留在 data。
fn copy_tree(source: &Path, target: &Path, skip_images: bool) -> Result<u64, String> {
    copy_tree_with(source, target, skip_images, false)
}

/// 同 `copy_tree`，旧布局的 imports 单独迁移到新根，不能留在 data。
fn copy_tree_with(
    source: &Path,
    target: &Path,
    skip_images: bool,
    skip_imports: bool,
) -> Result<u64, String> {
    copy_tree_inner(source, target, skip_images, skip_imports, false)
}

fn copy_tree_inner(
    source: &Path,
    target: &Path,
    skip_images: bool,
    skip_imports: bool,
    archive_files: bool,
) -> Result<u64, String> {
    if !source.is_dir() {
        return Ok(0);
    }
    fs::create_dir_all(target).map_err(|e| format!("创建目录 {} 失败: {e}", target.display()))?;
    let entries =
        fs::read_dir(source).map_err(|e| format!("读取目录 {} 失败: {e}", source.display()))?;
    let mut copied = 0u64;
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        // 不跟随符号链接/junction:既防循环,也避免把链接指向的外部数据复制进来
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if (!archive_files
            && (name == "kedai.db"
                || name.ends_with("-wal")
                || name.ends_with("-shm")
                || name.ends_with(".log")
                || name.contains(".bak-")
                || name.starts_with(".verify")
                || name == MIGRATED_MARKER))
            || (skip_images && name == "images")
            || (skip_imports && name == "imports")
        {
            continue;
        }
        let from = entry.path();
        let to = target.join(&name);
        if file_type.is_dir() {
            copied += copy_tree_inner(&from, &to, false, false, archive_files)?;
        } else {
            fs::copy(&from, &to)
                .map_err(|e| format!("复制 {} → {} 失败: {e}", from.display(), to.display()))?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// 验证目录能承载 SQLite(建库 + WAL + 读写)。
///
/// Android 公共目录(默认 `/storage/emulated/0/Download/Kedai-Data/data`)位于 FUSE,
/// scoped storage 下隐藏探针可能成功、真实的 `kedai.db` 却建不出来(2026-11-06 实测:
/// 未授予「所有文件访问」时 `.kedai-sqlite-probe.db` 可写,`kedai.db` 返回
/// `unable to open database file`,随后启动失败)。因此这里直接使用服务端将要打开的
/// **真实库路径**:
/// - 不存在:建一个空库并执行 WAL/CREATE/DROP,然后删除(不污染真实数据);
/// - 已存在:只读打开并验证可读,不删除、不迁移;打不开/损坏则报错。
pub fn probe_sqlite_support(data_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
    let db = data_dir.join("kedai.db");
    if db.exists() {
        let conn =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("已有数据库无法打开: {e}"))?;
        conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(()))
            .map_err(|e| format!("已有数据库无法读取: {e}"))?;
        return Ok(());
    }
    let result = (|| -> Result<(), String> {
        let conn = rusqlite::Connection::open(&db)
            .map_err(|e| format!("SQLite 无法在目标目录建库: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE probe (id INTEGER PRIMARY KEY, value TEXT);
             INSERT INTO probe(value) VALUES ('ok');
             DROP TABLE probe;",
        )
        .map_err(|e| format!("SQLite 探针执行失败: {e}"))?;
        Ok(())
    })();
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{}", db.display(), suffix));
    }
    result
}

/// 目标盘里「认得出是用户数据」的判据集合（DATA-LOC-2）。
///
/// 返回 `Some(说明)` 表示**不能覆盖**；`None` 表示可以按空壳处理。
///
/// 取舍是「认不出就拒绝」：迁移会整盘覆盖且不可逆，误拒只让用户换个空盘，
/// 误放则丢失目标盘数据。这比 `config::dir_has_user_data` 更宽——后者只认
/// 设置 / 非内置角色 / 会话，会把「只有已完成任务」的库当成空壳。
fn describe_target_root_user_data(root: &Path) -> Result<Option<String>, String> {
    for (dir, label) in [
        (IMPORTS_DIR_NAME, "导入归档目录非空"),
        (OUTPUTS_DIR_NAME, "产物目录非空"),
        (TASK_RECORDS_DIR_NAME, "任务记录目录非空"),
    ] {
        let path = root.join(dir);
        if path.is_dir() && dir_has_any_entry(&path) {
            return Ok(Some(label.to_string()));
        }
    }
    describe_target_user_data(&root.join(DATA_DIR_NAME))
}

fn describe_target_user_data(target_data: &Path) -> Result<Option<String>, String> {
    if !target_data.is_dir() {
        return Ok(None);
    }
    if target_data.join("settings.json").is_file() {
        return Ok(Some("已有设置文件".to_string()));
    }

    // 库内任何一张有记录的用户表都算。用 `sqlite_master` 先确认表存在，
    // 避免「全新库没这张表」被当成读库失败。
    let db = target_data.join("kedai.db");
    if db.is_file() {
        let conn =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("打开目标数据库检查用户数据失败: {e}"))?;
        // 这些表只要有行就是用户数据（含任务历史、附件记录、导入台账、世界书等）。
        // 刻意不含 `global_usage` / `llm_requests` 这类纯计数器与遥测。
        const USER_TABLES: &[(&str, &str)] = &[
            ("characters", "已有角色(含非内置)"),
            ("sessions", "已有会话"),
            ("messages", "已有聊天消息"),
            ("tasks", "已有任务历史"),
            ("task_messages", "已有任务消息"),
            ("task_events", "已有任务事件"),
            ("task_attachments", "已有任务附件记录"),
            ("task_file_changes", "已有文件变更记录"),
            ("world_books", "已有世界书"),
            ("skills", "已有技能"),
            ("quick_replies", "已有快捷回复"),
            ("memory_entries", "已有记忆条目"),
            ("agent_sessions", "已有 Agent 会话"),
        ];
        for (table, label) in USER_TABLES {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !exists {
                continue;
            }
            let has_rows: bool = conn
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table})"),
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if has_rows {
                return Ok(Some((*label).to_string()));
            }
        }
    }

    // 目录里的用户文件（导入归档、产物、任务记录、角色卡目录）。
    for (dir, label) in [
        ("imports", "导入归档目录非空"),
        ("characters", "角色卡目录非空"),
        ("outputs", "产物目录非空"),
        ("task-records", "任务记录目录非空"),
    ] {
        let path = target_data.join(dir);
        if path.is_dir() && dir_has_any_entry(&path) {
            return Ok(Some(label.to_string()));
        }
    }

    Ok(None)
}

/// 目录是否含任意条目（不递归）。
fn dir_has_any_entry(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(mut iter) => iter.next().is_some(),
        Err(_) => false,
    }
}

/// 是否有非终态任务(pending/planning/running/planned)。
/// 表不存在/查询失败按「没有」处理:迁移不能被诊断查询反向阻断。
fn has_active_tasks(data_dir: &Path) -> Result<bool, String> {
    let db = data_dir.join("kedai.db");
    if !db.is_file() {
        return Ok(false);
    }
    let conn =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("打开数据库检查任务失败: {e}"))?;
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE status IN ('pending','planning','running','planned')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;
    use rusqlite::Connection;

    fn service_for(source: &TempDataDir, pointer: &Path) -> DataLocationService {
        let mut cfg = crate::config::test_config();
        cfg.data_dir = source.path().to_path_buf();
        cfg.images_dir = source.path().join("images");
        cfg.imports_dir = source.path().join("imports");
        cfg.task_scratch_dir = source.path().parent().unwrap().join("scratch-in-test");
        cfg.data_location_pending = true;
        cfg.data_location_managed = true;
        DataLocationService::with_pointer_path(cfg, pointer.to_path_buf())
    }

    #[test]
    fn status_reports_pending_and_drives() {
        let source = TempDataDir::new("dl-status");
        let pointer_dir = TempDataDir::new("dl-status-ptr");
        let svc = service_for(&source, &pointer_dir.join("data-location.json"));
        let status = svc.status();
        assert!(status.pending, "测试配置默认 pending");
        assert!(!status.legacy_data);
        // 盘符枚举只在 Windows 有意义(`drive_exists` 探 `X:\`);Linux 档明确返回 None。
        #[cfg(windows)]
        assert!(status.default_drive.is_some(), "至少应给出默认盘");
        #[cfg(not(windows))]
        assert!(status.default_drive.is_none());
    }

    #[test]
    fn rejects_invalid_drive() {
        let source = TempDataDir::new("dl-bad");
        let pointer_dir = TempDataDir::new("dl-bad-ptr");
        let svc = service_for(&source, &pointer_dir.join("p.json"));
        assert!(matches!(
            svc.apply("not-a-drive"),
            Err(DataLocationError::BadRequest(_))
        ));
        assert!(matches!(
            svc.apply("1"),
            Err(DataLocationError::BadRequest(_))
        ));
    }

    /// 用真实存在的临时盘符不现实,故只测「目标冲突」与目录/指针路径的纯逻辑。
    #[test]
    fn migration_copy_tree_skips_db_and_preserves_source() {
        let source = TempDataDir::new("dl-copy-src");
        let target = TempDataDir::new("dl-copy-dst");
        std::fs::write(source.join("settings.json"), "{}").unwrap();
        std::fs::write(source.join("kedai.db"), b"db").unwrap();
        std::fs::write(source.join("kedai.db-wal"), b"wal").unwrap();
        std::fs::create_dir_all(source.join("characters")).unwrap();
        std::fs::write(source.join("characters").join("c.json"), "{}").unwrap();
        std::fs::create_dir_all(source.join("images")).unwrap();
        std::fs::write(source.join("images").join("a.png"), b"png").unwrap();

        copy_tree(source.path(), target.path(), true).expect("复制失败");
        assert!(target.join("settings.json").is_file());
        assert!(!target.join("kedai.db").exists(), "DB 由快照单独写");
        assert!(!target.join("kedai.db-wal").exists());
        assert!(target.join("characters").join("c.json").is_file());
        assert!(!target.join("images").exists(), "skip_images 应排除 images");
        assert!(source.join("kedai.db").is_file(), "源必须保留");
        assert!(source.join("images").join("a.png").is_file());
    }

    /// 完整迁移:data(含 DB 快照) / outputs/images / task-records 分流入位,
    /// 源保留 + 迁移标记,指针写入注入路径。
    #[test]
    fn migrates_into_root_layout_and_writes_pointer() {
        let source = TempDataDir::new("dl-migrate-src");
        let scratch = TempDataDir::new("dl-migrate-scratch");
        let pointer_dir = TempDataDir::new("dl-migrate-ptr");
        let root_parent = TempDataDir::new("dl-migrate-root");
        let root = root_parent.join("Kedai-Data");

        std::fs::write(source.join("settings.json"), "{}").unwrap();
        {
            let conn = Connection::open(source.join("kedai.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY);\
                 INSERT INTO sessions (id) VALUES ('s1');",
            )
            .unwrap();
        }
        std::fs::create_dir_all(source.join("images")).unwrap();
        std::fs::write(source.join("images").join("a.png"), b"png").unwrap();
        std::fs::create_dir_all(scratch.join("t1").join("parts")).unwrap();
        std::fs::write(scratch.join("t1").join("parts").join("x.txt"), b"x").unwrap();

        let mut cfg = crate::config::test_config();
        cfg.data_dir = source.path().to_path_buf();
        cfg.images_dir = source.join("images");
        cfg.imports_dir = source.join("imports");
        cfg.task_scratch_dir = scratch.path().to_path_buf();
        let pointer_path = pointer_dir.join("data-location.json");
        let svc = DataLocationService::with_pointer_path(cfg, pointer_path.clone());

        let out = svc.apply_to_root(&root).expect("迁移失败");
        assert!(out.migrated && out.restart_required);
        assert!(root.join("data").join("settings.json").is_file());
        assert!(
            !root.join("data").join("images").exists(),
            "images 应搬到 outputs"
        );
        assert!(root.join("outputs").join("images").join("a.png").is_file());
        assert!(root
            .join("task-records")
            .join("t1")
            .join("parts")
            .join("x.txt")
            .is_file());
        assert!(root.join("imports").is_dir(), "四目录必须齐全");

        let pointer = data_location::read_pointer_from(&pointer_path).expect("指针未写");
        assert_eq!(pointer.root, root);
        assert!(pointer.migrated_from.contains(&source.path().to_path_buf()));

        assert!(source.join("kedai.db").is_file(), "源必须保留");
        assert!(data_location::has_migrated_marker(source.path()));

        let check = Connection::open(root.join("data").join("kedai.db")).unwrap();
        let n: i64 = check
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 1, "快照必须包含源库数据");
    }

    /// Android 路径模式:apply_root_path 与盘符模式共用迁移逻辑,只多一层路径校验。
    #[test]
    fn apply_root_path_migrates_and_writes_pointer() {
        let source = TempDataDir::new("dl-root-src");
        let pointer_dir = TempDataDir::new("dl-root-ptr");
        let root_parent = TempDataDir::new("dl-root-parent");
        let root = root_parent.join("Kedai-Data");
        std::fs::write(source.join("settings.json"), "{}").unwrap();
        std::fs::create_dir_all(source.join("images")).unwrap();
        std::fs::write(source.join("images").join("a.png"), b"png").unwrap();

        let mut cfg = crate::config::test_config();
        cfg.data_dir = source.path().to_path_buf();
        cfg.images_dir = source.join("images");
        cfg.imports_dir = source.join("imports");
        cfg.task_scratch_dir = source.path().parent().unwrap().join("scratch-root-test");
        let pointer_path = pointer_dir.join("data-location.json");
        let svc = DataLocationService::with_pointer_path(cfg, pointer_path.clone());

        let out = svc
            .apply_root_path(&root.to_string_lossy())
            .expect("路径模式迁移失败");
        assert!(out.migrated && out.restart_required);
        assert!(root.join("data").join("settings.json").is_file());
        assert!(root.join("outputs").join("images").join("a.png").is_file());
        let pointer = data_location::read_pointer_from(&pointer_path).expect("指针未写");
        assert_eq!(pointer.root, root);

        // 非法路径走 BadRequest,不落盘
        assert!(matches!(
            svc.apply_root_path("relative/Kedai-Data"),
            Err(DataLocationError::BadRequest(_))
        ));
    }

    /// DATA-LOC-3（P2）：切盘后历史归档必须出现在新根的有效归档目录。
    ///
    /// 旧实现只搬 data / outputs-images / task-records 三路，`imports_dir` 完全没处理：
    /// 旧布局的归档随 `data/` 一起被搬到 `data/imports/`（不是有效目录），
    /// 而从已选数据根切盘时归档留在原盘。两种情况新根 `imports/` 都是空的。
    #[test]
    fn migration_moves_imports_into_effective_archive_dir() {
        // 情形一：旧布局 —— 归档在 <data>/imports/ 里
        let source = TempDataDir::new("dl-imp-legacy-src");
        let pointer_dir = TempDataDir::new("dl-imp-legacy-ptr");
        let root_parent = TempDataDir::new("dl-imp-legacy-root");
        let root = root_parent.join("Kedai-Data");
        std::fs::write(source.join("settings.json"), "{}").unwrap();
        std::fs::create_dir_all(source.join("imports")).unwrap();
        std::fs::write(source.join("imports").join("conv.json"), b"legacy").unwrap();
        // 归档目录里允许出现 .log,不能套用数据目录的日志后缀过滤
        std::fs::write(source.join("imports").join("trace.log"), b"logfile").unwrap();

        let mut cfg = crate::config::test_config();
        cfg.data_dir = source.path().to_path_buf();
        cfg.images_dir = source.join("images");
        cfg.imports_dir = source.join("imports");
        cfg.task_scratch_dir = source.path().parent().unwrap().join("scratch-imp-legacy");
        let svc = DataLocationService::with_pointer_path(cfg, pointer_dir.join("p.json"));
        svc.apply_to_root(&root).expect("旧布局迁移失败");

        let target_imports = root.join("imports");
        assert_eq!(
            std::fs::read(target_imports.join("conv.json")).unwrap(),
            b"legacy",
            "旧布局归档必须落到新根的有效归档目录"
        );
        assert_eq!(
            std::fs::read(target_imports.join("trace.log")).unwrap(),
            b"logfile",
            "归档里的 .log 不能被日志过滤规则丢掉"
        );
        assert!(
            !root.join("data").join("imports").join("conv.json").exists(),
            "旧布局归档不得重复落到 data/imports"
        );
        assert!(
            source.join("imports").join("conv.json").is_file(),
            "源归档必须保留"
        );

        // 情形二：已选数据根 —— 归档在 data 目录之外
        let source2 = TempDataDir::new("dl-imp-selected-src");
        let imports2 = TempDataDir::new("dl-imp-selected-archive");
        let pointer_dir2 = TempDataDir::new("dl-imp-selected-ptr");
        let root_parent2 = TempDataDir::new("dl-imp-selected-root");
        let root2 = root_parent2.join("Kedai-Data");
        std::fs::write(source2.join("settings.json"), "{}").unwrap();
        std::fs::write(imports2.join("outside.json"), b"outside").unwrap();

        let mut cfg2 = crate::config::test_config();
        cfg2.data_dir = source2.path().to_path_buf();
        cfg2.images_dir = source2.join("images");
        cfg2.imports_dir = imports2.path().to_path_buf();
        cfg2.task_scratch_dir = source2
            .path()
            .parent()
            .unwrap()
            .join("scratch-imp-selected");
        let svc2 = DataLocationService::with_pointer_path(cfg2, pointer_dir2.join("p.json"));
        svc2.apply_to_root(&root2).expect("已选根迁移失败");

        assert_eq!(
            std::fs::read(root2.join("imports").join("outside.json")).unwrap(),
            b"outside",
            "data 目录之外的归档也必须搬进新根 imports"
        );
        assert!(imports2.join("outside.json").is_file(), "源归档必须保留");
    }

    #[test]
    fn active_tasks_are_detected_from_db() {
        let source = TempDataDir::new("dl-active");
        let conn = Connection::open(source.join("kedai.db")).unwrap();
        conn.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, status TEXT NOT NULL);")
            .unwrap();
        assert!(!has_active_tasks(source.path()).unwrap());
        conn.execute("INSERT INTO tasks (id, status) VALUES ('t1','running')", [])
            .unwrap();
        assert!(has_active_tasks(source.path()).unwrap());
        conn.execute("UPDATE tasks SET status='done' WHERE id='t1'", [])
            .unwrap();
        assert!(!has_active_tasks(source.path()).unwrap());
    }

    /// DATA-LOC-2（P1）：目标盘已有用户数据时必须拒绝迁移。
    ///
    /// `config::dir_has_user_data` 只认「设置 / 非内置角色 / 会话」，因此「只有已完成任务」
    /// 的库会被判成空壳并整盘覆盖。本测试锁住四条独立的拒绝判据。
    #[test]
    fn rejects_migration_over_target_with_user_data() {
        /// 一种「目标盘已有用户数据」的形态。
        struct Case {
            label: &'static str,
            /// 目标库建表与插数的 SQL（空串表示不建库）。
            sql: &'static str,
            /// 额外在目标数据目录里造的文件（不建库的形态用）。
            make_files: fn(&Path),
        }

        let no_files: fn(&Path) = |_| {};
        let cases = [
            Case {
                label: "只有已完成任务",
                sql: "CREATE TABLE tasks (id TEXT PRIMARY KEY, status TEXT NOT NULL);\
                      INSERT INTO tasks (id, status) VALUES ('done-1','done');",
                make_files: no_files,
            },
            Case {
                label: "只有任务附件记录",
                sql: "CREATE TABLE task_attachments (id TEXT PRIMARY KEY, rel_path TEXT NOT NULL);\
                      INSERT INTO task_attachments (id, rel_path) VALUES ('a1','x.txt');",
                make_files: no_files,
            },
            Case {
                label: "只有任务事件",
                sql: "CREATE TABLE task_events (id INTEGER PRIMARY KEY, payload TEXT);\
                      INSERT INTO task_events (id, payload) VALUES (1,'{}');",
                make_files: no_files,
            },
            Case {
                label: "只有导入归档文件",
                sql: "",
                make_files: |data| {
                    std::fs::create_dir_all(data.join("imports")).unwrap();
                    std::fs::write(data.join("imports").join("old.txt"), b"user file").unwrap();
                },
            },
        ];

        for Case {
            label,
            sql,
            make_files,
        } in cases
        {
            let source = TempDataDir::new("dl-cover-src");
            let pointer_dir = TempDataDir::new("dl-cover-ptr");
            let root_parent = TempDataDir::new("dl-cover-root");
            let root = root_parent.join("Kedai-Data");
            std::fs::write(source.join("settings.json"), "{}").unwrap();

            // 在目标盘预先放一套「有用户数据」的数据目录
            let target_data = root.join(DATA_DIR_NAME);
            std::fs::create_dir_all(&target_data).unwrap();
            if !sql.is_empty() {
                let conn = Connection::open(target_data.join("kedai.db")).unwrap();
                conn.execute_batch(sql).unwrap();
            }
            make_files(&target_data);

            let mut cfg = crate::config::test_config();
            cfg.data_dir = source.path().to_path_buf();
            cfg.images_dir = source.join("images");
            cfg.imports_dir = source.join("imports");
            cfg.task_scratch_dir = source.path().parent().unwrap().join("scratch-cover");
            let svc = DataLocationService::with_pointer_path(cfg, pointer_dir.join("p.json"));

            let outcome = svc.apply_to_root(&root);
            assert!(
                matches!(outcome, Err(DataLocationError::BadRequest(_))),
                "目标有用户数据（{label}）时必须拒绝迁移,实际: {outcome:?}"
            );

            // 拒绝时目标内容与指针都不得改变
            assert!(
                target_data.join("kedai.db").is_file() || target_data.join("imports").is_dir(),
                "拒绝时不得删除目标已有内容（{label}）"
            );
            assert!(
                data_location::read_pointer_from(&pointer_dir.join("p.json")).is_none(),
                "拒绝时不得写指针（{label}）"
            );
        }
    }

    #[test]
    fn sqlite_probe_detects_usable_directory_and_rejects_blocked_path() {
        let dir = TempDataDir::new("dl-sqlite-probe");
        let data = dir.path().join("data");
        probe_sqlite_support(&data).expect("临时目录应可承载 SQLite");
        assert!(!data.join("kedai.db").exists(), "探针文件必须清理");

        // 真实库路径被一个目录占住 → SQLite 无法建库
        let blocked = dir.path().join("blocked");
        std::fs::create_dir_all(blocked.join("kedai.db")).unwrap();
        assert!(
            probe_sqlite_support(&blocked).is_err(),
            "不可建库的目录必须报错"
        );
    }

    #[test]
    fn sqlite_probe_keeps_existing_database_untouched() {
        let dir = TempDataDir::new("dl-sqlite-existing");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        Connection::open(data.join("kedai.db"))
            .unwrap()
            .execute_batch("CREATE TABLE keep(id INTEGER); INSERT INTO keep VALUES (7);")
            .unwrap();

        probe_sqlite_support(&data).expect("已有库应可读");
        let n: i64 = Connection::open(data.join("kedai.db"))
            .unwrap()
            .query_row("SELECT id FROM keep", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 7, "探针不得改动已有库");
    }
}

#[cfg(test)]
mod audit_regressions {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    #[test]
    fn audit_root_siblings_block_migration_without_overwrite() {
        for dir in [IMPORTS_DIR_NAME, OUTPUTS_DIR_NAME, TASK_RECORDS_DIR_NAME] {
            for with_data in [false, true] {
                let source = TempDataDir::new("audit-source");
                let target = TempDataDir::new("audit-target");
                let pointer = TempDataDir::new("audit-pointer");
                let mut cfg = crate::config::test_config();
                cfg.data_dir = source.path().to_path_buf();
                cfg.imports_dir = source.join("imports");
                cfg.images_dir = source.join("images");
                cfg.task_scratch_dir = source.join("scratch");
                std::fs::write(source.join("settings.json"), "新设置").unwrap();
                if with_data {
                    std::fs::create_dir_all(target.join("data")).unwrap();
                }
                let existing = target.join(dir).join("preserve.json");
                std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
                std::fs::write(&existing, "原有数据").unwrap();
                let pointer_path = pointer.join("pointer.json");
                let svc = DataLocationService::with_pointer_path(cfg, pointer_path.clone());
                assert!(
                    matches!(
                        svc.apply_to_root(target.path()),
                        Err(DataLocationError::BadRequest(_))
                    ),
                    "{dir}, data={with_data}"
                );
                assert_eq!(std::fs::read_to_string(existing).unwrap(), "原有数据");
                assert!(!target.join("data/settings.json").exists());
                assert!(!pointer_path.exists());
                assert!(!source.join(MIGRATED_MARKER).exists());
            }
        }
    }

    #[test]
    fn audit_import_archives_keep_database_like_names() {
        let source = TempDataDir::new("audit-archives");
        let target = TempDataDir::new("audit-archives-target");
        let pointer = TempDataDir::new("audit-archives-pointer");
        let mut cfg = crate::config::test_config();
        cfg.data_dir = source.path().to_path_buf();
        cfg.imports_dir = source.join("imports");
        cfg.images_dir = source.join("images");
        cfg.task_scratch_dir = source.join("scratch");
        let archive = cfg.imports_dir.join("20261109-120000");
        std::fs::create_dir_all(&archive).unwrap();
        let names = [
            "preset.bak-202611.json",
            "kedai.db",
            "notes-wal",
            "notes-shm",
            ".verify-card",
            "notes.log",
            MIGRATED_MARKER,
        ];
        for name in names {
            std::fs::write(archive.join(name), name).unwrap();
        }
        let svc = DataLocationService::with_pointer_path(cfg, pointer.join("p.json"));
        svc.apply_to_root(target.path()).unwrap();
        for name in names {
            assert_eq!(
                std::fs::read_to_string(target.join("imports/20261109-120000").join(name)).unwrap(),
                name
            );
            assert_eq!(std::fs::read_to_string(archive.join(name)).unwrap(), name);
        }
        assert!(!target.join("data/imports").exists());
    }
}
