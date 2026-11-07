//! 用户数据根(盘符选择)的纯逻辑层。
//!
//! 职责边界:只做路径/指针/盘符的纯计算与文件读写,**不碰 SQLite、不依赖 tokio**,
//! 便于单测;真正的目录创建、迁移与冲突判定在 `services/data_location_service.rs`。
//!
//! 目录契约(选择盘符后):
//! ```text
//! <root>/            例 D:\Kedai-Data
//! ├── data/           SQLite / 设置 / 角色 / 插件(现有 DATA_DIR 布局)
//! ├── outputs/        应用产物(images/ 承接现有 DATA_DIR/images)
//! ├── task-records/   任务记录与草稿(承接现有 task_scratch)
//! └── imports/        综合导入原始文件归档
//! ```
//! 目录名刻意用英文:任务记录根会作为 bash 的缺省 cwd,中文路径在 cmd/PowerShell
//! 的代码页组合下出现过乱码与命令失败,而 UI 文案仍是中文。
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 用户可见的根目录名(位于所选盘符根下)。
pub const ROOT_DIR_NAME: &str = "Kedai-Data";
/// Android 默认数据根:公共「下载」目录下的 `Kedai-Data`(2026-11-06 实机反馈)。
///
/// Android 没有盘符概念,`available_drives()` 恒为空,盘符选择器在手机上不可用;
/// 产品裁定(0.4.5 修复)把默认位置定在用户可见的 Download 目录,既符合「数据在
/// 根目录可管理」的诉求,也避免数据锁在应用私有目录里随卸载一起丢失。
/// 写入公共目录需要「所有文件访问」权限(Android 11+),前端设置页提供授权入口。
pub const ANDROID_DEFAULT_ROOT: &str = "/storage/emulated/0/Download/Kedai-Data";
/// 系统数据目录名(与既有 `DATA_DIR` 布局一致)。
pub const DATA_DIR_NAME: &str = "data";
/// 应用产物目录名(当前承接 `images/`)。
pub const OUTPUTS_DIR_NAME: &str = "outputs";
/// 任务记录/草稿目录名(承接 `task_scratch`)。
pub const TASK_RECORDS_DIR_NAME: &str = "task-records";
/// 综合导入原始文件目录名。
pub const IMPORTS_DIR_NAME: &str = "imports";
/// 迁移完成后写在**旧** data 目录里的标记;DB-7 分叉检测据此跳过已迁移源。
pub const MIGRATED_MARKER: &str = ".migrated-to.json";

/// 指针文件内容(`%APPDATA%\com.kedai.app\data-location.json`)。
///
/// 指针刻意不放进 `settings.json`:它决定 settings/数据库本身的位置,形成
/// 「配置在数据里、数据位置在配置里」的循环;独立文件也让 Tauri 壳在服务启动前
/// 就能读到它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataLocationPointer {
    /// 当前只认 1;未来结构变更时用于拒绝未知版本(避免误读后写坏)。
    pub version: u32,
    /// 用户选择的根(例 `D:\Kedai-Data`)。
    pub root: PathBuf,
    /// 选择时间(仅留档)。
    #[serde(default)]
    pub chosen_at: String,
    /// 迁移来源(旧数据目录),用于去重与「可删除旧目录」提示。
    #[serde(default)]
    pub migrated_from: Vec<PathBuf>,
}

/// 根模式派生的四个绝对路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedPaths {
    pub data_dir: PathBuf,
    pub images_dir: PathBuf,
    pub task_records_dir: PathBuf,
    pub imports_dir: PathBuf,
}

impl DataLocationPointer {
    pub fn new(root: PathBuf) -> Self {
        Self {
            version: 1,
            root,
            chosen_at: String::new(),
            migrated_from: Vec::new(),
        }
    }
}

/// 由根计算四个子目录。纯函数,不做 IO。
pub fn derived_paths(root: &Path) -> DerivedPaths {
    DerivedPaths {
        data_dir: root.join(DATA_DIR_NAME),
        images_dir: root.join(OUTPUTS_DIR_NAME).join("images"),
        task_records_dir: root.join(TASK_RECORDS_DIR_NAME),
        imports_dir: root.join(IMPORTS_DIR_NAME),
    }
}

/// 指针文件路径。
///
/// 优先级:`KEDAI_DATA_LOCATION_POINTER`(测试/隔离实例显式指定) >
/// `%APPDATA%\com.kedai.app\data-location.json`;两者都不可用时返回 None(走旧解析)。
/// 环境变量覆盖是隔离实测的关键:否则任何测试实例写指针都会改到真实产品的启动位置。
pub fn pointer_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("KEDAI_DATA_LOCATION_POINTER").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let appdata = std::env::var_os("APPDATA").filter(|v| !v.is_empty())?;
    Some(
        PathBuf::from(appdata)
            .join("com.kedai.app")
            .join("data-location.json"),
    )
}

/// 读指针;文件缺失/损坏/版本未知一律返回 None(按「未选择」处理,只会多弹一次)。
pub fn read_pointer_from(path: &Path) -> Option<DataLocationPointer> {
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: DataLocationPointer = serde_json::from_str(&raw).ok()?;
    if parsed.version != 1 || parsed.root.as_os_str().is_empty() {
        return None;
    }
    Some(parsed)
}

pub fn read_pointer() -> Option<DataLocationPointer> {
    read_pointer_from(&pointer_path()?)
}

/// 原子写指针:同目录临时文件 + rename。
/// Windows 的 `rename` 不覆盖已存在目标,故先删旧文件;失败时旧文件已删、新文件未就位,
/// 最坏退化为「未选择」并再弹一次,不会写坏内容。
pub fn write_pointer_to(path: &Path, pointer: &DataLocationPointer) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("指针路径没有父目录: {}", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("创建指针目录失败: {e}"))?;
    let body =
        serde_json::to_vec_pretty(pointer).map_err(|e| format!("序列化数据位置失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &body).map_err(|e| format!("写临时指针失败: {e}"))?;
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| format!("移除旧指针失败: {e}"))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("替换指针失败: {e}"))?;
    Ok(())
}

pub fn write_pointer(pointer: &DataLocationPointer) -> Result<PathBuf, String> {
    let path = pointer_path().ok_or_else(|| "无法解析 %APPDATA%,不能保存数据位置".to_string())?;
    write_pointer_to(&path, pointer)?;
    Ok(path)
}

/// 把用户输入归一成盘符(接受 `d` / `D` / `D:` / `D:\`)。
pub fn normalize_drive(input: &str) -> Option<char> {
    let trimmed = input.trim();
    let without_sep = trimmed
        .strip_suffix('\\')
        .or_else(|| trimmed.strip_suffix('/'))
        .unwrap_or(trimmed)
        .trim_end_matches(':');
    let mut chars = without_sep.chars();
    let letter = chars.next()?;
    if chars.next().is_some() || !letter.is_ascii_alphabetic() {
        return None;
    }
    Some(letter.to_ascii_uppercase())
}

/// 盘符是否存在且为可访问目录(无媒体的光驱/软驱会被过滤掉)。
pub fn drive_exists(letter: char) -> bool {
    let Some(letter) = normalize_drive(&letter.to_string()) else {
        return false;
    };
    Path::new(&format!("{letter}:\\")).is_dir()
}

/// `X:\Kedai-Data`;盘符不存在返回 None。
pub fn root_for_drive(letter: char) -> Option<PathBuf> {
    let letter = normalize_drive(&letter.to_string())?;
    if !drive_exists(letter) {
        return None;
    }
    Some(Path::new(&format!("{letter}:\\")).join(ROOT_DIR_NAME))
}

/// 枚举存在的盘符(C–Z);非 Windows 上为空。
pub fn available_drives() -> Vec<char> {
    ('C'..='Z').filter(|c| drive_exists(*c)).collect()
}

/// 默认盘:优先 D,其次 C,再其次第一个可用盘(可注入存在性判断,便于单测)。
pub fn default_drive_from(exists: impl Fn(char) -> bool) -> Option<char> {
    if exists('D') {
        return Some('D');
    }
    if exists('C') {
        return Some('C');
    }
    ('E'..='Z').find(|c| exists(*c))
}

/// 默认选择:UI 打开时预选 D/C。
pub fn default_drive() -> Option<char> {
    default_drive_from(drive_exists)
}

pub fn default_root() -> Option<PathBuf> {
    default_drive().and_then(root_for_drive)
}

/// 运行平台标识(供数据位置状态接口区分盘符选择器与 Android 路径模式)。
pub fn platform() -> &'static str {
    if cfg!(target_os = "android") {
        "android"
    } else {
        "desktop"
    }
}

/// 平台建议的数据根:Android = `/storage/emulated/0/Download/Kedai-Data`;
/// 桌面 = None(仍走盘符选择)。
pub fn suggested_root() -> Option<PathBuf> {
    if cfg!(target_os = "android") {
        Some(PathBuf::from(ANDROID_DEFAULT_ROOT))
    } else {
        None
    }
}

/// 目录是否真的可写:建目录 + 写入并删除探针文件。
///
/// 不能只看 `create_dir_all`:目录已存在时它对无权限路径也返回 Ok(Android scoped
/// storage 下尤其常见),必须实际写一次才能区分「有目录」与「能写入」。
///
/// **探针必须落在真实数据子目录(`data/characters`)且用可见文件名**:Android scoped
/// storage 下,应用在根目录写自己的文件可能成功,但迁移复制到 `data/characters` 会被拒
/// (实测未授予「所有文件访问」时 `fs::copy` 报 `File exists`);隐藏文件名也可能绕过
/// 检查。探针要和真实写入路径同形,才不会给上层一个假的「可写」结论。
pub fn probe_writable(root: &Path) -> bool {
    let dir = root.join(DATA_DIR_NAME).join("characters");
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let probe = dir.join("kedai-write-probe");
    match std::fs::write(&probe, b"1") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 把用户输入的自定义根归一为绝对路径(Android 路径模式)。
///
/// 约束:必须绝对、最后一段必须是 `Kedai-Data`、父目录已存在、路径中无 `..`。
/// 父目录存在是刻意的——Android 只应写入已存在的公共目录(Download),不应让
/// 一个手滑路径在多级不存在的位置上创建数据根。
pub fn normalize_custom_root(input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("数据根路径为空".into());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(format!("数据根必须是绝对路径:{trimmed}"));
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("数据根路径不允许包含 ..".into());
    }
    if path.file_name().and_then(|n| n.to_str()) != Some(ROOT_DIR_NAME) {
        return Err(format!("数据根目录名必须是 {ROOT_DIR_NAME}:{trimmed}"));
    }
    match path.parent() {
        Some(parent) if parent.is_dir() => Ok(path),
        Some(parent) => Err(format!("上级目录不存在:{}", parent.display())),
        None => Err(format!("数据根没有上级目录:{trimmed}")),
    }
}

/// 建根与四个子目录(幂等)。`data` 也在这里建,后续 `Db::open` 只负责其中的库文件。
pub fn ensure_root_layout(root: &Path) -> Result<(), String> {
    if root.as_os_str().is_empty() {
        return Err("数据根路径为空".to_string());
    }
    for name in [
        DATA_DIR_NAME,
        OUTPUTS_DIR_NAME,
        TASK_RECORDS_DIR_NAME,
        IMPORTS_DIR_NAME,
    ] {
        std::fs::create_dir_all(root.join(name))
            .map_err(|e| format!("创建目录 {} 失败: {e}", root.join(name).display()))?;
    }
    Ok(())
}

/// 指针根是否仍可用:根目录与 `<root>/data` 都存在。
///
/// 刻意**不**在解析阶段自动重建缺失的根:盘符被拔掉或用户删了目录时,静默建一个空库
/// 会让用户以为「聊天记录没了」;这里只报告不可用,由前端强制重选。
pub fn pointer_usable(root: &Path) -> bool {
    root.is_dir() && root.join(DATA_DIR_NAME).is_dir()
}

/// 迁移完成标记(写进旧 data 目录)。
pub fn write_migrated_marker(old_data_dir: &Path, root: &Path) -> Result<(), String> {
    let marker = serde_json::json!({
        "migrated_to": root.to_string_lossy(),
        "migrated_at": chrono::Local::now().to_rfc3339(),
    });
    let body = serde_json::to_vec_pretty(&marker).map_err(|e| e.to_string())?;
    std::fs::write(old_data_dir.join(MIGRATED_MARKER), body)
        .map_err(|e| format!("写迁移标记失败: {e}"))
}

/// 旧目录是否已被迁移走(DB-7 分叉检测用)。
pub fn has_migrated_marker(dir: &Path) -> bool {
    dir.join(MIGRATED_MARKER).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    #[test]
    fn normalize_drive_accepts_common_forms() {
        for input in ["d", "D", "d:", "D:", "D:\\", "D:/"] {
            assert_eq!(normalize_drive(input), Some('D'), "输入 {input}");
        }
        for input in ["", "DD", "1", "C:\\temp", "Z:extra"] {
            assert_eq!(normalize_drive(input), None, "输入 {input}");
        }
    }

    #[test]
    fn default_drive_prefers_d_then_c() {
        assert_eq!(default_drive_from(|c| c == 'C' || c == 'D'), Some('D'));
        assert_eq!(default_drive_from(|c| c == 'C'), Some('C'));
        assert_eq!(default_drive_from(|c| c == 'E'), Some('E'));
        assert_eq!(default_drive_from(|_| false), None);
    }

    #[test]
    fn derived_paths_follow_contract() {
        // 期望值用 join 拼:断言只锁目录名契约,不把平台分隔符写死进期望路径
        // (Linux 上 `to_string_lossy` 是 `/`,与 Windows 反斜杠不同)。
        let root = Path::new("D:\\Kedai-Data");
        let p = derived_paths(root);
        assert_eq!(p.data_dir, root.join(DATA_DIR_NAME));
        assert_eq!(p.images_dir, root.join(OUTPUTS_DIR_NAME).join("images"));
        assert_eq!(p.task_records_dir, root.join(TASK_RECORDS_DIR_NAME));
        assert_eq!(p.imports_dir, root.join(IMPORTS_DIR_NAME));
    }

    #[test]
    fn ensure_root_layout_creates_four_dirs() {
        let dir = TempDataDir::new("dl-layout");
        let root = dir.path().join("Kedai-Data");
        ensure_root_layout(&root).expect("建目录失败");
        for name in ["data", "outputs", "task-records", "imports"] {
            assert!(root.join(name).is_dir(), "缺少 {name}");
        }
        // 幂等
        ensure_root_layout(&root).expect("重复建目录失败");
    }

    #[test]
    fn pointer_roundtrip_and_corrupt_fallback() {
        let dir = TempDataDir::new("dl-pointer");
        let path = dir.join("data-location.json");
        let pointer = DataLocationPointer {
            version: 1,
            root: PathBuf::from("D:\\Kedai-Data"),
            chosen_at: "2026-11-05T12:00:00+08:00".into(),
            migrated_from: vec![PathBuf::from("C:\\old\\data")],
        };
        write_pointer_to(&path, &pointer).expect("写指针失败");
        assert_eq!(read_pointer_from(&path), Some(pointer));
        // 损坏/未知版本 → None(按未选择处理)
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(read_pointer_from(&path), None);
        std::fs::write(&path, r#"{"version":2,"root":"D:\\x"}"#).unwrap();
        assert_eq!(read_pointer_from(&path), None);
    }

    #[test]
    fn pointer_usable_requires_data_dir() {
        let dir = TempDataDir::new("dl-usable");
        let root = dir.path().join("Kedai-Data");
        assert!(!pointer_usable(&root), "根不存在不得视为可用");
        std::fs::create_dir_all(&root).unwrap();
        assert!(!pointer_usable(&root), "缺 data 子目录不得视为可用");
        std::fs::create_dir_all(root.join("data")).unwrap();
        assert!(pointer_usable(&root));
    }

    #[test]
    fn normalize_custom_root_accepts_only_named_absolute_dir_with_existing_parent() {
        let dir = TempDataDir::new("dl-custom-root");
        let root = dir.path().join("Kedai-Data");
        assert_eq!(
            normalize_custom_root(&root.to_string_lossy()).unwrap(),
            root
        );
        // 相对路径 / 末段不是 Kedai-Data / 父目录不存在 → 全部拒绝
        assert!(normalize_custom_root("Kedai-Data").is_err());
        assert!(normalize_custom_root(&dir.path().join("Other").to_string_lossy()).is_err());
        assert!(
            normalize_custom_root(&dir.path().join("missing/Kedai-Data").to_string_lossy())
                .is_err()
        );
        // `..` 不允许
        assert!(normalize_custom_root(&format!("{}/../Kedai-Data", dir.path().display())).is_err());
    }

    #[test]
    fn probe_writable_detects_real_write_access() {
        let dir = TempDataDir::new("dl-probe");
        let root = dir.path().join("Kedai-Data");
        assert!(probe_writable(&root), "临时目录应可写");
        let nested = root.join(DATA_DIR_NAME).join("characters");
        assert!(nested.is_dir(), "探针会顺带建真实数据子目录");
        assert!(
            !nested.join("kedai-write-probe").exists(),
            "探针文件必须删除"
        );
    }

    #[test]
    fn android_platform_helpers_are_desktop_safe() {
        // 本测试在桌面跑:平台判定与建议根必须与 Android 分支互斥,
        // 保证盘符模式不会被路径模式污染。
        assert_eq!(
            platform(),
            if cfg!(target_os = "android") {
                "android"
            } else {
                "desktop"
            }
        );
        if cfg!(target_os = "android") {
            assert_eq!(suggested_root(), Some(PathBuf::from(ANDROID_DEFAULT_ROOT)));
        } else {
            assert_eq!(suggested_root(), None);
        }
    }

    #[test]
    fn migrated_marker_roundtrip() {
        let dir = TempDataDir::new("dl-marker");
        std::fs::write(dir.join("kedai.db"), b"").unwrap();
        assert!(!has_migrated_marker(dir.path()));
        write_migrated_marker(dir.path(), Path::new("D:\\Kedai-Data")).unwrap();
        assert!(has_migrated_marker(dir.path()));
    }
}
