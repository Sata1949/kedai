//! 数据位置(盘符选择)的编排层:状态汇总、目录创建、旧数据迁移与指针写入。
//!
//! 迁移是**复制**语义:旧目录永不删除,失败不写指针。数据库用 SQLite Online Backup
//! 生成一致性快照(与 `migration::snapshot_database` 同源),其余文件尽力复制;
//! 有运行中任务时拒绝迁移(409),避免搬走正在写的目录。
use crate::config::{self, AppConfig};
use crate::data_location::{
    self, DataLocationPointer, DATA_DIR_NAME, MIGRATED_MARKER, OUTPUTS_DIR_NAME,
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
        // 可写探测只在 Android 且尚未选定根时做:已选定且可用的根不需要权限提示,
        // 而每次打开设置都建目录/写探针也没有必要。
        let needs_permission = suggested_root.as_ref().is_some_and(|root| {
            self.config.data_root.is_none() && !data_location::probe_writable(root)
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
        if target_data.join("kedai.db").is_file() {
            if config::dir_has_user_data(&target_data) {
                return Err(DataLocationError::BadRequest(format!(
                    "{} 已存在 Kedai 用户数据,不能直接覆盖;请换一个盘,或先备份/清理该目录",
                    root.display()
                )));
            }
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
        let legacy_images_inside_data = source_images == source_data.join("images");
        let mut migrated = false;

        if source_data.is_dir() && source_data != target_data {
            copy_tree(&source_data, &target_data, legacy_images_inside_data)
                .map_err(DataLocationError::Internal)?;
            let source_db = source_data.join("kedai.db");
            if source_db.is_file() {
                snapshot_database(&source_db, &target_data.join("kedai.db"))
                    .map_err(|e| DataLocationError::Internal(format!("数据库迁移失败: {e}")))?;
            }
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
        if name == "kedai.db"
            || name.ends_with("-wal")
            || name.ends_with("-shm")
            || name.ends_with(".log")
            || name.contains(".bak-")
            || name.starts_with(".verify")
            || name == MIGRATED_MARKER
            || (skip_images && name == "images")
        {
            continue;
        }
        let from = entry.path();
        let to = target.join(&name);
        if file_type.is_dir() {
            copied += copy_tree(&from, &to, false)?;
        } else {
            fs::copy(&from, &to)
                .map_err(|e| format!("复制 {} → {} 失败: {e}", from.display(), to.display()))?;
            copied += 1;
        }
    }
    Ok(copied)
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
}
