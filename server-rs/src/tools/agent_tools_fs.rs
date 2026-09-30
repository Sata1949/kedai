// 工作区文件工具族(编码通道批次):fs_read / fs_write / fs_edit / fs_glob / fs_grep。
//
// 为什么叫 `fs_*` 而不是复用 `read`/`write`/`replace`/`create`:既有四个工具是**角色扮演
// 语义**(target=bubble|file、相对路径经 safe_rel_path 收口在 data/character_files/{id}/),
// 改它们会牵动既有测试与线上行为;新族只在「任务绑定了工作区」时下发,角色扮演路径逐字节不变。
//
// 三条硬约束(缺一即失去本批的意义):
//   ① 路径一律经 `workspace_guard::safe_workspace_path` 解析——越界/符号链接逃逸即拒;
//   ② 未绑定工作区时本族**一律**返回「未绑定工作区」错误(下发侧也会剔除,这里是执行侧兜底:
//      任务运行途中绑定不可能变化,但模型可能凭历史上下文臆造调用);
//   ③ 写类工具强制先读后写(防盲写),写成功后刷新读记录。
//
// 依赖纪律:遍历用手写 `std::fs::read_dir` 递归(仓库无 walkdir/glob/ignore,本批不加依赖);
// 输出排序稳定(同一输入两次调用结果一致),便于模型缓存与测试断言。
use crate::models::types::{ExecScope, ToolContext, ToolDefinition};
use crate::services::task_change_service;
use crate::tools::registry::ToolRegistry;
use crate::tools::workspace_guard::safe_workspace_path;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::agent_tools::ToolDeps;
// 遍历实现与排除表/相对路径展示:2026-09-30 批次 4b 起统一在 workspace_scan
// (fs_glob/fs_grep 与 bash 侧扫描共用同一份遍历,见该模块头注释)
use super::workspace_scan::{display_rel, walk_tree, EXCLUDED_SEGMENTS};

/// 任务侧文件变更记账(2026-09-30 批次 4,PRODCAP-4「交付可审计」)。
///
/// 只在**任务会话**记账(`ctx.session_id` 带 `task:` 前缀);聊天侧的文件写由
/// `services/undo_service` 的 `undo_snapshots` 负责(角色文件区、键是 session_id),
/// 两份语义不同,不合并。
/// 记账失败**不改变写操作的结果**(`record` 内部只 warn)——台账少一行是可以解释的,
/// 把已经落盘的成功写入判成失败反而更糟。
fn note_change(
    deps: &ToolDeps,
    ctx: &ToolContext,
    rel: &str,
    path: &Path,
    baseline: &task_change_service::Baseline,
) {
    let Some(task_id) = task_change_service::task_id_of(&ctx.session_id) else {
        return;
    };
    let op = if baseline.exists { "modify" } else { "create" };
    task_change_service::record(&deps.db, &task_id, rel, op, "tool", baseline, Some(path));
}

/// 工具名清单(任务工具策略的任务名例外与场景白名单以此为准:
/// `tools::tool_sets::WORKSPACE_TOOLS` 必须与本清单一致)
pub const READ_TOOL: &str = "fs_read";
pub const WRITE_TOOL: &str = "fs_write";
pub const EDIT_TOOL: &str = "fs_edit";
pub const GLOB_TOOL: &str = "fs_glob";
pub const GREP_TOOL: &str = "fs_grep";

/// 单文件读写上限(超过即**明确报错**,不静默截断——截断一个源码文件再写回就是数据损坏)
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// fs_grep 的单文件扫描上限(超过即跳过并计数;grep 是只读检索,跳过比报错更可用)
const MAX_GREP_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// fs_read 单页行数默认与上限
const READ_DEFAULT_LINES: usize = 200;
const READ_MAX_LINES: usize = 2000;
/// fs_glob / fs_grep 的默认与上限命中数
const MATCH_DEFAULT_LIMIT: usize = 200;
const MATCH_MAX_LIMIT: usize = 1000;

/// 注册工作区文件工具族。`deps` 提供 DATA_DIR(路径闸门的二次防线用)。
///
/// 可见性:始终注册,下发与否由任务工具策略按「本任务是否绑定工作区」过滤
/// (见 `services/task_engine/tool_policy.rs`);聊天路径经 `tool_sets::exclude_workspace`
/// 剔除。故本族不进任何只读场景白名单(规划侦察/子 agent/反思的能力面保持不变)。
pub fn register_fs_tools(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    register_read_tool(registry, deps.clone());
    register_write_tool(registry, deps.clone());
    register_edit_tool(registry, deps.clone());
    register_glob_tool(registry, deps.clone());
    register_grep_tool(registry, deps);
}

/// 取本调用的工作区作用域。未绑定即不可用(文案说明原因与出路,不给「未注册」这种误导)。
fn scope_of(ctx: &ToolContext) -> Result<Arc<ExecScope>, String> {
    ctx.scope
        .clone()
        .ok_or_else(|| "本任务未绑定工作区,工作区文件工具不可用".to_string())
}

/// 解析工作区内的路径(闸门入口 + 作用域取值合一,五个工具共用)
fn resolve(
    deps: &ToolDeps,
    ctx: &ToolContext,
    raw: &str,
) -> Result<(Arc<ExecScope>, PathBuf), String> {
    let scope = scope_of(ctx)?;
    let path = safe_workspace_path(scope.workspace(), raw, Some(&deps.data_dir))?;
    Ok((scope, path))
}

// ==================== fs_read ====================

fn register_read_tool(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: READ_TOOL.into(),
            description: "读取本任务工作区内的文本文件(带行号分页)。修改任何文件前先用本工具读它。"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "工作区内的相对路径(如 src/main.rs);也可给工作区内的绝对路径" },
                    "offset": { "type": "integer", "description": "起始行号,1 起(默认 1)" },
                    "limit": { "type": "integer", "description": "本次最多读取行数(默认 200,上限 2000)" }
                },
                "required": ["path"]
            }),
        },
        Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move { run_read(&deps, &ctx, &args) })
        }),
    );
}

fn run_read(
    deps: &ToolDeps,
    ctx: &ToolContext,
    args: &serde_json::Value,
) -> Result<String, String> {
    let raw = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let (scope, path) = resolve(deps, ctx, &raw)?;
    let meta = std::fs::metadata(&path)
        .map_err(|e| format!("读取失败({raw}):{e};请先用 fs_glob 确认路径拼写"))?;
    if !meta.is_file() {
        return Err(format!(
            "{raw} 不是文件(可能是目录);目录内容请用 fs_glob 查看"
        ));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "文件过大({} 字节,上限 {MAX_FILE_BYTES});请改用 fs_grep 按模式检索",
            meta.len()
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("读取失败({raw}):{e}"))?;
    if bytes.contains(&0) {
        return Err(format!(
            "{raw} 含 NUL 字节,判定为二进制文件,本工具不读;请用 bash 处理"
        ));
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    // 读记录:写类工具据此判「读没读过、读后被没被外部改动」
    if let Ok(stamp) = ExecScope::stamp_of(&path) {
        scope.note_read(&path, stamp);
    }
    let rel = display_rel(scope.workspace(), &path);
    let offset = args
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(READ_DEFAULT_LINES as u64)
        .clamp(1, READ_MAX_LINES as u64) as usize;
    let start = offset.min(total.saturating_add(1));
    // `end` 是「取前 end 行」的行数上限(配合下方 take/skip),故等于 start-1+limit:
    // 若写成 start+limit 会多给一行(分页边界 off-by-one)。
    let end = (start - 1).saturating_add(limit).min(total);
    let mut out = format!("{rel} (共 {total} 行)\n");
    for (i, line) in lines
        .iter()
        .enumerate()
        .take(end)
        .skip(start.saturating_sub(1))
    {
        out.push_str(&format!("{:>6}\t{line}\n", i + 1));
    }
    if end < total {
        out.push_str(&format!(
            "(已截断:本次到第 {end} 行,继续读取请用 offset={})\n",
            end + 1
        ));
    }
    Ok(out)
}

// ==================== fs_write ====================

fn register_write_tool(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: WRITE_TOOL.into(),
            description: "整体写入本任务工作区内的文件(新建或覆盖)。覆盖已有文件前必须先用 fs_read 读过它。"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "工作区内的相对路径;父目录会自动创建" },
                    "content": { "type": "string", "description": "文件完整内容(整体替换,不是追加)" }
                },
                "required": ["path", "content"]
            }),
        },
        Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move { run_write(&deps, &ctx, &args) })
        }),
    );
}

fn run_write(
    deps: &ToolDeps,
    ctx: &ToolContext,
    args: &serde_json::Value,
) -> Result<String, String> {
    let raw = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let (scope, path) = resolve(deps, ctx, &raw)?;
    // 先读后写:只对「已存在的文件」要求(覆盖别人写过的内容必须基于读过的版本);
    // 目标不存在时是新建,没有可盲写的旧内容——否则本工具将永远无法新建文件。
    if path.exists() {
        scope.check_read_before_write(&path)?;
    }
    // 基线必须在落盘**之前** capture:落盘之后就拿不到「改动前正文」,diff 与回滚都无从谈起
    let baseline = task_change_service::Baseline::capture(&path);
    crate::utils::fs_atomic::write_atomic(&path, content.as_bytes())
        .map_err(|e| format!("写入失败({raw}):{e}"))?;
    // 写后刷新读记录:否则本次写入会让 stamp 变化,连着改两次会被自己拦下
    if let Ok(stamp) = ExecScope::stamp_of(&path) {
        scope.note_read(&path, stamp);
    }
    let rel = display_rel(scope.workspace(), &path);
    note_change(deps, ctx, &rel, &path, &baseline);
    Ok(format!("已写入 {rel}({} 字节)", content.len()))
}

// ==================== fs_edit ====================

fn register_edit_tool(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: EDIT_TOOL.into(),
            description: "把本任务工作区内文件的一段文本替换为另一段(定点修改,比 fs_write 省 token)。\
                          必须先 fs_read;默认只在该文本**唯一出现**时替换。"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "工作区内的相对路径" },
                    "old": { "type": "string", "description": "要被替换的原文(含足够上下文以保证唯一)" },
                    "new": { "type": "string", "description": "替换后的新文本(空串 = 删除该段)" },
                    "replace_all": { "type": "boolean", "description": "true = 替换全部出现处(默认 false:仅唯一匹配时才替换)" }
                },
                "required": ["path", "old", "new"]
            }),
        },
        Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move { run_edit(&deps, &ctx, &args) })
        }),
    );
}

fn run_edit(
    deps: &ToolDeps,
    ctx: &ToolContext,
    args: &serde_json::Value,
) -> Result<String, String> {
    let raw = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let old = args
        .get("old")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let new = args
        .get("new")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if old.is_empty() {
        return Err("old 不能为空:请给出要被替换的原文(含足够上下文以保证唯一)".into());
    }
    let (scope, path) = resolve(deps, ctx, &raw)?;
    // 先读后写(与 fs_write 同口径):这里目标文件必须存在,故无「新建」例外
    scope.check_read_before_write(&path)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("读取失败({raw}):{e}"))?;
    if bytes.contains(&0) {
        return Err(format!("{raw} 含 NUL 字节,判定为二进制文件,本工具不改"));
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| format!("{raw} 不是 UTF-8 文本,本工具不改;请用 bash 处理"))?;
    let hits = content.matches(&old).count();
    if hits == 0 {
        return Err("未找到要替换的文本".into());
    }
    if hits > 1 && !replace_all {
        return Err(format!(
            "匹配到 {hits} 处,请给出更长的唯一上下文,或显式 replace_all=true"
        ));
    }
    let updated = if replace_all {
        content.replace(&old, &new)
    } else {
        content.replacen(&old, &new, 1)
    };
    let baseline = task_change_service::Baseline::capture(&path);
    crate::utils::fs_atomic::write_atomic(&path, updated.as_bytes())
        .map_err(|e| format!("写入失败({raw}):{e}"))?;
    // 写后刷新读记录(与 fs_write 同款:否则连续编辑第二次会被自己拦下)
    if let Ok(stamp) = ExecScope::stamp_of(&path) {
        scope.note_read(&path, stamp);
    }
    let rel = display_rel(scope.workspace(), &path);
    note_change(deps, ctx, &rel, &path, &baseline);
    Ok(format!("已修改 {rel}(替换 {hits} 处)"))
}

// ==================== fs_glob ====================

fn register_glob_tool(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: GLOB_TOOL.into(),
            description: "按通配模式列出本任务工作区内的文件(相对路径,字典序)。\
                          `*` 不跨目录、`**` 跨目录(如 src/**/*.rs);.git/target/node_modules 等目录自动排除。"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "相对工作区的通配模式,如 **/*.rs、src/*.ts" },
                    "max": { "type": "integer", "description": "最多返回条数(默认 200,上限 1000)" }
                },
                "required": ["pattern"]
            }),
        },
        Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move { run_glob(&deps, &ctx, &args) })
        }),
    );
}

fn run_glob(
    deps: &ToolDeps,
    ctx: &ToolContext,
    args: &serde_json::Value,
) -> Result<String, String> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if pattern.is_empty() {
        return Err("pattern 不能为空(如 **/*.rs)".into());
    }
    let scope = scope_of(ctx)?;
    // 数据目录二次防线:工作区本身在创建期已排除与 DATA_DIR 的重叠,这里对根再确认一次
    if let Some(reason) =
        crate::tools::workspace_guard::data_dir_conflict(scope.workspace(), &deps.data_dir)
    {
        return Err(reason);
    }
    let matcher = glob_matcher(&pattern)?;
    let max = args
        .get("max")
        .and_then(|v| v.as_u64())
        .unwrap_or(MATCH_DEFAULT_LIMIT as u64)
        .clamp(1, MATCH_MAX_LIMIT as u64) as usize;
    let mut hits: BTreeSet<String> = BTreeSet::new();
    walk_files(scope.workspace(), &mut |path| {
        let rel = display_rel(scope.workspace(), path);
        if matcher.is_match(&rel) {
            hits.insert(rel);
        }
    });
    let total = hits.len();
    Ok(format_matches(
        hits.into_iter().collect(),
        max,
        Some(total),
        "匹配",
    ))
}

// ==================== fs_grep ====================

fn register_grep_tool(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: GREP_TOOL.into(),
            description: "在本任务工作区内按正则检索文本,返回「相对路径:行号:行内容」。\
                          只读,适合先定位再 fs_read。"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "正则表达式(如 fn\\s+main、TODO)" },
                    "glob": { "type": "string", "description": "可选的文件名通配过滤,同 fs_glob 语法(如 **/*.rs)" },
                    "max": { "type": "integer", "description": "最多返回条数(默认 200,上限 1000)" }
                },
                "required": ["pattern"]
            }),
        },
        Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move { run_grep(&deps, &ctx, &args) })
        }),
    );
}

fn run_grep(
    deps: &ToolDeps,
    ctx: &ToolContext,
    args: &serde_json::Value,
) -> Result<String, String> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if pattern.is_empty() {
        return Err("pattern 不能为空(正则表达式)".into());
    }
    // 正则编译失败时把 regex 的原错原样带出(模型据此自纠模式,而不是收到笼统「参数错误」)
    let re = regex::Regex::new(&pattern).map_err(|e| format!("正则表达式非法:{e}"))?;
    let file_filter = match args
        .get("glob")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(p) => Some(glob_matcher(p)?),
        None => None,
    };
    let scope = scope_of(ctx)?;
    if let Some(reason) =
        crate::tools::workspace_guard::data_dir_conflict(scope.workspace(), &deps.data_dir)
    {
        return Err(reason);
    }
    let max = args
        .get("max")
        .and_then(|v| v.as_u64())
        .unwrap_or(MATCH_DEFAULT_LIMIT as u64)
        .clamp(1, MATCH_MAX_LIMIT as u64) as usize;
    let mut hits: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut skipped_binary = 0usize;
    let mut skipped_large = 0usize;
    walk_files(scope.workspace(), &mut |path| {
        if truncated {
            return;
        }
        let rel = display_rel(scope.workspace(), path);
        if let Some(f) = &file_filter {
            if !f.is_match(&rel) {
                return;
            }
        }
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        if meta.len() > MAX_GREP_FILE_BYTES {
            skipped_large += 1;
            return;
        }
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        if bytes.contains(&0) {
            skipped_binary += 1;
            return;
        }
        for (i, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            if !re.is_match(line) {
                continue;
            }
            if hits.len() >= max {
                truncated = true;
                return;
            }
            hits.push(format!("{rel}:{}:{}", i + 1, line.trim_end()));
        }
    });
    // 命中数已到上限即提前收手(truncated):此时总数未知,如实说「还有更多」而不是编一个数
    let shown = hits.len();
    let mut out = format_matches(
        hits,
        max,
        if truncated { None } else { Some(shown) },
        "命中",
    );
    if skipped_binary > 0 || skipped_large > 0 {
        out.push_str(&format!(
            "(已跳过 {skipped_binary} 个二进制文件、{skipped_large} 个超过 {MAX_GREP_FILE_BYTES} 字节的文件)\n"
        ));
    }
    Ok(out)
}

// ==================== 共用件 ====================

/// 遍历工作区文件(目录不存在/不可读即静默跳过——闸门已确保根可用;
/// 排除表按**路径段**命中,故 `.git/objects/x` 与 `a/.git/x` 都跳过)。
/// 回调收到的是文件的绝对路径;遍历按目录内子项排序,顺序稳定。
/// 实现统一在 `tools::workspace_scan::walk_tree`(2026-09-30 批次 4b 收拢为单一出处,
/// 并把每条目的元数据调用从三次降为一次 lstat)。
fn walk_files(dir: &Path, f: &mut impl FnMut(&Path)) {
    let _ = walk_tree(dir, EXCLUDED_SEGMENTS, None, &mut |path, _md| f(path));
}

/// 通配模式 → 正则:先归一分隔符,再 `**`→`.*`、`*`→`[^/]*`、`?`→`[^/]`,
/// 其余字符逐字转义(唯一新依赖是仓库既有的 `regex`)。
///
/// 注意语义边界(已写进工具描述,避免模型误用):`**` 只等价「任意字符序列」,
/// 故 `**/*.rs` 需要路径里真有一个 `/` 才匹配——工作区根下的 `main.rs` 用 `*.rs`。
fn glob_matcher(pattern: &str) -> Result<regex::Regex, String> {
    let normalized = pattern.trim().replace('\\', "/");
    let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
    let mut re = String::from("^");
    let mut chars = normalized.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    re.push_str(".*");
                } else {
                    re.push_str("[^/]*");
                }
            }
            '?' => re.push_str("[^/]"),
            _ => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    regex::Regex::new(&re).map_err(|e| format!("通配模式非法:{e}"))
}

/// 命中清单的统一输出:相对路径/命中行逐行 + 截断与空集如实说明。
/// `known_total`:Some(n) = 已知总数(全量遍历后统计);None = 已按上限截断且总数未知
/// (grep 命中到上限即提前收手)——此时只报「已显示前 N 条、还有更多」,不编造总数。
fn format_matches(
    items: Vec<String>,
    max: usize,
    known_total: Option<usize>,
    label: &str,
) -> String {
    if items.is_empty() {
        return format!("(无{label})\n");
    }
    let shown = items.len().min(max);
    let mut out = String::new();
    for item in items.iter().take(shown) {
        out.push_str(item);
        out.push('\n');
    }
    match known_total {
        Some(total) if total > shown => out.push_str(&format!(
            "(已截断:仅显示前 {shown} 条,{label}共 {total} 条,还有更多)\n"
        )),
        Some(_) => out.push_str(&format!("({label} {shown} 条)\n")),
        None => out.push_str(&format!(
            "(已截断:仅显示前 {shown} 条,还有更多{label};请缩小模式或提高 max)\n"
        )),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::types::ExecScope;
    use crate::utils::test_support::TempDataDir;

    /// 注册工具族所需的依赖(测试用空依赖)与工作区
    struct Fixture {
        _dir: TempDataDir,
        deps: Arc<ToolDeps>,
        ws: PathBuf,
        dir: TempDataDir,
        /// 每任务**一份**作用域:生产同款(TaskRunContext 构造一次,随 ToolContext 克隆共享)。
        /// 若每次调用现建一份,读记录就存不住,「先读后写」永远不成立——那会掩盖被测语义。
        scope: Arc<ExecScope>,
    }

    fn fixture(tag: &str) -> Fixture {
        let (_guard, deps) = ToolDeps::dummy_for_test();
        let dir = TempDataDir::new(&format!("fstools-{tag}"));
        let ws = dir.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("src/main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        std::fs::write(ws.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::create_dir_all(ws.join("node_modules/pkg")).unwrap();
        std::fs::write(ws.join("node_modules/pkg/x.js"), "nope\n").unwrap();
        std::fs::create_dir_all(ws.join(".git")).unwrap();
        std::fs::write(ws.join(".git/config"), "nope\n").unwrap();
        std::fs::create_dir_all(ws.join("target")).unwrap();
        std::fs::write(ws.join("target/out.bin"), "nope\n").unwrap();
        let scope = {
            // 先取 owned 字符串:`scope_for_task` 收 `Option<&str>`,直接借用临时的 Cow
            // 会因临时值在语句末被丢弃而报借用错误
            let ws_str = ws.to_string_lossy().into_owned();
            crate::tools::workspace_guard::scope_for_task(Some(&ws_str))
                .unwrap()
                .unwrap()
        };
        Fixture {
            _dir: _guard,
            deps: Arc::new(deps),
            ws,
            dir,
            scope,
        }
    }

    impl Fixture {
        fn scope(&self) -> Arc<ExecScope> {
            self.scope.clone()
        }

        fn ctx(&self) -> ToolContext {
            ToolContext {
                session_id: "task:t1".into(),
                character_id: String::new(),
                agent_depth: 0,
                scope: Some(self.scope()),
            }
        }

        fn bindless_ctx() -> ToolContext {
            ToolContext {
                session_id: "s1".into(),
                character_id: String::new(),
                agent_depth: 0,
                scope: None,
            }
        }

        fn registry(&self) -> ToolRegistry {
            let reg = ToolRegistry::new();
            register_fs_tools(&reg, self.deps.clone());
            reg
        }

        /// 已裁决放行:fs_write / fs_edit 是危险级工具,任务模式下由 ToolGate 名单放行
        /// (见 task_engine/tool_policy.rs 的工具名例外);测试走同一「已获授权」路径,
        /// 否则会被 permissions 的通用授权弹窗分支拦下。未授权路径本身由 permissions
        /// 的既有测试覆盖,此处不重复。
        fn allow() -> crate::tools::permissions::PermissionDecision {
            crate::tools::permissions::PermissionDecision {
                allowed: true,
                risk: crate::tools::permissions::ToolRisk::Dangerous,
                reason: "测试放行".into(),
            }
        }

        async fn call(&self, name: &str, args: serde_json::Value) -> Result<String, String> {
            self.registry()
                .execute_with_decision(name, &args.to_string(), self.ctx(), &Self::allow())
                .await
        }

        async fn call_unbound(
            &self,
            name: &str,
            args: serde_json::Value,
        ) -> Result<String, String> {
            self.registry()
                .execute_with_decision(
                    name,
                    &args.to_string(),
                    Self::bindless_ctx(),
                    &Self::allow(),
                )
                .await
        }
    }

    /// 未绑定工作区:5 个工具一律拒绝,且文案点明「未绑定工作区」
    #[tokio::test]
    async fn unbound_scope_rejects_all_five_tools() {
        let f = fixture("unbound");
        let cases: Vec<(&str, serde_json::Value)> = vec![
            (READ_TOOL, json!({ "path": "README.md" })),
            (WRITE_TOOL, json!({ "path": "a.txt", "content": "x" })),
            (
                EDIT_TOOL,
                json!({ "path": "README.md", "old": "hello", "new": "hi" }),
            ),
            (GLOB_TOOL, json!({ "pattern": "*.md" })),
            (GREP_TOOL, json!({ "pattern": "hello" })),
        ];
        for (name, args) in cases {
            let err = f.call_unbound(name, args).await.unwrap_err();
            assert!(
                err.contains("未绑定工作区"),
                "{name} 在未绑定时应报未绑定工作区: {err}"
            );
        }
    }

    /// fs_read:带行号分页 + 读记录写入(供 fs_write 的先读后写)
    #[tokio::test]
    async fn read_numbers_lines_and_notes_read() {
        let f = fixture("read");
        let out = f
            .call(READ_TOOL, json!({ "path": "src/main.rs" }))
            .await
            .unwrap();
        assert!(
            out.starts_with("src/main.rs (共 3 行)"),
            "首行应含路径与总行数: {out}"
        );
        assert!(out.contains("     1\tfn main() {"), "应带行号正文: {out}");
        // 分页:只取第 2 行(输出 = 路径行 + 1 行正文 + 截断提示)
        let page = f
            .call(
                READ_TOOL,
                json!({ "path": "src/main.rs", "offset": 2, "limit": 1 }),
            )
            .await
            .unwrap();
        assert!(page.contains("     2\t"), "应只给第 2 行: {page}");
        assert!(!page.contains("fn main"), "第 1 行不应出现: {page}");
        assert!(page.contains("已截断"), "应提示后续 offset: {page}");
        // 越界路径被闸门拒
        assert!(f
            .call(READ_TOOL, json!({ "path": "../README.md" }))
            .await
            .is_err());
    }

    /// fs_write:先读后写(未读拒、读过通过),新建文件不受先读限制
    #[tokio::test]
    async fn write_requires_prior_read_for_existing_file() {
        let f = fixture("write");
        let err = f
            .call(WRITE_TOOL, json!({ "path": "README.md", "content": "new" }))
            .await
            .unwrap_err();
        assert!(err.contains("尚未读取"), "未读应先被拦: {err}");
        // 新建文件:没有可盲写的旧内容 → 放行
        f.call(
            WRITE_TOOL,
            json!({ "path": "src/new.rs", "content": "// new\n" }),
        )
        .await
        .unwrap();
        assert!(f.ws.join("src/new.rs").exists(), "新文件应落盘");
        // 读过之后可以覆盖,且连续两次不被自己拦下(写后刷新读记录)
        f.call(READ_TOOL, json!({ "path": "README.md" }))
            .await
            .unwrap();
        f.call(
            WRITE_TOOL,
            json!({ "path": "README.md", "content": "one\n" }),
        )
        .await
        .unwrap();
        f.call(
            WRITE_TOOL,
            json!({ "path": "README.md", "content": "two\n" }),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(f.ws.join("README.md")).unwrap(),
            "two\n"
        );
    }

    /// fs_write/fs_edit 的越界与符号链接逃逸一律被闸门拦下(执行侧兜底)
    #[tokio::test]
    async fn write_and_edit_are_gated_by_guard() {
        let f = fixture("gated");
        let outside = f.dir.join("outside.txt");
        std::fs::write(&outside, "x").unwrap();
        let abs = outside.to_string_lossy().into_owned();
        for (name, args) in [
            (WRITE_TOOL, json!({ "path": abs, "content": "y" })),
            (
                EDIT_TOOL,
                json!({ "path": "../outside.txt", "old": "x", "new": "y" }),
            ),
        ] {
            assert!(f.call(name, args).await.is_err(), "{name} 越界应被拒");
        }
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "x",
            "外部文件不得被改"
        );
    }

    /// fs_edit 的三种分支:0 处 / 2 处(未给 replace_all)/ replace_all=true
    #[tokio::test]
    async fn edit_uniqueness_branches() {
        let f = fixture("edit");
        std::fs::write(f.ws.join("dup.txt"), "aaa\nbbb\naaa\n").unwrap();
        f.call(READ_TOOL, json!({ "path": "dup.txt" }))
            .await
            .unwrap();
        let miss = f
            .call(
                EDIT_TOOL,
                json!({ "path": "dup.txt", "old": "zzz", "new": "y" }),
            )
            .await
            .unwrap_err();
        assert!(miss.contains("未找到"), "0 处文案: {miss}");
        let many = f
            .call(
                EDIT_TOOL,
                json!({ "path": "dup.txt", "old": "aaa", "new": "y" }),
            )
            .await
            .unwrap_err();
        assert!(many.contains("匹配到 2 处"), "2 处应回报实际次数: {many}");
        // 加上下文后唯一,可替换
        f.call(
            EDIT_TOOL,
            json!({ "path": "dup.txt", "old": "bbb\naaa", "new": "bbb\nzzz" }),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(f.ws.join("dup.txt")).unwrap(),
            "aaa\nbbb\nzzz\n"
        );
        // replace_all 放行全部出现处(先重读:文件已被自己改过)
        f.call(READ_TOOL, json!({ "path": "dup.txt" }))
            .await
            .unwrap();
        let all = f
            .call(
                EDIT_TOOL,
                json!({ "path": "dup.txt", "old": "zzz", "new": "w", "replace_all": true }),
            )
            .await
            .unwrap();
        assert!(all.contains("替换 1 处"), "应回报替换处数: {all}");
        // 空 old 直接拒
        assert!(f
            .call(
                EDIT_TOOL,
                json!({ "path": "dup.txt", "old": "", "new": "x" })
            )
            .await
            .is_err());
    }

    /// fs_glob:字典序相对路径 + 排除表生效 + 截断如实说明
    #[tokio::test]
    async fn glob_sorts_and_excludes() {
        let f = fixture("glob");
        let out = f
            .call(GLOB_TOOL, json!({ "pattern": "**/*.rs" }))
            .await
            .unwrap();
        assert!(out.contains("src/main.rs"), "应命中工作区源码: {out}");
        assert!(
            !out.contains("node_modules"),
            "node_modules 应被排除: {out}"
        );
        let all = f.call(GLOB_TOOL, json!({ "pattern": "**" })).await.unwrap();
        for excluded in [".git", "target", "node_modules"] {
            assert!(!all.contains(excluded), "{excluded} 应被排除: {all}");
        }
        let none = f
            .call(GLOB_TOOL, json!({ "pattern": "*.ts" }))
            .await
            .unwrap();
        assert!(none.contains("无匹配"), "空集应如实说明: {none}");
        let capped = f
            .call(GLOB_TOOL, json!({ "pattern": "**", "max": 1 }))
            .await
            .unwrap();
        assert!(capped.contains("还有更多"), "截断应如实说明: {capped}");
    }

    /// fs_grep:命中格式 path:line:text、glob 过滤、非法正则回原错
    #[tokio::test]
    async fn grep_reports_path_line_and_filters() {
        let f = fixture("grep");
        let out = f
            .call(GREP_TOOL, json!({ "pattern": "fn main" }))
            .await
            .unwrap();
        assert!(out.contains("src/main.rs:1:fn main() {"), "命中格式: {out}");
        let filtered = f
            .call(GREP_TOOL, json!({ "pattern": "hello", "glob": "**/*.rs" }))
            .await
            .unwrap();
        assert!(
            filtered.contains("无命中"),
            "glob 过滤后应无命中: {filtered}"
        );
        let bad = f
            .call(GREP_TOOL, json!({ "pattern": "([", }))
            .await
            .unwrap_err();
        assert!(bad.contains("正则表达式非法"), "应回 regex 原错: {bad}");
        let skipped = f
            .call(GREP_TOOL, json!({ "pattern": "nope" }))
            .await
            .unwrap();
        assert!(
            skipped.contains("无命中"),
            "被排除目录不应被检索: {skipped}"
        );
    }
}
