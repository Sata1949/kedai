// 密钥迁移(DPAPI):settings.json 加载/保存;API Key 落盘前 secret_store::protect 加密、
// load 时 unprotect 解密,旧版明文配置读取后就地重写为密文(一次性迁移,失败仅告警)。
use crate::config::AppConfig;
use crate::services::secret_store;
use std::collections::HashMap;
use std::path::Path;

use super::connection::DEFAULT_SEARCH_ENDPOINT;
use super::params::{
    default_compaction_keep_recent, default_compaction_mode, default_compaction_snip_bytes,
    default_compaction_threshold, default_loop_guard_semantic_max_distinct,
    default_loop_guard_semantic_min_calls, default_loop_guard_semantic_window,
    default_memory_inject_char_budget, default_memory_inject_limit, default_memory_max_entries,
    default_session_budget_action, default_session_token_budget, default_subagent_max_concurrency,
    default_subagent_max_depth, default_subagent_result_max_chars, default_task_idle_timeout_secs,
    default_task_step_budget_secs, default_task_tool_policy, default_task_total_budget_secs,
    default_tool_authorization_timeout_secs, default_tool_history_budget_tokens,
    default_tool_history_keep_rounds, migrate_authorization_mode, task_idle_floor_secs,
};
use super::{
    is_valid_literary_recommend_preset, is_valid_literary_style_preset, RoleplayPromptConfig,
    RuntimeSettings,
};

impl RuntimeSettings {
    /// 从 data/settings.json 加载;缺失或损坏则回退环境配置。
    /// API Key 解密为明文(旧版无前缀明文原样读取);解密失败视为未配置,需在设置页重填。
    pub fn load(data_dir: &Path, cfg: &AppConfig) -> Self {
        let path = data_dir.join("settings.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(mut s) = serde_json::from_str::<RuntimeSettings>(&text) {
                // 授权模式迁移(批次授权改造):旧配置只有 bypass_mode 布尔值,没有
                // authorization_mode 键。按旧值映射:true → Bypass(旧「除黑名单外全放行」),
                // false → Strict(旧「非安全工具都需授权」最贴近且更安全)。
                // 判据用原始 JSON 是否含该键,避免覆盖新装默认(loose)与用户已写值。
                if !text.contains("\"authorization_mode\"") {
                    s.authorization_mode = migrate_authorization_mode(s.bypass_mode);
                }
                // 旧「放行模式黑名单」默认值是四个不存在的工具名,迁移为空的
                // 「始终需授权」清单;仅当内容全是旧假名时清空,保留用户自定义项。
                let legacy_fake = [
                    "delete_file",
                    "format_disk",
                    "modify_system",
                    "registry_write",
                ];
                if !s.bypass_blacklist.is_empty()
                    && s.bypass_blacklist
                        .iter()
                        .all(|n| legacy_fake.contains(&n.as_str()))
                {
                    s.bypass_blacklist.clear();
                }
                // 授权等待超时钳制 30..=1800 秒(越界回退默认,防 0 秒立即超时或长挂)
                if !(30..=1800).contains(&s.tool_authorization_timeout_secs) {
                    s.tool_authorization_timeout_secs = default_tool_authorization_timeout_secs();
                }
                // 任务工具策略仅接受三值,非法回退默认(与 API 校验同规则)
                if !matches!(
                    s.task_tool_policy.as_str(),
                    "all" | "deny_dangerous" | "allowlist"
                ) {
                    s.task_tool_policy = default_task_tool_policy();
                }
                // LIT-6/LIT-7:两个选择型字段的存量归一化——未知取值回退空串(对齐上一段
                // 「非法回退默认」的先例)。API 层对**新写入**是 400,此处只兜存量配置与手改
                // 的 settings.json,避免把未知档静默注入提示词。
                if !is_valid_literary_style_preset(&s.literary_style_preset) {
                    s.literary_style_preset.clear();
                }
                if !is_valid_literary_recommend_preset(&s.literary_recommend_preset) {
                    s.literary_recommend_preset.clear();
                }
                // 旧版 settings.json 无 search_endpoint:回退默认搜索端点
                if s.search_endpoint.trim().is_empty() {
                    s.search_endpoint = DEFAULT_SEARCH_ENDPOINT.to_string();
                }
                // 旧版 settings.json 无 mvu_vars_position:回退 system
                if s.mvu_vars_position.trim().is_empty() {
                    s.mvu_vars_position = "system".to_string();
                }
                // 角色扮演 Agent 系统提示词为空时物化内置默认(对齐 search_endpoint 回退模式):
                // 空串语义 = 使用内置默认模板(设置页文案与 docs/契约-协议与配置.md 同口径),
                // 故此前安装(settings.json 已存在且该字段为空)也回退到内置默认,与首装/Android 端一致;
                // 用户已保存的非空文本优先,不会被本回填覆盖。
                // LIT-2:经唯一判据入口——文学能力包开关开时物化文学增强版(开关值已由上面
                // 反序列化读出,此处直接用)。
                if s.agent_system_prompt.0.trim().is_empty() {
                    s.agent_system_prompt = RoleplayPromptConfig(s.roleplay_default_prompt());
                }
                // LIT-4:反思提示词的「未自定义」判定**不是空串**——该字段的空串是「显式关闭
                // 反思、回退机械规则」的有效值(见 `api/settings.rs` 同名注释),故判据是
                // **逐字等于内置默认文本**:命中任一版本 → 按当前包开关重物化(关 → 逐字不变;
                // 开 → 追加文学维度检查项)。自定义文本与空串都不动。设置写入期同款调用在
                // `api/settings.rs`(开关翻转运行期即时生效,不等重启)。
                s.rehydrate_default_reflect_prompt();

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
                // 记忆槽字符预算钳制到 0..=20000(0 = 不限制;升级工作流 B2)
                if s.memory_inject_char_budget > 20_000 {
                    s.memory_inject_char_budget = default_memory_inject_char_budget();
                }
                // 每角色记忆容量上限钳制到 0..=10000(0 = 不淘汰;升级工作流 B3)
                if s.memory_max_entries > 10_000 {
                    s.memory_max_entries = default_memory_max_entries();
                }
                // embedding 向量维度钳制:0 = 未探测(首次调用回填),否则 16..=8192;
                // 越界视为异常配置回退 0,由下次测试连接重新探测
                if s.embedding_dim != 0 && !(16..=8192).contains(&s.embedding_dim) {
                    s.embedding_dim = 0;
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
                // HB-1:单次生成 token 预算与超限动作(手改 settings.json 的边界:
                // 预算 0 = 关闭,否则 1024..=1e9;动作只认 warn/stop,越界回默认)
                if s.session_token_budget != 0
                    && !(1024..=1_000_000_000).contains(&s.session_token_budget)
                {
                    s.session_token_budget = default_session_token_budget();
                }
                if !matches!(s.session_budget_action.as_str(), "warn" | "stop") {
                    s.session_budget_action = default_session_budget_action();
                }
                // HB-7:mvu_model 卫生清理(settings.json 可手改:trim、空白/超长视为未配置;
                // 与 PUT 校验同规则,避免「看似配置了却打不通」)
                if let Some(m) = s.mvu_model.as_mut() {
                    let trimmed = m.trim().to_string();
                    if trimmed.is_empty() || trimmed.chars().count() > 200 {
                        s.mvu_model = None;
                    } else {
                        *m = trimmed;
                    }
                }
                // HB-2:语义熔断参数钳制(窗口/下限 4..=64,去重上限 1..=8;越界回默认)
                if !(4..=64).contains(&s.loop_guard_semantic_window) {
                    s.loop_guard_semantic_window = default_loop_guard_semantic_window();
                }
                if s.loop_guard_semantic_min_calls != 0
                    && !(4..=64).contains(&s.loop_guard_semantic_min_calls)
                {
                    s.loop_guard_semantic_min_calls = default_loop_guard_semantic_min_calls();
                }
                if !(1..=8).contains(&s.loop_guard_semantic_max_distinct) {
                    s.loop_guard_semantic_max_distinct = default_loop_guard_semantic_max_distinct();
                }
                // 提交 3(D3/D7):任务侧两道闸的参数钳制。step_budget 刻意不设下限
                // (1 秒合法:低于单次调用看门狗的值 = 「第一轮结束就收尾」的合法语义,
                // 测试也靠它触发预算路径);idle 下限走 `task_idle_floor_secs()` 单一出处
                // (批次 2 之前这里与 PUT 校验各写一份 601,抬 bash 上限就会两处漂移)。
                if s.task_step_budget_secs != 0 && !(1..=86_400).contains(&s.task_step_budget_secs)
                {
                    s.task_step_budget_secs = default_task_step_budget_secs();
                }
                // 任务级总预算(PRODCAP-2):同款区间钳制(0 = 关,否则 1..=86400)
                if s.task_total_budget_secs != 0
                    && !(1..=86_400).contains(&s.task_total_budget_secs)
                {
                    s.task_total_budget_secs = default_task_total_budget_secs();
                }
                if s.task_idle_timeout_secs != 0
                    && !(task_idle_floor_secs()..=86_400).contains(&s.task_idle_timeout_secs)
                {
                    s.task_idle_timeout_secs = default_task_idle_timeout_secs();
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
                // embedding Key 同策略:解密到内存;旧版明文在下方统一触发一次写回迁移
                let embedding_was_plaintext = !s.embedding_api_key.is_empty()
                    && !secret_store::is_protected(&s.embedding_api_key);
                s.embedding_api_key = secret_store::unprotect(&s.embedding_api_key);
                // 多套连接(批次 4):逐条解密 —— 连接里的 Key 与顶层同策略(落盘密文 / 内存明文)。
                // 必须早于下面的播种:播种的连接要从**已解密**的扁平字段取值。
                let mut connections_was_plaintext = false;
                for p in s.connections.iter_mut() {
                    if !p.api_key.is_empty() && !secret_store::is_protected(&p.api_key) {
                        connections_was_plaintext = true;
                    }
                    p.api_key = secret_store::unprotect(&p.api_key);
                }
                // 多套连接(批次 4):幂等播种 → 逐条卫生清理/id 去重 → 默认连接回退 → 扁平字段投影。
                // 必须早于下面的「明文 Key 迁移写回」:那次 save 会把播种后的 connections 一并落盘,
                // 否则迁移写回漏掉连接数组,用户要等到下一次保存才能看到默认连接。
                s.seed_connections_from_flat();
                s.normalize_connections();
                // 旧版明文配置:立即重写为密文(一次性迁移,失败仅告警不影响启动)
                if (was_plaintext && !s.openai_api_key.is_empty())
                    || (embedding_was_plaintext && !s.embedding_api_key.is_empty())
                    || connections_was_plaintext
                {
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
    ///
    /// 防「密钥静默清空」(2026-09-13 批次 2):`unprotect` 解密失败时返回空串
    /// (视为未配置,避免把密文当 Key 发上游),若直接保存就会用 `protect("")` 的
    /// 空串覆盖磁盘上的密文 —— 用户密钥从此永久丢失且无提示。故保存前读一次磁盘:
    /// 内存为空而磁盘仍是非空 `enc:v1:` 密文时,原样回写该密文(保留原值)。
    /// PUT /api/settings 对空值直接忽略(`api/settings.rs` 连接信息路径),
    /// 要把已配置密钥改为空须走连接数组路径的 `clear_api_key` 显式标记,
    /// 由 [`Self::save_with_cleared_connection_keys`] 携带名单绕开本保真兜底。
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        self.save_with_cleared_connection_keys(data_dir, &[])
    }

    /// 带显式清空名单的保存(2026-10-03 API 设置补全):`cleared_key_ids` 中的
    /// profile id 绕开「磁盘密文保真」兜底,密钥落盘为空。仅 PUT /api/settings 的
    /// 连接数组路径(`clear_api_key: true`)会传入;其余保存调用一律走 [`Self::save`]。
    pub fn save_with_cleared_connection_keys(
        &self,
        data_dir: &Path,
        cleared_key_ids: &[String],
    ) -> Result<(), String> {
        // 仅持久化副本加密,不改动内存中的明文 Key(连接器仍需直接使用)
        let mut persisted = self.clone();
        // 落盘前重算投影:扁平三字段始终是「默认连接的派生视图」,保证磁盘自洽
        // (内存态若被外部改过,以 connections 为准)
        persisted.normalize_connections();
        let on_disk = read_preserved_ciphertexts(data_dir);
        // 逐条加密:磁盘密文按 profile **id** 匹配,绝不按下标 —— 删除中间一条或调整顺序后
        // 下标会整体错位,把 A 的密文写到 B 头上(密钥张冠李戴且原值不可恢复)。
        // 活跃连接额外允许回退顶层旧密文:旧单份配置首次落盘时,磁盘上还没有该 id 的条目。
        let active_id = persisted.active_connection_id.clone();
        for p in persisted.connections.iter_mut() {
            let is_active = active_id.as_deref() == Some(p.id.as_str());
            let explicitly_cleared = cleared_key_ids.iter().any(|id| id == &p.id);
            let preserved = on_disk
                .by_profile_id
                .get(&p.id)
                .map(|s| s.as_str())
                .or(if is_active {
                    on_disk.openai.as_deref()
                } else {
                    None
                });
            p.api_key = encrypt_or_preserve(&p.api_key, preserved, explicitly_cleared)?;
        }
        // 顶层兼容字段 = 默认连接的密文(直接取上一步的结果,不二次加密);
        // 没有可用连接(全停用/删空)时写空 —— 那是用户的显式操作,不属于「解密失败要保真」的场景。
        let active_key = persisted
            .active_connection()
            .map(|p| p.api_key.clone())
            .unwrap_or_default();
        persisted.openai_api_key = active_key;
        persisted.embedding_api_key =
            encrypt_or_preserve(&self.embedding_api_key, on_disk.embedding.as_deref(), false)?;
        let text = serde_json::to_string_pretty(&persisted).map_err(|e| e.to_string())?;
        crate::utils::fs_atomic::write_atomic(&data_dir.join("settings.json"), text.as_bytes())
            .map_err(|e| e.to_string())
    }
}

/// 磁盘上仍是非空密文的敏感字段(供「内存为空」时回写保真)
#[derive(Default)]
struct OnDiskCiphertexts {
    /// 顶层兼容字段 openai_api_key(旧单份配置;也是活跃连接首次落盘的兜底来源)
    openai: Option<String>,
    embedding: Option<String>,
    /// connections[].api_key,键为 profile id:密文必须「跟 id 走」而不是「跟下标走」
    by_profile_id: HashMap<String, String>,
}

/// 读取 settings.json 中仍为 `enc:v1:` 非空密文的 Key 字段。
/// 文件缺失/损坏/字段缺失一律返回 None(按「无旧值」处理,写入新值)。
fn read_preserved_ciphertexts(data_dir: &Path) -> OnDiskCiphertexts {
    let Ok(text) = std::fs::read_to_string(data_dir.join("settings.json")) else {
        return OnDiskCiphertexts::default();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return OnDiskCiphertexts::default();
    };
    let ciphertext_of = |v: Option<&serde_json::Value>| -> Option<String> {
        let s = v?.as_str()?.trim();
        if s.is_empty() || !secret_store::is_protected(s) {
            return None;
        }
        Some(s.to_string())
    };
    let mut out = OnDiskCiphertexts {
        openai: ciphertext_of(json.get("openai_api_key")),
        embedding: ciphertext_of(json.get("embedding_api_key")),
        by_profile_id: HashMap::new(),
    };
    if let Some(list) = json.get("connections").and_then(|v| v.as_array()) {
        for item in list {
            // 空 id 或无密文的条目直接跳过(与该 id 对不上的内存态自然按「无旧值」处理)
            let Some(id) = item.get("id").and_then(|v| v.as_str()).map(str::trim) else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            if let Some(ct) = ciphertext_of(item.get("api_key")) {
                out.by_profile_id.insert(id.to_string(), ct);
            }
        }
    }
    out
}

/// 加密待落盘值;内存为空但磁盘仍有密文时保留磁盘原密文(防静默清空)。
/// `explicitly_cleared`(2026-10-03 API 设置补全):该 profile 被 `clear_api_key`
/// 显式标记清空时为 true —— 跳过保真兜底直接落空,否则用户永远无法清除已配密钥。
fn encrypt_or_preserve(
    in_memory: &str,
    on_disk_ciphertext: Option<&str>,
    explicitly_cleared: bool,
) -> Result<String, String> {
    if in_memory.is_empty() {
        if !explicitly_cleared {
            if let Some(ct) = on_disk_ciphertext {
                eprintln!(
                    "[settings] API Key 在内存中为空但磁盘仍为密文(此前解密失败?),保留原密文不覆盖;请在设置页重填以恢复"
                );
                return Ok(ct.to_string());
            }
        }
        return secret_store::protect(""); // 空串保持空串
    }
    secret_store::protect(in_memory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_config;
    use crate::services::settings_service::ConnectionProfile;
    use crate::utils::test_support::TempDataDir;

    /// 隔离临时数据目录(uuid 唯一 + 作用域结束自动清理)
    fn temp_dir(tag: &str) -> TempDataDir {
        TempDataDir::new(&format!("secret-preserve-{tag}"))
    }

    fn write_settings(dir: &Path, json: &str) {
        std::fs::write(dir.join("settings.json"), json.as_bytes()).unwrap();
    }

    /// 核心契约:内存 Key 为空而磁盘是密文 → 保存必须保留原密文,绝不写成空
    /// (2026-09-13 批次 2:修复「解密失败后一次保存即静默清空密钥」)
    #[test]
    fn save_preserves_ciphertext_when_memory_key_is_empty() {
        let dir = temp_dir("keep");
        write_settings(
            &dir,
            r#"{"openai_api_key":"enc:v1:KEPT-CIPHERTEXT","embedding_api_key":"enc:v1:KEPT-EMB"}"#,
        );
        let mut s = RuntimeSettings::from_config(&test_config());
        s.openai_api_key = String::new(); // 模拟解密失败后的内存态
        s.embedding_api_key = String::new();
        s.save(&dir).expect("保存应成功");

        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            text.contains("enc:v1:KEPT-CIPHERTEXT"),
            "openai_api_key 必须保留原密文:{text}"
        );
        assert!(
            text.contains("enc:v1:KEPT-EMB"),
            "embedding_api_key 必须保留原密文:{text}"
        );
    }

    /// 未配置(磁盘无密文)时保存仍写空串;新的非空 Key 正常加密覆盖
    #[cfg(windows)]
    #[test]
    fn save_writes_empty_or_new_key_normally() {
        let dir = temp_dir("normal");
        let mut s = RuntimeSettings::from_config(&test_config());
        // 本批次起 connections 才是真源,扁平字段是它的派生视图 → 写 Key 要写默认连接
        s.connections[0].api_key = String::new();
        s.save(&dir).unwrap();
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            text.contains("\"openai_api_key\": \"\""),
            "未配置应写空串:{text}"
        );

        s.connections[0].api_key = "sk-new-key-1234".to_string();
        s.save(&dir).unwrap();
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            text.contains("enc:v1:") && !text.contains("sk-new-key-1234"),
            "新 Key 应加密落盘:{text}"
        );
    }

    /// 旧版明文 Key 的迁移路径不受影响:load 解密(直读)后重新加密,
    /// 播种的默认连接同样拿到明文(否则迁移写回会把连接里的 Key 写成空)
    #[cfg(windows)]
    #[test]
    fn legacy_plaintext_key_still_migrates_to_ciphertext() {
        let dir = temp_dir("legacy");
        // 完整旧版文件(无 connections 键)+ 明文 Key:走 load 的迁移写回
        let mut base = RuntimeSettings::from_config(&test_config());
        base.openai_base_url = "https://legacy.example/v1".to_string();
        base.openai_api_key = "sk-legacy-plain".to_string();
        let mut json = serde_json::to_value(&base).unwrap();
        json.as_object_mut().unwrap().remove("connections");
        json.as_object_mut().unwrap().remove("active_connection_id");
        write_settings(&dir, &serde_json::to_string_pretty(&json).unwrap());

        let loaded = RuntimeSettings::load(&dir, &test_config());
        assert_eq!(
            loaded.openai_api_key, "sk-legacy-plain",
            "旧版明文应能正常读取"
        );
        assert_eq!(loaded.connections.len(), 1, "旧文件应播种一条默认连接");
        assert_eq!(
            loaded.connections[0].api_key, "sk-legacy-plain",
            "播种的连接必须继承明文 Key"
        );
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            text.contains("enc:v1:") && !text.contains("sk-legacy-plain"),
            "明文应迁移为密文:{text}"
        );
        // 迁移后再次 load 仍应得到同一明文(连接与顶层字段都还原)
        let again = RuntimeSettings::load(&dir, &test_config());
        assert_eq!(again.openai_api_key, "sk-legacy-plain");
        assert_eq!(again.connections[0].api_key, "sk-legacy-plain");
    }

    // ---------- 多套连接(2026-09-22 批次 4) ----------

    fn profile(id: &str, key: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: id.to_string(),
            name: format!("连接-{id}"),
            connector_type: "openai-compatible".to_string(),
            base_url: format!("https://{id}.example/v1"),
            api_key: key.to_string(),
            model: "m".to_string(),
            api_style: crate::connectors::openai_compatible::API_STYLE_CHAT.to_string(),
            enabled: true,
            supports_vision: false,
            supports_structured_output: false,
            supports_prefix_completion: false,
            supports_mid_conversation_system: false,
            image_auto_split: false,
        }
    }

    fn read_json(dir: &Path) -> serde_json::Value {
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    /// 播种(用例 1):判据只有「数组为空」,内容来自扁平字段;重复播种与已有连接都不动它
    #[test]
    fn connections_seeded_from_flat_config_and_idempotent() {
        let mut s = RuntimeSettings::from_config(&test_config());
        s.connections.clear();
        s.active_connection_id = None;
        s.openai_base_url = "https://flat.example/v1".to_string();
        s.model = "flat-model".to_string();

        s.seed_connections_from_flat();
        assert_eq!(s.connections.len(), 1);
        assert_eq!(s.connections[0].id, "default");
        assert_eq!(s.connections[0].name, "默认连接");
        assert_eq!(s.connections[0].base_url, "https://flat.example/v1");
        assert_eq!(s.connections[0].model, "flat-model");
        assert_eq!(s.active_connection_id.as_deref(), Some("default"));

        // 幂等:再播种一次不新增、不换 id
        let before = serde_json::to_string(&s.connections).unwrap();
        s.seed_connections_from_flat();
        assert_eq!(serde_json::to_string(&s.connections).unwrap(), before);

        // 用户已有连接时绝不覆盖
        s.connections[0].name = "我的连接".to_string();
        s.seed_connections_from_flat();
        assert_eq!(s.connections.len(), 1);
        assert_eq!(s.connections[0].name, "我的连接");
    }

    /// 旧文件兼容(用例 2):无 connections 键必须能正常反序列化 ——
    /// 漏写 `#[serde(default)]` 会让整文件解析失败、load 回退环境配置(用户设置静默丢失)
    #[test]
    fn old_settings_json_without_connections_key_still_loads() {
        let dir = temp_dir("legacy-no-conn");
        let mut base = RuntimeSettings::from_config(&test_config());
        base.openai_base_url = "https://kept.example/v1".to_string();
        base.model = "kept-model".to_string();
        let mut json = serde_json::to_value(&base).unwrap();
        json.as_object_mut().unwrap().remove("connections");
        json.as_object_mut().unwrap().remove("active_connection_id");
        write_settings(&dir, &serde_json::to_string_pretty(&json).unwrap());

        let loaded = RuntimeSettings::load(&dir, &test_config());
        // 判别性:整文件回退 env 时这里会是 test_config 的 example.com / test-model
        assert_eq!(loaded.openai_base_url, "https://kept.example/v1");
        assert_eq!(loaded.model, "kept-model");
        assert_eq!(loaded.connections.len(), 1, "缺键 → 空数组 → 播种");
        assert_eq!(loaded.connections[0].base_url, "https://kept.example/v1");
    }

    /// 能力位兼容(视觉能力包 D1):旧 JSON 的连接条目缺 5 个能力位新键 → 一律读作 false。
    /// 同上一用例的锁死纪律:漏写 serde(default) 会让整文件反序列化失败、设置静默丢失。
    #[test]
    fn old_connection_entries_without_capability_keys_default_false() {
        let dir = temp_dir("legacy-no-caps");
        let mut base = RuntimeSettings::from_config(&test_config());
        base.search_endpoint = "https://kept-caps.example/search".to_string();
        let mut json = serde_json::to_value(&base).unwrap();
        let conn = json["connections"][0]
            .as_object_mut()
            .expect("from_config 应播种一条默认连接");
        for key in [
            "supports_vision",
            "supports_structured_output",
            "supports_prefix_completion",
            "supports_mid_conversation_system",
            "image_auto_split",
        ] {
            conn.remove(key);
        }
        write_settings(&dir, &serde_json::to_string_pretty(&json).unwrap());

        let loaded = RuntimeSettings::load(&dir, &test_config());
        // 判别性:整文件回退 env 时这里会是默认搜索端点,不是标记值
        assert_eq!(loaded.search_endpoint, "https://kept-caps.example/search");
        assert_eq!(loaded.connections.len(), 1);
        let p = &loaded.connections[0];
        assert!(!p.supports_vision);
        assert!(!p.supports_structured_output);
        assert!(!p.supports_prefix_completion);
        assert!(!p.supports_mid_conversation_system);
        assert!(!p.image_auto_split);
    }

    /// 密文按 id 匹配而非按下标(用例 3):顺序颠倒后保存,每条仍拿回自己的密文
    #[test]
    fn profile_ciphertexts_are_matched_by_id_not_index() {
        let dir = temp_dir("by-id");
        write_settings(
            &dir,
            r#"{"connections":[
                {"id":"a","api_key":"enc:v1:CT-A"},
                {"id":"b","api_key":"enc:v1:CT-B"}]}"#,
        );
        let mut s = RuntimeSettings::from_config(&test_config());
        s.connections = vec![profile("a", ""), profile("b", "")]; // 模拟解密失败后的内存态
        s.active_connection_id = Some("a".to_string());
        s.connections.reverse(); // 顺序颠倒:按下标实现会把 CT-A 写到 b 头上

        s.save(&dir).unwrap();

        let json = read_json(&dir);
        assert_eq!(json["connections"][0]["id"], "b");
        assert_eq!(json["connections"][0]["api_key"], "enc:v1:CT-B");
        assert_eq!(json["connections"][1]["id"], "a");
        assert_eq!(json["connections"][1]["api_key"], "enc:v1:CT-A");
        assert_eq!(
            json["openai_api_key"], "enc:v1:CT-A",
            "顶层兼容字段应跟随默认连接 a"
        );
    }

    /// 解密失败保真(用例 4):内存空 + 磁盘密文 → 该 id 的密文原样回写,绝不被空串覆盖
    #[test]
    fn save_preserves_ciphertext_for_profile_on_disk() {
        let dir = temp_dir("keep-profile");
        write_settings(
            &dir,
            r#"{"connections":[{"id":"main","api_key":"enc:v1:CT-MAIN"}]}"#,
        );
        let mut s = RuntimeSettings::from_config(&test_config());
        s.connections = vec![profile("main", "")];
        s.active_connection_id = Some("main".to_string());

        s.save(&dir).unwrap();

        let json = read_json(&dir);
        assert_eq!(json["connections"][0]["api_key"], "enc:v1:CT-MAIN");
        assert_eq!(json["openai_api_key"], "enc:v1:CT-MAIN");
    }

    /// 显式清空(2026-10-03 API 设置补全):名单中的 id 绕过保真兜底,密钥落空;
    /// 名单外的密文仍保真;活跃连接被清时顶层投影同步为空
    #[test]
    fn explicitly_cleared_key_bypasses_preservation() {
        let dir = temp_dir("explicit-clear");
        write_settings(
            &dir,
            r#"{"connections":[
                {"id":"main","api_key":"enc:v1:CT-MAIN"},
                {"id":"side","api_key":"enc:v1:CT-SIDE"}]}"#,
        );
        let mut s = RuntimeSettings::from_config(&test_config());
        s.connections = vec![profile("main", ""), profile("side", "")];
        s.active_connection_id = Some("main".to_string());

        s.save_with_cleared_connection_keys(&dir, &["main".to_string()])
            .unwrap();

        let json = read_json(&dir);
        assert_eq!(json["connections"][0]["api_key"], "", "被清连接应落空");
        assert_eq!(
            json["openai_api_key"], "",
            "活跃连接被清 → 顶层投影同步为空"
        );
        assert_eq!(
            json["connections"][1]["api_key"], "enc:v1:CT-SIDE",
            "名单外密文仍保真"
        );

        // 对照:名单为空的常规 save 对同一空内存仍保真(防保真兜底被误伤回归)
        let mut again = RuntimeSettings::from_config(&test_config());
        again.connections = vec![profile("side", "")];
        again.active_connection_id = Some("side".to_string());
        again.save(&dir).unwrap();
        let json = read_json(&dir);
        assert_eq!(
            json["connections"][0]["api_key"], "enc:v1:CT-SIDE",
            "未在名单中的空内存密钥仍走保真回写"
        );
    }

    /// 落盘无明文(用例 5):多套连接的明文 Key 都不得出现在文件里,load 后逐条还原
    #[cfg(windows)]
    #[test]
    fn save_writes_no_plaintext_key_in_any_profile() {
        let dir = temp_dir("no-plain");
        let mut s = RuntimeSettings::from_config(&test_config());
        s.connections = vec![profile("a", "sk-aaa-1111"), profile("b", "sk-bbb-2222")];
        s.active_connection_id = Some("b".to_string());

        s.save(&dir).unwrap();

        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(!text.contains("sk-aaa-1111"), "明文不得落盘:{text}");
        assert!(!text.contains("sk-bbb-2222"), "明文不得落盘:{text}");
        assert!(text.contains("enc:v1:"), "应写入密文:{text}");

        let loaded = RuntimeSettings::load(&dir, &test_config());
        assert_eq!(loaded.openai_api_key, "sk-bbb-2222", "顶层应还原默认连接");
        assert_eq!(loaded.connections[0].api_key, "sk-aaa-1111");
        assert_eq!(loaded.connections[1].api_key, "sk-bbb-2222");
        assert_eq!(loaded.active_connection_id.as_deref(), Some("b"));
    }

    /// 投影(用例 6):扁平字段 = 默认连接的派生视图;save 以 connections 为准重算(手改扁平字段无效)
    #[test]
    fn active_connection_projection_written_to_flat_fields() {
        let dir = temp_dir("projection");
        let mut s = RuntimeSettings::from_config(&test_config());
        s.connections = vec![profile("a", ""), profile("b", "")];
        s.active_connection_id = Some("a".to_string());
        s.normalize_connections();
        assert_eq!(s.openai_base_url, "https://a.example/v1");

        s.active_connection_id = Some("b".to_string());
        s.normalize_connections();
        assert_eq!(
            s.openai_base_url, "https://b.example/v1",
            "切默认连接应改变投影"
        );
        assert_eq!(s.model, "m");

        // 手改扁平字段不再是真源:落盘值按 connections 重算
        s.openai_base_url = "https://hand-edited.example/v1".to_string();
        s.save(&dir).unwrap();
        let json = read_json(&dir);
        assert_eq!(
            json["openai_base_url"], "https://b.example/v1",
            "save 应以 connections 为准重算投影"
        );
        assert_eq!(json["active_connection_id"], "b");
    }

    /// 默认连接回退(用例 7):指向不存在/已停用 → 第一个启用的;全停用 → 空投影
    #[test]
    fn active_falls_back_to_first_enabled_when_missing_or_disabled() {
        let mut s = RuntimeSettings::from_config(&test_config());
        let mut disabled = profile("a", "");
        disabled.enabled = false;
        s.connections = vec![disabled, profile("b", "")];

        s.active_connection_id = Some("ghost".to_string());
        s.normalize_connections();
        assert_eq!(s.active_connection_id.as_deref(), Some("b"));

        s.active_connection_id = Some("a".to_string());
        s.normalize_connections();
        assert_eq!(
            s.active_connection_id.as_deref(),
            Some("b"),
            "指向已停用 → 回退"
        );

        // 全停用:投影为空(连接器回退 mock,与「未配置」语义一致)
        for p in s.connections.iter_mut() {
            p.enabled = false;
        }
        s.normalize_connections();
        assert_eq!(s.active_connection_id, None);
        assert!(s.openai_base_url.is_empty());
        assert!(s.openai_api_key.is_empty());
        assert!(s.model.is_empty());
    }

    /// 卫生清理(用例 8):类型回退、空名补名、超长截断、空 id 与重复 id 重新分配(已有 id 不改写)
    #[test]
    fn profile_type_and_name_sanitized_on_load() {
        let dir = temp_dir("sanitize");
        let mut s = RuntimeSettings::from_config(&test_config());
        s.connections = vec![
            ConnectionProfile {
                id: String::new(),
                name: "   ".to_string(),
                connector_type: "ollama".to_string(),
                base_url: "127.0.0.1:1234".to_string(),
                api_key: String::new(),
                model: "m".repeat(250),
                api_style: String::new(),
                enabled: true,
                supports_vision: false,
                supports_structured_output: false,
                supports_prefix_completion: false,
                supports_mid_conversation_system: false,
                image_auto_split: false,
            },
            profile("dup", ""),
            profile("dup", ""),
        ];
        s.active_connection_id = Some("dup".to_string());
        // 直接序列化(绕过 save 的规范化),模拟手改 / 跨库合并进来的脏数据
        write_settings(&dir, &serde_json::to_string_pretty(&s).unwrap());

        let loaded = RuntimeSettings::load(&dir, &test_config());
        assert_eq!(loaded.connections.len(), 3);
        assert_eq!(loaded.connections[0].name, "连接 1", "空名应补默认名");
        assert_eq!(
            loaded.connections[0].connector_type, "openai-compatible",
            "未知连接器类型应回退"
        );
        assert_eq!(
            loaded.connections[0].base_url, "http://127.0.0.1:1234/v1",
            "地址应规范化"
        );
        assert_eq!(
            loaded.connections[0].model.chars().count(),
            200,
            "超长应截断"
        );
        assert!(!loaded.connections[0].id.is_empty(), "空 id 应补新 id");
        assert_eq!(loaded.connections[1].id, "dup", "首个 dup 保留原 id");
        assert_ne!(loaded.connections[2].id, "dup", "重复 id 应重新分配");
        assert_eq!(
            loaded.connections[2].name, "连接-dup",
            "其余字段不受去重影响"
        );
        assert_eq!(loaded.active_connection_id.as_deref(), Some("dup"));
    }

    /// 连接不进模式覆盖层(用例 9 / 口径 P5):task 与 roleplay 看到的连接完全一致
    #[test]
    fn connections_do_not_enter_mode_overlay() {
        use crate::services::settings_service::AppMode;
        let mut s = RuntimeSettings::from_config(&test_config());
        s.connections = vec![profile("a", ""), profile("b", "")];
        s.active_connection_id = Some("b".to_string());
        s.normalize_connections();

        let task = s.for_mode(AppMode::Task);
        let roleplay = s.for_mode(AppMode::Roleplay);
        assert_eq!(task.connections.len(), 2);
        assert_eq!(task.active_connection_id.as_deref(), Some("b"));
        assert_eq!(task.openai_base_url, "https://b.example/v1");
        assert_eq!(task.model, "m");
        assert_eq!(
            serde_json::to_string(&task.connections).unwrap(),
            serde_json::to_string(&roleplay.connections).unwrap(),
            "连接信息始终共享,不得按模式隔离"
        );
    }
}
