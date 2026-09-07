// 生成参数:模式隔离类型(Roleplay/Task 提示词、ModeSettings 覆盖层)、serde 默认值函数、
// from_config 默认构建与 for_mode 按模式覆盖合并。
use crate::config::AppConfig;
use serde::{Deserialize, Serialize};

use super::connection::DEFAULT_SEARCH_ENDPOINT;
use super::{AppMode, RuntimeSettings};

/// 角色扮演 Agent 系统提示词(类型级模式隔离,WP7 抗多模式提示词混淆):
/// RuntimeSettings 扁平字段的权威类型。serde transparent = 序列化为裸字符串,
/// settings.json 与 /api/settings JSON 线格式逐字节不变。
/// 语义:空串 = 使用内置默认角色扮演模板。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(transparent)]
pub struct RoleplayPromptConfig(pub String);

/// 任务模式 Agent 系统提示词覆盖值(类型级模式隔离,WP7)。
/// 仅出现在 task 覆盖层(`Option<TaskPromptConfig>`),三态语义与旧 Option<String> 同构:
/// 缺字段/null → None(沿用,for_mode 注入内置任务向默认词);
/// `""` → Some(空串)(显式清空,不注入);`"v"` → Some(v)(覆盖)。
/// serde transparent = 序列化为裸字符串;None 由外层 Option 的 skip_serializing_if 省略,
/// 不落 null,settings.json 与 API 线格式零变化。
/// 与 RoleplayPromptConfig 类型不同源:task 覆盖值无法被误赋给 roleplay 扁平字段,
/// 「task 不继承 roleplay 人设词」由类型系统而非注释约定保证(for_mode 是唯一转换点)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct TaskPromptConfig(pub String);

/// MCP 服务器配置(批次 6.2,L3 隔离):stdio 托管子进程。
/// v1 仅在启动时装配(改设置后重启生效,无热重连)。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct McpServerConfig {
    /// 服务器名(工具名前缀来源,装配时 sanitize 为 [a-z0-9_])
    #[serde(default)]
    pub name: String,
    /// 可执行命令(如 npx / node / 某个 exe)
    #[serde(default)]
    pub command: String,
    /// 命令行参数
    #[serde(default)]
    pub args: Vec<String>,
    /// 该服务器是否启用(默认 true;false = 保留配置但启动时不装配)
    #[serde(default = "default_mcp_server_enabled")]
    pub enabled: bool,
}

/// 按模式的设置覆盖项:所有字段 `Option`,`Some` 表示覆盖共享默认,`None` 表示沿用共享值。
/// 仅覆盖生成参数与 Agent 配置;连接信息(openai_base_url/openai_api_key/model)始终共享。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModeSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u32>,
    /// task 覆盖层的 Agent 系统提示词(类型化为 TaskPromptConfig,与 roleplay 扁平值类型隔离)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_system_prompt: Option<TaskPromptConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mvu_vars_position: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflect_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_tail_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_tail_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflect_advice_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflect_advice_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_blacklist: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_rounds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_html: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_threshold: Option<f32>,
    /// 压缩后保留的最近消息条数(缓存感知管线,默认 4)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_keep_recent: Option<u32>,
    /// snip 零成本裁剪的消息长度阈值(字节;0 = 禁用 snip,默认 8192)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_snip_bytes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_request_log: Option<bool>,
    /// 跨会话记忆蒸馏开关(落地项 2;默认关闭)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_distill_enabled: Option<bool>,
    /// 记忆槽注入条数上限(0 = 关闭注入;默认 8,钳 0..=50)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_inject_limit: Option<u32>,
    /// 技能渐进披露开关(落地项 3;默认 true)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_progressive_disclosure: Option<bool>,
    /// 回退快照开关(批次 6.1;默认 true):写工具执行前留逆操作快照,可「回退到此处」
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_enabled: Option<bool>,
    /// 子智能体最大嵌套深度(默认 2,钳 1..=4;主 Agent 为第 0 层)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_max_depth: Option<u32>,
    /// 子智能体并发上限(默认 6,钳 1..=16;顺序执行下为在飞计数守卫)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_max_concurrency: Option<u32>,
    /// 子智能体结果最大字符数(默认 2000,钳 500..=8000;超出截断并附尾注)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_result_max_chars: Option<u32>,
    /// MCP stdio 客户端总开关(批次 6.2;默认关,仅启动时装配,改后重启生效)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_enabled: Option<bool>,
    /// MCP 服务器列表(覆盖语义与 bypass_blacklist 一致)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<McpServerConfig>>,
    /// 执行者人设完整开关(R3a):None/false = 精简(description+personality 两段),
    /// true = 完整(再加 scenario+mes_example)。仅任务模式人设拼装消费;
    /// roleplay 引擎侧无人设注入点,扁平值仅作 task 覆盖层 None 时的沿用值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_persona_full: Option<bool>,
}

/// 默认工具循环轮次上限
pub(super) fn default_max_tool_rounds() -> u32 {
    32
}

/// 默认工具循环历史保留轮数(R3b):最近 4 轮完整,更早轮摘要化
pub(super) fn default_tool_history_keep_rounds() -> u32 {
    4
}

/// 默认工具循环历史 token 预算(R3b):16K 估算 token,覆盖主流模型单轮工具结果规模;
/// 实测膨胀形态(3 步 26K+)在此预算内被收敛到最近几轮
pub(super) fn default_tool_history_budget_tokens() -> u32 {
    16_384
}

/// 默认放行模式黑名单
pub(super) fn default_bypass_blacklist() -> Vec<String> {
    vec![
        "delete_file".to_string(),
        "format_disk".to_string(),
        "modify_system".to_string(),
        "registry_write".to_string(),
    ]
}

/// 默认尾部注入角色(user;旧配置缺省保持 user 行为)
pub(super) fn default_user_role() -> String {
    "user".to_string()
}

/// 默认上下文压缩模式(off = 不压缩,保持既有行为)
pub(super) fn default_compaction_mode() -> String {
    "off".to_string()
}

/// 默认上下文压缩触发阈值(0.8 = 历史 token 达上下文窗口 80% 时自动压缩)
pub(super) fn default_compaction_threshold() -> f32 {
    0.8
}

/// 默认压缩后保留的最近消息条数(缓存感知管线;沿用原 KEEP_RECENT_MESSAGES 常量值)
pub(super) fn default_compaction_keep_recent() -> u32 {
    4
}

/// 默认 snip 零成本裁剪消息长度阈值(8KB)
pub(super) fn default_compaction_snip_bytes() -> u32 {
    8192
}

/// 默认记忆槽注入条数上限(落地项 2)
pub(super) fn default_memory_inject_limit() -> u32 {
    8
}

/// 默认开启技能渐进披露(落地项 3)
pub(super) fn default_skill_progressive_disclosure() -> bool {
    true
}

/// 默认开启回退快照(批次 6.1)
pub(super) fn default_undo_enabled() -> bool {
    true
}

/// 默认子智能体最大嵌套深度(落地项 3)
pub(super) fn default_subagent_max_depth() -> u32 {
    2
}

/// 默认子智能体并发上限(落地项 3)
pub(super) fn default_subagent_max_concurrency() -> u32 {
    6
}

/// 默认子智能体结果最大字符数(落地项 3)
pub(super) fn default_subagent_result_max_chars() -> u32 {
    2000
}

/// MCP 服务器条目默认启用(批次 6.2;显式 enabled:false 才跳过装配)
fn default_mcp_server_enabled() -> bool {
    true
}

/// 任务模式缺省 Agent 系统提示词(任务向,与 task_service 执行者指令互补)。
/// 仅 task 覆盖层未显式设置(None)时使用;显式清空(Some(""))表示不注入。
pub fn default_task_agent_prompt() -> String {
    "你是高效的任务执行智能体,直接、准确地完成用户给出的目标。只输出结果本身:\
     不复述指令、不模拟对话、不以角色扮演口吻写作;除非用户明确要求,不使用 Markdown 标题。\
     输出语言跟随用户目标的语言。"
        .into()
}

impl RuntimeSettings {
    /// 从环境配置构建默认设置
    pub fn from_config(cfg: &AppConfig) -> Self {
        RuntimeSettings {
            openai_base_url: cfg.openai_base_url.clone(),
            openai_api_key: cfg.openai_api_key.clone(),
            model: cfg.openai_model.clone(),
            default_temperature: cfg.default_temperature,
            default_top_p: cfg.default_top_p,
            default_max_tokens: cfg.default_max_tokens,
            max_context_tokens: cfg.default_max_context_tokens,
            agent_system_prompt: RoleplayPromptConfig(String::new()),
            search_endpoint: DEFAULT_SEARCH_ENDPOINT.to_string(),
            mvu_vars_position: "system".to_string(),
            mvu_temperature: None,
            mvu_model: None,
            reflect_prompt: String::new(),
            preset_tail_prompt: String::new(),
            preset_tail_role: "user".to_string(),
            reflect_advice_prompt: String::new(),
            reflect_advice_role: "user".to_string(),
            bypass_mode: false,
            bypass_blacklist: default_bypass_blacklist(),
            max_tool_rounds: default_max_tool_rounds(),
            tool_history_keep_rounds: default_tool_history_keep_rounds(),
            tool_history_budget_tokens: default_tool_history_budget_tokens(),
            render_html: false,
            compaction_mode: default_compaction_mode(),
            compaction_threshold: default_compaction_threshold(),
            compaction_keep_recent: default_compaction_keep_recent(),
            compaction_snip_bytes: default_compaction_snip_bytes(),
            llm_request_log: false,
            memory_distill_enabled: false,
            memory_inject_limit: default_memory_inject_limit(),
            skill_progressive_disclosure: default_skill_progressive_disclosure(),
            undo_enabled: default_undo_enabled(),
            subagent_max_depth: default_subagent_max_depth(),
            subagent_max_concurrency: default_subagent_max_concurrency(),
            subagent_result_max_chars: default_subagent_result_max_chars(),
            mcp_enabled: false,
            mcp_servers: Vec::new(),
            task_persona_full: false,
            task: ModeSettings::default(),
        }
    }

    /// 返回指定模式的合并后有效设置。扁平字段即 roleplay 权威值(引擎直接读);
    /// task 模式则把 task 覆盖层(Some)替换到扁平字段上,None 沿用扁平值。
    /// 旧 settings.json 无覆盖层时 task 返回扁平值,行为与改造前一致。
    pub fn for_mode(&self, mode: AppMode) -> RuntimeSettings {
        let ov = match mode {
            AppMode::Roleplay => return self.clone(),
            AppMode::Task => &self.task,
        };
        let mut out = self.clone();
        if let Some(v) = ov.default_temperature {
            out.default_temperature = v;
        }
        if let Some(v) = ov.default_top_p {
            out.default_top_p = v;
        }
        if let Some(v) = ov.default_max_tokens {
            out.default_max_tokens = v;
        }
        if let Some(v) = ov.max_context_tokens {
            out.max_context_tokens = v;
        }
        // agent_system_prompt 不回退扁平值:扁平值(RoleplayPromptConfig)多为角色扮演人设词,
        // 直接继承会污染任务执行;None 注入内置任务向默认词,Some("") 尊重用户显式留空。
        // TaskPromptConfig → RoleplayPromptConfig 的显式构造是本隔离的唯一转换点(类型不同源,
        // 绕过本 match 的隐式继承无法通过编译)。
        out.agent_system_prompt = match &ov.agent_system_prompt {
            Some(v) => RoleplayPromptConfig(v.0.clone()),
            None => RoleplayPromptConfig(default_task_agent_prompt()),
        };
        if let Some(v) = &ov.search_endpoint {
            out.search_endpoint = v.clone();
        }
        if let Some(v) = &ov.mvu_vars_position {
            out.mvu_vars_position = v.clone();
        }
        if let Some(v) = &ov.reflect_prompt {
            out.reflect_prompt = v.clone();
        }
        if let Some(v) = &ov.preset_tail_prompt {
            out.preset_tail_prompt = v.clone();
        }
        if let Some(v) = &ov.preset_tail_role {
            out.preset_tail_role = v.clone();
        }
        if let Some(v) = &ov.reflect_advice_prompt {
            out.reflect_advice_prompt = v.clone();
        }
        if let Some(v) = &ov.reflect_advice_role {
            out.reflect_advice_role = v.clone();
        }
        if let Some(v) = ov.bypass_mode {
            out.bypass_mode = v;
        }
        if let Some(v) = &ov.bypass_blacklist {
            out.bypass_blacklist = v.clone();
        }
        if let Some(v) = ov.max_tool_rounds {
            out.max_tool_rounds = v;
        }
        if let Some(v) = ov.render_html {
            out.render_html = v;
        }
        if let Some(v) = &ov.compaction_mode {
            out.compaction_mode = v.clone();
        }
        if let Some(v) = ov.compaction_threshold {
            out.compaction_threshold = v;
        }
        if let Some(v) = ov.compaction_keep_recent {
            out.compaction_keep_recent = v;
        }
        if let Some(v) = ov.compaction_snip_bytes {
            out.compaction_snip_bytes = v;
        }
        if let Some(v) = ov.llm_request_log {
            out.llm_request_log = v;
        }
        if let Some(v) = ov.memory_distill_enabled {
            out.memory_distill_enabled = v;
        }
        if let Some(v) = ov.memory_inject_limit {
            out.memory_inject_limit = v;
        }
        if let Some(v) = ov.skill_progressive_disclosure {
            out.skill_progressive_disclosure = v;
        }
        if let Some(v) = ov.undo_enabled {
            out.undo_enabled = v;
        }
        if let Some(v) = ov.subagent_max_depth {
            out.subagent_max_depth = v;
        }
        if let Some(v) = ov.subagent_max_concurrency {
            out.subagent_max_concurrency = v;
        }
        if let Some(v) = ov.subagent_result_max_chars {
            out.subagent_result_max_chars = v;
        }
        if let Some(v) = ov.mcp_enabled {
            out.mcp_enabled = v;
        }
        if let Some(v) = &ov.mcp_servers {
            out.mcp_servers = v.clone();
        }
        if let Some(v) = ov.task_persona_full {
            out.task_persona_full = v;
        }
        out
    }
}
