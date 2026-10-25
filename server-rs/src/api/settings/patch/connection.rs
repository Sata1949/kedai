//! 连接与向量化配置(全局共享层,不随 roleplay/task 模式隔离)。
use crate::connectors::openai_compatible::{normalize_api_style, API_STYLE_CHAT};
use crate::services::settings_service::{
    normalize_base_url, ConnectionProfile, RuntimeSettings, CONNECTOR_TYPE_MOCK,
    CONNECTOR_TYPE_OPENAI, MAX_CONNECTIONS,
};

use super::super::UpdateSettingsBody;
use super::PatchError;

pub(super) fn apply(
    s: &mut RuntimeSettings,
    body: &UpdateSettingsBody,
) -> Result<Vec<String>, PatchError> {
    // 显式清空密钥的连接 id:传给 save 绕开「磁盘密文保真」兜底 ——
    // 否则内存空 + 磁盘密文会被原样回写(密钥复活)。
    let mut cleared_key_ids: Vec<String> = Vec::new();

    // 连接信息:全局共享一份。写入落到**默认连接**上(旧客户端与设置页「API 连接」区
    // 走的都是这条路径 → 行为不变),由末尾的 normalize_connections 投影回扁平字段。
    // 三个字段都没出现时不动连接数组:避免无关 patch(如只改温度)也去建连接。
    if body.openai_base_url.is_some() || body.openai_api_key.is_some() || body.model.is_some() {
        let p = s.ensure_active_connection_mut();
        if let Some(v) = &body.openai_base_url {
            // 自动补全格式:补协议、补 /v1(缺 /v1 会导致 /models 请求 404)
            let t = normalize_base_url(v);
            if !t.is_empty() {
                p.base_url = t;
            }
        }
        if let Some(v) = &body.openai_api_key {
            let t = v.trim().to_string();
            if !t.is_empty() {
                p.api_key = t;
            }
        }
        if let Some(v) = &body.model {
            let t = v.trim().to_string();
            if !t.is_empty() {
                p.model = t;
            }
        }
        // 沿用既有「填了就生效」口径:给显式 mock 的连接把地址与密钥都填上后,自动改回真实连接器
        // 类型(否则用户在「API 连接」区填的配置永远不会生效 —— 旧实现在此处的同类修复)。
        // 判据与 resolve_connector_target 一致:两者都填齐才算「配置完成」。
        if p.connector_type == CONNECTOR_TYPE_MOCK
            && !p.base_url.is_empty()
            && !p.api_key.is_empty()
        {
            p.connector_type = CONNECTOR_TYPE_OPENAI.to_string();
        }
    }
    // 多套连接:全量数组语义(数组里没有的 id = 删除,密钥随之丢弃)
    if let Some(list) = &body.connections {
        if list.len() > MAX_CONNECTIONS {
            return Err(PatchError::new(
                "connections",
                format!("连接配置不能超过 {MAX_CONNECTIONS} 套"),
            ));
        }
        let mut next = Vec::with_capacity(list.len());
        for item in list {
            // 命中已有条目才谈「沿用」;未命中(含 id 缺省)一律新建,避免误接旧值
            let existing = item
                .id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .and_then(|id| s.connections.iter().find(|p| p.id == id));
            let new_key = item
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(str::to_string);
            // 显式清空密钥(clear_api_key):仅命中已有条目且未同时给出新 Key 时生效
            // (新输入优先);新建连接无密钥可清,忽略标记
            if item.clear_api_key == Some(true) && new_key.is_none() {
                if let Some(p) = existing {
                    cleared_key_ids.push(p.id.clone());
                }
            }
            next.push(ConnectionProfile {
                id: existing
                    .map(|p| p.id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                name: item
                    .name
                    .clone()
                    .unwrap_or_else(|| existing.map(|p| p.name.clone()).unwrap_or_default()),
                connector_type: item.connector_type.clone().unwrap_or_else(|| {
                    existing
                        .map(|p| p.connector_type.clone())
                        .unwrap_or_default()
                }),
                base_url: item
                    .base_url
                    .clone()
                    .unwrap_or_else(|| existing.map(|p| p.base_url.clone()).unwrap_or_default()),
                api_key: if item.clear_api_key == Some(true)
                    && new_key.is_none()
                    && existing.is_some()
                {
                    // 显式清空:不回退已存密钥(清空名单已收集,save 侧同步绕开保真兜底)
                    String::new()
                } else {
                    new_key
                        .or_else(|| existing.map(|p| p.api_key.clone()))
                        .unwrap_or_default()
                },
                model: item
                    .model
                    .clone()
                    .unwrap_or_else(|| existing.map(|p| p.model.clone()).unwrap_or_default()),
                api_style: item
                    .api_style
                    .as_deref()
                    .map(|s| normalize_api_style(s).to_string())
                    .or_else(|| existing.map(|p| p.api_style.clone()))
                    .unwrap_or_else(|| API_STYLE_CHAT.to_string()),
                enabled: item
                    .enabled
                    .unwrap_or_else(|| existing.map(|p| p.enabled).unwrap_or(true)),
                supports_vision: item
                    .supports_vision
                    .unwrap_or_else(|| existing.map(|p| p.supports_vision).unwrap_or(false)),
                supports_structured_output: item.supports_structured_output.unwrap_or_else(|| {
                    existing
                        .map(|p| p.supports_structured_output)
                        .unwrap_or(false)
                }),
                supports_prefix_completion: item.supports_prefix_completion.unwrap_or_else(|| {
                    existing
                        .map(|p| p.supports_prefix_completion)
                        .unwrap_or(false)
                }),
                supports_mid_conversation_system: item
                    .supports_mid_conversation_system
                    .unwrap_or_else(|| {
                        existing
                            .map(|p| p.supports_mid_conversation_system)
                            .unwrap_or(false)
                    }),
                image_auto_split: item
                    .image_auto_split
                    .unwrap_or_else(|| existing.map(|p| p.image_auto_split).unwrap_or(false)),
            });
        }
        s.connections = next;
    }
    if let Some(id) = &body.active_connection_id {
        let t = id.trim();
        s.active_connection_id = if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        };
    }
    // 校验后的统一收口:卫生清理 → id 去重 → 默认连接回退 → 扁平字段投影
    // (与 load / save 走同一条路径,避免三处口径漂移)
    s.normalize_connections();

    // 向量化配置:与连接信息同属全局共享层(不按模式隔离)
    if let Some(v) = body.embedding_enabled {
        s.embedding_enabled = v;
    }
    if let Some(v) = &body.embedding_base_url {
        let t = normalize_base_url(v);
        // 允许清空(关闭向量化时用户可能想抹掉地址)
        s.embedding_base_url = if t.is_empty() { String::new() } else { t };
    }
    if let Some(v) = &body.embedding_api_key {
        let t = v.trim().to_string();
        if !t.is_empty() {
            s.embedding_api_key = t;
        }
    }
    if let Some(v) = &body.embedding_model {
        s.embedding_model = v.trim().to_string();
    }
    if let Some(v) = body.embedding_dim {
        if v == 0 || (16..=8192).contains(&v) {
            s.embedding_dim = v;
        } else {
            return Err(PatchError::new(
                "embedding_dim",
                "embedding_dim 必须为 0(自动)或 16..=8192",
            ));
        }
    }
    Ok(cleared_key_ids)
}
