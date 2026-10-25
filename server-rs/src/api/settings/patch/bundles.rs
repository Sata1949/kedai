//! 能力包开关与文学包选择型字段。
//!
//! **必须是最后一域**:文学推荐档会写生成/压缩三项;两个默认词重物化也要在
//! 本次请求的全部字段应用完之后判定,才能做到「开关翻转运行期即时生效」。
use crate::services::settings_service::{
    is_valid_literary_recommend_preset, is_valid_literary_style_preset, literary_recommend_values,
    LiteraryRecommendSnapshot, RuntimeSettings,
};

use super::super::UpdateSettingsBody;
use super::apply_mode;
use super::PatchError;

pub(super) fn apply(
    s: &mut RuntimeSettings,
    body: &UpdateSettingsBody,
    is_task: bool,
) -> Result<(), PatchError> {
    // 编码能力包开关(2026-09-28;bool 免校验,task 写覆盖层)。只影响提示词缺省值:
    // 启用后任务模式缺省默认词改用编码执行者模板,用户自定义值仍逐字优先。
    if let Some(v) = body.task_coding_bundle_enabled {
        apply_mode!(s, is_task, task_coding_bundle_enabled, v);
    }
    // 文学能力包开关(LIT-1;bool 免校验)。角色扮演侧开关是**纯扁平字段**——
    // 该侧没有覆盖层(for_mode 对 Roleplay 直接 clone),走 apply! 会写出无意义的
    // task 覆盖层,故照 exec_* 直写(s.xxx = v,与请求的 mode 无关);
    // 任务侧开关对称编码包,走覆盖层。
    if let Some(v) = body.literary_bundle_enabled {
        s.literary_bundle_enabled = v;
    }
    if let Some(v) = body.task_literary_bundle_enabled {
        apply_mode!(s, is_task, task_literary_bundle_enabled, v);
    }
    // 文学包文风预设(LIT-6):空 = 不注入;未知取值 400(不静默回退)。
    // 角色扮演侧纯扁平字段(同上方开关的直写口径)。
    if let Some(v) = &body.literary_style_preset {
        let t = v.trim().to_string();
        if !is_valid_literary_style_preset(&t) {
            return Err(PatchError::new(
                "literary_style_preset",
                "literary_style_preset 仅支持 plain / classical / lightnovel / hardboiled(空 = 不注入)",
            ));
        }
        s.literary_style_preset = t;
    }
    // 文学包长程一致性推荐档(LIT-7):**显式选档才写入既有三项数值**,可回退。
    //  - 首次选档:先拍「写入前原值」快照再写档值;档间切换**不重拍**(快照始终是采纳前);
    //  - 选回空串:有快照按快照恢复(**写入前原值,不是默认值**),无快照则不动;
    //  - 未知取值 400;越界仍由 secret.rs 的既有钳制兜底(不新增校验口径)。
    // 位置:本域在 generation 之后——同一请求里两者同时出现时以「选档」为准
    // (选档是更明确的用户意图);前端同一分区不会同时发两者。
    if let Some(v) = &body.literary_recommend_preset {
        let t = v.trim().to_string();
        if !is_valid_literary_recommend_preset(&t) {
            return Err(PatchError::new(
                "literary_recommend_preset",
                "literary_recommend_preset 仅支持 medium / long(空 = 不改变)",
            ));
        }
        if t.is_empty() {
            if let Some(snap) = s.literary_recommend_snapshot.take() {
                s.compaction_mode = snap.compaction_mode;
                s.compaction_threshold = snap.compaction_threshold;
                s.compaction_keep_recent = snap.compaction_keep_recent;
            }
        } else if let Some(values) = literary_recommend_values(&t) {
            if s.literary_recommend_snapshot.is_none() {
                s.literary_recommend_snapshot = Some(LiteraryRecommendSnapshot {
                    compaction_mode: s.compaction_mode.clone(),
                    compaction_threshold: s.compaction_threshold,
                    compaction_keep_recent: s.compaction_keep_recent,
                });
            }
            s.compaction_mode = values.compaction_mode.to_string();
            s.compaction_threshold = values.compaction_threshold;
            s.compaction_keep_recent = values.compaction_keep_recent;
        }
        s.literary_recommend_preset = t;
    }
    // LIT-4:开关翻转**运行期即时生效**——按「逐字等于内置默认」的同一判据重物化
    // 反思提示词(与 load 期同源;自定义文本与空串都不动)。放在文学字段全部应用之后,
    // 保证用的是本次请求后的开关值。
    s.rehydrate_default_reflect_prompt();
    // WB-E 批 2(遗留 L32 配套):角色扮演默认词同款重物化——逐字等于旧内置默认
    // (或其文学版)者升级为新版(常驻说明头已移到引擎,旧串不升级会成两处说明头)。
    // 与 load 期同源;放在文学开关应用之后,保证开关翻转运行期即时生效、不等重启。
    s.rehydrate_default_roleplay_prompt();
    Ok(())
}
