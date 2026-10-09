// 设置路由:/api/settings(连接测试/模型列表/信息/模型切换/运行期设置读写)
use crate::api::app_state::AppState;
use crate::api::json_body::{JsonBody, JsonBodyOpt};
use crate::api::{err_with_code, internal, not_found, validation, ErrorCode};
use crate::connectors::openai_compatible::{normalize_api_style, API_STYLE_CHAT};
use crate::services::agent_flow_service::{
    MAX_FLOW_CALLS_PER_TASK_LIMIT, MAX_FLOW_CALL_DEPTH_LIMIT, MIN_FLOW_CALLS_PER_TASK,
    MIN_FLOW_CALL_DEPTH,
};
use crate::services::settings_service::{
    is_valid_literary_recommend_preset, is_valid_literary_style_preset, literary_recommend_values,
    normalize_base_url, resolve_connector_target, task_idle_floor_secs, AppMode, ConnectionProfile,
    LiteraryRecommendSnapshot, McpServerConfig, RuntimeSettings, CONNECTOR_TYPE_MOCK,
    CONNECTOR_TYPE_OPENAI, DEFAULT_SEARCH_ENDPOINT, MAX_CONNECTIONS,
};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct SwitchModelBody {
    pub model: String,
}

/// 设置读写按模式路由:?mode=roleplay|task(缺省 = roleplay,兼容旧客户端)。
#[derive(Deserialize, Default)]
pub struct ModeQuery {
    #[serde(default)]
    pub mode: Option<String>,
}

impl ModeQuery {
    fn app_mode(&self) -> AppMode {
        AppMode::parse(self.mode.as_deref().unwrap_or("roleplay"))
    }
}

#[derive(Deserialize, Default)]
pub struct UpdateSettingsBody {
    #[serde(default)]
    pub openai_base_url: Option<String>,
    /// 非空才替换;空/缺省表示保持不变
    #[serde(default)]
    pub openai_api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// 多套连接(全量数组语义:数组里没有的 id 即删除;api_key 空/缺省 = 保持该连接原有密钥)
    #[serde(default)]
    pub connections: Option<Vec<ConnectionProfileInput>>,
    /// 默认连接 id(空串 = 清除,由 normalize_connections 回退到第一个启用连接)
    #[serde(default)]
    pub active_connection_id: Option<String>,
    #[serde(default)]
    pub default_temperature: Option<f64>,
    #[serde(default)]
    pub default_top_p: Option<f64>,
    #[serde(default)]
    pub default_max_tokens: Option<u32>,
    #[serde(default)]
    pub max_context_tokens: Option<u32>,
    /// Agent 系统提示词(空 = 用内置默认);设置里编辑,支持占位符
    #[serde(default)]
    pub agent_system_prompt: Option<String>,
    /// 搜索端点(空 = 用默认 DuckDuckGo HTML 接口)
    #[serde(default)]
    pub search_endpoint: Option<String>,
    /// mvu 变量状态注入位置(system / user_tail;非法值忽略)
    #[serde(default)]
    pub mvu_vars_position: Option<String>,
    /// 反思提示词(空 = 机械规则检查;非空 = LLM 反思;允许清空回退规则检查)
    #[serde(default)]
    pub reflect_prompt: Option<String>,
    /// 复杂模式预设尾部提示词(空 = 禁用;允许清空)
    #[serde(default)]
    pub preset_tail_prompt: Option<String>,
    /// 预设尾部提示词注入角色(user / assistant)
    #[serde(default)]
    pub preset_tail_role: Option<String>,
    /// 反思失败建议提示词(空 = 禁用;允许清空)
    #[serde(default)]
    pub reflect_advice_prompt: Option<String>,
    /// 反思失败建议提示词注入角色(user / assistant)
    #[serde(default)]
    pub reflect_advice_role: Option<String>,
    /// 旧放行模式开关(deprecated;true → bypass,false → strict)
    #[serde(default)]
    pub bypass_mode: Option<bool>,
    /// 授权模式三档(strict / loose / bypass);优先于旧 bypass_mode
    #[serde(default)]
    pub authorization_mode: Option<String>,
    /// 「始终需授权」清单
    #[serde(default)]
    pub bypass_blacklist: Option<Vec<String>>,
    /// 授权等待超时(秒;30..=1800)
    #[serde(default)]
    pub tool_authorization_timeout_secs: Option<u32>,
    /// 任务模式工具策略(all / deny_dangerous / allowlist)
    #[serde(default)]
    pub task_tool_policy: Option<String>,
    /// 任务模式工具白名单(allowlist 策略生效)
    #[serde(default)]
    pub task_tool_allowlist: Option<Vec<String>>,
    /// AGENT/CUSTOM 模式工具循环轮次上限(1..=200;缺省保持不变)
    #[serde(default)]
    pub max_tool_rounds: Option<u32>,
    /// 流程动态调用深度上限(A 批 A3;1..=5)
    #[serde(default)]
    pub max_flow_call_depth: Option<u32>,
    /// 每任务流程调用次数上限(A 批 A3;1..=64)
    #[serde(default)]
    pub max_flow_calls_per_task: Option<u32>,
    /// 节点默认上下文上限(A 批 A4;0 = 不裁剪,否则 256..=1048576)
    #[serde(default)]
    pub default_node_max_context: Option<u32>,
    /// HTML 渲染开关(状态栏脚本执行前置条件)
    #[serde(default)]
    pub render_html: Option<bool>,
    /// 上下文压缩模式(off / manual / auto;非法值忽略)
    #[serde(default)]
    pub compaction_mode: Option<String>,
    /// 上下文压缩触发阈值(0.5..=0.95)
    #[serde(default)]
    pub compaction_threshold: Option<f32>,
    /// 压缩后保留的最近消息条数(2..=200)
    #[serde(default)]
    pub compaction_keep_recent: Option<u32>,
    /// snip 零成本裁剪的消息长度阈值(字节;0 = 禁用,上限 1MB)
    #[serde(default)]
    pub compaction_snip_bytes: Option<u32>,
    /// LLM 请求快照开关(第四点·主题 A)
    #[serde(default)]
    pub llm_request_log: Option<bool>,
    /// 跨会话记忆蒸馏开关(落地项 2)
    #[serde(default)]
    pub memory_distill_enabled: Option<bool>,
    /// 记忆槽注入条数上限(0..=50;0 = 关闭注入)
    #[serde(default)]
    pub memory_inject_limit: Option<u32>,
    /// 记忆槽字符预算(0..=20000;0 = 不限制)
    #[serde(default)]
    pub memory_inject_char_budget: Option<u32>,
    /// 每角色记忆容量上限(0..=10000;0 = 不淘汰)
    #[serde(default)]
    pub memory_max_entries: Option<u32>,
    /// 剧情推演词条同步·角色档(RPFLOW-2;默认 true)
    #[serde(default)]
    pub worldbook_sync_character_enabled: Option<bool>,
    /// 剧情推演词条同步·全局档(RPFLOW-2;默认 false)
    #[serde(default)]
    pub worldbook_sync_global_enabled: Option<bool>,
    /// 向量化开关(开启后记忆写入生成向量、召回走混合打分)
    #[serde(default)]
    pub embedding_enabled: Option<bool>,
    /// embedding 服务地址(OpenAI 兼容 /embeddings)
    #[serde(default)]
    pub embedding_base_url: Option<String>,
    /// embedding API Key(空 = 不变更;与聊天 Key 同策略加密存储)
    #[serde(default)]
    pub embedding_api_key: Option<String>,
    /// embedding 模型名
    #[serde(default)]
    pub embedding_model: Option<String>,
    /// 向量维度(0 = 自动探测)
    #[serde(default)]
    pub embedding_dim: Option<u32>,
    /// 技能渐进披露开关(true = system 只注入「name:description」清单,正文按需 read)
    #[serde(default)]
    pub skill_progressive_disclosure: Option<bool>,
    /// 回退快照开关(批次 6.1;true = 写工具执行前留快照,可回退)
    #[serde(default)]
    pub undo_enabled: Option<bool>,
    /// 子智能体最大嵌套深度(1..=4)
    #[serde(default)]
    pub subagent_max_depth: Option<u32>,
    /// 子智能体最大并发数(1..=16)
    #[serde(default)]
    pub subagent_max_concurrency: Option<u32>,
    /// 子智能体结果最大字符数(500..=8000,超出截断带尾注)
    #[serde(default)]
    pub subagent_result_max_chars: Option<u32>,
    /// MCP stdio 客户端总开关(批次 6.2;仅启动时装配,改后重启生效)
    #[serde(default)]
    pub mcp_enabled: Option<bool>,
    /// MCP 服务器列表(全量替换语义,与 bypass_blacklist 一致)
    #[serde(default)]
    pub mcp_servers: Option<Vec<McpServerConfig>>,
    /// 命令执行总开关(阶段 E;缺省保持不变)。默认关闭。
    #[serde(default)]
    pub exec_enabled: Option<bool>,
    /// Android 允许 ROOT 档(缺省保持不变)。默认关闭。
    #[serde(default)]
    pub exec_allow_root: Option<bool>,
    /// Android 允许 Shizuku 档(缺省保持不变)。默认关闭。
    #[serde(default)]
    pub exec_allow_shizuku: Option<bool>,
    /// Android 允许沙箱档(缺省保持不变)。默认关闭。
    #[serde(default)]
    pub exec_allow_sandbox: Option<bool>,
    /// 「视觉与截图」总开关(2026-10-02 视觉能力包 D5;缺省保持不变)。默认关闭;
    /// 全局扁平字段(与 exec_* 同口径:能力开关是进程级事实,不分模式)。
    #[serde(default)]
    pub vision_screenshot_enabled: Option<bool>,
    /// 编码能力包开关(2026-09-28):true = 任务模式缺省默认词用编码执行者模板;
    /// false = 通用任务默认词(默认);缺省保持不变。用户自定义提示词逐字优先。
    #[serde(default)]
    pub task_coding_bundle_enabled: Option<bool>,
    /// 文学能力包开关(2026-10-05,LIT-1;缺省保持不变)。默认关闭。角色扮演侧为纯扁平
    /// 字段(该侧无覆盖层,即使请求带 mode=task 也直写扁平);生效面见 LIT-2/LIT-3。
    #[serde(default)]
    pub literary_bundle_enabled: Option<bool>,
    /// 任务模式文学能力包开关(2026-10-05;缺省保持不变)。扁平 + task 覆盖层,
    /// 与 `task_coding_bundle_enabled` 同构。
    #[serde(default)]
    pub task_literary_bundle_enabled: Option<bool>,
    /// 文学包文风预设(LIT-6;缺省保持不变)。空串 = 不注入;非空必须是
    /// `plain` / `classical` / `lightnovel` / `hardboiled` 之一(否则 400)。
    /// 角色扮演侧纯扁平字段。
    #[serde(default)]
    pub literary_style_preset: Option<String>,
    /// 文学包长程一致性推荐档(LIT-7;缺省保持不变)。空串 = 不改变(有快照则按快照恢复);
    /// 非空必须是 `medium` / `long` 之一(否则 400)。
    #[serde(default)]
    pub literary_recommend_preset: Option<String>,
    /// 任务模式默认连接(TM-SET-1):空串 = 清除(跟随默认连接);非空必须是已存在且
    /// 启用的连接 id(否则 400);缺省 = 保持不变
    #[serde(default)]
    pub task_default_connection_id: Option<String>,
    /// 工具循环历史保留的最近完整轮数(R3b;1..=32;缺省保持不变)
    #[serde(default)]
    pub tool_history_keep_rounds: Option<u32>,
    /// 工具循环历史 token 预算(R3b;0 = 禁用预算闸门,否则 1024..=1M;缺省保持不变)
    #[serde(default)]
    pub tool_history_budget_tokens: Option<u32>,
    /// 单次生成 token 预算(HB-1;0 = 关闭,否则 1024..=1e9;缺省保持不变)
    #[serde(default)]
    pub session_token_budget: Option<u32>,
    /// 预算超限动作(HB-1;warn/stop;缺省保持不变)
    #[serde(default)]
    pub session_budget_action: Option<String>,
    /// 语义熔断窗口(HB-2;4..=64;缺省保持不变)
    #[serde(default)]
    pub loop_guard_semantic_window: Option<u32>,
    /// 语义熔断同工具调用次数下限(HB-2;4..=64;缺省保持不变)
    #[serde(default)]
    pub loop_guard_semantic_min_calls: Option<u32>,
    /// 语义熔断输出指纹去重上限(HB-2;1..=8;缺省保持不变)
    #[serde(default)]
    pub loop_guard_semantic_max_distinct: Option<u32>,
    /// 任务步骤墙钟预算秒数(提交 3 · D3;0 = 关,否则 1..=86400;缺省保持不变)
    #[serde(default)]
    pub task_step_budget_secs: Option<u32>,
    /// 任务级总预算秒数(PRODCAP-2;0 = 关,否则 1..=86400;缺省保持不变)
    #[serde(default)]
    pub task_total_budget_secs: Option<u32>,
    /// 任务空闲超时秒数(提交 3 · D7;0 = 关,否则 `task_idle_floor_secs()`..=86400;
    /// 下限由「单条命令上限 + 单次模型调用上限 + 1」派生,勿在此手写数值)
    #[serde(default)]
    pub task_idle_timeout_secs: Option<u32>,
    /// 变量两步生成独立模型(HB-7;空串 = 清除(回到与正文共用);非空 = 覆盖;缺省保持不变)
    #[serde(default)]
    pub mvu_model: Option<String>,
    /// 变量两步生成独立温度(HB-7;0.0..=2.0 = 设置,负值 = 清除(回到内置 0.3);缺省保持不变)
    #[serde(default)]
    pub mvu_temperature: Option<f64>,
}

/// 多套连接的写入项(与 `ConnectionProfile` 的差异:各字段可选,缺省 = 沿用该 id 的现有值)。
/// `api_key` 与顶层 `openai_api_key` 同口径:**空/缺省表示保持不变**;
/// 要把已配置的密钥改为空,须显式传 `clear_api_key: true`(与 api_key 互斥,新输入优先)。
#[derive(Deserialize, Default)]
pub struct ConnectionProfileInput {
    /// 缺省或未命中已有 id = 新建(uuid);命中则沿用原 id(它是磁盘密文的配对键)
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub connector_type: Option<String>,
    /// 允许显式空串(清空地址);缺省 = 沿用
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    /// 显式清空该连接已保存的密钥(2026-10-03 API 设置补全;空串已被「保持不变」占用,
    /// 故清除语义用独立布尔表达)。与 `api_key` 互斥——新输入的非空 Key 优先;
    /// 仅对命中已有 id 的条目生效,新建连接无密钥可清。
    #[serde(default)]
    pub clear_api_key: Option<bool>,
    #[serde(default)]
    pub model: Option<String>,
    /// 接口方言显式覆盖(`chat-completions` | `responses` | `anthropic`;非法值回退默认档)。
    /// 缺省 = 沿用已有值,并在 sanitize 时按 URL 端点后缀推断(默认档才会被推断改写)。
    #[serde(default)]
    pub api_style: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// 模型能力位(2026-10-02 视觉能力包):缺省 = 沿用该 id 的现有值(新建缺省 false)。
    /// 只有显式传 false 才会清空既有声明——与其它字段的「缺省=沿用」同口径。
    #[serde(default)]
    pub supports_vision: Option<bool>,
    #[serde(default)]
    pub supports_structured_output: Option<bool>,
    #[serde(default)]
    pub supports_prefix_completion: Option<bool>,
    #[serde(default)]
    pub supports_mid_conversation_system: Option<bool>,
    #[serde(default)]
    pub image_auto_split: Option<bool>,
}

/// 逐连接探测请求体(connect / refresh-models 共用;2026-10-03 API 设置补全)。
/// 取数优先级:显式 `connection` 草稿参数(字段级覆盖)> `connection_id` 命中已存连接
/// (缺省字段取已存值)> 两者皆缺省 = 当前生效连接器(旧客户端行为不变)。
#[derive(Deserialize, Default)]
pub struct ConnectionProbeBody {
    /// 已保存连接 id:探测参数缺省时取该连接的已存值(含密钥;密钥仅服务端持有)
    #[serde(default)]
    pub connection_id: Option<String>,
    /// 显式草稿参数:未保存的新行也可测试;字段级优先于已存值
    #[serde(default)]
    pub connection: Option<ConnectionProbeInput>,
}

/// 探测目标连接的显式参数。字段缺省(None)= 回退 `connection_id` 对应已存值;
/// `base_url`/`model` 显式空串 = 按空值探测(与连接保存的「允许显式清空地址」同口径);
/// `api_key` 空/缺省 = 回退已存密钥(与「空 Key 忽略」同口径,探测已存连接不要求重填 Key)。
#[derive(Deserialize, Default)]
pub struct ConnectionProbeInput {
    #[serde(default)]
    pub connector_type: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_style: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// 序列化运行期设置(API Key 脱敏)。
/// 字段数已达 serde_json::json! 宏的递归展开上限,故拆成两段构建再合并
/// (比给整个 crate 提高 recursion_limit 影响面更小)。
fn settings_json(s: &RuntimeSettings) -> Value {
    let mut v = json!({
        "openai_base_url": s.openai_base_url,
        "api_key_masked": s.masked_api_key(),
        "has_api_key": !s.openai_api_key.is_empty(),
        "model": s.model,
        "default_temperature": s.default_temperature,
        "default_top_p": s.default_top_p,
        "default_max_tokens": s.default_max_tokens,
        "max_context_tokens": s.max_context_tokens,
        "agent_system_prompt": s.agent_system_prompt,
        "search_endpoint": s.search_endpoint,
        "mvu_vars_position": s.mvu_vars_position,
        "reflect_prompt": s.reflect_prompt,
        "preset_tail_prompt": s.preset_tail_prompt,
        "preset_tail_role": s.preset_tail_role,
        "reflect_advice_prompt": s.reflect_advice_prompt,
        "reflect_advice_role": s.reflect_advice_role,
        "bypass_mode": s.bypass_mode,
        "authorization_mode": s.authorization_mode.as_str(),
        "bypass_blacklist": s.bypass_blacklist,
        "tool_authorization_timeout_secs": s.tool_authorization_timeout_secs,
        "task_tool_policy": s.task_tool_policy,
        "task_tool_allowlist": s.task_tool_allowlist,
        "max_tool_rounds": s.max_tool_rounds,
        "max_flow_call_depth": s.max_flow_call_depth,
        "max_flow_calls_per_task": s.max_flow_calls_per_task,
        "default_node_max_context": s.default_node_max_context,
        "render_html": s.render_html,
        "compaction_mode": s.compaction_mode,
        "compaction_threshold": s.compaction_threshold,
        "compaction_keep_recent": s.compaction_keep_recent,
        "compaction_snip_bytes": s.compaction_snip_bytes,
        "llm_request_log": s.llm_request_log,
        "memory_distill_enabled": s.memory_distill_enabled,
        "memory_inject_limit": s.memory_inject_limit,
        "memory_inject_char_budget": s.memory_inject_char_budget,
        "memory_max_entries": s.memory_max_entries,
        "worldbook_sync_character_enabled": s.worldbook_sync_character_enabled,
        "worldbook_sync_global_enabled": s.worldbook_sync_global_enabled,
    });
    let rest = json!({
        "embedding_enabled": s.embedding_enabled,
        "embedding_base_url": s.embedding_base_url,
        "embedding_api_key_masked": s.masked_embedding_api_key(),
        "has_embedding_api_key": !s.embedding_api_key.is_empty(),
        "embedding_model": s.embedding_model,
        "embedding_dim": s.embedding_dim,
        "skill_progressive_disclosure": s.skill_progressive_disclosure,
        "undo_enabled": s.undo_enabled,
        "subagent_max_depth": s.subagent_max_depth,
        "subagent_max_concurrency": s.subagent_max_concurrency,
        "subagent_result_max_chars": s.subagent_result_max_chars,
        "mcp_enabled": s.mcp_enabled,
        "mcp_servers": s.mcp_servers,
        "exec_enabled": s.exec_enabled,
        "exec_allow_root": s.exec_allow_root,
        "exec_allow_shizuku": s.exec_allow_shizuku,
        "exec_allow_sandbox": s.exec_allow_sandbox,
        // 视觉与截图总开关(视觉能力包 D5;全局扁平)
        "vision_screenshot_enabled": s.vision_screenshot_enabled,
        "task_coding_bundle_enabled": s.task_coding_bundle_enabled,
        // 文学能力包两开关(LIT-1;角色扮演侧为纯扁平、任务侧为扁平+覆盖层)
        "literary_bundle_enabled": s.literary_bundle_enabled,
        "task_literary_bundle_enabled": s.task_literary_bundle_enabled,
        // 文学包选择型字段(LIT-6/LIT-7;快照是内部状态,不投影给前端)
        "literary_style_preset": s.literary_style_preset,
        "literary_recommend_preset": s.literary_recommend_preset,
        "task_default_connection_id": s.task_default_connection_id,
        "tool_history_keep_rounds": s.tool_history_keep_rounds,
        "tool_history_budget_tokens": s.tool_history_budget_tokens,
        "session_token_budget": s.session_token_budget,
        "session_budget_action": s.session_budget_action,
        "loop_guard_semantic_window": s.loop_guard_semantic_window,
        "loop_guard_semantic_min_calls": s.loop_guard_semantic_min_calls,
        "loop_guard_semantic_max_distinct": s.loop_guard_semantic_max_distinct,
        // 任务侧两道闸(提交 3 · D3/D7):扁平字段,任务侧消费(步骤墙钟预算 / 空闲看守)
        "task_step_budget_secs": s.task_step_budget_secs,
        // 任务级总预算(PRODCAP-2):扁平字段,任务侧消费(0 = 关)
        "task_total_budget_secs": s.task_total_budget_secs,
        "task_idle_timeout_secs": s.task_idle_timeout_secs,
        // HB-7 接线:变量两步生成的独立模型/温度档(此前可落盘但无 API 通路)
        "mvu_model": s.mvu_model,
        "mvu_temperature": s.mvu_temperature,
    });
    if let (Some(dst), Some(src)) = (v.as_object_mut(), rest.as_object()) {
        for (k, val) in src {
            dst.insert(k.clone(), val.clone());
        }
    }
    // 多套连接:独立第三段构建,避免继续加深 json! 宏的递归展开;
    // 只下发掩码与布尔,明文 Key 永不出现在响应里。
    let connections = Value::Array(
        s.connections
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "connector_type": p.connector_type,
                    "base_url": p.base_url,
                    "model": p.model,
                    "api_style": p.api_style,
                    "enabled": p.enabled,
                    // 模型能力位(2026-10-02 视觉能力包 D1;五项均为连接级声明)
                    "supports_vision": p.supports_vision,
                    "supports_structured_output": p.supports_structured_output,
                    "supports_prefix_completion": p.supports_prefix_completion,
                    "supports_mid_conversation_system": p.supports_mid_conversation_system,
                    "image_auto_split": p.image_auto_split,
                    "api_key_masked": p.masked_api_key(),
                    "has_api_key": p.has_api_key(),
                })
            })
            .collect(),
    );
    if let Some(dst) = v.as_object_mut() {
        dst.insert("connections".to_string(), connections);
        dst.insert(
            "active_connection_id".to_string(),
            json!(s.active_connection_id),
        );
    }
    v
}

/// GET /api/settings:当前运行期设置(供前端表单回填)。?mode= 返回该模式合并后的有效值。
pub async fn get_settings(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ModeQuery>,
) -> Json<Value> {
    let s = state.settings_snapshot();
    let mode = query.app_mode();
    Json(settings_json(&s.for_mode(mode)))
}

/// PUT /api/settings:更新运行期设置(部分字段;Base URL/Key 变更立即重建连接器,模型变更立即切换)。
/// ?mode= 指定写入哪个模式的覆盖层;连接信息(Base URL/Key/模型)始终写共享层。
pub async fn update_settings(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ModeQuery>,
    JsonBody(body): JsonBody<UpdateSettingsBody>,
) -> Response {
    let mode = query.app_mode();
    // 事务锁覆盖“读取当前值 → 应用 patch → 原子落盘 → 替换内存”，防止并发部分更新丢字段。
    // 只持有 tokio MutexGuard；std::sync::MutexGuard 均在同步代码块内释放，不跨 await。
    let _update_guard = state.guards.settings_update.lock().await;
    let (mut candidate, old_base, old_key, old_model, old_active) = {
        let current = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (
            current.clone(),
            current.openai_base_url.clone(),
            current.openai_api_key.clone(),
            current.model.clone(),
            current.active_connection().cloned(),
        )
    };
    // 显式清空密钥的连接 id(2026-10-03 API 设置补全):由连接数组路径收集,
    // 传给 save 绕开「磁盘密文保真」兜底 —— 否则内存空 + 磁盘密文会被原样回写(密钥复活)。
    let mut cleared_key_ids: Vec<String> = Vec::new();
    {
        let s = &mut candidate;

        // 连接信息:全局共享一份。自多套连接批次起,写入落到**默认连接**上
        // (旧客户端与设置页「API 连接」区走的都是这条路径 → 行为不变),
        // 由末尾的 normalize_connections 投影回扁平字段。
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
                return validation(format!("连接配置不能超过 {MAX_CONNECTIONS} 套"));
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
                    base_url: item.base_url.clone().unwrap_or_else(|| {
                        existing.map(|p| p.base_url.clone()).unwrap_or_default()
                    }),
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
                    supports_structured_output: item.supports_structured_output.unwrap_or_else(
                        || {
                            existing
                                .map(|p| p.supports_structured_output)
                                .unwrap_or(false)
                        },
                    ),
                    supports_prefix_completion: item.supports_prefix_completion.unwrap_or_else(
                        || {
                            existing
                                .map(|p| p.supports_prefix_completion)
                                .unwrap_or(false)
                        },
                    ),
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
                return validation("embedding_dim 必须为 0(自动)或 16..=8192");
            }
        }

        // 生成参数 + Agent 配置:task 模式写 task 覆盖层;roleplay 模式写扁平字段
        // (引擎直接读扁平字段,故 roleplay 不落覆盖层)。校验逻辑两模式一致。
        {
            let is_task = mode == AppMode::Task;
            // 通过校验后按模式写入:task 覆盖层或扁平字段。
            macro_rules! apply {
                ($s:expr, $is_task:expr, $field:ident, $value:expr) => {{
                    let v = $value;
                    if $is_task {
                        $s.task.$field = Some(v);
                    } else {
                        $s.$field = v;
                    }
                }};
            }
            if let Some(v) = body.default_temperature {
                if (0.0..=2.0).contains(&v) {
                    apply!(s, is_task, default_temperature, v);
                }
            }
            if let Some(v) = body.default_top_p {
                if (0.0..=1.0).contains(&v) {
                    apply!(s, is_task, default_top_p, v);
                }
            }
            if let Some(v) = body.default_max_tokens {
                if v == 0 || v > 131_072 {
                    return validation("default_max_tokens 必须在 1..=131072");
                }
                apply!(s, is_task, default_max_tokens, v);
            }
            if let Some(v) = body.max_context_tokens {
                if !(65_536..=1_048_576).contains(&v) {
                    return validation("max_context_tokens 必须在 65536..=1048576(64K~1M)");
                }
                apply!(s, is_task, max_context_tokens, v);
            }
            // Agent 系统提示词:允许清空(空 = 用内置默认)。
            // 类型级模式隔离(WP7):roleplay 写 RoleplayPromptConfig 扁平字段,
            // task 写 Option<TaskPromptConfig> 覆盖层;两分支类型不同源,不走通用 apply! 宏。
            if let Some(v) = &body.agent_system_prompt {
                if is_task {
                    s.task.agent_system_prompt = Some(
                        crate::services::settings_service::TaskPromptConfig(v.clone()),
                    );
                } else {
                    s.agent_system_prompt =
                        crate::services::settings_service::RoleplayPromptConfig(v.clone());
                }
            }
            // 搜索端点:允许清空(空 = 用默认 DDG 端点)。
            // TM-SET-3:直写扁平(全局字段)——search 工具运行期读全局快照,模式覆盖
            // 永不生效;此前 task 模式写入覆盖层是「改了不生效」的静默死写。
            if let Some(v) = &body.search_endpoint {
                let t = v.trim().to_string();
                let endpoint = if t.is_empty() {
                    DEFAULT_SEARCH_ENDPOINT.to_string()
                } else {
                    t
                };
                s.search_endpoint = endpoint;
            }
            // mvu 变量注入位置:仅接受 system / user_tail
            if let Some(v) = &body.mvu_vars_position {
                let t = v.trim().to_string();
                if t == "system" || t == "user_tail" {
                    apply!(s, is_task, mvu_vars_position, t);
                }
            }
            // 反思提示词:允许清空(空 = 回退机械规则检查)
            if let Some(v) = &body.reflect_prompt {
                apply!(s, is_task, reflect_prompt, v.clone());
            }
            // 预设尾部提示词:允许清空(空 = 禁用位置0 预设尾部)
            if let Some(v) = &body.preset_tail_prompt {
                apply!(s, is_task, preset_tail_prompt, v.clone());
            }
            // 预设尾部注入角色:仅接受 user / assistant
            if let Some(v) = &body.preset_tail_role {
                let t = v.trim().to_string();
                if t == "user" || t == "assistant" {
                    apply!(s, is_task, preset_tail_role, t);
                }
            }
            // 反思失败建议提示词:允许清空(空 = 禁用)
            if let Some(v) = &body.reflect_advice_prompt {
                apply!(s, is_task, reflect_advice_prompt, v.clone());
            }
            // 反思建议注入角色:仅接受 user / assistant
            if let Some(v) = &body.reflect_advice_role {
                let t = v.trim().to_string();
                if t == "user" || t == "assistant" {
                    apply!(s, is_task, reflect_advice_role, t);
                }
            }
            // 授权模式(三档;旧 bypass_mode 仍接受并映射为对应档位)。
            // TM-SET-3:三档/黑名单/等待超时直写扁平(全局字段)——工具授权裁决不经
            // for_mode,写覆盖层是静默死写;任务工具循环同样读全局矩阵(名单外直接拒绝,
            // 不空等),故「两模式共用一份」就是运行期事实,写侧与之对齐。
            if let Some(v) = &body.authorization_mode {
                let parsed = match crate::tools::permissions::AuthorizationMode::parse(v.trim()) {
                    Some(m) => m,
                    None => {
                        return validation("authorization_mode 仅支持 strict / loose / bypass");
                    }
                };
                s.authorization_mode = parsed;
            }
            // 旧字段兼容:bypass_mode=true → bypass,false → strict
            if let Some(v) = body.bypass_mode {
                let mapped = if v {
                    crate::tools::permissions::AuthorizationMode::Bypass
                } else {
                    crate::tools::permissions::AuthorizationMode::Strict
                };
                s.authorization_mode = mapped;
            }
            // 「始终需授权」清单:校验工具名存在性,未知名返回 warning(不阻塞保存)
            if let Some(v) = &body.bypass_blacklist {
                s.bypass_blacklist = v.clone();
            }
            // 授权等待超时(秒):30..=1800
            if let Some(v) = body.tool_authorization_timeout_secs {
                if !(30..=1800).contains(&v) {
                    return validation("tool_authorization_timeout_secs 必须在 30..=1800");
                }
                s.tool_authorization_timeout_secs = v;
            }
            // 任务模式工具策略:all / deny_dangerous / allowlist
            if let Some(v) = &body.task_tool_policy {
                if !matches!(v.as_str(), "all" | "deny_dangerous" | "allowlist") {
                    return validation("task_tool_policy 仅支持 all / deny_dangerous / allowlist");
                }
                apply!(s, is_task, task_tool_policy, v.clone());
            }
            if let Some(v) = &body.task_tool_allowlist {
                apply!(s, is_task, task_tool_allowlist, v.clone());
            }
            // 工具循环轮次上限:仅接受 1..=200(防止误填 0 或超大值打爆模型请求)
            if let Some(v) = body.max_tool_rounds {
                if !(1..=200).contains(&v) {
                    return validation("max_tool_rounds 必须在 1..=200");
                }
                apply!(s, is_task, max_tool_rounds, v);
            }
            // 流程调用闸(A 批 A3):动态调用深度与每任务调用次数。
            // 上限刻意保守——两条都是**成本**闸,调大等于允许更长的模型自主链。
            if let Some(v) = body.max_flow_call_depth {
                if !(MIN_FLOW_CALL_DEPTH..=MAX_FLOW_CALL_DEPTH_LIMIT).contains(&v) {
                    return validation("max_flow_call_depth 必须在 1..=5");
                }
                apply!(s, is_task, max_flow_call_depth, v);
            }
            if let Some(v) = body.max_flow_calls_per_task {
                if !(MIN_FLOW_CALLS_PER_TASK..=MAX_FLOW_CALLS_PER_TASK_LIMIT).contains(&v) {
                    return validation("max_flow_calls_per_task 必须在 1..=64");
                }
                apply!(s, is_task, max_flow_calls_per_task, v);
            }
            // 节点默认上下文上限(A 批 A4):0 = 不裁剪;非 0 时与节点级字段同口径
            // (下限 256 的理由见 `agent_flow_service::MIN_STEP_MAX_CONTEXT`)。
            if let Some(v) = body.default_node_max_context {
                let ok = v == 0
                    || (crate::services::agent_flow_service::MIN_STEP_MAX_CONTEXT
                        ..=crate::services::agent_flow_service::MAX_STEP_MAX_CONTEXT)
                        .contains(&v);
                if !ok {
                    return validation("default_node_max_context 必须为 0(不裁剪)或 256..=1048576");
                }
                apply!(s, is_task, default_node_max_context, v);
            }
            // HTML 渲染开关
            if let Some(v) = body.render_html {
                apply!(s, is_task, render_html, v);
            }
            // 上下文压缩模式:仅接受 off / manual / auto
            if let Some(v) = &body.compaction_mode {
                let t = v.trim().to_string();
                if t == "off" || t == "manual" || t == "auto" {
                    apply!(s, is_task, compaction_mode, t);
                }
            }
            // 上下文压缩阈值:仅接受 0.5..=0.95
            if let Some(v) = body.compaction_threshold {
                if !(0.5..=0.95).contains(&v) {
                    return validation("compaction_threshold 必须在 0.5..=0.95");
                }
                apply!(s, is_task, compaction_threshold, v);
            }
            // 压缩保留条数(缓存感知管线):2..=200,缺省保持不变
            if let Some(v) = body.compaction_keep_recent {
                if (2..=200).contains(&v) {
                    apply!(s, is_task, compaction_keep_recent, v);
                }
            }
            // snip 零成本裁剪阈值(字节):0 = 禁用,1MB 上限
            if let Some(v) = body.compaction_snip_bytes {
                if v <= 1_048_576 {
                    apply!(s, is_task, compaction_snip_bytes, v);
                }
            }
            // LLM 请求快照开关
            if let Some(v) = body.llm_request_log {
                apply!(s, is_task, llm_request_log, v);
            }
            // 记忆蒸馏开关(落地项 2)
            if let Some(v) = body.memory_distill_enabled {
                apply!(s, is_task, memory_distill_enabled, v);
            }
            // 剧情推演词条同步两档(RPFLOW-2):角色档默认开、全局档默认关
            if let Some(v) = body.worldbook_sync_character_enabled {
                apply!(s, is_task, worldbook_sync_character_enabled, v);
            }
            if let Some(v) = body.worldbook_sync_global_enabled {
                apply!(s, is_task, worldbook_sync_global_enabled, v);
            }
            // 记忆注入上限(0..=50;0 = 关闭注入,越界忽略)
            if let Some(v) = body.memory_inject_limit {
                if v <= 50 {
                    apply!(s, is_task, memory_inject_limit, v);
                }
            }
            // 记忆槽字符预算(0..=20000;0 = 不限制,越界忽略)
            if let Some(v) = body.memory_inject_char_budget {
                if v <= 20_000 {
                    apply!(s, is_task, memory_inject_char_budget, v);
                }
            }
            // 每角色记忆容量上限(0..=10000;0 = 不淘汰,越界忽略)
            if let Some(v) = body.memory_max_entries {
                if v <= 10_000 {
                    apply!(s, is_task, memory_max_entries, v);
                }
            }
            // 技能渐进披露开关(落地项 3)
            if let Some(v) = body.skill_progressive_disclosure {
                apply!(s, is_task, skill_progressive_disclosure, v);
            }
            // 回退快照开关(批次 6.1)。TM-SET-3:直写扁平(全局字段)——UndoService
            // 直接读基础值,模式覆盖永不生效。
            if let Some(v) = body.undo_enabled {
                s.undo_enabled = v;
            }
            // 子智能体嵌套深度上限(1..=4,越界拒绝)。TM-SET-3:直写扁平(全局字段)——
            // agentgo 工具运行期读全局快照,模式覆盖永不生效。
            if let Some(v) = body.subagent_max_depth {
                if !(1..=4).contains(&v) {
                    return validation("subagent_max_depth 必须在 1..=4");
                }
                s.subagent_max_depth = v;
            }
            // 子智能体并发上限(1..=16,越界拒绝;同直写扁平口径)
            if let Some(v) = body.subagent_max_concurrency {
                if !(1..=16).contains(&v) {
                    return validation("subagent_max_concurrency 必须在 1..=16");
                }
                s.subagent_max_concurrency = v;
            }
            // 子智能体结果字符上限(500..=8000,越界拒绝;同直写扁平口径)
            if let Some(v) = body.subagent_result_max_chars {
                if !(500..=8000).contains(&v) {
                    return validation("subagent_result_max_chars 必须在 500..=8000");
                }
                s.subagent_result_max_chars = v;
            }
            // MCP 总开关(批次 6.2;PLGM 3.4 起**直写扁平**):MCP 是进程级全局能力
            // (装配与 restart 端点读扁平权威值),不走模式覆盖层——原 `apply!` 的覆盖语义
            // 只会制造「看似可配、实际无效」的字段,DC-2/CFG-2 收口时删除。
            // 运行期改动经 POST /api/mcp/servers/{name}/restart 一键生效。
            if let Some(v) = body.mcp_enabled {
                s.mcp_enabled = v;
            }
            // MCP 服务器列表(全量替换):卫生清理同 load(trim 名称/命令,丢弃不可用条目)
            if let Some(v) = &body.mcp_servers {
                let mut servers = v.clone();
                servers.retain_mut(|srv| {
                    srv.name = srv.name.trim().to_string();
                    srv.command = srv.command.trim().to_string();
                    !srv.name.is_empty() && !srv.command.is_empty()
                });
                s.mcp_servers = servers;
            }
            // 命令执行开关(阶段 E):bool 免校验。这些是**全局**能力开关,
            // 不随 roleplay/task 覆盖层分叉(exec: 权限是进程级事实,不是模式偏好),
            // 故直接写扁平字段而非走 apply!(is_task,...)。
            if let Some(v) = body.exec_enabled {
                s.exec_enabled = v;
            }
            if let Some(v) = body.exec_allow_root {
                s.exec_allow_root = v;
            }
            if let Some(v) = body.exec_allow_shizuku {
                s.exec_allow_shizuku = v;
            }
            if let Some(v) = body.exec_allow_sandbox {
                s.exec_allow_sandbox = v;
            }
            // 视觉与截图总开关(视觉能力包 D5):全局能力开关,直写扁平(口径同 exec_*)。
            if let Some(v) = body.vision_screenshot_enabled {
                s.vision_screenshot_enabled = v;
            }
            // 编码能力包开关(2026-09-28;bool 免校验,task 写覆盖层)。只影响提示词缺省值:
            // 启用后任务模式缺省默认词改用编码执行者模板,用户自定义值仍逐字优先。
            if let Some(v) = body.task_coding_bundle_enabled {
                apply!(s, is_task, task_coding_bundle_enabled, v);
            }
            // 文学能力包开关(LIT-1;bool 免校验)。角色扮演侧开关是**纯扁平字段**——
            // 该侧没有覆盖层(for_mode 对 Roleplay 直接 clone),走 apply! 会写出无意义的
            // task 覆盖层,故照 exec_* 直写(s.xxx = v,与请求的 mode 无关);
            // 任务侧开关对称编码包,走覆盖层。
            if let Some(v) = body.literary_bundle_enabled {
                s.literary_bundle_enabled = v;
            }
            if let Some(v) = body.task_literary_bundle_enabled {
                apply!(s, is_task, task_literary_bundle_enabled, v);
            }
            // 文学包文风预设(LIT-6):空 = 不注入;未知取值 400(不静默回退)。
            // 角色扮演侧纯扁平字段(同上方开关的直写口径)。
            if let Some(v) = &body.literary_style_preset {
                let t = v.trim().to_string();
                if !is_valid_literary_style_preset(&t) {
                    return validation(
                        "literary_style_preset 仅支持 plain / classical / lightnovel / hardboiled(空 = 不注入)",
                    );
                }
                s.literary_style_preset = t;
            }
            // 文学包长程一致性推荐档(LIT-7):**显式选档才写入既有三项数值**,可回退。
            //  - 首次选档:先拍「写入前原值」快照再写档值;档间切换**不重拍**(快照始终是采纳前);
            //  - 选回空串:有快照按快照恢复(**写入前原值,不是默认值**),无快照则不动;
            //  - 未知取值 400;越界仍由 secret.rs 的既有钳制兜底(不新增校验口径)。
            // 位置:本块在 compaction_* 直写之后——同一请求里两者同时出现时以「选档」为准
            // (选档是更明确的用户意图);前端同一分区不会同时发两者。
            if let Some(v) = &body.literary_recommend_preset {
                let t = v.trim().to_string();
                if !is_valid_literary_recommend_preset(&t) {
                    return validation(
                        "literary_recommend_preset 仅支持 medium / long(空 = 不改变)",
                    );
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
            // 任务模式默认连接(TM-SET-1):空串 = 清除(回到跟随默认连接);非空必须在
            // **本请求应用后的**连接列表里存在且启用,否则 400(本块位于连接数组处理与
            // normalize_connections 之后,故同请求里先改连接再设默认也能正确校验)。
            // 校验只拦「此刻无效」;用户之后删/停用该连接时,运行期 `task_mode_default_connection`
            // 软回退默认连接(创建期的硬校验 + 运行期的软回退,两处口径并存不矛盾)。
            if let Some(v) = &body.task_default_connection_id {
                let t = v.trim().to_string();
                if t.is_empty() {
                    apply!(s, is_task, task_default_connection_id, String::new());
                } else {
                    match s.connections.iter().find(|p| p.id == t) {
                        Some(p) if p.enabled => {
                            apply!(s, is_task, task_default_connection_id, t);
                        }
                        Some(p) => {
                            return validation(format!(
                                "选择的连接「{}」已停用;请启用它,或选其它连接",
                                crate::services::settings_service::connection_label(p)
                            ));
                        }
                        None => {
                            return validation(
                                "选择的连接不存在(可能已被删除);请在设置里恢复该连接,或选其它连接",
                            );
                        }
                    }
                }
            }
            // 工具历史回灌上限(R3b):扁平全局字段(引擎 run_tool_loop 直读扁平值,
            // 不入模式覆盖层——任务/聊天工具循环共用同一上限,与 subagent 参数的
            // 引擎侧消费口径一致);越界拒绝,与 load 钳制区间一致
            if let Some(v) = body.tool_history_keep_rounds {
                if !(1..=32).contains(&v) {
                    return validation("tool_history_keep_rounds 必须在 1..=32");
                }
                s.tool_history_keep_rounds = v;
            }
            if let Some(v) = body.tool_history_budget_tokens {
                if v != 0 && !(1024..=1_048_576).contains(&v) {
                    return validation("tool_history_budget_tokens 须为 0(禁用)或 1024..=1048576");
                }
                s.tool_history_budget_tokens = v;
            }
            // HB-1 成本护栏:扁平全局字段(引擎 run_tool_loop 直读扁平值,与工具历史
            // 预算同款——聊天与任务工具循环共用同一上限);越界拒绝,与 load 钳制同区间
            if let Some(v) = body.session_token_budget {
                if v != 0 && !(1024..=1_000_000_000).contains(&v) {
                    return validation("session_token_budget 须为 0(关闭)或 1024..=1000000000");
                }
                s.session_token_budget = v;
            }
            if let Some(v) = &body.session_budget_action {
                if !matches!(v.as_str(), "warn" | "stop") {
                    return validation("session_budget_action 须为 warn 或 stop");
                }
                s.session_budget_action = v.clone();
            }
            // HB-2 语义熔断参数(扁平全局:引擎 run_tool_loop 直读);越界拒绝,与 load 同区间
            if let Some(v) = body.loop_guard_semantic_window {
                if !(4..=64).contains(&v) {
                    return validation("loop_guard_semantic_window 必须在 4..=64");
                }
                s.loop_guard_semantic_window = v;
            }
            if let Some(v) = body.loop_guard_semantic_min_calls {
                if v != 0 && !(4..=64).contains(&v) {
                    return validation("loop_guard_semantic_min_calls 须为 0(关闭)或 4..=64");
                }
                s.loop_guard_semantic_min_calls = v;
            }
            if let Some(v) = body.loop_guard_semantic_max_distinct {
                if !(1..=8).contains(&v) {
                    return validation("loop_guard_semantic_max_distinct 必须在 1..=8");
                }
                s.loop_guard_semantic_max_distinct = v;
            }
            // 任务侧两道闸(提交 3 · D3/D7):扁平全局字段(任务侧消费,不进模式覆盖层);
            // 越界拒绝,与 load 钳制区间一致。step_budget 刻意不设下限(1 秒合法:
            // 低于单次调用看门狗的值 = 「第一轮结束就收尾」的合法语义)。
            if let Some(v) = body.task_step_budget_secs {
                if v != 0 && !(1..=86_400).contains(&v) {
                    return validation("task_step_budget_secs 须为 0(关闭)或 1..=86400");
                }
                s.task_step_budget_secs = v;
            }
            // 任务级总预算(PRODCAP-2):同款区间(0 = 关,否则 1..=86400)
            if let Some(v) = body.task_total_budget_secs {
                if v != 0 && !(1..=86_400).contains(&v) {
                    return validation("task_total_budget_secs 须为 0(关闭)或 1..=86400");
                }
                s.task_total_budget_secs = v;
            }
            if let Some(v) = body.task_idle_timeout_secs {
                // 下限走 `task_idle_floor_secs()` 单一出处(= 单条命令上限 + 单次模型调用上限 + 1);
                // 文案里也插值同一个数,**不再手写「601」与它的推导式**——批次 2 抬 bash 上限时,
                // 手写文案会与真实区间不一致,用户照文案调参会被莫名拒绝。
                let floor = task_idle_floor_secs();
                if v != 0 && !(floor..=86_400).contains(&v) {
                    return validation(format!(
                        "task_idle_timeout_secs 须为 0(关闭)或 {floor}..=86400(下限 = 单条命令上限 + 单次模型调用上限 + 1)"
                    ));
                }
                s.task_idle_timeout_secs = v;
            }
            // HB-7:变量两步生成的独立模型/温度档。清除语义用哨兵值表达,避免引入
            // 「Option<Option<T>>」这类与既有 PATCH 体例不符的写法:
            // mvu_model 空串 = 清除(回到与正文共用同一连接器/模型);
            // mvu_temperature 负值 = 清除(回到内置 0.3;温度本身不允许负数)
            if let Some(v) = &body.mvu_model {
                let trimmed = v.trim();
                if trimmed.chars().count() > 200 {
                    return validation("mvu_model 长度不得超过 200 字符");
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
                    return validation("mvu_temperature 须为 0.0..=2.0,或负值表示清除");
                } else {
                    s.mvu_temperature = Some(v);
                }
            }
        }
    }

    // B-1:save 含 API Key 加密(DPAPI)+ 同步 JSON 落盘,挪阻塞线程池;
    // candidate 所有权随闭包往返,后续连接器/模型比较逻辑不变
    let data_dir = state.config.data_dir.clone();
    let save_outcome = state
        .db_call(move || {
            let result = candidate.save_with_cleared_connection_keys(&data_dir, &cleared_key_ids);
            (candidate, result)
        })
        .await;
    let (candidate, save_result) = match save_outcome {
        Ok(pair) => pair,
        Err(e) => {
            // 泄露封堵(批次 1):JoinError/IO 原文只进日志,回给用户稳定文案 + code
            tracing::error!(error = %e, "设置保存失败:阻塞任务");
            return err_with_code(
                ErrorCode::Internal,
                "设置保存失败,详情见服务端日志",
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    if let Err(e) = save_result {
        tracing::error!(error = %e, "设置保存失败:写配置文件");
        return err_with_code(
            ErrorCode::Internal,
            "设置保存失败,详情见服务端日志",
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    let connector_changed =
        candidate.openai_base_url != old_base || candidate.openai_api_key != old_key;
    let model_changed = candidate.model != old_model;
    let model_name = candidate.model.clone();
    // 当前连接器类型(重建判定与目标解析共用这一次读锁快照)
    let current_type = state
        .engine
        .connector
        .read()
        .await
        .clone()
        .type_name()
        .to_string();
    // 目标类型变化本身也算变更:切换默认连接时若两条连接的值完全相同、只有类型不同
    // (一个显式 mock、一个走真实 API),只看扁平字段就漏判了。
    let target = resolve_connector_target(candidate.active_connection(), &current_type);
    let target_changed = target != resolve_connector_target(old_active.as_ref(), &current_type);
    // 接口方言变化同样要重建:只改「接口格式」而地址/密钥/模型逐字不变时,
    // 若不判它,连接器会继续用旧方言解析到下次重启。
    let old_style = old_active
        .as_ref()
        .map(|p| p.api_style.clone())
        .unwrap_or_default();
    let new_style = candidate
        .active_connection()
        .map(|p| p.api_style.clone())
        .unwrap_or_else(|| API_STYLE_CHAT.to_string());
    let style_changed = old_style != new_style;
    // 能力位变化(视觉能力包 D1;2026-10-03 VISION-L6 收口:五项全部参与)——
    // 或改变序列化行为(图像发送/大图拆分),或改变请求参数(结构化输出/中途 system
    // 处置),与地址/密钥/模型/方言同款触发重建。
    let caps_of = |p: Option<&ConnectionProfile>| {
        p.map(ConnectionProfile::connector_capabilities)
            .unwrap_or_default()
    };
    let caps_changed = caps_of(candidate.active_connection()) != caps_of(old_active.as_ref());
    *state.settings.lock().unwrap_or_else(|e| e.into_inner()) = candidate.clone();

    // Base URL / API Key / 模型 / 目标类型 / 接口方言 / 能力位变更 → 重建连接器。
    // 目标类型由默认连接解析(与启动装配同一个函数):显式 mock 用 mock、无可用连接用 mock、
    // 空配置保持 mock、其余 openai-compatible —— 否则用户填写的 API 设置永远不会生效
    // (模型列表始终只有 mock-demo)。
    if connector_changed || model_changed || target_changed || style_changed || caps_changed {
        let (base_url, api_key, model) = (
            candidate.openai_base_url.clone(),
            candidate.openai_api_key.clone(),
            candidate.model.clone(),
        );
        let new_caps = candidate
            .active_connection()
            .map(|p| p.connector_capabilities())
            .unwrap_or_default();
        let new_connector = crate::connectors::build_connector(
            target, &base_url, &api_key, &model, &new_style, new_caps,
        );
        *state.engine.connector.write().await = new_connector;
    }

    // 模型变更 → 同步 engine 与 AppState.model
    if model_changed {
        state.engine.switch_model(&model_name).await;
        *state.model.lock().unwrap_or_else(|e| e.into_inner()) = model_name;
    }

    // 编码能力包开关 → 同步内置流程库(CODE-5):开包并入缺失的包流程(幂等),关包不回收。
    // 放在设置**已落盘且内存已替换**之后,且不让失败反过来影响设置保存——流程库是增强面,
    // 设置保存是主路径;真写失败时 sync 内部已记 error,这里只补一条「下次启动会再同步」的线索。
    // 读有效值用 for_mode(Task):该字段是任务侧设置,task 覆盖层可能覆盖扁平值
    // (与任务侧读法同源,见 settings_service::params)。
    // 文学包流程(LIT-5)取**两侧开关的并**(同 `api/app_state.rs` 的构造期口径):
    // 流程库是双模式共用设施,任一侧显式启用即并入。
    let coding_enabled = candidate.for_mode(AppMode::Task).task_coding_bundle_enabled;
    let literary_enabled = candidate.literary_bundle_enabled
        || candidate
            .for_mode(AppMode::Task)
            .task_literary_bundle_enabled;
    let flow = state.flow.clone();
    if let Err(e) = state
        .db_call(move || {
            flow.lock()
                .unwrap_or_else(|e| e.into_inner())
                .sync_pack_flows(coding_enabled, literary_enabled);
        })
        .await
    {
        tracing::error!(
            error = e,
            "能力包流程同步失败(设置已保存,流程库下次启动会再同步)"
        );
    }

    Json(json!({ "ok": true, "settings": settings_json(&candidate.for_mode(mode)) }))
        .into_response()
}

/// 解析逐连接探测的目标连接器(2026-10-03 API 设置补全):
/// 显式 `connection` 草稿参数(字段级覆盖)> `connection_id` 命中已存连接(缺省字段取
/// 已存值,密钥仅服务端持有)> 两者皆缺省 = 当前生效连接器快照(旧客户端行为不变)。
/// 探测用一次性临时连接器:不落库、不改动 `engine.connector`、不进连接器池缓存。
async fn resolve_probe_connector(
    state: &Arc<AppState>,
    body: Option<ConnectionProbeBody>,
) -> crate::connectors::Connector {
    let Some(body) = body else {
        return state.engine.connector.read().await.clone();
    };
    let saved = body
        .connection_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .and_then(|id| {
            state
                .settings_snapshot()
                .connections
                .into_iter()
                .find(|p| p.id == id)
        });
    if body.connection.is_none() && saved.is_none() {
        return state.engine.connector.read().await.clone();
    }
    let explicit = body.connection.unwrap_or_default();
    let trim_non_empty =
        |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let connector_type = trim_non_empty(explicit.connector_type)
        .or_else(|| saved.as_ref().map(|p| p.connector_type.clone()))
        .unwrap_or_else(|| CONNECTOR_TYPE_OPENAI.to_string());
    // 显式空串 = 按空值探测(draft 清空地址后探测应如实报不可达,而非拿旧地址测试);
    // normalize 与保存路径同口径,「host:1234」这类裸地址在探测时也能按补全后的形态访问
    let base_url = explicit
        .base_url
        .map(|v| normalize_base_url(&v))
        .or_else(|| saved.as_ref().map(|p| p.base_url.clone()))
        .unwrap_or_default();
    // api_key 空/缺省 = 回退已存密钥(与「空 Key 忽略」同口径:已存连接不要求重填 Key)
    let api_key = trim_non_empty(explicit.api_key)
        .or_else(|| saved.as_ref().map(|p| p.api_key.clone()))
        .unwrap_or_default();
    let api_style = trim_non_empty(explicit.api_style)
        .map(|s| normalize_api_style(&s).to_string())
        .or_else(|| saved.as_ref().map(|p| p.api_style.clone()))
        .unwrap_or_else(|| API_STYLE_CHAT.to_string());
    let model = explicit
        .model
        .or_else(|| saved.as_ref().map(|p| p.model.clone()))
        .unwrap_or_default();
    // 类型白名单:探测不走「未知类型回退 mock」的兜底(那会把手滑的类型测成假绿),
    // 未知/空一律按 openai-compatible
    let connector_type = if connector_type == CONNECTOR_TYPE_MOCK {
        CONNECTOR_TYPE_MOCK.to_string()
    } else {
        CONNECTOR_TYPE_OPENAI.to_string()
    };
    crate::connectors::build_connector(
        &connector_type,
        &base_url,
        &api_key,
        &model,
        &api_style,
        crate::connectors::ConnectorCapabilities::default(),
    )
}

/// POST /api/settings/refresh-models:向已保存的 API 请求可用模型列表(立即生效,不保存)。
/// 支持逐连接探测(可选 body,见 [`ConnectionProbeBody`]);缺省行为不变(测当前生效连接器)。
pub async fn refresh_models(
    State(state): State<Arc<AppState>>,
    JsonBodyOpt(body): JsonBodyOpt<ConnectionProbeBody>,
) -> Json<serde_json::Value> {
    let connector = resolve_probe_connector(&state, body).await;
    let models = connector.available_models().await;
    // openai-compatible 下若只拿到回退的 1 个当前模型,大概率是服务不支持 /models 接口
    // 或 Base URL / 接口格式与真实端点不匹配(后者会让 /models 也 404/400)
    let message = if connector.type_name() == "openai-compatible" && models.len() <= 1 {
        Some(format!(
            "API 未返回完整模型列表(该服务可能不支持 /models 接口,或 Base URL 与接口格式不匹配);已保留当前模型「{}」",
            models.first().cloned().unwrap_or_default()
        ))
    } else {
        None
    };
    Json(json!({ "ok": true, "models": models, "message": message }))
}

/// POST /api/settings/connect:测试连接(支持逐连接探测,可选 body 见 [`ConnectionProbeBody`])
pub async fn connect(
    State(state): State<Arc<AppState>>,
    JsonBodyOpt(body): JsonBodyOpt<ConnectionProbeBody>,
) -> Json<serde_json::Value> {
    let connector = resolve_probe_connector(&state, body).await;
    Json(connector.test().await)
}

/// GET /api/settings/models:可用模型列表
pub async fn models(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let models = state
        .engine
        .connector
        .read()
        .await
        .clone()
        .available_models()
        .await;
    Json(json!({ "models": models }))
}

/// POST /api/settings/embedding/test:测试向量化连接(嵌入一条固定文本)。
/// 成功回传实际维度与耗时;失败回传错误原因。不写库、不改配置。
///
/// 形状(批次 1):保留 200 + `ok:false`,`ok:false` 是**正常业务上报**(测试结果),
/// 不是 HTTP 错误——前端 settings.ts 的 testEmbedding 把该对象直接渲染到设置页,
/// 改成非 2xx 会让 request() 抛 ApiError,把「测试未通过」变成异常弹窗(破坏既有 UX)。
/// 这里补 `code`/`error` 供程序化分支;`message` 保留原文(用户需要据此修正自己的配置)。
pub async fn test_embedding(State(state): State<Arc<AppState>>) -> Response {
    let settings = state.settings_snapshot();
    let svc = crate::services::embedding_service::EmbeddingService::new();
    match svc.test(&settings).await {
        Ok((dim, ms)) => Json(json!({
            "ok": true,
            "dim": dim,
            "latency_ms": ms,
            "message": format!("连接成功:向量维度 {dim},耗时 {ms} ms"),
        }))
        .into_response(),
        Err(e) => {
            // 未配置 → VALIDATION(用户可自行修正);已配置但调用失败 → UPSTREAM(上游)
            let code = match &e {
                crate::services::embedding_service::EmbedError::NotConfigured(_) => {
                    ErrorCode::Validation
                }
                crate::services::embedding_service::EmbedError::Failed(_) => ErrorCode::Upstream,
            };
            let message = e.to_string();
            Json(
                json!({ "ok": false, "code": code.as_str(), "error": message, "message": message }),
            )
            .into_response()
        }
    }
}

/// GET /api/settings/info:连接器信息 + 模型列表 + 可用连接器
pub async fn info(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let connector = state.engine.connector.read().await.clone();
    let type_name = connector.type_name();
    let models = connector.available_models().await;
    let model = connector.model().to_string();
    Json(json!({
        "connector": type_name,
        "model": model,
        "models": models,
        "availableConnectors": crate::connectors::available_connector_types(),
    }))
}

/// PUT /api/settings/model:切换模型(立即生效)
pub async fn switch_model(
    State(state): State<Arc<AppState>>,
    JsonBody(body): JsonBody<SwitchModelBody>,
) -> Response {
    let model = body.model.trim().to_string();
    if model.is_empty() {
        return validation("缺少 model");
    }
    let changed = state.engine.model() != model;
    if changed {
        state.engine.switch_model(&model).await;
        *state.model.lock().unwrap_or_else(|e| e.into_inner()) = model.clone();
    }
    Json(json!({ "ok": true, "model": model, "changed": changed })).into_response()
}

/// GET /api/settings/model:当前模型
pub async fn get_model(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({ "model": state.engine.model() }))
}

/// GET /api/settings/agent-prompt:读取 DATA_DIR 中的运行时主 Agent 提示词。
/// 新文件缺失时由共享文件服务兼容读取/迁移旧 AGENTS_RUNTIME.md。
pub async fn get_agent_prompt(State(state): State<Arc<AppState>>) -> Response {
    let path = state.runtime_prompt.path().display().to_string();
    match state.runtime_prompt.read() {
        Ok(content) => {
            Json(json!({ "ok": true, "path": path, "content": content })).into_response()
        }
        Err(e) => not_found(e),
    }
}

#[derive(Deserialize)]
pub struct SaveAgentPromptBody {
    pub content: String,
}

#[derive(Deserialize, Default)]
pub struct PromptPreviewQuery {
    pub session_id: Option<String>,
    pub character_id: Option<String>,
    /// 预览按哪个模式的合并设置:roleplay(缺省,兼容旧客户端)| task
    pub mode: Option<String>,
}

#[derive(Serialize)]
struct PromptPreviewLayer {
    source: String,
    role: String,
    layer: u8,
    order: usize,
    content: String,
}

fn push_preview_layer(
    layers: &mut Vec<PromptPreviewLayer>,
    source: impl Into<String>,
    role: impl Into<String>,
    layer: u8,
    content: impl Into<String>,
) {
    let content = content.into();
    if content.trim().is_empty() {
        return;
    }
    layers.push(PromptPreviewLayer {
        source: source.into(),
        role: role.into(),
        layer,
        order: layers.len(),
        content,
    });
}

fn stable_history_hash(text: &str) -> String {
    // 非加密用途：只用于预览中识别“内容是否变化”，不返回原文。
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// GET /api/settings/prompt-preview：按最终注入顺序输出来源层。
/// 历史层仅含 role/字符长度/稳定哈希，绝不返回聊天正文或连接密钥。
pub async fn prompt_preview(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PromptPreviewQuery>,
) -> Response {
    let mut layers = Vec::new();
    match state.runtime_prompt.read_optional() {
        Ok(Some(runtime)) => {
            push_preview_layer(&mut layers, "runtime_prompt", "system", 5, runtime)
        }
        Ok(None) => {}
        Err(error) => return internal(error),
    }

    // 预览按模式走 for_mode 合并值(docs/契约-协议与配置.md 第五节):
    // 缺省/未知值按 roleplay(旧客户端零变化);task 为覆盖层合并后的有效设置,
    // agent_system_prompt 经类型级隔离转换(None 已注入内置任务默认词)。
    let mode = match query.mode.as_deref() {
        Some("task") => AppMode::Task,
        _ => AppMode::Roleplay,
    };
    // 设置快照:不留锁跨 await(for_mode 为纯计算,锁在 snapshot 内即释放)
    let settings = state.settings_snapshot().for_mode(mode);
    // 文学能力包(LIT-3):两段值取自设置层方法(开关判定 + 文本单一出处)。此处先取好——
    // 下方若干 push 会按值移动 settings 的字段(如 reflect_advice_prompt),之后再借用会编译错。
    let literary_tail = settings.literary_system_tail();
    let literary_note = settings.literary_user_note();
    // 文学能力包(LIT-6):位置 0 文风素材段(开关关或预设为空 → None)
    let literary_style = settings.literary_style_note();
    // agent_system_prompt 为 RoleplayPromptConfig(WP7),.0 取字符串
    if settings.agent_system_prompt.0.trim().is_empty() {
        // 空值回退文案按模式区分:roleplay 空 = 用内置人设模板;
        // task 走到这里 = 覆盖层 Some("") 显式清空(None 已被 for_mode 注入任务默认词)
        let fallback = match mode {
            AppMode::Task => "任务模式 Agent 系统提示词已显式清空,执行时仅注入下方三层固定提示词",
            AppMode::Roleplay => "内置默认角色扮演 / 文学创作模板（运行时按角色展开）",
        };
        push_preview_layer(&mut layers, "default_template", "system", 5, fallback);
    } else {
        push_preview_layer(
            &mut layers,
            "custom_template",
            "system",
            5,
            settings.agent_system_prompt.0.clone(),
        );
    }

    // task 模式追加规划器/执行者/汇总者三层固定提示词(单一来源 task_service/prompt.rs)
    if matches!(mode, AppMode::Task) {
        use crate::services::task_service::prompt as task_prompts;
        push_preview_layer(
            &mut layers,
            "task_planner_prompt",
            "system",
            5,
            task_prompts::PLANNER_PROMPT,
        );
        push_preview_layer(
            &mut layers,
            "task_executor_prompt",
            "system",
            5,
            task_prompts::EXECUTOR_PROMPT,
        );
        // 执行者工具纪律段(提交 3 · D3-c):**条件注入**——只有本轮真的下发了工具时
        // 才追加在 EXECUTOR_PROMPT 之后(legacy 的步骤没有工具,不发这一段)。
        // 预览无「本轮有没有工具」的概念,故按「工具档」形态展示;口径写进
        // docs/契约-协议与配置.md 第五节,避免读者以为它恒在。
        push_preview_layer(
            &mut layers,
            "task_executor_tool_discipline",
            "system",
            5,
            task_prompts::EXECUTOR_TOOL_DISCIPLINE,
        );
        // 视觉验证纪律段(视觉能力包 D4;修复批次扩充到「任一图像工具」):同样是
        // **条件注入**——仅当本轮工具面含图像工具(视觉三件 ∪ screenshot)时追加在
        // 工具纪律之后。预览按「工具档」形态展示(与上一条同口径)。
        push_preview_layer(
            &mut layers,
            "task_executor_vision_discipline",
            "system",
            5,
            task_prompts::VISION_VERIFY_DISCIPLINE,
        );
        push_preview_layer(
            &mut layers,
            "task_summarizer_prompt",
            "system",
            5,
            task_prompts::SUMMARIZER_PROMPT,
        );
    }

    // 任务模式注入**恒隔离**(TM-SET-2):task 模式一律不推送注入层——预览必须与真实
    // 下发一致(docs/契约-协议与配置.md 第五节)。该行为原由 task_prompt_inject_enabled
    // 门控(2026-09-10 F2 实跑修复的默认侧);开关退役后固定为「不注入」,无恢复通道。
    let inject_gated = matches!(mode, AppMode::Task);
    if !inject_gated {
        let inject = state
            .prompt_inject
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get()
            .clone();
        match inject.mode {
            crate::services::prompt_inject_service::InjectMode::Simple => push_preview_layer(
                &mut layers,
                "simple_inject",
                "system",
                4,
                inject.simple_inject_text(),
            ),
            crate::services::prompt_inject_service::InjectMode::Complex => {
                for floor in inject.enabled_floors_sorted() {
                    push_preview_layer(
                        &mut layers,
                        format!("complex_floor:{}", floor.name),
                        floor.role.as_str(),
                        4,
                        floor.content.clone(),
                    );
                }
            }
        }
    }

    // 文学能力包(LIT-3):位置 4(system 尾)增强段——在注入层之后(真实下发顺序:
    // 简单注入/楼层 → 禁词提示 → 文学增强段 → system 压入消息)。判据与文本单一出处
    // 在设置层方法;角色扮演侧专属(task 模式不经引擎装配点,不推送)。
    if matches!(mode, AppMode::Roleplay) {
        if let Some(tail) = literary_tail {
            push_preview_layer(&mut layers, "literary_enhancement", "system", 4, tail);
        }
    }

    if let Some(character_id) = query.character_id.as_deref() {
        // 角色卡 + 世界书条目读取(同步 SQLite)合并进同一阻塞任务(DB 并发改造)
        let cid = character_id.to_string();
        let characters = state.characters.clone();
        let world_books = state.world_books.clone();
        let character = state
            .db_call(move || {
                let character = characters.get(&cid);
                let entries = character
                    .as_ref()
                    .map(|_| world_books.collect_entries_for_character(&cid))
                    .unwrap_or_default();
                (character, entries)
            })
            .await
            .unwrap_or((None, Vec::new()));
        if let (Some(character), wb_entries) = character {
            let entries = {
                let mut v = character
                    .data_raw
                    .as_ref()
                    .map(crate::parsing::world_book::character_book_entries)
                    .unwrap_or_default();
                v.extend(wb_entries);
                v
            };
            push_preview_layer(
                &mut layers,
                "character_card",
                "system",
                3,
                format!(
                    "角色名：{}\n角色描述：{}",
                    character.chara_name, character.description
                ),
            );
            let summary = entries
                .iter()
                .filter(|entry| entry.enabled)
                .map(|entry| {
                    format!(
                        "[{}] kind={} role={} length={} hash={}",
                        entry.comment,
                        if entry.constant {
                            "constant"
                        } else {
                            "triggered"
                        },
                        entry.role.as_deref().unwrap_or(if entry.constant {
                            "system"
                        } else {
                            "user"
                        }),
                        entry.content.chars().count(),
                        stable_history_hash(&entry.content),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            push_preview_layer(&mut layers, "world_book", "metadata", 3, summary);
        }
    }

    if let Some(session_id) = query.session_id.as_deref() {
        // 历史摘要读取(同步 SQLite)挪进阻塞线程池(DB 并发改造)
        let sessions = state.sessions.clone();
        let sid = session_id.to_string();
        let history = state
            .db_call(move || sessions.get_messages(&sid))
            .await
            .unwrap_or_default();
        let summary = history
            .iter()
            .map(|message| {
                format!(
                    "role={} length={} hash={}",
                    message.role,
                    message.content.chars().count(),
                    stable_history_hash(&message.content)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        push_preview_layer(&mut layers, "history_summary", "metadata", 2, summary);
    }

    if let Some(flow) = state
        .flow
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get()
        .filter(|flow| flow.enabled)
    {
        for (index, step) in flow.steps.iter().filter(|step| step.enabled).enumerate() {
            if let Some(prompt) = step
                .system_prompt
                .as_deref()
                .filter(|text| !text.trim().is_empty())
            {
                push_preview_layer(
                    &mut layers,
                    format!("step_instruction:{}", index + 1),
                    "system",
                    0,
                    prompt,
                );
            }
        }
    }

    if !settings.reflect_prompt.trim().is_empty() {
        push_preview_layer(
            &mut layers,
            "reflection",
            "system",
            0,
            settings.reflect_prompt,
        );
    }
    if !settings.reflect_advice_prompt.trim().is_empty() {
        push_preview_layer(
            &mut layers,
            "reflection_advice",
            settings.reflect_advice_role,
            0,
            settings.reflect_advice_prompt,
        );
    }
    // 文学能力包(LIT-3/LIT-6):位置 0 文风段与 Author's Note 段——都在反思建议之后、
    // 预设尾部之前,且**文风段在 AN 段之前**(与真实拼装同序);随尾部角色
    // (preset_tail_role,与真实下发同判据);按 untrusted_boundary 包裹展示
    // (「预览即真实下发」:真实拼装侧即按此包裹)。
    if matches!(mode, AppMode::Roleplay) {
        if let Some(style) = literary_style {
            push_preview_layer(
                &mut layers,
                "literary_style",
                settings.preset_tail_role.clone(),
                0,
                crate::services::prompt_kit::untrusted_boundary("literary_style", style),
            );
        }
        if let Some(note) = literary_note {
            push_preview_layer(
                &mut layers,
                "literary_note",
                settings.preset_tail_role.clone(),
                0,
                crate::services::prompt_kit::untrusted_boundary("literary_note", note),
            );
        }
    }
    if !settings.preset_tail_prompt.trim().is_empty() {
        push_preview_layer(
            &mut layers,
            "preset_tail",
            settings.preset_tail_role,
            0,
            settings.preset_tail_prompt,
        );
    }

    Json(json!({
        "ok": true,
        "note": "历史仅显示角色、长度与哈希；预览不包含聊天正文或 API Key。步骤指令与工具指南在具体运行步骤确定后追加。",
        "layers": layers,
    }))
    .into_response()
}

/// PUT /api/settings/agent-prompt:原子保存主 Agent 提示词到 DATA_DIR。
pub async fn save_agent_prompt(
    State(state): State<Arc<AppState>>,
    JsonBody(body): JsonBody<SaveAgentPromptBody>,
) -> Response {
    let path = state.runtime_prompt.path().display().to_string();
    match state.runtime_prompt.write(&body.content) {
        Ok(_) => Json(json!({ "ok": true, "path": path })).into_response(),
        Err(e) => validation(e),
    }
}
