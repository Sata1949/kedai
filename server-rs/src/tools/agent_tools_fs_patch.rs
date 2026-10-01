// fs_patch:工作区文件工具族第六员(Q6,2026-10-01)——结构化补丁,一次调用跨多文件。
//
// 为什么有它:fs_edit 一次只能改一个文件的一段文本,批量重构(多文件同步改/搬删)要 N 次
// 调用、N 段上下文;结构化补丁把这些收进一条文档,模型侧 token 与轮次都省。语法取
// Codex 风格(Add / Update / Delete / Move + 多 hunk),模型对这套形状有既有熟悉度。
//
// 两条关键语义(与调研报告 §3.5「先校验后落盘;部分失败语义」逐条对):
//   ① **先整体校验,再统一落盘**:解析后逐操作在内存里演算——路径闸门/先读后写/唯一匹配/
//      二进制与 UTF-8/大小上限**全部**过才动手;任一问题 → 整条补丁不落盘,一次报全部问题;
//   ② **顺序语义**:操作按出现顺序依次应用,后面的操作看到前面操作的结果(同一文件可被
//      多次 Update;Delete 后再 Add 也合法)。Move = 新路径写入 + 旧路径删除,两笔都进台账。
//
// 与既有纪律的关系:路径一律经 `workspace_guard::safe_workspace_path`(与其余 fs_* 同门);
// Update/Delete/Move 源文件强制**先读后写**;每次落盘前 capture 基线并记 task_file_changes
// (写用 create/modify、删记 delete,source=tool)——故整任务回滚 / diff / patch 导出
// 对补丁改动与单文件工具同等有效(先快照再写/删,可逆)。
//
// 包闸门:本工具是**包专属工具**——仅当 `task_coding_bundle_enabled`(task 合并值)开启时
// 由任务工具策略下发(见 services/task_engine/tool_policy.rs 的 coding_pack_gate)。
//
// 行尾边界(如实登记,勿当缺陷):匹配在「去掉行尾 \r」归一后做;**重写时按文件检测到的
// 风格**统一重排行尾(全部 CRLF 才用 CRLF,其余按 LF;混合行尾会被归一为 LF),
// 原文无尾换行的文件在未触及最后一行时保持无尾换行;新增文件用 LF + 尾换行(内容非空)。
// 上限(超限明确报错,不静默截断):补丁正文 256 KiB、操作数 32、目标文件 8 MiB(与族同)。
use super::agent_tools::ToolDeps;
use super::agent_tools_fs::{note_change, scope_of, MAX_FILE_BYTES, PATCH_TOOL};
use super::workspace_scan::display_rel;
use crate::models::types::{ExecScope, ToolContext, ToolDefinition};
use crate::services::task_change_service;
use crate::tools::registry::ToolRegistry;
use crate::tools::workspace_guard::safe_workspace_path;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 补丁正文上限(字节)
const MAX_PATCH_BYTES: usize = 256 * 1024;
/// 单条补丁最大操作数
const MAX_PATCH_OPS: usize = 32;

// 操作头前缀(解析与锚点扫描的**单一出处**)
const ADD_HEADER: &str = "*** Add File:";
const UPDATE_HEADER: &str = "*** Update File:";
const DELETE_HEADER: &str = "*** Delete File:";
const MOVE_HEADER: &str = "*** Move to:";

#[derive(Debug)]
enum PatchOp {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
    },
    Delete {
        path: String,
    },
}

#[derive(Debug)]
struct Hunk {
    /// 改动前的旧块(上下文行 ' ' + 删除行 '-',前缀已剥)
    before: Vec<String>,
    /// 改动后的新块(上下文行 ' ' + 新增行 '+',前缀已剥)
    after: Vec<String>,
    /// 该 hunk 起始行号(1 起,错误文案用)
    line_no: usize,
}

/// 落盘动作(计划阶段的产物;校验全过后按序执行)
enum PlannedAction {
    Write {
        path: PathBuf,
        rel: String,
        content: String,
    },
    Delete {
        path: PathBuf,
        rel: String,
    },
}

/// 解析器内部状态
enum Cur {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
        buf: Vec<(char, String)>,
        /// 当前补丁块的起始行号(错误文案用)
        hunk_start: usize,
        /// Update 头所在行号(收束期错误文案用)
        op_line: usize,
    },
}

/// 收束当前操作并推入 ops
fn flush_cur(cur: &mut Option<Cur>, ops: &mut Vec<PatchOp>) -> Result<(), String> {
    match cur.take() {
        None => Ok(()),
        Some(Cur::Add { path, lines, .. }) => {
            ops.push(PatchOp::Add { path, lines });
            Ok(())
        }
        Some(Cur::Update {
            path,
            move_to,
            mut hunks,
            buf,
            hunk_start,
            op_line,
        }) => {
            if !buf.is_empty() {
                hunks.push(build_hunk(buf, hunk_start));
            }
            if hunks.is_empty() && move_to.is_none() {
                return Err(format!(
                    "第 {op_line} 行:Update File: {path} 既无修改内容也无 *** Move to"
                ));
            }
            ops.push(PatchOp::Update {
                path,
                move_to,
                hunks,
            });
            Ok(())
        }
    }
}

/// 解析补丁文档。
///
/// 语法:
///   `*** Begin Patch` / `*** End Patch`   可选首尾包装(缺失也接受;出现则须在正确位置)
///   `*** Add File: <path>`                其后内容行逐行以 `+` 开头(空行写成单个 `+`)
///   `*** Update File: <path>`             [`*** Move to: <newpath>` 紧随] + 0..n 个 hunk
///   `*** Delete File: <path>`             无内容行
///   `@@ ...`                              hunk 分隔(说明文字忽略)
///   ` `/`-`/`+` 前缀行                     上下文 / 删除 / 新增
///
/// 解析是 fail-fast(带行号);语义校验(唯一匹配、先读后写等)在解析之后**整批**做。
fn parse_patch(text: &str) -> Result<Vec<PatchOp>, String> {
    // 行尾归一(\r 去掉)——模型输出可能带 CRLF
    let raw: Vec<&str> = text
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let n = raw.len();
    let mut i = 0usize;

    while i < n && raw[i].trim().is_empty() {
        i += 1;
    }
    if i < n && raw[i].trim() == "*** Begin Patch" {
        i += 1;
    }

    let mut ops: Vec<PatchOp> = Vec::new();
    let mut cur: Option<Cur> = None;

    while i < n {
        let line = raw[i];
        let t = line.trim_start();
        let line_no = i + 1;

        if t.starts_with("***") {
            // ---- 操作头 ----
            if let Some(path) = header_path(t, ADD_HEADER) {
                flush_cur(&mut cur, &mut ops)?;
                if path.is_empty() {
                    return Err(format!("第 {line_no} 行:{ADD_HEADER} 缺少路径"));
                }
                cur = Some(Cur::Add {
                    path,
                    lines: Vec::new(),
                });
                i += 1;
                continue;
            }
            if let Some(path) = header_path(t, UPDATE_HEADER) {
                flush_cur(&mut cur, &mut ops)?;
                if path.is_empty() {
                    return Err(format!("第 {line_no} 行:{UPDATE_HEADER} 缺少路径"));
                }
                cur = Some(Cur::Update {
                    path,
                    move_to: None,
                    hunks: Vec::new(),
                    buf: Vec::new(),
                    hunk_start: line_no + 1,
                    op_line: line_no,
                });
                i += 1;
                continue;
            }
            if let Some(path) = header_path(t, DELETE_HEADER) {
                flush_cur(&mut cur, &mut ops)?;
                if path.is_empty() {
                    return Err(format!("第 {line_no} 行:{DELETE_HEADER} 缺少路径"));
                }
                ops.push(PatchOp::Delete { path });
                i += 1;
                continue;
            }
            if t.starts_with(MOVE_HEADER) {
                let Some(Cur::Update { move_to, buf, .. }) = cur.as_mut() else {
                    return Err(format!(
                        "第 {line_no} 行:*** Move to 只能紧跟在 Update File 之后"
                    ));
                };
                if move_to.is_some() || !buf.is_empty() {
                    return Err(format!(
                        "第 {line_no} 行:*** Move to 必须紧跟 Update File 行(在修改内容之前)"
                    ));
                }
                let path = header_path(t, MOVE_HEADER).unwrap_or_default();
                if path.is_empty() {
                    return Err(format!("第 {line_no} 行:{MOVE_HEADER} 缺少路径"));
                }
                *move_to = Some(path);
                i += 1;
                continue;
            }
            let keyword = t.trim_start_matches("***").trim();
            if keyword == "Begin Patch" {
                return Err(format!(
                    "第 {line_no} 行:*** Begin Patch 只能出现在补丁开头"
                ));
            }
            if keyword == "End Patch" {
                flush_cur(&mut cur, &mut ops)?;
                i += 1;
                // 之后只允许空行
                while i < n {
                    if !raw[i].trim().is_empty() {
                        return Err(format!("第 {} 行:*** End Patch 之后不应再有内容", i + 1));
                    }
                    i += 1;
                }
                break;
            }
            return Err(format!(
                "第 {line_no} 行:未知的操作行 \"{t}\"(支持 Add/Update/Delete File 与 Move to)"
            ));
        }

        // ---- 内容行:先分派给当前操作,再按顶层语义报错 ----
        match cur.as_mut() {
            Some(Cur::Add { lines, .. }) => {
                if let Some(content) = line.strip_prefix('+') {
                    lines.push(content.to_string());
                } else {
                    return Err(format!(
                        "第 {line_no} 行:Add File 的内容行须以 \"+\" 开头(空行写成单独的 \"+\")"
                    ));
                }
            }
            Some(Cur::Update {
                buf,
                hunks,
                hunk_start,
                ..
            }) => {
                if line.starts_with("@@") {
                    if !buf.is_empty() {
                        hunks.push(build_hunk(std::mem::take(buf), *hunk_start));
                    }
                } else if let Some(first) = line.chars().next() {
                    if first == ' ' || first == '-' || first == '+' {
                        if buf.is_empty() {
                            *hunk_start = line_no;
                        }
                        buf.push((first, line[first.len_utf8()..].to_string()));
                    } else {
                        return Err(format!(
                            "第 {line_no} 行:修改行须以 \" \"(上下文)/\"-\"(删)/\"+\"(增)开头;空行写成单个 \" \""
                        ));
                    }
                } else {
                    return Err(format!(
                        "第 {line_no} 行:修改块内不能有完全空行(空行写成单个 \" \" 作上下文)"
                    ));
                }
            }
            None => {
                if !line.trim().is_empty() {
                    return Err(format!(
                        "第 {line_no} 行:期待 \"*** Add/Update/Delete File …\" 操作行"
                    ));
                }
            }
        }
        i += 1;
    }
    flush_cur(&mut cur, &mut ops)?;
    Ok(ops)
}

/// 从头部行取路径(前缀匹配后取余下并去首尾空白;路径内的空格保留)
fn header_path(line: &str, header: &str) -> Option<String> {
    line.strip_prefix(header)
        .map(|rest| rest.trim().to_string())
}

/// hunk 缓冲 → Hunk(before = ' '/'-' 行,after = ' '/'+' 行)
fn build_hunk(buf: Vec<(char, String)>, line_no: usize) -> Hunk {
    let before: Vec<String> = buf
        .iter()
        .filter(|(p, _)| *p == ' ' || *p == '-')
        .map(|(_, s)| s.clone())
        .collect();
    let after: Vec<String> = buf
        .iter()
        .filter(|(p, _)| *p == ' ' || *p == '+')
        .map(|(_, s)| s.clone())
        .collect();
    Hunk {
        before,
        after,
        line_no,
    }
}

/// 文件行切分(行尾归一):(行文本, 行尾风格, 是否有尾换行)。
/// 全部换行均为 \r\n → CRLF;其余(含混合)→ LF。行文本不含 \r。
fn split_lines(content: &str) -> (Vec<String>, &'static str, bool) {
    if content.is_empty() {
        return (Vec::new(), "\n", false);
    }
    let had_nl = content.ends_with('\n');
    let mut parts: Vec<&str> = content.split('\n').collect();
    if had_nl {
        parts.pop();
    }
    let mut lines = Vec::with_capacity(parts.len());
    let mut all_crlf = !parts.is_empty();
    for p in parts {
        match p.strip_suffix('\r') {
            Some(s) => lines.push(s.to_string()),
            None => {
                all_crlf = false;
                lines.push(p.to_string());
            }
        }
    }
    let eol = if all_crlf { "\r\n" } else { "\n" };
    (lines, eol, had_nl)
}

fn join_lines(lines: &[String], eol: &str, had_nl: bool) -> String {
    let mut out = lines.join(eol);
    if had_nl && !lines.is_empty() {
        out.push_str(eol);
    }
    out
}

/// 在 lines 中唯一查找 before 块;0 处 / 多处均给出可行动的原因。
fn find_unique(lines: &[String], before: &[String]) -> Result<usize, String> {
    if before.is_empty() {
        return Err("该修改块没有定位上下文(全是 + 行),无法确定插入位置;请带上一两行上下文".into());
    }
    if before.len() > lines.len() {
        return Err("文件内容不足以容纳该上下文块(检查上下文是否抄自当前版本)".into());
    }
    let mut hit: Option<usize> = None;
    let mut count = 0usize;
    for start in 0..=(lines.len() - before.len()) {
        if lines[start..start + before.len()] == before[..] {
            count += 1;
            hit = Some(start);
        }
    }
    match count {
        0 => Err("未找到与上下文匹配的旧内容(确认已 fs_read 最新版、空白与行序一致)".into()),
        1 => Ok(hit.unwrap_or(0)),
        k => Err(format!("上下文匹配到 {k} 处、不唯一;请带足上下文使其唯一")),
    }
}

/// 取某个路径的「当前」文本(虚拟态优先:同一条补丁内后续操作看到前面操作的结果)。
/// 盘上文件读取时做与 fs_read 同款的护栏:必须存在、是文件、≤ 8 MiB、非二进制、UTF-8。
fn current_text(
    path: &Path,
    raw: &str,
    vstate: &HashMap<PathBuf, Option<String>>,
) -> Result<Option<String>, String> {
    if let Some(v) = vstate.get(path) {
        return Ok(v.clone());
    }
    if !path.exists() {
        return Ok(None);
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("读取失败({raw}):{e}"))?;
    if !meta.is_file() {
        return Err(format!("{raw} 不是文件"));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "文件过大({} 字节,上限 {MAX_FILE_BYTES});请改用 fs_edit 分段处理",
            meta.len()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败({raw}):{e}"))?;
    if bytes.contains(&0) {
        return Err(format!("{raw} 含 NUL 字节,判定为二进制文件,本工具不改"));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("{raw} 不是 UTF-8 文本,本工具不改;请用 bash 处理"))?;
    Ok(Some(text))
}

/// 校验并演算整条补丁(不落盘)。返回 (动作序列, 摘要行) 或全部问题清单。
fn plan_patch(
    deps: &ToolDeps,
    ctx: &ToolContext,
    ops: &[PatchOp],
) -> Result<(Vec<PlannedAction>, Vec<String>), Vec<String>> {
    let scope = match scope_of(ctx) {
        Ok(s) => s,
        Err(e) => return Err(vec![e]),
    };
    // 虚拟文件态:None = 在该操作发生时不存在(已删 / 尚未建)
    let mut vstate: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut actions: Vec<PlannedAction> = Vec::new();
    let mut summaries: Vec<String> = Vec::new();
    let mut problems: Vec<String> = Vec::new();

    for op in ops {
        match op {
            PatchOp::Add { path: raw, lines } => {
                let path = match safe_workspace_path(scope.workspace(), raw, Some(&deps.data_dir)) {
                    Ok(p) => p,
                    Err(e) => {
                        problems.push(format!("新增 {raw}:{e}"));
                        continue;
                    }
                };
                match current_text(&path, raw, &vstate) {
                    Ok(Some(_)) => {
                        problems.push(format!("新增 {raw}:文件已存在(要改它请用 Update File)"));
                        continue;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        problems.push(format!("新增 {raw}:{e}"));
                        continue;
                    }
                }
                let content = if lines.is_empty() {
                    String::new()
                } else {
                    let mut s = lines.join("\n");
                    s.push('\n');
                    s
                };
                let rel = display_rel(scope.workspace(), &path);
                vstate.insert(path.clone(), Some(content.clone()));
                actions.push(PlannedAction::Write {
                    path,
                    rel: rel.clone(),
                    content,
                });
                summaries.push(format!("新增 {rel}({} 行)", lines.len()));
            }
            PatchOp::Update {
                path: raw,
                move_to,
                hunks,
            } => {
                let path = match safe_workspace_path(scope.workspace(), raw, Some(&deps.data_dir)) {
                    Ok(p) => p,
                    Err(e) => {
                        problems.push(format!("修改 {raw}:{e}"));
                        continue;
                    }
                };
                let content = match current_text(&path, raw, &vstate) {
                    Ok(Some(c)) => c,
                    Ok(None) => {
                        problems.push(format!("修改 {raw}:文件不存在(新建请用 Add File)"));
                        continue;
                    }
                    Err(e) => {
                        problems.push(format!("修改 {raw}:{e}"));
                        continue;
                    }
                };
                // 先读后写:本补丁已触碰过的文件跳过(前面的写入会刷新读记录);
                // 其余与 fs_edit 同口径——没读过不许改
                if !vstate.contains_key(&path) {
                    if let Err(e) = scope.check_read_before_write(&path) {
                        problems.push(format!("修改 {raw}:{e}"));
                        continue;
                    }
                }
                let (mut lines, eol, had_nl) = split_lines(&content);
                let mut hunk_fail: Option<String> = None;
                for hunk in hunks {
                    match find_unique(&lines, &hunk.before) {
                        Ok(idx) => {
                            lines.splice(idx..idx + hunk.before.len(), hunk.after.iter().cloned());
                        }
                        Err(e) => {
                            hunk_fail =
                                Some(format!("修改 {raw}:第 {} 行附近的修改块:{e}", hunk.line_no));
                            break;
                        }
                    }
                }
                if let Some(e) = hunk_fail {
                    problems.push(e);
                    continue;
                }
                let new_content = join_lines(&lines, eol, had_nl);
                if new_content.len() > MAX_FILE_BYTES as usize {
                    problems.push(format!(
                        "修改 {raw}:结果超过单文件上限 {MAX_FILE_BYTES} 字节"
                    ));
                    continue;
                }
                let rel = display_rel(scope.workspace(), &path);

                // Move:先写新路径(create),再删旧路径(delete)——两笔都进台账,回滚可逆
                if let Some(target_raw) = move_to {
                    let target = match safe_workspace_path(
                        scope.workspace(),
                        target_raw,
                        Some(&deps.data_dir),
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            problems.push(format!("移动 {raw} → {target_raw}:{e}"));
                            continue;
                        }
                    };
                    if target == path {
                        problems.push(format!("移动 {raw}:目标与原路径相同"));
                        continue;
                    }
                    match current_text(&target, target_raw, &vstate) {
                        Ok(Some(_)) => {
                            problems.push(format!("移动 {raw} → {target_raw}:目标已存在"));
                            continue;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            problems.push(format!("移动 {raw} → {target_raw}:{e}"));
                            continue;
                        }
                    }
                    let rel_new = display_rel(scope.workspace(), &target);
                    vstate.insert(target.clone(), Some(new_content.clone()));
                    vstate.insert(path.clone(), None);
                    actions.push(PlannedAction::Write {
                        path: target,
                        rel: rel_new.clone(),
                        content: new_content,
                    });
                    actions.push(PlannedAction::Delete {
                        path,
                        rel: rel.clone(),
                    });
                    if hunks.is_empty() {
                        summaries.push(format!("移动 {rel} → {rel_new}"));
                    } else {
                        summaries.push(format!("移动 {rel} → {rel_new}({} 处修改)", hunks.len()));
                    }
                } else {
                    vstate.insert(path.clone(), Some(new_content.clone()));
                    actions.push(PlannedAction::Write {
                        path,
                        rel: rel.clone(),
                        content: new_content,
                    });
                    summaries.push(format!("修改 {rel}({} 处)", hunks.len()));
                }
            }
            PatchOp::Delete { path: raw } => {
                let path = match safe_workspace_path(scope.workspace(), raw, Some(&deps.data_dir)) {
                    Ok(p) => p,
                    Err(e) => {
                        problems.push(format!("删除 {raw}:{e}"));
                        continue;
                    }
                };
                match current_text(&path, raw, &vstate) {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        problems.push(format!("删除 {raw}:文件不存在"));
                        continue;
                    }
                    Err(e) => {
                        problems.push(format!("删除 {raw}:{e}"));
                        continue;
                    }
                }
                if !vstate.contains_key(&path) {
                    if let Err(e) = scope.check_read_before_write(&path) {
                        problems.push(format!("删除 {raw}:{e}"));
                        continue;
                    }
                }
                let rel = display_rel(scope.workspace(), &path);
                vstate.insert(path.clone(), None);
                actions.push(PlannedAction::Delete {
                    path,
                    rel: rel.clone(),
                });
                summaries.push(format!("删除 {rel}"));
            }
        }
    }

    if problems.is_empty() {
        Ok((actions, summaries))
    } else {
        Err(problems)
    }
}

pub(super) fn register_patch_tool(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: PATCH_TOOL.into(),
            description: "对工作区内文件应用结构化补丁(Codex 风格):一次调用可跨多文件新增/修改/删除/移动,先整体校验后落盘——任一问题则全部不应用。适合批量重构;单文件小改用 fs_edit 更省。"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "patch": { "type": "string", "description": "结构化补丁文本:以 *** Add/Update/Delete File: 开头分块,Update 块内 \"@@\" 分隔多个修改块,\" \" 上下文行、\"-\" 删除行、\"+\" 新增行;可整体包在 *** Begin Patch / *** End Patch 之间" }
                },
                "required": ["patch"]
            }),
        },
        Arc::new(move |args: serde_json::Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move { run_patch(&deps, &ctx, &args) })
        }),
    );
}

/// 工具执行入口(注册与单测共用)
fn run_patch(
    deps: &ToolDeps,
    ctx: &ToolContext,
    args: &serde_json::Value,
) -> Result<String, String> {
    let raw = args.get("patch").and_then(|v| v.as_str()).unwrap_or("");
    if raw.trim().is_empty() {
        return Err("patch 不能为空:请给出结构化补丁文本".into());
    }
    if raw.len() > MAX_PATCH_BYTES {
        return Err(format!(
            "补丁过大({} 字节,上限 {MAX_PATCH_BYTES});请拆分为多条 fs_patch 调用",
            raw.len()
        ));
    }
    let ops = parse_patch(raw)?;
    if ops.is_empty() {
        return Err("补丁不包含任何操作(没有 *** Add/Update/Delete File 行)".into());
    }
    if ops.len() > MAX_PATCH_OPS {
        return Err(format!(
            "操作数过多({},上限 {MAX_PATCH_OPS});请拆分为多条 fs_patch 调用",
            ops.len()
        ));
    }
    let (actions, summaries) = plan_patch(deps, ctx, &ops).map_err(|mut problems| {
        let mut msg = format!("补丁未应用(共 {} 个问题;所有文件保持原样):", problems.len());
        for (i, p) in problems.drain(..).enumerate() {
            msg.push_str(&format!("\n{}. {p}", i + 1));
        }
        msg
    })?;

    let scope = scope_of(ctx)?;
    let total = actions.len();
    for (done, action) in actions.iter().enumerate() {
        let res = match action {
            PlannedAction::Write { path, rel, content } => {
                let baseline = task_change_service::Baseline::capture(path);
                match crate::utils::fs_atomic::write_atomic(path, content.as_bytes()) {
                    Ok(()) => {
                        // 写后刷新读记录(与 fs_write 同款:否则同会话连续操作会被自己拦下)
                        if let Ok(stamp) = ExecScope::stamp_of(path) {
                            scope.note_read(path, stamp);
                        }
                        note_change(deps, ctx, rel, path, &baseline);
                        Ok(())
                    }
                    Err(e) => Err(format!("{rel}:{e}")),
                }
            }
            PlannedAction::Delete { path, rel } => {
                // 基线必须在删除**之前** capture(同 fs_write 口径),否则回滚无源
                let baseline = task_change_service::Baseline::capture(path);
                match std::fs::remove_file(path) {
                    Ok(()) => {
                        if let Some(task_id) = task_change_service::task_id_of(&ctx.session_id) {
                            task_change_service::record(
                                &deps.db, &task_id, rel, "delete", "tool", &baseline, None,
                            );
                        }
                        Ok(())
                    }
                    Err(e) => Err(format!("{rel}:{e}")),
                }
            }
        };
        if let Err(e) = res {
            return Err(format!(
                "补丁**部分应用**:已完成 {done}/{total} 项,在 {e} 失败;后续 {} 项未执行。已落盘的改动均已记账,可用任务回滚收拾",
                total - done - 1
            ));
        }
    }

    let mut out = format!("已应用补丁(共 {} 项操作):", summaries.len());
    for s in &summaries {
        out.push_str(&format!("\n- {s}"));
    }
    Ok(out)
}

/// fs_patch 的「工作状态锚点」:补丁里的目标路径清单(去重、按出现顺序、至多 3 个)。
///
/// 供 `agents/engine/messages/trim.rs` 的锚点消费(批次 3 机制)取值——补丁是结构化工具,
/// 首行("*** Begin Patch")没有信息量,锚点应是目标文件坐标。这里只做轻量行扫描
/// (与 parse 共用同一套头部前缀常量);锚点消费在历史裁剪热路径上,对畸形补丁
/// **尽力而为**、绝不报错。
pub(crate) fn patch_anchor(name: &str, arguments: &str) -> Option<String> {
    if name != PATCH_TOOL {
        return None;
    }
    let patch = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()?
        .get("patch")?
        .as_str()?
        .to_string();
    let mut paths: Vec<String> = Vec::new();
    for line in patch.lines() {
        let t = line.trim_start();
        for header in [ADD_HEADER, UPDATE_HEADER, DELETE_HEADER, MOVE_HEADER] {
            if let Some(rest) = t.strip_prefix(header) {
                let p = rest.trim();
                if !p.is_empty() && !paths.iter().any(|x| x == p) {
                    paths.push(p.to_string());
                }
            }
            if paths.len() >= 3 {
                break;
            }
        }
        if paths.len() >= 3 {
            break;
        }
    }
    if paths.is_empty() {
        None
    } else {
        Some(paths.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::types::ExecScope;
    use crate::utils::test_support::TempDataDir;

    struct Fixture {
        _guard: TempDataDir,
        /// 工作区父目录守卫:必须随夹具存活(否则目录在 fixture() 返回时即被删)
        _ws_dir: TempDataDir,
        deps: Arc<ToolDeps>,
        ws: PathBuf,
        scope: Arc<ExecScope>,
    }

    fn fixture(tag: &str) -> Fixture {
        let (guard, deps) = ToolDeps::dummy_for_test();
        // 台账 task_file_changes.task_id 是 tasks(id) 外键:夹具先种一条任务行——
        // 否则记账被 FK 拒绝(record 只 warn 不报错),changes() 恒空、断言无从成立
        {
            let conn = deps.db.write();
            conn.execute(
                "INSERT INTO tasks (id, title, created_at, updated_at) VALUES ('t1', '补丁夹具任务', '2026-10-01T00:00:00Z', '2026-10-01T00:00:00Z')",
                [],
            )
            .unwrap();
        }
        let dir = TempDataDir::new(&format!("fspatch-{tag}"));
        let ws = dir.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("src/main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        std::fs::write(ws.join("README.md"), "hello\nworld\n").unwrap();
        let scope = {
            let ws_str = ws.to_string_lossy().into_owned();
            crate::tools::workspace_guard::scope_for_task(Some(&ws_str))
                .unwrap()
                .unwrap()
        };
        Fixture {
            _guard: guard,
            _ws_dir: dir,
            deps: Arc::new(deps),
            ws,
            scope,
        }
    }

    impl Fixture {
        fn ctx(&self) -> ToolContext {
            ToolContext {
                session_id: "task:t1".into(),
                character_id: String::new(),
                agent_depth: 0,
                scope: Some(self.scope.clone()),
            }
        }

        fn registry(&self) -> ToolRegistry {
            let reg = ToolRegistry::new();
            super::super::agent_tools_fs::register_fs_tools(&reg, self.deps.clone());
            reg
        }

        async fn call(&self, name: &str, args: serde_json::Value) -> Result<String, String> {
            let decision = crate::tools::permissions::PermissionDecision {
                allowed: true,
                risk: crate::tools::permissions::ToolRisk::Dangerous,
                reason: "测试放行".into(),
            };
            self.registry()
                .execute_with_decision(name, &args.to_string(), self.ctx(), &decision)
                .await
        }

        fn read(&self, path: &str) -> String {
            std::fs::read_to_string(self.ws.join(path)).unwrap()
        }

        fn changes(&self) -> Vec<crate::models::types::TaskFileChangeRecord> {
            crate::services::task_change_service::list(&self.deps.db, "t1")
        }
    }

    /// 一条补丁同时覆盖 新增 / 修改(多 hunk)/ 删除 / 移动,顺序落盘并全部记账。
    #[tokio::test]
    async fn applies_add_update_delete_move_in_one_patch() {
        let f = fixture("multi");
        // 修改前先读(先读后写纪律)
        f.call("fs_read", json!({ "path": "src/main.rs" }))
            .await
            .unwrap();
        f.call("fs_read", json!({ "path": "README.md" }))
            .await
            .unwrap();

        let patch = "\
*** Begin Patch
*** Add File: src/new.rs
+pub fn a() -> i32 { 1 }
*** Update File: src/main.rs
@@
 fn main() {
-    println!(\"hi\");
+    println!(\"hello\");
 }
*** Update File: README.md
*** Move to: docs/README.md
@@
-hello
+hello, kedai
 world
*** Delete File: src/main.rs
*** End Patch";
        let out = f
            .call("fs_patch", json!({ "patch": patch }))
            .await
            .expect("补丁应成功");
        assert!(out.contains("新增 src/new.rs"), "{out}");
        assert!(out.contains("修改 src/main.rs(1 处)"), "{out}");
        assert!(
            out.contains("移动 README.md → docs/README.md(1 处修改)"),
            "{out}"
        );
        assert!(out.contains("删除 src/main.rs"), "{out}");

        // 盘上状态:新增存在、main.rs 已删、README 已搬并改
        assert_eq!(f.read("src/new.rs"), "pub fn a() -> i32 { 1 }\n");
        assert!(!f.ws.join("src/main.rs").exists(), "move 后应删除旧路径");
        assert!(!f.ws.join("README.md").exists());
        assert_eq!(f.read("docs/README.md"), "hello, kedai\nworld\n");

        // 台账:全部记账(source=tool),回滚有源
        let ops: Vec<(String, String)> = f
            .changes()
            .into_iter()
            .map(|c| (c.path.clone(), c.op.clone()))
            .collect();
        for want in [
            ("src/new.rs", "create"),
            ("src/main.rs", "modify"),
            ("README.md", "delete"),
            ("docs/README.md", "create"),
            ("src/main.rs", "delete"),
        ] {
            assert!(
                ops.iter().any(|(p, o)| p == want.0 && o == want.1),
                "台账应含 {want:?};实际:{ops:?}"
            );
        }
    }

    /// 任一修改块不唯一/未匹配 → **整条补丁不落盘**,问题一次报全。
    #[tokio::test]
    async fn any_problem_rejects_whole_patch() {
        let f = fixture("reject");
        f.call("fs_read", json!({ "path": "src/main.rs" }))
            .await
            .unwrap();
        let before = f.read("src/main.rs");

        let patch = "\
*** Add File: ok.txt
+fine
*** Update File: src/main.rs
@@
-不存在的旧内容
+替换
";
        let err = f
            .call("fs_patch", json!({ "patch": patch }))
            .await
            .unwrap_err();
        assert!(err.contains("补丁未应用"), "{err}");
        assert!(err.contains("修改 src/main.rs"), "{err}");
        // 整条拒绝:新增文件也不得出现
        assert!(!f.ws.join("ok.txt").exists(), "整条拒绝时不得落盘任何文件");
        assert_eq!(f.read("src/main.rs"), before);
        assert!(f.changes().is_empty(), "拒绝路径不应产生台账");
    }

    /// 先读后写:未读过的文件不许改/删;读过之后放行。
    #[tokio::test]
    async fn update_requires_read_first() {
        let f = fixture("read");
        let patch = "\
*** Update File: src/main.rs
@@
 fn main() {
-    println!(\"hi\");
+    println!(\"hi2\");
 }
";
        let err = f
            .call("fs_patch", json!({ "patch": patch }))
            .await
            .unwrap_err();
        assert!(err.contains("防盲写"), "应提示先读后写:{err}");

        f.call("fs_read", json!({ "path": "src/main.rs" }))
            .await
            .unwrap();
        let ok = f
            .call("fs_patch", json!({ "patch": patch }))
            .await
            .expect("读过之后应成功");
        assert!(ok.contains("修改 src/main.rs"), "{ok}");
    }

    /// 越界路径(../)、二进制、不唯一上下文:都按问题清单拒绝,不落盘。
    #[tokio::test]
    async fn rejects_escape_binary_and_ambiguous() {
        let f = fixture("guard");
        let escape = "\
*** Add File: ../evil.txt
+x
";
        let err = f
            .call("fs_patch", json!({ "patch": escape }))
            .await
            .unwrap_err();
        assert!(err.contains("补丁未应用"), "{err}");

        std::fs::write(f.ws.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        f.call("fs_read", json!({ "path": "src/main.rs" }))
            .await
            .unwrap();
        let binary = "\
*** Delete File: bin.dat
";
        let err = f
            .call("fs_patch", json!({ "patch": binary }))
            .await
            .unwrap_err();
        assert!(err.contains("二进制"), "{err}");

        // 不唯一:同一段出现两次
        std::fs::write(f.ws.join("dup.txt"), "x\ny\nx\ny\n").unwrap();
        f.call("fs_read", json!({ "path": "dup.txt" }))
            .await
            .unwrap();
        let ambiguous = "\
*** Update File: dup.txt
@@
-x
+y
";
        let err = f
            .call("fs_patch", json!({ "patch": ambiguous }))
            .await
            .unwrap_err();
        assert!(err.contains("不唯一"), "{err}");
        assert_eq!(f.read("dup.txt"), "x\ny\nx\ny\n", "拒绝后原文不动");
    }

    /// CRLF 文件:匹配归一、重写保持 CRLF;新增文件用 LF。
    #[tokio::test]
    async fn preserves_crlf_of_existing_file() {
        let f = fixture("crlf");
        std::fs::write(f.ws.join("win.txt"), "a\r\nb\r\n").unwrap();
        f.call("fs_read", json!({ "path": "win.txt" }))
            .await
            .unwrap();
        let patch = "\
*** Update File: win.txt
@@
-a
+A
";
        f.call("fs_patch", json!({ "patch": patch })).await.unwrap();
        assert_eq!(f.read("win.txt"), "A\r\nb\r\n", "CRLF 应保持");

        let add = "\
*** Add File: lf.txt
+n
";
        f.call("fs_patch", json!({ "patch": add })).await.unwrap();
        assert_eq!(f.read("lf.txt"), "n\n");
    }

    /// 解析器:错误行号与提示;Move 位置约束;End 之后有内容报错。
    #[test]
    fn parser_reports_line_level_errors() {
        let e = parse_patch("*** Unknown: x\n").unwrap_err();
        assert!(e.contains("第 1 行"), "{e}");

        let e = parse_patch("*** Update File: a\n*** Move to: b\n@@\n-x\n+y\n*** Move to: c\n")
            .unwrap_err();
        assert!(e.contains("Move to"), "{e}");

        let e = parse_patch("*** Add File: a\n+x\n*** End Patch\nnope\n").unwrap_err();
        assert!(e.contains("End Patch"), "{e}");

        let e = parse_patch("*** Update File: a\n\n").unwrap_err();
        assert!(e.contains("空行"), "{e}");
    }

    /// 上限:操作数超限与空补丁都明确报错。
    #[tokio::test]
    async fn caps_and_empty_patch() {
        let f = fixture("caps");
        let err = f
            .call("fs_patch", json!({ "patch": "   " }))
            .await
            .unwrap_err();
        assert!(err.contains("不能为空"), "{err}");

        let mut big = String::new();
        for i in 0..40 {
            big.push_str(&format!("*** Add File: f{i}.txt\n+x\n"));
        }
        let err = f
            .call("fs_patch", json!({ "patch": big }))
            .await
            .unwrap_err();
        assert!(err.contains("操作数过多"), "{err}");
    }

    /// 锚点:取补丁目标路径(至多 3 个、去重);非 fs_patch 返回 None。
    #[test]
    fn anchor_extracts_target_paths() {
        let args = json!({ "patch": "*** Begin Patch\n*** Add File: src/a.rs\n+x\n*** Update File: src/b.rs\n*** Move to: src/c.rs\n@@\n-a\n+b\n*** Update File: src/b.rs\n@@\n-x\n+y\n*** Delete File: src/d.rs\n*** End Patch" }).to_string();
        let a = patch_anchor("fs_patch", &args).expect("应取到锚点");
        assert_eq!(a, "src/a.rs, src/b.rs, src/c.rs");
        assert!(patch_anchor("fs_edit", &args).is_none());
        assert!(patch_anchor("fs_patch", "not json").is_none());
    }
}
