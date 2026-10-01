// 连接器池(二维批次 5b):按连接配置(ConnectionProfile)产出连接器实例,带缓存。
//
// 为什么需要它:节点级 provider 路由要求「同一任务内不同节点走不同连接」。连接器是
// Clone 的实例枚举(build_connector 是纯工厂),天然支持多实例;需要补的是**解析与缓存**
// ——每次模型调用都重建一个 reqwest Client 是纯浪费,而缓存又必须能在用户改完设置后
// 立即失效。
//
// 失效口径:**指纹比对**而不是「在 PUT 路径上挂钩清缓存」。缓存的键是 profile id,值带着
// 构建时的指纹(类型/地址/密钥/模型四段);解析时指纹不一致即重建。这样设置保存路径
// (api/settings.rs)不需要知道本池子的存在,也就不会出现「漏挂一处失效钩子 → 改完设置
// 不生效」的经典缺陷。
//
// 单一出处:本池子只在 AppState 装配一次,经 EngineCore 注入 AgentEngine;TaskService
// 复用引擎上的同一实例(计划改动点 1 明令「不要在 TaskService 里另建一份连接器缓存」,
// 否则数据源分裂)。
//
// 与默认连接的关系:请求未指定 connection_id 时**不走本池子**——默认连接仍由
// `engine.connector` 的读锁快照提供(行为与 5b 之前逐字节一致);本池子只服务
// 「节点/请求显式指定了连接」的那条路径。
use std::collections::HashMap;
use std::sync::Mutex;

use super::connection::ConnectionProfile;

use crate::connectors::{build_connector, Connector};

/// 缓存条目上限:连接套数本就有上限(MAX_CONNECTIONS),这里再多给一点余量。
/// 超限整体清空 —— 手改 JSON 反复换指纹时缓存不会无界增长(缓存只是加速,清空无损)。
const MAX_CACHED: usize = 32;

struct Cached {
    /// 构建时的 profile 指纹;不一致即重建(见文件头「失效口径」)
    fingerprint: String,
    connector: Connector,
}

#[derive(Default)]
pub struct ConnectorPool {
    cache: Mutex<HashMap<String, Cached>>,
}

impl ConnectorPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// 按连接配置取连接器(带缓存)。
    ///
    /// `connector_type` 只认 `mock` 与 `openai-compatible`(由 `ConnectionProfile::sanitize`
    /// 在 load/PUT 时规范化);其余走 `build_connector` 的既有回退(mock)。
    pub fn connector_for(&self, profile: &ConnectionProfile) -> Connector {
        let fingerprint = fingerprint_of(profile);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = cache.get(&profile.id) {
            if hit.fingerprint == fingerprint {
                return hit.connector.clone();
            }
        }
        let connector = build_connector(
            &profile.connector_type,
            &profile.base_url,
            &profile.api_key,
            &profile.model,
            &profile.api_style,
        );
        if cache.len() >= MAX_CACHED {
            cache.clear();
        }
        cache.insert(
            profile.id.clone(),
            Cached {
                fingerprint,
                connector: connector.clone(),
            },
        );
        connector
    }
}

/// 连接配置指纹:只含连接器构建真正消费的五个字段(类型/地址/密钥/模型/接口方言)。
/// 不含 `name`/`enabled`/`id` —— 改名与启停不该导致重建(id 本就是缓存键)。
fn fingerprint_of(p: &ConnectionProfile) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}",
        p.connector_type, p.base_url, p.api_key, p.model, p.api_style
    )
}

/// 连接配置的展示名(错误文案与前端选项共用口径):空名回退 id。
pub fn connection_label(p: &ConnectionProfile) -> String {
    if p.name.trim().is_empty() {
        p.id.clone()
    } else {
        p.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::settings_service::CONNECTOR_TYPE_MOCK;

    fn profile(id: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: id.to_string(),
            name: format!("连接 {id}"),
            connector_type: super::super::CONNECTOR_TYPE_OPENAI.to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            model: "model-a".to_string(),
            api_style: crate::connectors::openai_compatible::API_STYLE_CHAT.to_string(),
            enabled: true,
        }
    }

    /// 缓存命中:同一 profile 连续解析拿到同一份连接(不重建 Client)
    #[test]
    fn same_profile_reuses_cached_connector() {
        let pool = ConnectorPool::new();
        let p = profile("a");
        pool.connector_for(&p);
        assert_eq!(pool.cache.lock().unwrap().len(), 1);
        pool.connector_for(&p);
        assert_eq!(pool.cache.lock().unwrap().len(), 1);
    }

    /// 指纹变即重建:改地址/密钥/模型/类型中的任意一个,解析出的连接器立即是新值
    /// (这是「改完设置不生效」的判别性断言)
    #[test]
    fn fingerprint_change_rebuilds_connector() {
        let pool = ConnectorPool::new();
        let mut p = profile("a");
        assert_eq!(pool.connector_for(&p).model(), "model-a");
        p.model = "model-b".to_string();
        assert_eq!(pool.connector_for(&p).model(), "model-b");
        // 类型改成 mock:模型名回落到连接器固定值,证明走的是重建而不是缓存
        p.connector_type = CONNECTOR_TYPE_MOCK.to_string();
        assert_eq!(pool.connector_for(&p).type_name(), "mock");
    }

    /// 接口方言进指纹:方言变化必须重建(只改「接口格式」而地址/密钥/模型逐字不变时,
    /// 若不进指纹,连接器会继续用旧方言解析到下次重启)
    #[test]
    fn api_style_change_rebuilds_connector() {
        use crate::connectors::openai_compatible::API_STYLE_ANTHROPIC;
        let pool = ConnectorPool::new();
        let mut p = profile("a");
        pool.connector_for(&p);
        let fingerprint_before = pool.cache.lock().unwrap()["a"].fingerprint.clone();
        p.api_style = API_STYLE_ANTHROPIC.to_string();
        pool.connector_for(&p);
        assert_ne!(
            pool.cache.lock().unwrap()["a"].fingerprint,
            fingerprint_before,
            "方言变更应使指纹变化(触发重建)"
        );
    }

    /// 改名与启停不触发重建(指纹只含连接器构建消费的四字段)
    #[test]
    fn rename_and_toggle_do_not_rebuild() {
        let pool = ConnectorPool::new();
        let mut p = profile("a");
        pool.connector_for(&p);
        let fingerprint_before = pool.cache.lock().unwrap()["a"].fingerprint.clone();
        p.name = "改个名字".to_string();
        p.enabled = false;
        pool.connector_for(&p);
        assert_eq!(
            pool.cache.lock().unwrap()["a"].fingerprint,
            fingerprint_before
        );
    }

    /// 多套连接各自独立:两个 profile 得到两个连接器,模型名互不串门
    #[test]
    fn distinct_profiles_get_distinct_connectors() {
        let pool = ConnectorPool::new();
        let a = profile("a");
        let mut b = profile("b");
        b.model = "model-b".to_string();
        assert_eq!(pool.connector_for(&a).model(), "model-a");
        assert_eq!(pool.connector_for(&b).model(), "model-b");
        assert_eq!(pool.cache.lock().unwrap().len(), 2);
    }

    /// mock 连接的模型名由连接器固定(mock-demo),连接里写的 model 不生效
    /// —— 演示模式的既有语义,前端选择器需按此提示
    #[test]
    fn mock_profile_model_is_fixed_by_connector() {
        let pool = ConnectorPool::new();
        let mut p = profile("m");
        p.connector_type = CONNECTOR_TYPE_MOCK.to_string();
        p.model = "用户随便写的".to_string();
        let c = pool.connector_for(&p);
        assert_eq!(c.type_name(), "mock");
        assert_eq!(c.model(), "mock-demo");
    }

    /// 条目超上限整体清空(缓存只是加速,清空不放行任何错误行为)
    #[test]
    fn cache_clears_when_over_limit() {
        let pool = ConnectorPool::new();
        for i in 0..MAX_CACHED {
            let mut p = profile(&format!("p{i}"));
            p.model = format!("m{i}");
            pool.connector_for(&p);
        }
        assert_eq!(pool.cache.lock().unwrap().len(), MAX_CACHED);
        let mut extra = profile("extra");
        extra.model = "mx".to_string();
        pool.connector_for(&extra);
        let len = pool.cache.lock().unwrap().len();
        assert!(len < MAX_CACHED, "超限应清空旧条目,实际 {len}");
    }

    #[test]
    fn connection_label_falls_back_to_id() {
        let mut p = profile("a");
        assert_eq!(connection_label(&p), "连接 a");
        p.name = "   ".to_string();
        assert_eq!(connection_label(&p), "a");
    }
}
