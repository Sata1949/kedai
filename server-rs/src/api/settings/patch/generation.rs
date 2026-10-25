//! 生成参数与上下文压缩(roleplay 写扁平;task 写覆盖层——`for_mode` 会合并)。
use crate::services::settings_service::RuntimeSettings;

use super::super::UpdateSettingsBody;
use super::apply_mode;
use super::PatchError;

pub(super) fn apply(
    s: &mut RuntimeSettings,
    body: &UpdateSettingsBody,
    is_task: bool,
) -> Result<(), PatchError> {
    if let Some(v) = body.default_temperature {
        if !(0.0..=2.0).contains(&v) {
            return Err(PatchError::new(
                "default_temperature",
                "default_temperature 必须在 0.0..=2.0",
            ));
        }
        apply_mode!(s, is_task, default_temperature, v);
    }
    if let Some(v) = body.default_top_p {
        if !(0.0..=1.0).contains(&v) {
            return Err(PatchError::new(
                "default_top_p",
                "default_top_p 必须在 0.0..=1.0",
            ));
        }
        apply_mode!(s, is_task, default_top_p, v);
    }
    if let Some(v) = body.default_max_tokens {
        if v == 0 || v > 131_072 {
            return Err(PatchError::new(
                "default_max_tokens",
                "default_max_tokens 必须在 1..=131072",
            ));
        }
        apply_mode!(s, is_task, default_max_tokens, v);
    }
    if let Some(v) = body.max_context_tokens {
        if !(65_536..=1_048_576).contains(&v) {
            return Err(PatchError::new(
                "max_context_tokens",
                "max_context_tokens 必须在 65536..=1048576(64K~1M)",
            ));
        }
        apply_mode!(s, is_task, max_context_tokens, v);
    }
    // HTML 渲染开关
    if let Some(v) = body.render_html {
        apply_mode!(s, is_task, render_html, v);
    }
    // 上下文压缩模式:仅接受 off / manual / auto(非法值 400,不再静默忽略)
    if let Some(v) = &body.compaction_mode {
        let t = v.trim().to_string();
        if !matches!(t.as_str(), "off" | "manual" | "auto") {
            return Err(PatchError::new(
                "compaction_mode",
                "compaction_mode 仅支持 off / manual / auto",
            ));
        }
        apply_mode!(s, is_task, compaction_mode, t);
    }
    // 上下文压缩阈值:仅接受 0.5..=0.95
    if let Some(v) = body.compaction_threshold {
        if !(0.5..=0.95).contains(&v) {
            return Err(PatchError::new(
                "compaction_threshold",
                "compaction_threshold 必须在 0.5..=0.95",
            ));
        }
        apply_mode!(s, is_task, compaction_threshold, v);
    }
    // 压缩保留条数(缓存感知管线):2..=200
    if let Some(v) = body.compaction_keep_recent {
        if !(2..=200).contains(&v) {
            return Err(PatchError::new(
                "compaction_keep_recent",
                "compaction_keep_recent 必须在 2..=200",
            ));
        }
        apply_mode!(s, is_task, compaction_keep_recent, v);
    }
    // snip 零成本裁剪阈值(字节):0 = 禁用,上限 1MB
    if let Some(v) = body.compaction_snip_bytes {
        if v > 1_048_576 {
            return Err(PatchError::new(
                "compaction_snip_bytes",
                "compaction_snip_bytes 必须在 0..=1048576",
            ));
        }
        apply_mode!(s, is_task, compaction_snip_bytes, v);
    }
    // LLM 请求快照开关
    if let Some(v) = body.llm_request_log {
        apply_mode!(s, is_task, llm_request_log, v);
    }
    // HB-7:变量两步生成的独立模型/温度档。清除语义用哨兵值表达,避免引入
    // 「Option<Option<T>>」这类与既有 PATCH 体例不符的写法:
    // mvu_model 空串 = 清除(回到与正文共用同一连接器/模型);
    // mvu_temperature 负值 = 清除(回到内置 0.3;温度本身不允许负数)
    if let Some(v) = &body.mvu_model {
        let trimmed = v.trim();
        if trimmed.chars().count() > 200 {
            return Err(PatchError::new(
                "mvu_model",
                "mvu_model 长度不得超过 200 字符",
            ));
        }
        s.mvu_model = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }
    if let Some(v) = body.mvu_temperature {
        if v < 0.0 {
            s.mvu_temperature = None;
        } else if !(0.0..=2.0).contains(&v) {
            return Err(PatchError::new(
                "mvu_temperature",
                "mvu_temperature 须为 0.0..=2.0,或负值表示清除",
            ));
        } else {
            s.mvu_temperature = Some(v);
        }
    }
    Ok(())
}
