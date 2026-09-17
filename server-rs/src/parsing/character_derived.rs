// 角色卡派生字段的**单一计算源**(2026-09-17 P-11)。
//
// 背景:`characters.data_raw` 是完整 V2/V3 JSON,而列表接口需要的只是 6 个派生字段
// (first_mes / alternate_greetings / regex_scripts / creator / character_version /
// creator_notes)。此前这些字段在读取路径上**每次现算**(整份 `serde_json::from_str`),
// 列表成本随「卡数 × 卡体积」线性增长。
//
// 现在派生结果缓存进 `characters.derived_json` 列。该列是**缓存而非事实源**
// (读取侧在列为空/非法时回退解析 data_raw),故本模块只负责「给定 data_raw 算出派生 JSON」,
// 不含任何 IO、不感知列的存在——写入点(服务层)与迁移回填(migrations)共用同一实现,
// 避免两处各写一份提取规则而漂移。
//
// 代际:L1(与所在 parsing 模块一致)——纯函数,零上层依赖。

use serde_json::{json, Value};

/// 从 data_raw 提取备用开场列表(alternate_greetings):取非空字符串;空/非法返回 None。
///
/// 语义与既有读取路径**逐字对齐**(非空判断用 `trim()` 排除纯空白项)。
pub fn extract_alternate_greetings(data_raw: &Value) -> Option<Vec<String>> {
    let arr = data_raw.get("alternate_greetings")?.as_array()?;
    let list: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

/// 计算 `characters.derived_json` 的值(序列化后的 JSON 字符串)。
///
/// 输出是一个 JSON 对象,键与列表接口的派生字段同名,便于读取侧直接取字段:
/// `{ "first_mes": "...", "alternate_greetings": [...], "regex_scripts": [...],
///   "creator": "...", "character_version": "...", "creator_notes": "..." }`
///
/// 缺省语义(与既有 `row_to_character` 一致,**不得改动**,否则列表行为会静默变化):
/// - `first_mes`:非空字符串才保留(`filter(|s| !s.is_empty())`——注意此处**不用** trim,
///   与下方 pick_str 的三件套口径不同,照抄既有实现);
/// - `alternate_greetings`:非空字符串数组才保留;
/// - `regex_scripts`:正则脚本数组,空数组则省略(前端据此判断是否走 HTML 渲染分支);
/// - 三件套(`creator`/`character_version`/`creator_notes`):**trim 后非空**才保留。
///
/// 序列化失败返回 `"{}"`(空缓存):读取侧见空值会回退解析 data_raw,故不会丢数据。
pub fn compute_derived_json(data_raw: &Value) -> String {
    let first_mes = data_raw
        .get("first_mes")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let alternate_greetings = extract_alternate_greetings(data_raw);

    let regex_scripts = crate::parsing::regex_script::extract_regex_scripts(data_raw);
    let regex_scripts = if regex_scripts.is_empty() {
        None
    } else {
        Some(regex_scripts)
    };

    // 卡元数据三件套(远程资源页口令推导用):trim 后非空才计入
    let pick_str = |key: &str| {
        data_raw
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
    };

    let mut out = serde_json::Map::new();
    if let Some(v) = first_mes {
        out.insert("first_mes".into(), json!(v));
    }
    if let Some(v) = alternate_greetings {
        out.insert("alternate_greetings".into(), json!(v));
    }
    if let Some(v) = regex_scripts {
        out.insert("regex_scripts".into(), json!(v));
    }
    if let Some(v) = pick_str("creator") {
        out.insert("creator".into(), json!(v));
    }
    if let Some(v) = pick_str("character_version") {
        out.insert("character_version".into(), json!(v));
    }
    if let Some(v) = pick_str("creator_notes") {
        out.insert("creator_notes".into(), json!(v));
    }
    serde_json::to_string(&Value::Object(out)).unwrap_or_else(|_| "{}".into())
}

/// 从 `derived_json` 列值解出派生字段。
///
/// 返回 `None` 表示**该列不可用**(空、`"{}"`、非法 JSON 或非对象),调用方应回退解析
/// `data_raw`。这是「缓存列而非事实源」的落点:任何不确定都退回权威数据。
pub fn parse_derived_json(derived: &str) -> Option<Value> {
    let trimmed = derived.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return None;
    }
    match serde_json::from_str::<Value>(trimmed) {
        // 非对象(如 "[]"/"null")同样视为不可用
        Ok(v) if v.is_object() => Some(v),
        _ => None,
    }
}

/// 从已解析的派生对象取可选字符串(字段缺失/非字符串 → None)
pub fn derived_str(derived: &Value, key: &str) -> Option<String> {
    derived
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 从已解析的派生对象取非空字符串数组(字段缺失/空数组 → None)
pub fn derived_string_list(derived: &Value, key: &str) -> Option<Vec<String>> {
    let arr = derived.get(key)?.as_array()?;
    let list: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}
