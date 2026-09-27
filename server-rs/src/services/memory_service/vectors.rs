//! vec0 向量的字节编解码(blob ↔ f32)与 JSON 回退解析。

/// vec0 向量列以 BLOB 返回:小端 float32 紧密排列,每 4 字节一个分量。
/// 长度不是 4 的倍数时返回 None(防御脏数据)。
pub(super) fn blob_to_vec(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.is_empty() || !blob.len().is_multiple_of(4) {
        return None;
    }
    // `as_chunks` 而非 `chunks_exact(4)`(Rust 1.98 的 clippy 新增
    // `chunks_exact_to_as_chunks` 硬门禁):得到的 `&[u8; 4]` 是定长数组,
    // `from_le_bytes` 直接吃下,不必再做 `c[0..4]` 的边界检查。
    let (words, _) = blob.as_chunks::<4>();
    Some(words.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

/// 解析 sqlite-vec 返回的向量文本格式 `[1.0,2.0,...]`(容错:空白跳过,非法项返回 None)。
/// 注:vec0 的向量列在查询时以 BLOB 返回,此函数仅用于少数以 TEXT 形态取数的兼容路径。
#[allow(dead_code)]
pub(super) fn parse_vec_json(text: &str) -> Option<Vec<f32>> {
    let t = text.trim();
    let inner = t.strip_prefix('[')?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for part in inner.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        match p.parse::<f32>() {
            Ok(v) => out.push(v),
            Err(_) => return None,
        }
    }
    Some(out)
}
