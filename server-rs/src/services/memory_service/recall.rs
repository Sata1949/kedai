//! 召回排序:注入选择、向量+Jaccard 混合召回、分词与相似度。

use std::collections::HashSet;

// 子模块内统一用 super::* 拿到 mod.rs 的公共词汇(类型/常量)与 re-export 的自由函数。
use super::*;

/// 注入候选精选(纯函数):selected=1 的条目按
/// (pinned DESC, usage_count DESC, last_usage DESC, id DESC) 排序取前 limit 条。
/// id 作最终 tie-break 保证确定性——记忆集合未变时输出顺序逐字节稳定(前缀缓存前提)。
pub fn select_for_injection(entries: &[MemoryEntry], limit: usize) -> Vec<&MemoryEntry> {
    let mut picked: Vec<&MemoryEntry> = entries.iter().filter(|e| e.selected).collect();
    picked.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(b.usage_count.cmp(&a.usage_count))
            .then(b.last_usage.cmp(&a.last_usage))
            .then(b.id.cmp(&a.id))
    });
    picked.truncate(limit);
    picked
}

/// 检索召回(纯函数,升级工作流 B2 通道 2 + Phase 3 向量混合):
/// 按当前用户输入与记忆条目的相似度降序取前 limit 条,只返回 selected=1 且不在
/// exclude 内的条目。排序键 (相似度 DESC, pinned DESC, usage_count DESC, id DESC)
/// 确定性;相似度为 0 的条目不入召回(无相关就不注入)。exclude 为通道 1 已注入的 id。
///
/// `query_vec` 为查询向量(None = 向量化未启用/调用失败,退化为纯 Jaccard);
/// `entry_vecs` 为「记忆 id → 向量」映射,缺失该条目的向量时其向量分按 0 计。
pub fn select_recall<'a>(
    entries: &'a [MemoryEntry],
    query: &str,
    limit: usize,
    exclude: &[i64],
) -> Vec<&'a MemoryEntry> {
    select_recall_hybrid(
        entries,
        query,
        limit,
        exclude,
        None,
        &std::collections::HashMap::new(),
    )
}

/// 带向量的混合召回(纯函数,便于单测):
/// 有向量时 score = 向量余弦 × 0.7 + Jaccard × 0.3;无向量时 score = Jaccard。
pub fn select_recall_hybrid<'a>(
    entries: &'a [MemoryEntry],
    query: &str,
    limit: usize,
    exclude: &[i64],
    query_vec: Option<&[f32]>,
    entry_vecs: &std::collections::HashMap<i64, Vec<f32>>,
) -> Vec<&'a MemoryEntry> {
    let query_tokens = tokenize_for_similarity(query);
    if limit == 0 {
        return Vec::new();
    }
    // 查询既无词面 token 又无向量 → 无法判断相关性
    let vec_ready = query_vec.map(|v| !v.is_empty()).unwrap_or(false);
    if query_tokens.is_empty() && !vec_ready {
        return Vec::new();
    }
    let excluded: HashSet<i64> = exclude.iter().copied().collect();
    let mut scored: Vec<(f64, &MemoryEntry)> = entries
        .iter()
        .filter(|e| e.selected && !excluded.contains(&e.id))
        .filter_map(|e| {
            let jac = if query_tokens.is_empty() {
                0.0
            } else {
                jaccard_similarity(&query_tokens, &tokenize_for_similarity(&e.content))
            };
            let score = match (query_vec, entry_vecs.get(&e.id)) {
                (Some(qv), Some(ev)) if !qv.is_empty() && !ev.is_empty() => {
                    let cos = crate::services::embedding_service::cosine_similarity(qv, ev) as f64;
                    // 余弦可能为负(方向相反);钳到 0 避免负分压过词面匹配
                    let cos = cos.max(0.0);
                    cos * RECALL_VEC_WEIGHT + jac * RECALL_JACCARD_WEIGHT
                }
                _ => jac,
            };
            (score > 0.0).then_some((score, e))
        })
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| {
        sb.partial_cmp(sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.pinned.cmp(&a.pinned))
            .then(b.usage_count.cmp(&a.usage_count))
            .then(b.id.cmp(&a.id))
    });
    scored.truncate(limit);
    scored.into_iter().map(|(_, e)| e).collect()
}

/// 相似度用 token 切分:按非字母数字/非 CJK 字符切段,再对 CJK 段做二元组(bigram)
/// 展开(中文无空格,单字集合区分度低),ASCII 词保留整词并小写化。
pub fn tokenize_for_similarity(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut ascii = String::new();
    let mut cjk: Vec<char> = Vec::new();
    let flush_ascii = |ascii: &mut String, out: &mut HashSet<String>| {
        if !ascii.is_empty() {
            out.insert(ascii.to_lowercase());
            ascii.clear();
        }
    };
    let flush_cjk = |cjk: &mut Vec<char>, out: &mut HashSet<String>| {
        match cjk.len() {
            0 => {}
            1 => {
                out.insert(cjk[0].to_string());
            }
            _ => {
                for pair in cjk.windows(2) {
                    out.insert(pair.iter().collect::<String>());
                }
            }
        }
        cjk.clear();
    };
    for ch in text.chars() {
        let is_cjk = matches!(ch as u32,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF);
        if is_cjk {
            flush_ascii(&mut ascii, &mut out);
            cjk.push(ch);
        } else if ch.is_alphanumeric() {
            flush_cjk(&mut cjk, &mut out);
            ascii.push(ch);
        } else {
            flush_ascii(&mut ascii, &mut out);
            flush_cjk(&mut cjk, &mut out);
        }
    }
    flush_ascii(&mut ascii, &mut out);
    flush_cjk(&mut cjk, &mut out);
    out
}

/// token 集合 Jaccard 相似度(|交| / |并|);任一侧为空返回 0
pub fn jaccard_similarity(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    inter / union
}
