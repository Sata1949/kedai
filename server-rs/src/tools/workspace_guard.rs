// 工作区路径闸门(编码通道批次,本批安全核心):把模型给的路径解析为**工作区内**的真实路径,
// 任何逃逸尝试一律返回 Err(文案给下一步动作)。
//
// 使用方:
//   - `tools/agent_tools_fs.rs` 工作区文件工具族(fs_read/fs_write/fs_edit);
//   - `tools/bash.rs` 显式 cwd 的越界校验(jail 开启时);
//   - `api/tasks.rs` 创建期的工作区校验(复用 [`data_dir_conflict`] 这一判据)。
//
// 依赖纪律:本模块只用 std 与 `models`(L1),不反向依赖 services —— DATA_DIR 由调用方
// 作为参数传入(`Option<&Path>`),故 L2 的 tools 层不需要知道 services::config。
//
// 为什么不用 canonicalize 一把梭:canonicalize 会**跟随**符号链接,恰好把「链接指向工作区
// 之外」这种逃逸解析成一个看起来合法的路径。所以本模块的判定顺序是「先逐段拒链接、
// 再做前缀比对」——前缀比对只负责「在不在工作区内」,不负责「是不是写穿」。
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::models::types::ExecScope;

/// 把模型给的路径解析为工作区内的真实路径。
///
/// 规则(每条都有对应负路径测试):
///   1. 空串 / 只含空白 / 含 `\0` → 拒绝;
///   2. 相对路径基于 `scope_root` 解析,绝对路径直接采用,但**两者最终都必须落在 root 内**;
///   3. 从盘根起逐段 `symlink_metadata`:任一段是符号链接(Windows 上还包括 junction 这类
///      重解析点)即拒绝 —— 防「写穿」,这一条与第 2 条不重叠,光靠前缀比对拦不住;
///   4. `scope_root` 先 canonicalize;不存在的尾段按词法拼回(其父目录已确认非链接);
///      比对用 `Path::strip_prefix`(已存在段取 canonicalize 后的真实值,故 Windows 的
///      大小写不敏感问题在同一条口径里化解);
///   5. `data_dir` 给出时做二次防线:结果不得与数据目录互相包含(防「workspace 被指到
///      数据目录父级」这类创建期漏网的场景)。
pub fn safe_workspace_path(
    scope_root: &Path,
    raw: &str,
    data_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("路径不能为空:请给出工作区内的相对路径(如 src/main.rs)".into());
    }
    if raw.contains('\0') {
        return Err("路径含非法字符(NUL),已拒绝".into());
    }
    let root = std::fs::canonicalize(scope_root).map_err(|e| {
        format!(
            "本任务的工作区不可用({}):{e};请检查该目录是否仍存在",
            scope_root.display()
        )
    })?;
    let candidate = if Path::new(raw).is_absolute() {
        // UNC(`\\server\share\…` / `//server/share/…`)先拒:后面的逐段 stat 会真的去连
        // 网络主机(慢,且是安全判定里不该发生的网络访问),而 canonical 后的工作区前缀
        // 必然是本机盘符形态,原文与它不可能前缀匹配。要访问网络盘上的工作区请用相对路径。
        #[cfg(windows)]
        if raw.starts_with("\\\\") || raw.starts_with("//") {
            return Err(format!(
                "路径越界:{raw} 是网络(UNC)路径,不允许作为工作区文件路径;请改用工作区内的相对路径"
            ));
        }
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };
    let resolved = resolve_no_symlink(&candidate)?;
    if resolved.strip_prefix(&root).is_err() {
        return Err(format!(
            "路径越界:{raw} 不在本任务的工作区内;请改用工作区内的相对路径(工作区:{})",
            root.display()
        ));
    }
    if let Some(dir) = data_dir {
        if let Some(reason) = data_dir_conflict(&resolved, dir) {
            return Err(reason);
        }
    }
    Ok(resolved)
}

/// 数据目录隔离判据(创建期校验与工具期二次防线共用的**单一出处**):
/// 目标路径与数据目录**互相包含即冲突**(返回 Some(原因))。
///
/// 两个方向都要:只判「目标落在 DATA_DIR 内」漏掉「把工作区指到项目根、而 `data\` 恰在
/// 其下」——那种情况下工作区工具能顺着相对路径走进真实数据目录。
/// 两侧都先 canonicalize(大小写与符号链接按真实值比对);某一侧不存在时退回原值,
/// 此时比对退化为字面前缀比对(数据目录不存在也就没有可保护的数据)。
pub fn data_dir_conflict(target: &Path, data_dir: &Path) -> Option<String> {
    let target = canonical_or_self(target);
    let dir = canonical_or_self(data_dir);
    if target.starts_with(&dir) {
        return Some(format!(
            "路径落在应用数据目录内,已拒绝(数据目录不允许被工作区工具读写):{}",
            target.display()
        ));
    }
    if dir.starts_with(&target) {
        return Some(format!(
            "路径是应用数据目录的上级目录,已拒绝(工作区不得包含数据目录):{}",
            target.display()
        ));
    }
    None
}

/// canonicalize,失败则原样返回(仅用于比对,不用于落实写入目标)
fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 按任务冻结的 workspace 构造运行期作用域(任务派发点与 legacy 规划侦察共用的单一出处)。
///
/// 未绑定 → `Ok(None)`:工作区文件工具不下发、`bash` 维持改造前语义(缺省 cwd = DATA_DIR)。
/// 绑定但目录不可用(不存在/不是目录/无法 canonicalize)→ **`Err`**:调用方必须让任务以
/// 明确错误终止,**绝不静默降级为 None**——降级会让 bash 的 cwd 回落到 DATA_DIR 并去改
/// 用户真实数据,正是本批要消除的默认行为。
pub fn scope_for_task(workspace: Option<&str>) -> Result<Option<Arc<ExecScope>>, String> {
    let Some(raw) = workspace.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let path = Path::new(raw);
    if !path.is_dir() {
        return Err(format!(
            "任务绑定的工作区不可用({raw}):目录不存在或已不是目录;沙箱可能已被删除,\
             请重建该目录或另建未绑定工作区的任务"
        ));
    }
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| format!("任务绑定的工作区无法解析({raw}):{e};沙箱可能已被删除"))?;
    Ok(Some(Arc::new(ExecScope::new(canonical, true))))
}

/// 逐段解析:先词法归一,再从盘根起逐段 stat 拒链接。
///
/// 词法归一与内核语义的差异只出现在符号链接/`..` 组合上,而符号链接在本函数里一律被拒,
/// 故「词法」与「实时解析」在本闸门下等价;`..` 弹到盘根之外时保留字面 `..`,
/// 让随后必然失败的 `strip_prefix` 去拒绝(而不是在这里 panic 或猜一个路径)。
fn resolve_no_symlink(candidate: &Path) -> Result<PathBuf, String> {
    let normalized = lexically_normalize(candidate);
    let mut resolved = PathBuf::new();
    // 首个不存在的段之后的部分:父目录已确认存在且非链接,只需按名拼回
    let mut missing: Vec<OsString> = Vec::new();
    for comp in normalized.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {
                resolved.push(comp.as_os_str());
            }
            Component::Normal(name) => {
                if !missing.is_empty() {
                    missing.push(name.to_os_string());
                    continue;
                }
                let next = resolved.join(name);
                match std::fs::symlink_metadata(&next) {
                    Ok(meta) => {
                        if is_link_like(&meta) {
                            return Err(format!(
                                "路径含符号链接或重解析点,已拒绝(防止写穿到工作区外):{}",
                                next.display()
                            ));
                        }
                        // 已存在段取真实值:符号链接已被拒,canonicalize 在此只起
                        // 「统一大小写与分隔符」的作用(Windows 大小写由此化解)
                        resolved = std::fs::canonicalize(&next).unwrap_or(next);
                    }
                    // 不存在(或不可读):尾段按词法拼回,后续由调用方的前缀比对判定
                    Err(_) => missing.push(name.to_os_string()),
                }
            }
            // 归一化后不该再出现 `..`/`.`;真出现时按字面处理,交由前缀比对兜底
            other => resolved.push(other.as_os_str()),
        }
    }
    for name in missing {
        resolved.push(name);
    }
    Ok(resolved)
}

/// 词法归一:丢弃 `.`/空段,`..` 弹掉上一段(弹不动则保留字面 `..`)。
fn lexically_normalize(candidate: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in candidate.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::Normal(name) => out.push(name),
            Component::ParentDir => {
                let poppable = out
                    .components()
                    .next_back()
                    .is_some_and(|c| matches!(c, Component::Normal(_)));
                if poppable {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
        }
    }
    out
}

/// 是否为符号链接/重解析点。
///
/// **Windows 上 junction 的 `is_symlink()` 为 false**(它是 reparse point 但不是 symlink),
/// 只判 `is_symlink()` 会让「在自己沙箱里造一个 junction 指向别处」直接绕过闸门;
/// 故 Windows 分支额外看 `FILE_ATTRIBUTE_REPARSE_POINT`(0x400)。这是双平台门禁纪律
/// (docs/经验.md E43):平台差异分支必须两侧都写,不能只在一个平台上成立。
fn is_link_like(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    /// 工作区根(临时目录下的 ws/),以及一个「外部」目录(out/)与数据目录(data/)
    struct Fixture {
        _root: TempDataDir,
        ws: PathBuf,
        out: PathBuf,
        data: PathBuf,
    }

    fn fixture(tag: &str) -> Fixture {
        let root = TempDataDir::new(&format!("wsguard-{tag}"));
        let ws = root.join("ws");
        let out = root.join("out");
        let data = root.join("data");
        for d in [&ws, &out, &data] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(ws.join("a.txt"), "inside").unwrap();
        std::fs::write(out.join("secret.txt"), "outside").unwrap();
        Fixture {
            _root: root,
            ws,
            out,
            data,
        }
    }

    fn guard(f: &Fixture, raw: &str) -> Result<PathBuf, String> {
        safe_workspace_path(&f.ws, raw, Some(&f.data))
    }

    /// 正路径:root 内正常相对路径 / 绝对路径都通过,且解析结果落在 root 内
    #[test]
    fn accepts_paths_inside_root() {
        let f = fixture("ok");
        let rel = guard(&f, "a.txt").unwrap();
        assert_eq!(rel, std::fs::canonicalize(f.ws.join("a.txt")).unwrap());
        // 尚不存在的新文件(尾段按词法拼回)
        let new = guard(&f, "src/new.rs").unwrap();
        assert!(new.starts_with(std::fs::canonicalize(&f.ws).unwrap()));
        assert!(!new.exists());
        // 绝对路径(工作区内)
        let abs = guard(&f, &f.ws.join("a.txt").to_string_lossy()).unwrap();
        assert!(abs.ends_with("a.txt"));
        // 路径分隔符两种写法等价(模型可能给反斜杠)。
        // **只在 Windows 成立**:反斜杠在那里是分隔符;类 Unix 上它是普通文件名字符,
        // 把 `src\mod.rs` 当分隔符解释反而是错的,故此处按平台判定。
        if cfg!(windows) {
            assert_eq!(
                guard(&f, "src\\mod.rs").unwrap(),
                guard(&f, "src/mod.rs").unwrap()
            );
        }
    }

    /// 空串 / 空白 / NUL 一律拒绝
    #[test]
    fn rejects_empty_and_nul() {
        let f = fixture("empty");
        for raw in ["", "   ", "\t"] {
            assert!(guard(&f, raw).is_err(), "空路径应被拒: {raw:?}");
        }
        assert!(guard(&f, "a\0b.txt").is_err(), "NUL 应被拒");
    }

    /// `..` 逃逸:相对与绝对两种写法都拒绝,且 `..` 仍落在 root 内的合法写法照常通过
    #[test]
    fn rejects_parent_dir_escape() {
        let f = fixture("dotdot");
        for raw in [
            "..",
            "../out/secret.txt",
            "a/../../out/secret.txt",
            "./../out",
        ] {
            assert!(guard(&f, raw).is_err(), "越界路径应被拒: {raw}");
        }
        // 在 root 内绕一圈不算逃逸
        assert!(guard(&f, "sub/../a.txt").is_ok(), "root 内的 .. 不应误伤");
    }

    /// 绝对路径越界(工作区之外)被拒
    #[test]
    fn rejects_absolute_outside_root() {
        let f = fixture("abs");
        let outside = f.out.join("secret.txt").to_string_lossy().into_owned();
        let err = guard(&f, &outside).unwrap_err();
        assert!(err.contains("越界"), "文案应说明越界:{err}");
    }

    /// 盘符/UNC 形态:不同盘符与 UNC 一律落不进 root
    ///
    /// **只在 Windows 成立**:盘符(`Z:\…`、`Z:foo`)与 UNC(`\\server\share\…`)是 Windows 的
    /// 路径语义;类 Unix 上它们是含反斜杠的**普通相对路径**,拼到 root 下即在 root 内,本就不该拒。
    /// `//server/share/x` 这种类 Unix 绝对路径形态由 `rejects_absolute_outside_root` 在两侧都覆盖。
    #[cfg(windows)]
    #[test]
    fn rejects_other_drive_and_unc() {
        let f = fixture("drive");
        // 与工作区不同盘符的绝对路径(取一个几乎必然存在的其它盘;不存在也同样是拒绝)
        for raw in [
            "Z:\\somewhere\\x.txt",
            "\\\\server\\share\\x.txt",
            "//server/share/x",
        ] {
            assert!(guard(&f, raw).is_err(), "应被拒: {raw}");
        }
        // 盘符相对写法(`C:foo`)不是绝对路径,拼到 root 下也不成立 → 拒绝
        if cfg!(windows) {
            assert!(guard(&f, "Z:foo").is_err(), "盘符相对写法应被拒");
        }
    }

    /// 大小写变体:Windows 大小写不敏感,不该误伤;其它平台由文件系统语义决定
    #[cfg(windows)]
    #[test]
    fn accepts_case_variant_on_windows() {
        let f = fixture("case");
        let upper = f.ws.join("A.TXT").to_string_lossy().into_owned();
        assert!(guard(&f, &upper).is_ok(), "Windows 上大小写变体应通过");
    }

    /// 符号链接逃逸:链接指向工作区外 → 拒绝(平台各写一份)
    #[test]
    fn rejects_symlink_escape() {
        let f = fixture("symlink");
        let link = f.ws.join("link.txt");
        if !try_symlink_file(&f.out.join("secret.txt"), &link) {
            // 无权限创建符号链接(Windows 需开发者模式/管理员):跳过而非误报通过
            eprintln!("跳过:当前环境不允许创建符号链接");
            return;
        }
        let err = guard(&f, "link.txt").unwrap_err();
        assert!(
            err.contains("符号链接") || err.contains("重解析点"),
            "应因链接被拒:{err}"
        );
        // 链接本体也一样拒(绝对路径写法)
        assert!(guard(&f, &link.to_string_lossy()).is_err());
    }

    /// Windows junction(目录重解析点,`is_symlink()` 为 false)必须被拒
    #[cfg(windows)]
    #[test]
    fn rejects_windows_junction() {
        let f = fixture("junction");
        let link = f.ws.join("jlink");
        if !try_junction(&link, &f.out) {
            eprintln!("跳过:当前环境不允许创建 junction");
            return;
        }
        assert!(
            guard(&f, "jlink/secret.txt").is_err(),
            "junction 必须被拒(is_symlink() 为 false 的经典盲区)"
        );
    }

    /// 数据目录二次防线:结果落在 DATA_DIR 内、或工作区包含 DATA_DIR,都拒绝
    #[test]
    fn rejects_data_dir_overlap() {
        let f = fixture("datadir");
        // 工作区内的文件,但把 data_dir 传成工作区自身 → 互相包含 → 拒绝
        assert!(safe_workspace_path(&f.ws, "a.txt", Some(&f.ws)).is_err());
        // 工作区 = DATA_DIR 的父目录 → 拒绝
        let parent = f.ws.parent().unwrap();
        assert!(data_dir_conflict(parent, &f.ws).is_some());
        // 而无关目录不冲突
        assert!(data_dir_conflict(&f.ws, &f.data).is_none());
        assert!(safe_workspace_path(&f.ws, "a.txt", Some(&f.data)).is_ok());
    }

    /// 工作区根自身不可用时给明确错误(不 panic、不返回半成品路径)
    #[test]
    fn rejects_missing_root() {
        let f = fixture("noroot");
        let gone = f.ws.join("not-exists-dir");
        let err = safe_workspace_path(&gone, "a.txt", None).unwrap_err();
        assert!(
            err.contains("工作区不可用"),
            "错误文案应点明工作区不可用:{err}"
        );
    }

    /// Windows:创建文件符号链接(失败返回 false = 无权限)
    #[cfg(windows)]
    fn try_symlink_file(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    /// 非 Windows:创建文件符号链接(失败返回 false)
    #[cfg(not(windows))]
    fn try_symlink_file(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    /// Windows:用 `mklink /J` 建 junction(不需要开发者模式/管理员,故比 symlink_dir 更可用)
    #[cfg(windows)]
    fn try_junction(link: &Path, target: &Path) -> bool {
        let out = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .output();
        matches!(out, Ok(o) if o.status.success()) && link.exists()
    }
}
