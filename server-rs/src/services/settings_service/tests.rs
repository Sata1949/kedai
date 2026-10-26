// settings_service 测试：自 mod.rs 搬出（C-2F-1，纯搬运）。
// 首行 #![cfg(test)] 是 check-arch productionLineCount 的生产区切分标记，勿移。
#![cfg(test)]

use super::params::{
    default_literary_reflect_prompt, default_literary_roleplay_agent_prompt,
    default_reflect_prompt, LEGACY_LITERARY_REFLECT_ENHANCEMENT_V1,
    LEGACY_LITERARY_RP_ENHANCEMENT_V1, LEGACY_REFLECT_PROMPT_V1, LEGACY_ROLEPLAY_AGENT_PROMPT_V1,
};
use super::*;
use crate::config::AppConfig;
use crate::utils::test_support::TempDataDir;

#[test]
fn test_normalize_base_url() {
    // 无协议 → 补 https
    assert_eq!(
        normalize_base_url("api.openai.com"),
        "https://api.openai.com/v1"
    );
    // 有协议无路径 → 补 /v1
    assert_eq!(
        normalize_base_url("https://api.openai.com"),
        "https://api.openai.com/v1"
    );
    // 已带 /v1 → 保持不变(去尾部斜杠)
    assert_eq!(
        normalize_base_url("https://api.deepseek.com/v1/"),
        "https://api.deepseek.com/v1"
    );
    assert_eq!(
        normalize_base_url("https://api.deepseek.com/v1"),
        "https://api.deepseek.com/v1"
    );
    // 已有其他路径 → 不补 /v1
    assert_eq!(
        normalize_base_url("https://openrouter.ai/api/v1"),
        "https://openrouter.ai/api/v1"
    );
    // 本机地址 → 补 http + /v1
    assert_eq!(
        normalize_base_url("localhost:11434"),
        "http://localhost:11434/v1"
    );
    assert_eq!(
        normalize_base_url("http://localhost:11434"),
        "http://localhost:11434/v1"
    );
    assert_eq!(
        normalize_base_url("127.0.0.1:1234"),
        "http://127.0.0.1:1234/v1"
    );
    // 空输入
    assert_eq!(normalize_base_url("   "), "");
}

/// 端点后缀剥离(A 档,2026-10-01):用户在设置里粘贴的常是**完整端点 URL**
/// (控制台文档形态),连接器把 base 当目录再拼端点 —— 不剥离就会拼出
/// `…/chat/completions/models` 一类双段路径。真实事故:百炼 workspace 端点
/// 被整体粘入 → 模型列表静默回退成 1 个当前模型。
#[test]
fn strip_endpoint_suffix_covers_full_endpoint_paste() {
    use crate::connectors::openai_compatible::{
        API_STYLE_ANTHROPIC, API_STYLE_CHAT, API_STYLE_RESPONSES,
    };
    // 完整端点 + 方言推断
    let (u, s) = strip_endpoint_suffix(
        "https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/chat/completions",
    );
    assert_eq!(
        u,
        "https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(s, Some(API_STYLE_CHAT));
    // 尾部斜杠也要能匹配(trim 后再剥)
    let (u, s) = strip_endpoint_suffix("https://api.example.com/v1/chat/completions/");
    assert_eq!(u, "https://api.example.com/v1");
    assert_eq!(s, Some(API_STYLE_CHAT));
    // Responses / Anthropic(含 /v1/messages 先于 /messages 的顺序)
    let (u, s) = strip_endpoint_suffix(
        "https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/responses",
    );
    assert_eq!(
        u,
        "https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(s, Some(API_STYLE_RESPONSES));
    let (u, s) = strip_endpoint_suffix("https://api.anthropic.com/v1/messages");
    assert_eq!(u, "https://api.anthropic.com");
    assert_eq!(s, Some(API_STYLE_ANTHROPIC));
    let (u, s) = strip_endpoint_suffix(
        "https://ws-x.aliyuncs.com/api/v2/apps/claude-code-proxy/v1/messages",
    );
    assert_eq!(u, "https://ws-x.aliyuncs.com/api/v2/apps/claude-code-proxy");
    assert_eq!(s, Some(API_STYLE_ANTHROPIC));
    // /models 也剥(仅剥地址,不改方言)
    let (u, s) = strip_endpoint_suffix("https://api.example.com/v1/models");
    assert_eq!(u, "https://api.example.com/v1");
    assert_eq!(s, None);
    // 正常 base 不动;去尾斜杠后原样返回
    let (u, s) = strip_endpoint_suffix("https://api.deepseek.com/v1/");
    assert_eq!(u, "https://api.deepseek.com/v1");
    assert_eq!(s, None);
    // 剥离后再走常规规范化(无路径补 /v1 等)
    assert_eq!(
        normalize_base_url("https://api.openai.com/v1/chat/completions"),
        "https://api.openai.com/v1"
    );
    assert_eq!(
        normalize_base_url("api.deepseek.com/chat/completions"),
        "https://api.deepseek.com/v1"
    );
}

/// 方言推断只填补默认档;显式非默认档不被覆盖;两轮 sanitize 幂等(2026-10-01 批次)。
#[test]
fn sanitize_infers_api_style_and_is_idempotent() {
    use crate::connectors::openai_compatible::{
        API_STYLE_ANTHROPIC, API_STYLE_CHAT, API_STYLE_RESPONSES,
    };
    let mut p = ConnectionProfile {
        id: "c".to_string(),
        name: "连接".to_string(),
        connector_type: CONNECTOR_TYPE_OPENAI.to_string(),
        base_url: "https://ws-x/compatible-mode/v1/responses".to_string(),
        api_key: "sk".to_string(),
        model: "m".to_string(),
        api_style: String::new(),
        enabled: true,
        supports_vision: false,
        supports_structured_output: false,
        supports_prefix_completion: false,
        supports_mid_conversation_system: false,
        image_auto_split: false,
    };
    p.sanitize(0);
    assert_eq!(p.base_url, "https://ws-x/compatible-mode/v1");
    assert_eq!(p.api_style, API_STYLE_RESPONSES, "后缀推断应写入方言");
    // 幂等:第二轮 base 已无后缀,方言保持
    p.sanitize(0);
    assert_eq!(p.api_style, API_STYLE_RESPONSES);
    // 显式非默认档不被推断覆盖(URL 后缀与显式档冲突时,以显式档为准)
    p.base_url = "https://api.anthropic.com/v1/messages".to_string();
    p.api_style = API_STYLE_CHAT.to_string(); // chat 属默认档 → 会被推断改写
    p.sanitize(0);
    assert_eq!(p.api_style, API_STYLE_ANTHROPIC);
    let mut q = ConnectionProfile {
        api_style: API_STYLE_RESPONSES.to_string(),
        ..p.clone()
    };
    q.base_url = "https://api.anthropic.com/v1/messages".to_string();
    q.sanitize(0);
    assert_eq!(
        q.api_style, API_STYLE_RESPONSES,
        "显式非默认档不被 URL 后缀推断覆盖"
    );
}

/// 临时数据目录(测试隔离用;作用域结束自动清理)
fn tmp_dir(tag: &str) -> TempDataDir {
    TempDataDir::new(&format!("settings-test-{tag}"))
}

/// 目标连接器类型解析(批次 4):`openai_base_url` 的环境默认值本身非空,故
/// 「配置完成」的判据必须是**地址 + 密钥都非空** —— 只看地址会让全新安装与测试环境
/// 一启动就切到必然失败的真实连接器(2026-09-22 实测:该口径写错会让 tasks.rs 21 个目标全红)。
#[test]
fn resolve_connector_target_requires_address_and_key() {
    let mut p = ConnectionProfile {
        id: "c".to_string(),
        name: "连接".to_string(),
        connector_type: CONNECTOR_TYPE_OPENAI.to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: String::new(),
        model: "m".to_string(),
        api_style: crate::connectors::openai_compatible::API_STYLE_CHAT.to_string(),
        enabled: true,
        supports_vision: false,
        supports_structured_output: false,
        supports_prefix_completion: false,
        supports_mid_conversation_system: false,
        image_auto_split: false,
    };
    // 只有地址(环境默认值即如此)→ 保持 mock,不切真实连接器
    assert_eq!(resolve_connector_target(Some(&p), "mock"), "mock");
    // 地址 + 密钥都填了 → 切真实连接器
    p.api_key = "sk-x".to_string();
    assert_eq!(
        resolve_connector_target(Some(&p), "mock"),
        CONNECTOR_TYPE_OPENAI
    );
    // 显式声明 mock 的连接永远保持 mock(演示模式是用户的明确选择)
    p.connector_type = CONNECTOR_TYPE_MOCK.to_string();
    assert_eq!(
        resolve_connector_target(Some(&p), CONNECTOR_TYPE_OPENAI),
        "mock"
    );
    // 没有可用连接(全停用 / 删空)→ mock
    assert_eq!(
        resolve_connector_target(None, CONNECTOR_TYPE_OPENAI),
        "mock"
    );
    // 当前已是真实连接器且凭据没配齐 → 不主动降级
    p.connector_type = CONNECTOR_TYPE_OPENAI.to_string();
    p.api_key = String::new();
    assert_eq!(
        resolve_connector_target(Some(&p), CONNECTOR_TYPE_OPENAI),
        CONNECTOR_TYPE_OPENAI
    );
}

/// 最小 AppConfig(仅供设置加载/保存测试;不读环境变量,避免受本机 .env 影响)
fn test_cfg() -> AppConfig {
    crate::config::test_config()
}

/// API Key 落盘应为密文(文件不含明文),load 后还原为明文。
///
/// **只在 Windows 成立**:密文落盘依赖平台安全凭据存储(Windows DPAPI;Android 由
/// Keystore 承接),无存储的平台按设计**拒绝**写回明文 API Key(见 `secret_store::protect`)——
/// 那条拒绝路径由 `secret_store::non_windows_rejects_plaintext_by_default` 单独锁定,
/// 故此处不重复覆盖(2026-09-27 Linux 档实测:非 Windows 平台上本用例在 save 处即失败)。
#[cfg(windows)]
#[test]
fn api_key_is_encrypted_on_disk_and_restored_on_load() {
    let dir = tmp_dir("enc");
    let mut s = RuntimeSettings::from_config(&test_cfg());
    // 本批次起 connections 才是真源,扁平字段是它的派生视图 → 写 Key 要写默认连接
    s.connections[0].api_key = "sk-secret-value-9999".to_string();
    s.save(&dir).unwrap();

    let raw = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    assert!(
        !raw.contains("sk-secret-value-9999"),
        "settings.json 不应包含明文 API Key"
    );

    let loaded = RuntimeSettings::load(&dir, &test_cfg());
    assert_eq!(
        loaded.openai_api_key, "sk-secret-value-9999",
        "load 应还原明文"
    );
}

/// 旧版明文 settings.json:可正常读取,且 load 时自动就地迁移为密文
#[test]
fn legacy_plaintext_settings_are_migrated_on_load() {
    let dir = tmp_dir("migrate");
    let mut s = RuntimeSettings::from_config(&test_cfg());
    // 绕过 save 的加密直接写旧版明文文件:顶层与默认连接都写明文(旧文件两处都会是明文)
    s.connections[0].api_key = "sk-legacy-plain-1234".to_string();
    s.apply_active_projection();
    // 绕过 save 的加密,直接写旧版明文文件
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&s).unwrap(),
    )
    .unwrap();

    let loaded = RuntimeSettings::load(&dir, &test_cfg());
    assert_eq!(
        loaded.openai_api_key, "sk-legacy-plain-1234",
        "旧版明文应能正常读取"
    );
    // 「load 时就地迁移为密文」依赖平台安全凭据存储(Windows DPAPI / Android Keystore);
    // 无存储的平台按设计**拒绝**明文写回(见 `secret_store::protect`,并有独立用例锁定),
    // 故该断言只在 Windows 成立。
    #[cfg(windows)]
    {
        let raw = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !raw.contains("sk-legacy-plain-1234"),
            "load 后应已就地迁移为密文: {raw}"
        );
    }
    // 迁移后再次 load 仍应得到同一明文(有存储平台读的是密文,无存储平台读的仍是明文,
    // 两种情形都必须读出同一值)
    let again = RuntimeSettings::load(&dir, &test_cfg());
    assert_eq!(again.openai_api_key, "sk-legacy-plain-1234");
}

/// 未配置 Key:落盘保持空串,不产生密文噪声
#[test]
fn empty_api_key_stays_empty() {
    let dir = tmp_dir("empty");
    let mut s = RuntimeSettings::from_config(&test_cfg());
    s.openai_api_key = String::new();
    s.save(&dir).unwrap();
    let loaded = RuntimeSettings::load(&dir, &test_cfg());
    assert!(loaded.openai_api_key.is_empty(), "空 Key 应保持空");
}

/// 旧版 settings.json 缺少 render_html 时应兼容加载并默认关闭。
#[test]
fn legacy_settings_without_render_html_default_to_false() {
    let dir = tmp_dir("legacy-render-html");
    let s = RuntimeSettings::from_config(&test_cfg());
    let mut json = serde_json::to_value(&s).unwrap();
    json.as_object_mut().unwrap().remove("render_html");
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();

    let loaded = RuntimeSettings::load(&dir, &test_cfg());
    assert!(!loaded.render_html, "旧配置缺省时 HTML 渲染必须关闭");
}

/// 记忆设置(落地项 2):默认 关闭蒸馏 / 注入上限 8;
/// 旧版 settings.json 缺字段时 serde default 补齐;越界值钳回默认;保存往返还原。
#[test]
fn memory_settings_defaults_clamp_and_roundtrip() {
    let cfg = test_cfg();
    let s = RuntimeSettings::from_config(&cfg);
    assert!(!s.memory_distill_enabled, "蒸馏默认关闭");
    assert_eq!(s.memory_inject_limit, 8, "注入上限默认 8");
    assert_eq!(s.memory_inject_char_budget, 2000, "字符预算默认 2000");
    assert_eq!(s.memory_max_entries, 200, "每角色容量默认 200");

    // 旧版配置缺字段:serde default 补齐,行为与默认一致
    let dir = tmp_dir("memory-legacy");
    let mut json = serde_json::to_value(&s).unwrap();
    json.as_object_mut()
        .unwrap()
        .remove("memory_distill_enabled");
    json.as_object_mut().unwrap().remove("memory_inject_limit");
    json.as_object_mut()
        .unwrap()
        .remove("memory_inject_char_budget");
    json.as_object_mut().unwrap().remove("memory_max_entries");
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir, &cfg);
    assert!(!loaded.memory_distill_enabled);
    assert_eq!(loaded.memory_inject_limit, 8);
    assert_eq!(loaded.memory_inject_char_budget, 2000);
    assert_eq!(loaded.memory_max_entries, 200);

    // 越界钳回默认;0 合法(关闭注入/不限制淘汰)
    let dir2 = tmp_dir("memory-clamp");
    let mut s2 = RuntimeSettings::from_config(&cfg);
    s2.memory_distill_enabled = true;
    s2.memory_inject_limit = 500;
    s2.memory_inject_char_budget = 999_999;
    s2.memory_max_entries = 999_999;
    s2.save(&dir2).unwrap();
    let loaded2 = RuntimeSettings::load(&dir2, &cfg);
    assert!(loaded2.memory_distill_enabled, "开关应保存往返还原");
    assert_eq!(loaded2.memory_inject_limit, 8, "越界上限应钳回 8");
    assert_eq!(
        loaded2.memory_inject_char_budget, 2000,
        "越界预算应钳回 2000"
    );
    assert_eq!(loaded2.memory_max_entries, 200, "越界容量应钳回 200");

    let dir3 = tmp_dir("memory-zero");
    let mut s3 = RuntimeSettings::from_config(&cfg);
    s3.memory_inject_limit = 0;
    s3.memory_inject_char_budget = 0;
    s3.memory_max_entries = 0;
    s3.save(&dir3).unwrap();
    let loaded3 = RuntimeSettings::load(&dir3, &cfg);
    assert_eq!(loaded3.memory_inject_limit, 0, "0 是合法值(关闭注入)");
    assert_eq!(loaded3.memory_inject_char_budget, 0, "0 合法(不限制预算)");
    assert_eq!(loaded3.memory_max_entries, 0, "0 合法(不淘汰)");
}

/// embedding 向量化配置:默认关闭且空;旧配置缺字段 serde default 补齐;
/// Key 落盘加密、load 解密还原;维度越界回退 0(自动探测);合法值往返。
///
/// **只在 Windows 成立**:后半段(写入非空 Key 后 `save`/`load` 往返、越界值经 save 钳回)
/// 依赖平台安全凭据存储(Windows DPAPI / Android Keystore)——无存储的平台在 `save`
/// 写非空 Key 时按设计报错(见 `secret_store::protect`,拒绝行为有独立用例锁定)。
/// 前半段(默认值 / 旧配置缺字段补默认)是平台无关的,但与本用例的其余断言同处一体,
/// 单独拆出会让用例名与内容脱节,故整体按平台门控。
#[cfg(windows)]
#[test]
fn embedding_settings_defaults_clamp_and_roundtrip() {
    let cfg = test_cfg();
    let s = RuntimeSettings::from_config(&cfg);
    assert!(!s.embedding_enabled, "向量化默认关闭");
    assert!(s.embedding_base_url.is_empty());
    assert!(s.embedding_api_key.is_empty());
    assert!(s.embedding_model.is_empty());
    assert_eq!(s.embedding_dim, 0, "维度默认 0 = 自动探测");

    // 旧版 settings.json 缺 embedding 字段:serde default 补齐
    let dir = tmp_dir("embedding-legacy");
    let mut json = serde_json::to_value(&s).unwrap();
    for k in [
        "embedding_enabled",
        "embedding_base_url",
        "embedding_api_key",
        "embedding_model",
        "embedding_dim",
    ] {
        json.as_object_mut().unwrap().remove(k);
    }
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir, &cfg);
    assert!(!loaded.embedding_enabled);
    assert_eq!(loaded.embedding_dim, 0);

    // 往返:Key 加密落盘但内存明文可还原;维度合法值保留
    let dir2 = tmp_dir("embedding-roundtrip");
    let mut s2 = RuntimeSettings::from_config(&cfg);
    s2.embedding_enabled = true;
    s2.embedding_base_url = "https://api.example.com/v1".into();
    s2.embedding_api_key = "sk-embed-secret-1234".into();
    s2.embedding_model = "text-embedding-3-small".into();
    s2.embedding_dim = 1536;
    s2.save(&dir2).unwrap();
    // 落盘副本必须是密文(不出现明文 Key)
    let raw = std::fs::read_to_string(dir2.join("settings.json")).unwrap();
    assert!(
        !raw.contains("sk-embed-secret-1234"),
        "embedding Key 不应以明文落盘"
    );
    let loaded2 = RuntimeSettings::load(&dir2, &cfg);
    assert!(loaded2.embedding_enabled, "开关往返还原");
    assert_eq!(
        loaded2.embedding_api_key, "sk-embed-secret-1234",
        "Key 解密还原"
    );
    assert_eq!(loaded2.embedding_model, "text-embedding-3-small");
    assert_eq!(loaded2.embedding_dim, 1536);

    // 维度越界回退 0(自动探测)
    let dir3 = tmp_dir("embedding-clamp");
    let mut s3 = RuntimeSettings::from_config(&cfg);
    s3.embedding_dim = 99_999;
    s3.save(&dir3).unwrap();
    assert_eq!(
        RuntimeSettings::load(&dir3, &cfg).embedding_dim,
        0,
        "越界维度应回退 0"
    );
}

/// 落地项 3 设置:渐进披露默认开启、子代理三参数取默认;旧版 settings.json
/// 缺字段时 serde default 补齐;越界值钳回默认;保存往返还原;task 覆盖层生效。
#[test]
fn harness_settings_defaults_clamp_roundtrip_and_mode_override() {
    let cfg = test_cfg();
    let s = RuntimeSettings::from_config(&cfg);
    assert!(s.skill_progressive_disclosure, "渐进披露默认开启");
    assert_eq!(s.subagent_max_depth, 2);
    assert_eq!(s.subagent_max_concurrency, 6);
    assert_eq!(s.subagent_result_max_chars, 2000);

    // 旧版配置缺字段:serde default 补齐,行为与默认一致
    let dir = tmp_dir("harness-legacy");
    let mut json = serde_json::to_value(&s).unwrap();
    for key in [
        "skill_progressive_disclosure",
        "subagent_max_depth",
        "subagent_max_concurrency",
        "subagent_result_max_chars",
    ] {
        json.as_object_mut().unwrap().remove(key);
    }
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir, &cfg);
    assert!(loaded.skill_progressive_disclosure);
    assert_eq!(loaded.subagent_max_depth, 2);
    assert_eq!(loaded.subagent_max_concurrency, 6);
    assert_eq!(loaded.subagent_result_max_chars, 2000);

    // 越界钳回默认:深度 0/5 → 2;并发 0/17 → 6;结果 499/8001 → 2000
    let dir2 = tmp_dir("harness-clamp");
    let mut s2 = RuntimeSettings::from_config(&cfg);
    s2.subagent_max_depth = 0;
    s2.subagent_max_concurrency = 17;
    s2.subagent_result_max_chars = 499;
    s2.save(&dir2).unwrap();
    let clamped = RuntimeSettings::load(&dir2, &cfg);
    assert_eq!(clamped.subagent_max_depth, 2, "深度越界应钳回 2");
    assert_eq!(clamped.subagent_max_concurrency, 6, "并发越界应钳回 6");
    assert_eq!(
        clamped.subagent_result_max_chars, 2000,
        "结果上限越界应钳回 2000"
    );

    // 合法边界值通过;保存往返还原
    let dir3 = tmp_dir("harness-roundtrip");
    let mut s3 = RuntimeSettings::from_config(&cfg);
    s3.skill_progressive_disclosure = false;
    s3.subagent_max_depth = 4;
    s3.subagent_max_concurrency = 1;
    s3.subagent_result_max_chars = 8000;
    s3.save(&dir3).unwrap();
    let rt = RuntimeSettings::load(&dir3, &cfg);
    assert!(!rt.skill_progressive_disclosure, "关闭渐进披露应还原");
    assert_eq!(rt.subagent_max_depth, 4);
    assert_eq!(rt.subagent_max_concurrency, 1);
    assert_eq!(rt.subagent_result_max_chars, 8000);

    // TM-SET-3:子代理三参数改直写扁平(执行期读全局快照,模式覆盖永不生效)——
    // task 视图 = 扁平值,两模式共用;旧覆盖层残留字段已从 ModeSettings 移除
    // (编译期即不可写,旧 settings.json 里的残留键由 serde 忽略)。
    let mut s4 = RuntimeSettings::from_config(&cfg);
    s4.subagent_max_depth = 1;
    assert_eq!(
        s4.for_mode(AppMode::Task).subagent_max_depth,
        1,
        "task 视图读扁平值(全局设置)"
    );
    assert_eq!(
        s4.for_mode(AppMode::Roleplay).subagent_max_depth,
        1,
        "roleplay 同读扁平值"
    );

    // task 模式 agent_system_prompt 不回退 roleplay 人设词:
    // None → 内置任务向默认提示词;Some(v) → 覆盖;Some("") → 显式留空(注入方跳过)
    let mut s5 = RuntimeSettings::from_config(&cfg);
    s5.agent_system_prompt =
        RoleplayPromptConfig("你是 {{char}} 的扮演者,与用户进行沉浸式角色扮演".into());
    let task_default = s5.for_mode(AppMode::Task);
    assert_eq!(
        task_default.agent_system_prompt.0,
        default_task_agent_prompt(),
        "task 缺省应为内置任务向提示词,不得继承 roleplay 人设词"
    );
    assert!(
        !task_default.agent_system_prompt.0.contains("{{char}}"),
        "默认词不得含角色扮演宏"
    );
    s5.task.agent_system_prompt = Some(TaskPromptConfig("任务专用提示词".into()));
    assert_eq!(
        s5.for_mode(AppMode::Task).agent_system_prompt.0,
        "任务专用提示词",
        "task 覆盖层应优先"
    );
    s5.task.agent_system_prompt = Some(TaskPromptConfig(String::new()));
    assert_eq!(
        s5.for_mode(AppMode::Task).agent_system_prompt.0,
        "",
        "显式清空应保留空串"
    );
    assert!(
        s5.for_mode(AppMode::Roleplay)
            .agent_system_prompt
            .0
            .contains("{{char}}"),
        "roleplay 扁平值不受 task 缺省词影响"
    );
}

/// TM-GEN-1 / TM-SET-1:任务向缺省生成参数——`None` 时温度/Top-P 取任务缺省,
/// 输出上限取「任务缺省与扁平值只抬不压」的较大者;`Some` 显式值逐字优先;
/// roleplay 侧逐字节不变(读扁平权威值)。
#[test]
fn task_generation_defaults_apply_and_some_wins() {
    let cfg = test_cfg();
    let mut s = RuntimeSettings::from_config(&cfg);
    assert_eq!(
        s.for_mode(AppMode::Task).default_max_tokens,
        TASK_DEFAULT_OUTPUT_TOKENS,
        "None + 扁平 1024 → 抬到任务缺省 8192"
    );
    assert_eq!(
        s.for_mode(AppMode::Task).default_temperature,
        TASK_DEFAULT_TEMPERATURE,
        "None → 任务缺省温度 0.3(不沿用扁平 0.8)"
    );
    assert_eq!(
        s.for_mode(AppMode::Task).default_top_p,
        TASK_DEFAULT_TOP_P,
        "None → 任务缺省 Top-P 1.0(不沿用扁平 0.9)"
    );
    s.default_max_tokens = 16384;
    assert_eq!(
        s.for_mode(AppMode::Task).default_max_tokens,
        16384,
        "扁平值更大时保持更大(只抬不压)"
    );
    s.task.default_max_tokens = Some(2048);
    assert_eq!(
        s.for_mode(AppMode::Task).default_max_tokens,
        2048,
        "显式任务值逐字优先(低于任务缺省也不动)"
    );
    s.task.default_max_tokens = Some(32768);
    assert_eq!(s.for_mode(AppMode::Task).default_max_tokens, 32768);
    s.default_temperature = 1.1;
    assert_eq!(
        s.for_mode(AppMode::Task).default_temperature,
        TASK_DEFAULT_TEMPERATURE,
        "扁平温度变化不影响任务缺省"
    );
    s.task.default_temperature = Some(0.3);
    assert_eq!(s.for_mode(AppMode::Task).default_temperature, 0.3);
    s.task.default_top_p = Some(0.95);
    assert_eq!(s.for_mode(AppMode::Task).default_top_p, 0.95);
    assert_eq!(
        s.for_mode(AppMode::Roleplay).default_max_tokens,
        16384,
        "roleplay 读扁平权威值,不受任务缺省影响"
    );
    assert_eq!(
        s.for_mode(AppMode::Roleplay).default_temperature,
        1.1,
        "roleplay 温度读扁平权威值"
    );
}

/// TM-SET-1:任务模式默认连接的**软回退**解析——未配置/指向已删/已停用 → None
/// (调用方回退默认连接,不使任务失败);指向存在且启用 → Some(id);
/// 任务覆盖层可覆盖扁平值(Some("") 显式清除回到跟随默认连接)。
#[test]
fn task_mode_default_connection_soft_fallback_and_override() {
    let cfg = test_cfg();
    let mut s = RuntimeSettings::from_config(&cfg);
    assert!(
        task_mode_default_connection(&s.for_mode(AppMode::Task)).is_none(),
        "未配置 → None(跟随默认连接,零迁移)"
    );
    s.task.task_default_connection_id = Some("ghost".into());
    assert!(
        task_mode_default_connection(&s.for_mode(AppMode::Task)).is_none(),
        "指向已删除的连接 → 软回退 None(不报错)"
    );
    // from_config 已播种固定 id "default" 的默认连接且启用
    s.task.task_default_connection_id = Some("default".into());
    assert_eq!(
        task_mode_default_connection(&s.for_mode(AppMode::Task)).as_deref(),
        Some("default"),
        "存在且启用 → Some(id)"
    );
    for p in s.connections.iter_mut() {
        if p.id == "default" {
            p.enabled = false;
        }
    }
    assert!(
        task_mode_default_connection(&s.for_mode(AppMode::Task)).is_none(),
        "指向已停用的连接 → 软回退 None"
    );
    // 恢复启用,继续验合并优先级
    for p in s.connections.iter_mut() {
        if p.id == "default" {
            p.enabled = true;
        }
    }
    // task 覆盖层优先于扁平值;Some("") = 显式清除
    s.task_default_connection_id = "default".into();
    s.task.task_default_connection_id = None;
    assert_eq!(
        task_mode_default_connection(&s.for_mode(AppMode::Task)).as_deref(),
        Some("default"),
        "None 沿用扁平值"
    );
    s.task.task_default_connection_id = Some(String::new());
    assert!(
        task_mode_default_connection(&s.for_mode(AppMode::Task)).is_none(),
        "Some(\"\") 显式清除 → None"
    );
    // roleplay 视图不读本字段的消费点,但扁平值不受任务覆盖影响(隔离不变量)
    assert_eq!(
        s.for_mode(AppMode::Roleplay).task_default_connection_id,
        "default",
        "roleplay 视图 = 扁平权威值"
    );
}

/// MCP 设置(批次 6.2;**PLGM 3.4 起无模式覆盖层**):默认 关/空列表;旧版
/// settings.json 缺字段时 serde default 补齐;保存往返还原;缺名/缺命令条目被清理。
/// 「不支持模式级覆盖」由类型承载——ModeSettings 已无 mcp_* 字段(引用即编译失败),
/// 写入路径直写扁平(见 api/settings.rs 的注释)。
#[test]
fn mcp_settings_defaults_sanitize_and_roundtrip() {
    let cfg = test_cfg();
    let s = RuntimeSettings::from_config(&cfg);
    assert!(!s.mcp_enabled, "MCP 默认关闭");
    assert!(s.mcp_servers.is_empty(), "MCP 服务器列表默认空");

    // 旧版配置缺字段:serde default 补齐,行为与默认一致
    let dir = tmp_dir("mcp-legacy");
    let mut json = serde_json::to_value(&s).unwrap();
    json.as_object_mut().unwrap().remove("mcp_enabled");
    json.as_object_mut().unwrap().remove("mcp_servers");
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir, &cfg);
    assert!(!loaded.mcp_enabled);
    assert!(loaded.mcp_servers.is_empty());

    // 保存往返还原 + 卫生清理(缺命令条目被丢弃,名称/命令 trim)
    let dir2 = tmp_dir("mcp-roundtrip");
    let mut s2 = RuntimeSettings::from_config(&cfg);
    s2.mcp_enabled = true;
    s2.mcp_servers = vec![
        McpServerConfig {
            name: " fs ".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@mcp/fs".into()],
            enabled: true,
        },
        McpServerConfig {
            name: "broken".into(),
            command: String::new(),
            args: Vec::new(),
            enabled: true,
        },
    ];
    s2.save(&dir2).unwrap();
    let loaded2 = RuntimeSettings::load(&dir2, &cfg);
    assert!(loaded2.mcp_enabled, "开关应保存往返还原");
    assert_eq!(loaded2.mcp_servers.len(), 1, "缺命令条目应被清理");
    assert_eq!(loaded2.mcp_servers[0].name, "fs", "名称应 trim");
    assert_eq!(loaded2.mcp_servers[0].args.len(), 2);

    // task 视图与 roleplay 视图读同一份扁平权威值(PLGM 3.4:无覆盖层可偏离)
    let s3 = RuntimeSettings::from_config(&cfg);
    assert_eq!(
        s3.for_mode(AppMode::Task).mcp_enabled,
        s3.for_mode(AppMode::Roleplay).mcp_enabled,
        "两模式视图的 MCP 值恒一致(进程级全局能力)"
    );

    // 缺省字段(旧客户端手写条目只给 name/command):args 默认空、enabled 默认 true
    let partial: McpServerConfig = serde_json::from_str(r#"{"name":"a","command":"b"}"#).unwrap();
    assert!(partial.args.is_empty());
    assert!(partial.enabled, "条目 enabled 缺省应为 true");
}

/// 类型级模式隔离(WP7):RoleplayPromptConfig/TaskPromptConfig 的 serde 线格式
/// 与旧 String/Option<String> 逐字节一致——裸字符串、null→None、缺字段→None、
/// None 序列化时省略字段(不落 null)。
#[test]
fn prompt_config_serde_wire_format_unchanged() {
    // roleplay 扁平字段:transparent = 裸字符串,与旧 String 线格式一致
    let rp = RoleplayPromptConfig("扮演人设词".into());
    assert_eq!(
        serde_json::to_string(&rp).unwrap(),
        "\"扮演人设词\"",
        "RoleplayPromptConfig 应序列化为裸字符串"
    );
    let back: RoleplayPromptConfig =
        serde_json::from_str("\"扮演人设词\"").expect("旧格式裸字符串应可反序列化");
    assert_eq!(back, rp);

    // task 覆盖层三态:缺字段 → None(沿用);null → None;"" → Some(空串,显式清空)
    let missing: ModeSettings = serde_json::from_str("{}").unwrap();
    assert!(
        missing.agent_system_prompt.is_none(),
        "缺字段应为 None(沿用)"
    );
    let null: ModeSettings = serde_json::from_str(r#"{"agent_system_prompt": null}"#).unwrap();
    assert!(
        null.agent_system_prompt.is_none(),
        "null 应为 None(与旧 Option<String> 一致)"
    );
    let empty: ModeSettings = serde_json::from_str(r#"{"agent_system_prompt": ""}"#).unwrap();
    assert_eq!(
        empty.agent_system_prompt,
        Some(TaskPromptConfig(String::new())),
        "空串应为 Some(\"\")(显式清空)"
    );
    let value: ModeSettings =
        serde_json::from_str(r#"{"agent_system_prompt": "任务专用"}"#).unwrap();
    assert_eq!(
        value.agent_system_prompt,
        Some(TaskPromptConfig("任务专用".into()))
    );

    // 序列化:None 省略字段(不落 null),Some 落裸字符串——写回线格式与旧版一致
    assert!(
        !serde_json::to_string(&missing)
            .unwrap()
            .contains("agent_system_prompt"),
        "None 应省略字段而不是落 null"
    );
    assert!(
        serde_json::to_string(&value)
            .unwrap()
            .contains(r#""agent_system_prompt":"任务专用""#),
        "Some 应落裸字符串"
    );

    // 完整 RuntimeSettings 旧格式往返:扁平字符串字段读入→写回,键与值类型不变
    let cfg = test_cfg();
    let dir = tmp_dir("prompt-wire");
    let mut legacy = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    legacy["agent_system_prompt"] = serde_json::Value::from("旧版人设词 {{char}}");
    legacy.as_object_mut().unwrap().remove("task");
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&legacy).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir, &cfg);
    assert_eq!(
        loaded.agent_system_prompt,
        RoleplayPromptConfig("旧版人设词 {{char}}".into()),
        "旧格式扁平字符串应原样读入"
    );
    assert!(loaded.task.agent_system_prompt.is_none());
    loaded.save(&dir).unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
    assert_eq!(
        written["agent_system_prompt"],
        serde_json::Value::from("旧版人设词 {{char}}"),
        "写回应保持裸字符串形态"
    );
    assert!(
        written["task"]
            .as_object()
            .unwrap()
            .get("agent_system_prompt")
            .is_none(),
        "空覆盖层写回不得新增 agent_system_prompt 键"
    );
}

/// 类型级模式隔离(WP7):for_mode(Roleplay) 恒等于扁平权威值;
/// for_mode(Task) 三分支——None 注入内置任务词 / Some("") 显式清空 / Some(v) 覆盖;
/// 且 roleplay 扁平值不受 task 覆盖层任何写动影响。
#[test]
fn for_mode_prompt_isolation_is_type_level_invariant() {
    let cfg = test_cfg();
    let mut s = RuntimeSettings::from_config(&cfg);
    s.agent_system_prompt = RoleplayPromptConfig("RP 人设词".into());

    // roleplay 恒等扁平值(task 覆盖层有无均不影响)
    assert_eq!(
        s.for_mode(AppMode::Roleplay).agent_system_prompt,
        RoleplayPromptConfig("RP 人设词".into())
    );
    s.task.agent_system_prompt = Some(TaskPromptConfig("TASK 覆盖词".into()));
    assert_eq!(
        s.for_mode(AppMode::Roleplay).agent_system_prompt,
        RoleplayPromptConfig("RP 人设词".into()),
        "task 覆盖层不得污染 roleplay 扁平值"
    );
    assert_eq!(
        s.for_mode(AppMode::Task).agent_system_prompt.0,
        "TASK 覆盖词",
        "Some(v) 应覆盖"
    );

    s.task.agent_system_prompt = Some(TaskPromptConfig(String::new()));
    assert_eq!(
        s.for_mode(AppMode::Task).agent_system_prompt.0,
        "",
        "Some(\"\") 显式清空"
    );

    s.task.agent_system_prompt = None;
    assert_eq!(
        s.for_mode(AppMode::Task).agent_system_prompt.0,
        default_task_agent_prompt(),
        "None 应注入内置任务向默认词,而非扁平 RP 人设词"
    );
}

/// TM-SET-2:两开关退役后,旧 settings.json 里残留的 `task_persona_full` /
/// `task_prompt_inject_enabled`(扁平与覆盖层)必须被 serde **静默忽略**——
/// 零迁移:不报错、不影响任何有效值,行为固定为现行默认(无档位、注入恒隔离)。
#[test]
fn retired_task_switches_are_ignored_on_load() {
    let cfg = test_cfg();
    let dir = tmp_dir("retired-task-switches");
    let mut legacy = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    let obj = legacy.as_object_mut().unwrap();
    // 旧扁平字段与旧覆盖层字段一并回灌(模拟升级前落盘的 settings.json)
    obj.insert("task_persona_full".into(), serde_json::json!(true));
    obj.insert("task_prompt_inject_enabled".into(), serde_json::json!(true));
    obj.insert(
        "task".into(),
        serde_json::json!({
            "task_persona_full": true,
            "task_prompt_inject_enabled": true
        }),
    );
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&legacy).unwrap(),
    )
    .unwrap();
    // 不 panic、不缺字段:两字段已不在线格式里,忽略后其余设置照常读入
    let loaded = RuntimeSettings::load(&dir, &cfg);
    assert_eq!(
        loaded.for_mode(AppMode::Task).task_default_connection_id,
        "",
        "退役字段不得影响其余设置读入"
    );
}

/// 编码能力包(2026-09-28):缺省关、旧配置零迁移、覆盖层可覆盖;
/// 它只影响**缺省**默认词——用户自定义值逐字优先,角色扮演模式不受影响。
#[test]
fn task_coding_bundle_selects_coding_default_prompt() {
    // 覆盖层 serde 三态:缺字段/null → None(沿用扁平);None 序列化省略字段
    let missing: ModeSettings = serde_json::from_str("{}").unwrap();
    assert!(
        missing.task_coding_bundle_enabled.is_none(),
        "缺字段应为 None(沿用扁平值)"
    );
    let null: ModeSettings =
        serde_json::from_str(r#"{"task_coding_bundle_enabled": null}"#).unwrap();
    assert!(null.task_coding_bundle_enabled.is_none(), "null 应为 None");
    let some: ModeSettings =
        serde_json::from_str(r#"{"task_coding_bundle_enabled": true}"#).unwrap();
    assert_eq!(some.task_coding_bundle_enabled, Some(true));

    let cfg = test_cfg();
    // 旧版 settings.json:无 task 覆盖层、无扁平字段 → 读入后缺省关,
    // 且任务缺省默认词仍是通用版(**零行为变更**是本批次的第一验收断言)
    let dir = tmp_dir("task-coding-bundle");
    let mut legacy = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    let obj = legacy.as_object_mut().unwrap();
    obj.remove("task");
    obj.remove("task_coding_bundle_enabled");
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&legacy).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir, &cfg);
    assert!(
        !loaded.task_coding_bundle_enabled,
        "旧配置缺省应为关(false)"
    );
    assert_eq!(
        loaded.for_mode(AppMode::Task).agent_system_prompt.0,
        default_task_agent_prompt(),
        "包关时任务缺省默认词必须是通用版"
    );

    // 包开(扁平):任务缺省词改用编码模板;roleplay 默认词不经此处,不受影响
    let mut s = RuntimeSettings::from_config(&cfg);
    s.task_coding_bundle_enabled = true;
    assert_eq!(
        s.for_mode(AppMode::Task).agent_system_prompt.0,
        default_coding_task_agent_prompt(),
        "包开时任务缺省默认词应为编码执行者模板"
    );
    assert_eq!(
        s.for_mode(AppMode::Roleplay).agent_system_prompt.0,
        s.agent_system_prompt.0,
        "角色扮演默认词不经本分支,包开关不得影响"
    );

    // 覆盖层分支:task 覆盖层 Some(true) 等价启用
    let mut s2 = RuntimeSettings::from_config(&cfg);
    s2.task.task_coding_bundle_enabled = Some(true);
    assert_eq!(
        s2.for_mode(AppMode::Task).agent_system_prompt.0,
        default_coding_task_agent_prompt(),
        "覆盖层 Some(true) 应等价启用"
    );

    // 用户自定义值逐字优先:包开也不覆盖用户填过的文本(开关只管缺省值)
    let mut s3 = RuntimeSettings::from_config(&cfg);
    s3.task_coding_bundle_enabled = true;
    s3.task.agent_system_prompt = Some(TaskPromptConfig("我的任务词".into()));
    assert_eq!(
        s3.for_mode(AppMode::Task).agent_system_prompt.0,
        "我的任务词",
        "用户自定义提示词必须逐字优先,开关只影响缺省值"
    );
}

/// 编码执行者模板的文本约束(与通用默认词同族,见 params.rs 函数文档的「约束」段)
#[test]
fn coding_default_prompt_keeps_task_constraints() {
    let p = default_coding_task_agent_prompt();
    assert!(!p.trim().is_empty(), "模板不得为空");
    assert!(p.contains("任务执行智能体"), "必须保留任务向身份字样:{p}");
    assert!(!p.contains("{{char}}"), "不得含角色扮演宏:{p}");
    assert!(
        p.contains("先落盘再补读"),
        "编码场景纪律(先落盘再补读,TMR-1)必须在模板里:{p}"
    );
    assert!(
        !p.contains("先读后写:动手前先摸清相关文件"),
        "旧的「先读后写」绝对句不得残留(TMR-1):{p}"
    );
    // CODE-3(2026-09-30):项目约定文件从「泛指」升为「点名」——五家编码 harness 的
    // 主路径都是点名机制(CLAUDE.md / AGENTS.md),只写「既有约定」模型会漏读
    assert!(
        p.contains("AGENTS.md") && p.contains("CLAUDE.md"),
        "项目约定文件应被点名(CODE-3):{p}"
    );
}

/// 文学能力包双开关骨架(LIT-1,2026-10-05;LIT-2 起任务侧开关在缺省词二选一处生效):
/// 默认关、旧配置零迁移、覆盖层三态、角色扮演侧不受任务开关影响。
#[test]
fn literary_bundle_switch_skeleton() {
    // 覆盖层 serde 三态:缺字段/null → None(沿用扁平);None 序列化省略字段
    let missing: ModeSettings = serde_json::from_str("{}").unwrap();
    assert!(
        missing.task_literary_bundle_enabled.is_none(),
        "缺字段应为 None(沿用扁平值)"
    );
    let null: ModeSettings =
        serde_json::from_str(r#"{"task_literary_bundle_enabled": null}"#).unwrap();
    assert!(
        null.task_literary_bundle_enabled.is_none(),
        "null 应为 None"
    );
    let some: ModeSettings =
        serde_json::from_str(r#"{"task_literary_bundle_enabled": true}"#).unwrap();
    assert_eq!(some.task_literary_bundle_enabled, Some(true));

    // 旧版 settings.json:无两开关键 → 读入后均为 false(零迁移)
    let cfg = test_cfg();
    let dir = tmp_dir("literary-bundle-skeleton");
    let mut legacy = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    let obj = legacy.as_object_mut().unwrap();
    obj.remove("literary_bundle_enabled");
    obj.remove("task_literary_bundle_enabled");
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&legacy).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir, &cfg);
    assert!(!loaded.literary_bundle_enabled, "旧配置缺省应为关(false)");
    assert!(
        !loaded.task_literary_bundle_enabled,
        "旧配置缺省应为关(false)"
    );
    assert_eq!(
        loaded.for_mode(AppMode::Task).agent_system_prompt.0,
        default_task_agent_prompt(),
        "开关全关时任务缺省词必须是通用版"
    );

    // 开关全开(扁平 + 覆盖层):任务侧缺省词改取文学变体(LIT-2 生效;优先级见下条用例)
    let mut s = RuntimeSettings::from_config(&cfg);
    s.literary_bundle_enabled = true;
    s.task_literary_bundle_enabled = true;
    s.task.task_literary_bundle_enabled = Some(true);
    assert_eq!(
        s.for_mode(AppMode::Task).agent_system_prompt.0,
        default_literary_task_agent_prompt(),
        "任务侧开关开 → 任务缺省词取文学变体(LIT-2)"
    );
    // 角色扮演侧全程不经任务开关:for_mode(Roleplay) 与扁平值逐字段相等
    assert_eq!(
        s.for_mode(AppMode::Roleplay).agent_system_prompt.0,
        s.agent_system_prompt.0,
        "角色扮演默认词不经任务开关"
    );
    assert_eq!(
        s.for_mode(AppMode::Roleplay).literary_bundle_enabled,
        s.literary_bundle_enabled
    );

    // 覆盖层三态:None 沿用扁平 / Some(false) 压过扁平 true / Some(true) 等价启用
    let mut s2 = RuntimeSettings::from_config(&cfg);
    s2.task_literary_bundle_enabled = true;
    assert!(
        s2.for_mode(AppMode::Task).task_literary_bundle_enabled,
        "None 应沿用扁平 true"
    );
    s2.task.task_literary_bundle_enabled = Some(false);
    assert!(
        !s2.for_mode(AppMode::Task).task_literary_bundle_enabled,
        "Some(false) 应压过扁平 true"
    );
    let mut s3 = RuntimeSettings::from_config(&cfg);
    s3.task.task_literary_bundle_enabled = Some(true);
    assert!(
        s3.for_mode(AppMode::Task).task_literary_bundle_enabled,
        "Some(true) 应等价启用"
    );
}

/// 文学包缺省词文本约束与优先级(LIT-2,2026-10-05):
/// ① 角色扮演文学增强版 = 现行缺省词 + 增强段(前缀逐字节相同,保「开关关逐字等同」);
/// ② 任务向变体含「任务执行智能体」、不含 `{{char}}`(既有测试锁同款约束);
/// ③ 优先级 coding > literary > general(新增开关不得改变既有开关用户的既有行为);
/// ④ 用户自定义值逐字优先(开关只影响缺省值);
/// ⑤ load 空值回填经唯一判据入口:开关开物化文学增强版、关则现行版(对照)。
#[test]
fn literary_default_prompts_text_constraints_and_priority() {
    // ① 宏占位符齐全 + 增强段存在 + 现行词为逐字节前缀
    let rp_lit = default_literary_roleplay_agent_prompt();
    for ph in [
        "{{char}}",
        "{{personality}}",
        "{{scenario}}",
        "{{world_info}}",
    ] {
        assert!(rp_lit.contains(ph), "文学增强版必须保留占位符 {ph}");
    }
    assert!(rp_lit.contains("【文学增强纪律】"), "应含增强段标题");
    assert!(
        rp_lit.starts_with(&default_roleplay_agent_prompt()),
        "文学增强版 = 现行缺省词前缀 + 增强段"
    );

    // ② 任务向变体约束(与 default_task_agent_prompt 同款测试锁)
    let task_lit = default_literary_task_agent_prompt();
    assert!(
        task_lit.contains("任务执行智能体"),
        "任务向变体必须保留身份字样:{task_lit}"
    );
    assert!(!task_lit.contains("{{char}}"), "任务向变体不得含角色扮演宏");
    assert!(task_lit.contains("【文学写作任务增补】"));

    // ③ 优先级:coding 与 literary 同开 → 编码模板优先;仅文学开 → 文学变体
    let cfg = test_cfg();
    let mut s = RuntimeSettings::from_config(&cfg);
    s.task_coding_bundle_enabled = true;
    s.task_literary_bundle_enabled = true;
    assert_eq!(
        s.for_mode(AppMode::Task).agent_system_prompt.0,
        default_coding_task_agent_prompt(),
        "两包同开时编码模板优先(既有开关用户行为不变)"
    );
    s.task_coding_bundle_enabled = false;
    assert_eq!(
        s.for_mode(AppMode::Task).agent_system_prompt.0,
        default_literary_task_agent_prompt(),
        "仅文学包开 → 任务缺省词取文学变体"
    );

    // ④ 用户自定义值逐字优先(开关只影响缺省值)
    s.task.agent_system_prompt = Some(TaskPromptConfig("我的文学任务词".into()));
    assert_eq!(
        s.for_mode(AppMode::Task).agent_system_prompt.0,
        "我的文学任务词",
        "用户自定义提示词必须逐字优先"
    );

    // ⑤ load 空值回填走唯一判据入口(存量缺省路径)
    let dir = tmp_dir("literary-default-backfill");
    let mut v = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    {
        let obj = v.as_object_mut().unwrap();
        obj.insert("agent_system_prompt".into(), serde_json::json!(""));
        obj.insert("literary_bundle_enabled".into(), serde_json::json!(true));
    }
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir, &cfg);
    assert_eq!(
        loaded.agent_system_prompt.0,
        default_literary_roleplay_agent_prompt(),
        "开关开 + 空提示词 → 回填文学增强版"
    );

    // 对照:同一份配置开关关 → 现行版逐字(开关关逐字等同的回归钉子)
    v.as_object_mut()
        .unwrap()
        .insert("literary_bundle_enabled".into(), serde_json::json!(false));
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();
    let loaded2 = RuntimeSettings::load(&dir, &cfg);
    assert_eq!(
        loaded2.agent_system_prompt.0,
        default_roleplay_agent_prompt(),
        "开关关 → 回填现行缺省词(逐字不变)"
    );
}

/// 说明头随内容走(遗留 L32):内置默认角色扮演模板不再字面写「世界书(当前已命中的设定,…):」
/// 那句——该头由引擎在 `{{world_info}}` 展开处拥有,无常驻条目时不会留下句下空转的强制语气句。
/// 占位符本身必须保留(常驻条目仍靠它注入)。
#[test]
fn roleplay_default_prompt_does_not_write_world_header_line() {
    let p = default_roleplay_agent_prompt();
    assert!(
        !p.contains("世界书(当前已命中的设定"),
        "默认模板不应再字面写常驻说明头: {p}"
    );
    assert!(p.contains("{{world_info}}"), "占位符必须保留");
    assert!(
        !default_literary_roleplay_agent_prompt().contains("世界书(当前已命中的设定"),
        "文学增强版由默认版拼接而来,同样不得含该句"
    );
}

/// 存量迁移(遗留 L32 配套):load 期逐字等于**旧**内置默认(或其文学版)者视为未自定义,
/// 重物化为现行默认——否则旧串里那句字面头会与引擎新头同屏成双头,而这批用户从未自定义过文本。
/// 追加过内容的自定义文本必须逐字保留(不得被迁移覆盖);空串走既有回填分支,本迁移不动。
#[test]
fn legacy_roleplay_default_prompt_rehydrates_on_load() {
    let cfg = test_cfg();

    // 分支一:逐字旧默认(开关关)→ 现行默认
    let dir_plain = tmp_dir("rp-prompt-legacy-plain");
    let mut j = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    j["agent_system_prompt"] = serde_json::Value::from(LEGACY_ROLEPLAY_AGENT_PROMPT_V1);
    j["literary_bundle_enabled"] = serde_json::Value::from(false);
    std::fs::write(
        dir_plain.join("settings.json"),
        serde_json::to_string_pretty(&j).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir_plain, &cfg);
    assert_eq!(
        loaded.agent_system_prompt.0,
        default_roleplay_agent_prompt(),
        "逐字旧默认应重物化为现行默认"
    );

    // 分支二:逐字旧文学版出厂文本(旧默认 + 文学增强段)+ 开关开 → 现行文学版默认
    let dir_lit = tmp_dir("rp-prompt-legacy-literary");
    let legacy_literary = format!(
        "{}\n\n{}",
        LEGACY_ROLEPLAY_AGENT_PROMPT_V1, LEGACY_LITERARY_RP_ENHANCEMENT_V1
    );
    let mut j = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    j["agent_system_prompt"] = serde_json::Value::from(legacy_literary);
    j["literary_bundle_enabled"] = serde_json::Value::from(true);
    std::fs::write(
        dir_lit.join("settings.json"),
        serde_json::to_string_pretty(&j).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir_lit, &cfg);
    assert_eq!(
        loaded.agent_system_prompt.0,
        default_literary_roleplay_agent_prompt(),
        "旧文学版出厂文本应按当前开关重物化"
    );

    // 分支三:在旧默认上追加过内容 = 自定义,逐字保留
    let dir_custom = tmp_dir("rp-prompt-legacy-custom");
    let custom = format!("{LEGACY_ROLEPLAY_AGENT_PROMPT_V1}\n我的追加规则");
    let mut j = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    j["agent_system_prompt"] = serde_json::Value::from(custom.clone());
    std::fs::write(
        dir_custom.join("settings.json"),
        serde_json::to_string_pretty(&j).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir_custom, &cfg);
    assert_eq!(
        loaded.agent_system_prompt.0, custom,
        "自定义文本(哪怕以旧默认为前缀)不得被迁移覆盖"
    );
}

/// 新装(含 Android 首装)角色扮演默认提示词不再为空:from_config 直接物化 Win 端正用版,
/// 且必须保留四个宏占位符(宏系统按角色展开;与任务默认词「不得含 {{char}}」相反)。
#[test]
fn roleplay_default_prompt_is_materialized_on_new_install() {
    let cfg = test_cfg();
    let s = RuntimeSettings::from_config(&cfg);
    let p = &s.agent_system_prompt.0;
    assert!(
        !p.trim().is_empty(),
        "新装默认提示词不得为空(Android 首装同源)"
    );
    for ph in [
        "{{char}}",
        "{{personality}}",
        "{{scenario}}",
        "{{world_info}}",
    ] {
        assert!(p.contains(ph), "默认词必须保留占位符 {ph} 供宏系统展开");
    }
    // 与引擎内置兜底模板同源的标志性段落,便于维护者识别两份文本的对应关系
    for seg in [
        "【创作总纲】",
        "【角色扮演规则】",
        "【写作要求】",
        "【剧情结构(模块化)】",
        "【输出纪律】",
    ] {
        assert!(p.contains(seg), "默认词应含段落 {seg}");
    }
}

/// 已存在 settings.json 且该字段为空(此前安装/用户显式清空后未改)→ load 物化内置默认;
/// 用户已保存的非空文本不得被覆盖(文件值优先)。
#[test]
fn load_backfills_empty_roleplay_prompt_and_keeps_custom_text() {
    let cfg = test_cfg();

    // 分支一:空串 → 回填内置默认
    let dir_empty = tmp_dir("rp-prompt-backfill");
    let mut empty = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    empty["agent_system_prompt"] = serde_json::Value::from("");
    std::fs::write(
        dir_empty.join("settings.json"),
        serde_json::to_string_pretty(&empty).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir_empty, &cfg);
    assert_eq!(
        loaded.agent_system_prompt.0,
        default_roleplay_agent_prompt(),
        "空值应回填内置默认(与首装一致)"
    );

    // 分支二:用户自定义非空文本 → 原样保留,不得被默认词覆盖
    let dir_custom = tmp_dir("rp-prompt-custom");
    let mut custom = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    custom["agent_system_prompt"] = serde_json::Value::from("CUSTOM-RP-MARK 用户自己的提示词");
    std::fs::write(
        dir_custom.join("settings.json"),
        serde_json::to_string_pretty(&custom).unwrap(),
    )
    .unwrap();
    let loaded_custom = RuntimeSettings::load(&dir_custom, &cfg);
    assert_eq!(
        loaded_custom.agent_system_prompt.0, "CUSTOM-RP-MARK 用户自己的提示词",
        "用户非空文本优先,回填不得覆盖"
    );
}

/// 反思提示词文学变体与判定协议(LIT-4,2026-10-06;RPFLOW-1 随默认词改版同步):
/// ① 判据入口两态:开关关 → 逐字等于现行版;开关开 → 现行文本为逐字节前缀 + 追加检查项;
/// ② 判定协议不破:判定标记与「不把修改写进正文」纪律逐字保留(反思解析侧既有断言依赖)。
#[test]
fn literary_reflect_prompt_variant_and_protocol() {
    let cfg = test_cfg();
    let mut s = RuntimeSettings::from_config(&cfg);

    assert_eq!(
        s.reflect_default_prompt(),
        default_reflect_prompt(),
        "开关关 → 反思缺省词逐字等于现行版"
    );

    s.literary_bundle_enabled = true;
    let lit = s.reflect_default_prompt();
    assert!(
        lit.starts_with(&default_reflect_prompt()),
        "文学版 = 现行反思提示词前缀 + 追加检查项(开关关逐字等同由前缀保证)"
    );
    for item in ["文风漂移", "复读", "代答", "时间线矛盾"] {
        assert!(lit.contains(item), "文学版应含追加检查项:{item}");
    }
    // ② 协议与纪律逐字保留(RPFLOW-1 改版后的判定协议;解析侧只认标记行)
    for seg in [
        "判定词只能单独成行(PASS / 通过 / FAIL / 不通过)",
        "不要整段重写,也不要直接删词",
        "不要把修改过程或审查意见写进正文",
    ] {
        assert!(lit.contains(seg), "文学版不得改动判定协议:{seg}");
    }
    // ③ 追加段末尾的协议重申必须与现行版**同口径**(2026-10-09 修复批次):
    // 旧句「只判定不重写」与现行版「优先用工具做最小必要的定点修改」正面矛盾,
    // 而该句位于 system 末尾(注意力最强位)——文学包用户每轮反思都吃这两句互斥纪律。
    assert!(
        !lit.contains("只判定不重写"),
        "追加段不得再出现旧版「只判定不重写」纪律(与现行版的工具定点修改纪律冲突)"
    );
    assert!(
        lit.trim_end()
            .ends_with("判定词单独成行输出 PASS / 通过 / FAIL / 不通过,不达标时用一句话指出主要问题(可用上述新增标签)。"),
        "追加检查项后应以协议重申收尾,防稀释「输出协议在末尾」的注意力位置"
    );
}

/// 反思提示词「未自定义」判据 = **逐字等于内置默认文本**(LIT-4)。
///
/// 该字段的**空串是「显式关闭反思、回退机械规则」的有效值**(见 `api/settings.rs` 同名注释),
/// 故不能照抄 `agent_system_prompt` 的「空串即未自定义」回填——此处逐字比对两个内置版本:
/// ① 存量默认文本 + 开关开 → 重物化为文学版;② + 开关关 → 逐字仍是现行版;
/// ③ 存量文学版文本 + 开关关 → 回落现行版;④ 自定义文本 / 空串 → 两侧都不动。
#[test]
fn load_rehydrates_reflect_prompt_only_when_verbatim_builtin() {
    let cfg = test_cfg();
    let write = |dir: &std::path::Path, prompt: &str, on: bool| {
        let mut v = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.insert("reflect_prompt".into(), serde_json::json!(prompt));
        obj.insert("literary_bundle_enabled".into(), serde_json::json!(on));
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(&v).unwrap(),
        )
        .unwrap();
        RuntimeSettings::load(dir, &cfg)
    };

    // ① 存量默认文本 + 开关开 → 重物化为文学版
    let dir = tmp_dir("reflect-lit-rehydrate");
    let got = write(&dir, &default_reflect_prompt(), true);
    assert_eq!(
        got.reflect_prompt,
        default_literary_reflect_prompt(),
        "存量默认文本 + 开关开 → 重物化为文学版"
    );
    // ② 存量默认文本 + 开关关 → 逐字不变(回归钉子)
    let got = write(&dir, &default_reflect_prompt(), false);
    assert_eq!(
        got.reflect_prompt,
        default_reflect_prompt(),
        "开关关 → 逐字等同现行版"
    );
    // ③ 存量文学版文本 + 开关关 → 回落现行版(关包不得留在文学文本上)
    let got = write(&dir, &default_literary_reflect_prompt(), false);
    assert_eq!(
        got.reflect_prompt,
        default_reflect_prompt(),
        "关包应回落现行版"
    );
    // ④ 自定义文本 / 空串:两侧都不动
    let got = write(&dir, "CUSTOM-REFLECT-MARK 我的判定词", true);
    assert_eq!(
        got.reflect_prompt, "CUSTOM-REFLECT-MARK 我的判定词",
        "自定义文本不得被覆盖"
    );
    let got = write(&dir, "", true);
    assert_eq!(got.reflect_prompt, "", "空串 = 显式关闭反思,不得回填");
}

/// RPFLOW-1 + 2026-10-09 修复批次:旧版默认反思提示词(v1,或 v1/RPFLOW-1 窗口期
/// 与旧文学增强段拼成的文学版)迁移——存量 settings.json 里逐字等于这些历史文本者,
/// 升级后自动重物化为当前默认(否则存量安装永远停留在「只判定不重写」的旧文本上,
/// 新流程的批判修改纪律只对新装生效);自定义逐字优先。
#[test]
fn load_migrates_legacy_reflect_prompt() {
    let cfg = test_cfg();
    let write = |dir: &std::path::Path, prompt: &str, on: bool| {
        let mut v = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.insert("reflect_prompt".into(), serde_json::json!(prompt));
        obj.insert("literary_bundle_enabled".into(), serde_json::json!(on));
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(&v).unwrap(),
        )
        .unwrap();
        RuntimeSettings::load(dir, &cfg)
    };
    let dir = tmp_dir("reflect-legacy-migrate");
    // ① 旧版默认 + 开关关 → 迁到新版默认
    let got = write(&dir, LEGACY_REFLECT_PROMPT_V1, false);
    assert_eq!(
        got.reflect_prompt,
        default_reflect_prompt(),
        "旧版默认应迁移为新版默认"
    );
    // ② 旧版文学版(v1 默认词 + v1 文学增强段)+ 开关开 → 迁到新版文学默认
    let legacy_lit = format!(
        "{}\n\n{}",
        LEGACY_REFLECT_PROMPT_V1, LEGACY_LITERARY_REFLECT_ENHANCEMENT_V1
    );
    let got = write(&dir, &legacy_lit, true);
    assert_eq!(
        got.reflect_prompt,
        default_literary_reflect_prompt(),
        "旧版文学默认应迁移为新版文学默认"
    );
    // ③ RPFLOW-1 窗口的文学版(现行默认词 + v1 文学增强段)+ 开关开 → 同样重物化
    // (2026-10-09 修复批次:该组合的末尾是旧句「只判定不重写」,不迁移就永久自相矛盾)
    let rpflow1_lit = format!(
        "{}\n\n{}",
        default_reflect_prompt(),
        LEGACY_LITERARY_REFLECT_ENHANCEMENT_V1
    );
    let got = write(&dir, &rpflow1_lit, true);
    assert_eq!(
        got.reflect_prompt,
        default_literary_reflect_prompt(),
        "RPFLOW-1 窗口的文学默认应迁移为修复后的文学默认"
    );
    // ④ 自定义文本 → 逐字优先,不迁移
    let got = write(&dir, "CUSTOM-LEGACY-CHECK", true);
    assert_eq!(
        got.reflect_prompt, "CUSTOM-LEGACY-CHECK",
        "自定义文本不得被迁移覆盖"
    );
}

/// RPFLOW-2:剧情推演词条同步两档开关——默认值(角色档 true / 全局档 false)与落盘往返。
#[test]
fn worldbook_sync_switches_defaults_and_roundtrip() {
    let cfg = test_cfg();
    let s = RuntimeSettings::from_config(&cfg);
    assert!(s.worldbook_sync_character_enabled, "角色档默认开启");
    assert!(!s.worldbook_sync_global_enabled, "全局档默认关闭");
    // 往返:写入 false/true → 重载还原(缺字段回默认的路径由 serde default 保证)
    let dir = tmp_dir("worldbook-sync-roundtrip");
    let mut v = serde_json::to_value(&s).unwrap();
    let obj = v.as_object_mut().unwrap();
    obj.insert(
        "worldbook_sync_character_enabled".into(),
        serde_json::json!(false),
    );
    obj.insert(
        "worldbook_sync_global_enabled".into(),
        serde_json::json!(true),
    );
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();
    let got = RuntimeSettings::load(&dir, &cfg);
    assert!(!got.worldbook_sync_character_enabled, "关掉角色档应还原");
    assert!(got.worldbook_sync_global_enabled, "打开全局档应还原");
}

/// 文风预设判据与选择型字段归一化(LIT-6,2026-10-06):
/// ① 新装默认:两个选择型字段为空(不注入 / 不改变),快照为 None;
/// ② 判据入口三态:开关关 → None(即使选了档);开关开 + 空 → None;开关开 + 命中档 → Some(档文本);
/// ③ 合法性判据:空与四档键合法、其余非法(API 校验与 load 归一化**共用同一判据**);
/// ④ load 归一化:存量未知取值清空、合法值原样保留。
#[test]
fn literary_style_preset_judgement_and_load_normalization() {
    let cfg = test_cfg();
    let mut s = RuntimeSettings::from_config(&cfg);

    // ① 新装默认
    assert_eq!(s.literary_style_preset, "");
    assert_eq!(s.literary_recommend_preset, "");
    assert!(s.literary_recommend_snapshot.is_none());

    // ② 判据入口三态
    s.literary_style_preset = "plain".into();
    assert!(s.literary_style_note().is_none(), "开关关 → 不注入");
    s.literary_bundle_enabled = true;
    s.literary_style_preset = String::new();
    assert!(s.literary_style_note().is_none(), "空 = 不注入");
    s.literary_style_preset = "hardboiled".into();
    let text = s.literary_style_note().expect("命中档应注入");
    assert!(text.contains("悬疑冷硬"), "应取到「悬疑冷硬」档:{text}");
    assert!(
        text.contains("忌讳"),
        "档文本应含忌讳条(可编辑常量的一部分)"
    );

    // ③ 合法性判据
    for ok in ["", "plain", "classical", "lightnovel", "hardboiled"] {
        assert!(is_valid_literary_style_preset(ok), "{ok} 应合法");
    }
    for bad in ["noir", "PLAIN", " plain", "plain "] {
        assert!(!is_valid_literary_style_preset(bad), "{bad:?} 应非法");
    }

    // ④ load 归一化
    let dir = tmp_dir("lit-style-normalize");
    let cfg2 = test_cfg();
    let mut v = serde_json::to_value(RuntimeSettings::from_config(&cfg2)).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("literary_style_preset".into(), serde_json::json!("noir"));
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir, &cfg2);
    assert_eq!(loaded.literary_style_preset, "", "存量未知档应归一化为空串");

    v.as_object_mut().unwrap().insert(
        "literary_style_preset".into(),
        serde_json::json!("classical"),
    );
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();
    let loaded2 = RuntimeSettings::load(&dir, &cfg2);
    assert_eq!(
        loaded2.literary_style_preset, "classical",
        "合法值必须原样保留"
    );
}

/// 推荐档数值与合法性(LIT-7):两档取值固定、且全部落在 `secret.rs` 的既有钳制区间内
/// (不新增校验口径);空与两档键合法、其余非法;load 归一化同款。
#[test]
fn literary_recommend_preset_values_and_validation() {
    let medium = literary_recommend_values("medium").expect("中篇档应在清单里");
    assert_eq!(
        (
            medium.compaction_mode,
            medium.compaction_threshold,
            medium.compaction_keep_recent
        ),
        ("auto", 0.75, 6)
    );
    let long = literary_recommend_values("long").expect("长篇档应在清单里");
    assert_eq!(
        (
            long.compaction_mode,
            long.compaction_threshold,
            long.compaction_keep_recent
        ),
        ("auto", 0.7, 8)
    );
    for k in ["medium", "long"] {
        let v = literary_recommend_values(k).unwrap();
        assert!(
            matches!(v.compaction_mode, "off" | "manual" | "auto"),
            "{k} 的压缩模式取值非法"
        );
        assert!(
            (0.5..=0.95).contains(&v.compaction_threshold),
            "{k} 阈值越界"
        );
        assert!(
            (2..=200).contains(&v.compaction_keep_recent),
            "{k} 保留条数越界"
        );
    }
    assert!(literary_recommend_values("").is_none(), "空键不是档");
    assert!(is_valid_literary_recommend_preset(""));
    assert!(is_valid_literary_recommend_preset("long"));
    assert!(!is_valid_literary_recommend_preset("epic"));

    // load 归一化:存量未知档清空(合法值与空串见上)
    let dir = tmp_dir("lit-recommend-normalize");
    let cfg = test_cfg();
    let mut v = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
    v.as_object_mut().unwrap().insert(
        "literary_recommend_preset".into(),
        serde_json::json!("epic"),
    );
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();
    let loaded = RuntimeSettings::load(&dir, &cfg);
    assert_eq!(
        loaded.literary_recommend_preset, "",
        "存量未知档应归一化为空串"
    );
}

/// 模式隔离不因新增角色扮演默认词而退化:
/// task 缺省仍是任务向默认词(不含角色扮演宏),roleplay 默认词含宏;两者互不污染。
#[test]
fn roleplay_default_does_not_leak_into_task_default() {
    let cfg = test_cfg();
    let s = RuntimeSettings::from_config(&cfg);
    let task_default = s.for_mode(AppMode::Task).agent_system_prompt.0;
    assert_eq!(
        task_default,
        default_task_agent_prompt(),
        "task 缺省仍是任务向默认词"
    );
    assert!(
        !task_default.contains("{{char}}"),
        "task 默认词不得继承角色扮演宏"
    );
    assert!(
        s.for_mode(AppMode::Roleplay)
            .agent_system_prompt
            .0
            .contains("{{char}}"),
        "roleplay 默认词应含宏占位符"
    );
}

/// 看守下限的两个来源:命令上限 + 模型调用上限 + 1(直接引用引擎那个常量,不留第二份 300)。
#[test]
fn task_idle_floor_is_derived_from_both_timeouts() {
    let call_secs = crate::agents::engine::executor::TASK_TOOL_LOOP_CALL_TIMEOUT.as_secs();
    assert_eq!(
        task_idle_floor_secs() as u64,
        crate::services::exec::MAX_TIMEOUT_MS / 1_000 + call_secs + 1,
        "看守下限必须等于「单条命令上限 + 单次模型调用上限 + 1」"
    );
}

/// 批次 2(HARNESS3-4)的耦合钉子:**两个任务闸门的默认值都要容得下
/// 「一条跑满上限的命令 + 一次模型调用」**。
///
/// 为什么锁默认值而不是锁行为:两处闸门都在**轮末 / 无心跳**时判定,而 bash 上限抬到
/// 1800s 后,旧默认值(看守 900 / 步骤预算 1200)会分别在「命令跑到一半被自动收尾」与
/// 「自测跑完即收尾、没有下一轮去修」两处把自测闭环打死——后者尤其隐蔽,因为命令本身
/// 没被截断(预算在轮末判),丢的是后续轮次。
#[test]
fn task_gate_defaults_cover_one_full_command_plus_a_call() {
    let worst_silence = crate::services::exec::MAX_TIMEOUT_MS / 1_000
        + crate::agents::engine::executor::TASK_TOOL_LOOP_CALL_TIMEOUT.as_secs()
        + 1;
    let idle_default = default_task_idle_timeout_secs() as u64;
    assert!(
        idle_default >= task_idle_floor_secs() as u64,
        "看守默认 {idle_default}s 低于自己的下限 {}s:首装即被 load 钳制改写",
        task_idle_floor_secs()
    );
    assert!(
        idle_default >= worst_silence,
        "看守默认 {idle_default}s 容不下一条满时长命令 + 一次模型调用({worst_silence}s)"
    );
    let step_budget = default_task_step_budget_secs() as u64;
    assert!(
        step_budget >= worst_silence,
        "步骤预算默认 {step_budget}s 容不下一条满时长命令 + 一次模型调用({worst_silence}s):\
         自测闭环会退化成一轮抽签"
    );
    // 注册表侧整工具超时由同一常量派生 + 收尾余量:它只需盖住**命令本身**
    // (模型调用不在这条 future 里,不能拿 worst_silence 来要求它)。
    // 写死 330s 的旧形态下,命令上限一旦抬高,注册表会先掐断——长命令表现为
    // 「工具超时」而不是「命令超时」,exec 层的强杀与审计路径全部失效。
    let registry_allows = crate::services::exec::MAX_TIMEOUT_MS + 30_000;
    assert!(
        registry_allows > crate::services::exec::MAX_TIMEOUT_MS,
        "bash 的注册表超时须留出收尾余量: {registry_allows}"
    );
}
