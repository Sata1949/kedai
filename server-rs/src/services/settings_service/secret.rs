// 密钥迁移(DPAPI):settings.json 加载/保存;API Key 落盘前 secret_store::protect 加密、
// load 时 unprotect 解密,旧版明文配置读取后就地重写为密文(一次性迁移,失败仅告警)。
use crate::config::AppConfig;
use crate::services::secret_store;
use std::path::Path;

use super::connection::DEFAULT_SEARCH_ENDPOINT;
use super::params::{
    default_compaction_keep_recent, default_compaction_mode, default_compaction_snip_bytes,
    default_compaction_threshold, default_memory_inject_limit, default_subagent_max_concurrency,
    default_subagent_max_depth, default_subagent_result_max_chars,
    default_tool_history_budget_tokens, default_tool_history_keep_rounds,
};
use super::RuntimeSettings;

impl RuntimeSettings {
    /// 从 data/settings.json 加载;缺失或损坏则回退环境配置。
    /// API Key 解密为明文(旧版无前缀明文原样读取);解密失败视为未配置,需在设置页重填。
    pub fn load(data_dir: &Path, cfg: &AppConfig) -> Self {
        let path = data_dir.join("settings.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(mut s) = serde_json::from_str::<RuntimeSettings>(&text) {
                // 旧版 settings.json 无 search_endpoint:回退默认搜索端点
                if s.search_endpoint.trim().is_empty() {
                    s.search_endpoint = DEFAULT_SEARCH_ENDPOINT.to_string();
                }
                // 旧版 settings.json 无 mvu_vars_position:回退 system
                if s.mvu_vars_position.trim().is_empty() {
                    s.mvu_vars_position = "system".to_string();
                }
                // 变量独立温度钳制到 0..=2.0;非法值(旧配置/越界)回退 None(沿用内置默认)
                if let Some(t) = s.mvu_temperature {
                    if !(0.0..=2.0).contains(&t) {
                        s.mvu_temperature = None;
                    }
                }
                // 上下文窗口下限对齐:低于 64K(旧默认 8192 等)钳制到 64K,保证与
                // 前端滑块最小位一致;上限 1M 由 API 校验兜底
                if s.max_context_tokens < 65_536 {
                    s.max_context_tokens = 65_536;
                }
                // 压缩模式仅接受 off / manual / auto,旧配置或非法值回退 off
                if !matches!(s.compaction_mode.as_str(), "off" | "manual" | "auto") {
                    s.compaction_mode = default_compaction_mode();
                }
                // 压缩阈值钳制到 0.5..=0.95,旧配置缺省已由 serde default 填 0.8
                if !(0.5..=0.95).contains(&s.compaction_threshold) {
                    s.compaction_threshold = default_compaction_threshold();
                }
                // 压缩保留条数钳制到 >= 2(0/1 会导致压缩后无上下文或永不触发),上限 200
                if !(2..=200).contains(&s.compaction_keep_recent) {
                    s.compaction_keep_recent = default_compaction_keep_recent();
                }
                // snip 阈值钳制到 0..=1MB(0 = 禁用 snip;负数/超大值视为异常回退默认)
                if s.compaction_snip_bytes > 1_048_576 {
                    s.compaction_snip_bytes = default_compaction_snip_bytes();
                }
                // 记忆注入上限钳制到 0..=50(0 = 关闭注入;旧配置缺省由 serde default 填 8)
                if s.memory_inject_limit > 50 {
                    s.memory_inject_limit = default_memory_inject_limit();
                }
                // 落地项 3:子智能体调度参数钳制(深度 1..=4 / 并发 1..=16 / 结果 500..=8000,
                // 越界回退默认;渐进披露开关为 bool 无需钳制)
                if !(1..=4).contains(&s.subagent_max_depth) {
                    s.subagent_max_depth = default_subagent_max_depth();
                }
                if !(1..=16).contains(&s.subagent_max_concurrency) {
                    s.subagent_max_concurrency = default_subagent_max_concurrency();
                }
                if !(500..=8000).contains(&s.subagent_result_max_chars) {
                    s.subagent_result_max_chars = default_subagent_result_max_chars();
                }
                // R3b:工具历史回灌参数钳制(保留轮数 1..=32;预算 0=禁用,否则 1024..=1M,
                // 越界回退默认;旧配置缺省已由 serde default 填默认值)
                if !(1..=32).contains(&s.tool_history_keep_rounds) {
                    s.tool_history_keep_rounds = default_tool_history_keep_rounds();
                }
                if s.tool_history_budget_tokens != 0
                    && !(1024..=1_048_576).contains(&s.tool_history_budget_tokens)
                {
                    s.tool_history_budget_tokens = default_tool_history_budget_tokens();
                }
                // MCP 服务器列表(批次 6.2):settings.json 可手改,启动装配前做一次卫生清理
                // (trim 名称/命令,丢弃缺名或缺命令的不可用条目;与 PUT 校验同规则)
                s.mcp_servers.retain_mut(|srv| {
                    srv.name = srv.name.trim().to_string();
                    srv.command = srv.command.trim().to_string();
                    !srv.name.is_empty() && !srv.command.is_empty()
                });
                let was_plaintext =
                    !s.openai_api_key.is_empty() && !secret_store::is_protected(&s.openai_api_key);
                s.openai_api_key = secret_store::unprotect(&s.openai_api_key);
                // 旧版明文配置:立即重写为密文(一次性迁移,失败仅告警不影响启动)
                if was_plaintext && !s.openai_api_key.is_empty() {
                    if let Err(e) = s.save(data_dir) {
                        eprintln!("[settings] API Key 加密迁移写回失败(下次保存设置时重试):{e}");
                    } else {
                        eprintln!("[settings] 已将 settings.json 中的明文 API Key 迁移为加密存储");
                    }
                }
                return s;
            }
        }
        Self::from_config(cfg)
    }

    /// 持久化到 data/settings.json;API Key 加密后写入,不落明文。
    /// 统一走原子写(utils::fs_atomic:写临时文件 + 原子替换),
    /// 进程中断不会留下半截 JSON;Windows 下 ReplaceFileW 替换,消除了旧实现
    /// 「先删目标再 rename」的非原子窗口。
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        // 仅持久化副本加密,不改动内存中的明文 Key(连接器仍需直接使用)
        let mut persisted = self.clone();
        persisted.openai_api_key = secret_store::protect(&self.openai_api_key)?;
        let text = serde_json::to_string_pretty(&persisted).map_err(|e| e.to_string())?;
        crate::utils::fs_atomic::write_atomic(&data_dir.join("settings.json"), text.as_bytes())
            .map_err(|e| e.to_string())
    }
}
