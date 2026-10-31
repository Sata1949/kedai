// restore 前的文件快照项预检:payload 是持久化数据(可能被截断/手工污染),
// 字段与类型必须齐备——apply 侧把「old 非字符串」一律当「逆操作 = 删除」,
// 缺字段会静默删文件。单独成模块是为了让 undo_service.rs 保持在体积基线以内。
use crate::tools::agent_tools::safe_rel_path;
use serde_json::Value;

use super::MAX_SNAPSHOT_FILE_BYTES;

/// 单个文件快照项的预检:路径必须通过 safe_rel_path(防御 payload 内路径),
/// 截断标志必须为假(256KB 上限外的内容无法精确恢复)。
pub(super) fn precheck_file_entry(snapshot_id: &str, entry: &Value) -> Result<(), String> {
    let rel = entry
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("快照 {snapshot_id} 缺文件路径"))?;
    safe_rel_path(rel).map_err(|e| format!("快照 {snapshot_id} 文件路径非法({rel}): {e}"))?;
    // codex 复审 P1-3:payload 是持久化数据(可能被截断/手工污染),字段与类型必须
    // 齐备——apply 侧把「old 非字符串」一律当「逆操作 = 删除」,缺字段会静默删文件。
    match entry.get("entry_type").and_then(|v| v.as_str()) {
        Some("file") | Some("dir") => {}
        _ => {
            return Err(format!(
                "快照 {snapshot_id} 的文件项 {rel} 缺合法 entry_type(应为 file/dir)"
            ));
        }
    }
    match entry.get("old") {
        Some(Value::String(s)) => {
            // UNDOFIX2-L4:文件正文也必须有上限。快照写入侧本就 ≤256KB,
            // 超出者只可能来自持久化数据被污染/手工拼接;预检放行等于让它进 restore。
            if s.len() > MAX_SNAPSHOT_FILE_BYTES {
                return Err(format!(
                    "快照 {snapshot_id} 的文件项 {rel} 的 old 超过 {}KB 上限",
                    MAX_SNAPSHOT_FILE_BYTES / 1024
                ));
            }
        }
        Some(Value::Null) => {}
        _ => {
            return Err(format!(
                "快照 {snapshot_id} 的文件项 {rel} 的 old 缺失或类型非法(应为字符串或 null)"
            ));
        }
    }
    match entry.get("truncated") {
        Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => {
            return Err(format!(
                "快照 {snapshot_id} 的文件 {rel} 原始内容超过 256KB,快照不完整,无法回退"
            ));
        }
        _ => {
            return Err(format!(
                "快照 {snapshot_id} 的文件项 {rel} 的 truncated 缺失或类型非法(应为布尔)"
            ));
        }
    }
    // UNDOFIX2-L4:existed 必须**在场**且形态与 old 自洽。旧实现只校验「若在则必须是
    // 布尔」——缺 existed 时 `{entry_type:"file", old:null}` 会被 apply 侧当「逆操作 =
    // 删除」而静默删掉文件。写入侧(见 file_entry)恒显式给 existed,故不做兼容放宽。
    let existed = match entry.get("existed") {
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            return Err(format!(
                "快照 {snapshot_id} 的文件项 {rel} 的 existed 类型非法(应为布尔)"
            ));
        }
        None => {
            return Err(format!(
                "快照 {snapshot_id} 的文件项 {rel} 缺 existed,无法判断逆操作是回写还是删除"
            ));
        }
    };
    // 形态自洽:existed=true 的文件项必须带 old 字符串(否则被当删除);existed=false
    // 必须 old=null(否则回写会凭空造出文件)。
    let has_old = entry.get("old").is_some_and(|v| v.is_string());
    if existed && !has_old {
        return Err(format!(
            "快照 {snapshot_id} 的文件项 {rel} 标 existed=true 却无 old 内容,\
             回退会被误当成删除"
        ));
    }
    if !existed && has_old {
        return Err(format!(
            "快照 {snapshot_id} 的文件项 {rel} 标 existed=false 却带 old 内容,\
             回退会凭空写出文件"
        ));
    }
    Ok(())
}
