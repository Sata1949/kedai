// 运行期设置服务:API 连接(Base URL / Key / 模型)与生成参数(temperature/top_p/max_tokens/上下文窗口)
// 持久化到 data/settings.json;存在则优先于 .env,支持前端直接编辑(PUT /api/settings)。
//
// 凭据安全:openai_api_key 字段在内存中始终是明文(供连接器直接使用),但落盘前经
// secret_store::protect 加密(Windows DPAPI,绑定当前用户),load 时解密。旧版明文
// settings.json 可直接读取,并在下次保存时自动升级为密文。
use crate::config::AppConfig;
use crate::services::secret_store;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 顶层应用模式(角色扮演 / 任务工作台)。设置按此维度隔离:
/// 连接信息(Base URL / API Key / 模型)全局共享,生成参数与 Agent 配置按模式覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Roleplay,
    Task,
}

impl AppMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AppMode::Roleplay => "roleplay",
            AppMode::Task => "task",
        }
    }

    /// 从查询参数解析;缺省/非法值回退 roleplay(兼容旧客户端不带 mode)。
    pub fn parse(s: &str) -> AppMode {
        match s {
            "task" => AppMode::Task,
            _ => AppMode::Roleplay,
        }
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSettings {
    pub openai_base_url: String,
    /// API Key:内存中为明文;持久化时由 save() 加密、load() 解密(见 secret_store)
    pub openai_api_key: String,
    pub model: String,
    pub default_temperature: f64,
    pub default_top_p: f64,
    pub default_max_tokens: u32,
    /// 上下文窗口上限(token):历史超出后按时间裁剪
    pub max_context_tokens: u32,
    /// Agent 系统提示词(roleplay 权威值,类型化隔离;空 = 使用内置默认模板;
    /// 支持 {{character_name}} {{character_description}} {{world_info}} 占位符)。
    /// task 模式的有效值由 for_mode 经覆盖层计算,不直接读本字段。
    #[serde(default)]
    pub agent_system_prompt: RoleplayPromptConfig,
    /// 联网搜索端点(search 工具;默认 DuckDuckGo HTML 接口,可换成自建 SearXNG 等)
    #[serde(default)]
    pub search_endpoint: String,
    /// mvu 变量状态注入位置:system(默认,世界书并入 system 提示词)/
    /// user_tail(追加到最新用户消息尾部,system+早期历史前缀稳定,前缀缓存友好)
    #[serde(default)]
    pub mvu_vars_position: String,
    /// 变量两步生成独立温度档(默认 None = 沿用内置 0.3;范围 0..=2.0)。
    /// 与正文 default_temperature 解耦,允许变量调用单独降温度以提高结构化遵循度(G3)。
    #[serde(default)]
    pub mvu_temperature: Option<f64>,
    /// 变量两步生成独立模型(默认 None = 与正文共用同一连接器/模型)。
    /// P2 仅预留字段,暂不接双模型(文档 D-1:先独立温度档,独立模型按需)。
    #[serde(default)]
    pub mvu_model: Option<String>,
    /// 反思提示词(空 = 使用机械规则检查;非空 = 反思步骤改为调用 LLM 按本提示词判定)
    #[serde(default)]
    pub reflect_prompt: String,
    /// 复杂模式预设尾部提示词(空 = 禁用):拼入最新用户消息尾部,位于位置1 世界书激发之后
    #[serde(default)]
    pub preset_tail_prompt: String,
    /// 预设尾部提示词注入角色(user / assistant;system 钳制为 user)
    #[serde(default = "default_user_role")]
    pub preset_tail_role: String,
    /// 反思失败建议的可选补充说明(空 = 仅自动建议):主体建议由引擎自动生成(≤200 token),
    /// 反思未通过放弃重试时注入位置0(位置1 激发之后、预设尾部之前);本字段附加在自动建议之后
    #[serde(default)]
    pub reflect_advice_prompt: String,
    /// 反思失败建议注入角色(user / assistant;system 钳制为 user)
    #[serde(default = "default_user_role")]
    pub reflect_advice_role: String,
    /// 放行模式:true = 除黑名单工具外自动放行,不弹授权框
    #[serde(default)]
    pub bypass_mode: bool,
    /// 放行模式黑名单(即使放行模式开启,这些工具仍需要授权)
    #[serde(default = "default_bypass_blacklist")]
    pub bypass_blacklist: Vec<String>,
    /// AGENT/CUSTOM 模式工具循环轮次上限(默认 32;每轮可执行多个工具调用,
    /// 达到上限后停止调用工具并输出当前结果)
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: u32,
    /// 工具循环历史保留的最近完整轮数(R3b;默认 4,钳 1..=32):
    /// 超出后最老轮的 tool 结果原地替换为短摘要(配对不破坏),防止全量回灌无界膨胀
    #[serde(default = "default_tool_history_keep_rounds")]
    pub tool_history_keep_rounds: u32,
    /// 工具循环历史 token 预算(R3b;默认 16384,钳 1024..=1M;0 = 禁用预算闸门,
    /// 仅 keep_rounds 生效):估算超预算时从最老完整轮起继续摘要,保底最近 1 轮完整
    #[serde(default = "default_tool_history_budget_tokens")]
    pub tool_history_budget_tokens: u32,
    /// HTML 渲染开关(状态栏脚本执行前置条件):true = 开启(需用户主动授权脚本后再开启)
    #[serde(default)]
    pub render_html: bool,
    /// 上下文压缩模式:off(默认,不压缩)/ manual(手动触发)/ auto(token 超阈值自动压缩)
    #[serde(default = "default_compaction_mode")]
    pub compaction_mode: String,
    /// 上下文压缩触发阈值(0.5..=0.95,默认 0.8):auto 模式下历史 token 占比达到该值即压缩
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: f32,
    /// 压缩后保留的最近消息条数(缓存感知管线;默认 4,钳制 >= 2)
    #[serde(default = "default_compaction_keep_recent")]
    pub compaction_keep_recent: u32,
    /// snip 零成本裁剪的消息长度阈值(字节;默认 8192 = 8KB,0 = 禁用 snip)
    #[serde(default = "default_compaction_snip_bytes")]
    pub compaction_snip_bytes: u32,
    /// LLM 请求快照开关(第四点·主题 A):true = 每次下发前把完整消息数组落 llm_requests 表
    /// (含 system/注入/工具消息,可回放调试;默认关闭避免占用磁盘)
    #[serde(default)]
    pub llm_request_log: bool,
    /// 跨会话记忆蒸馏开关(落地项 2):开启后 /api/memory/distill 可用(默认关闭)
    #[serde(default)]
    pub memory_distill_enabled: bool,
    /// 记忆槽注入条数上限(默认 8,钳 0..=50;0 = 等价关闭注入)
    #[serde(default = "default_memory_inject_limit")]
    pub memory_inject_limit: u32,
    /// 技能渐进披露开关(落地项 3;默认开启):system 只注入技能 name+description
    /// 紧凑清单,正文按需 read(type=skill);关闭回退旧行为(不注入清单)
    #[serde(default = "default_skill_progressive_disclosure")]
    pub skill_progressive_disclosure: bool,
    /// 回退快照开关(批次 6.1;默认开启):写工具执行前把逆操作负载落 undo_snapshots 表,
    /// Agent 面板「回退到此处」按快照逆序恢复;关闭后不再产生新快照(存量快照仍可恢复)
    #[serde(default = "default_undo_enabled")]
    pub undo_enabled: bool,
    /// 子智能体最大嵌套深度(落地项 3;默认 2,钳 1..=4;主 Agent 为第 0 层)
    #[serde(default = "default_subagent_max_depth")]
    pub subagent_max_depth: u32,
    /// 子智能体并发上限(落地项 3;默认 6,钳 1..=16;当前顺序派发,作为在飞计数守卫)
    #[serde(default = "default_subagent_max_concurrency")]
    pub subagent_max_concurrency: u32,
    /// 子智能体结果最大字符数(落地项 3;默认 2000,钳 500..=8000;超出截断并附尾注)
    #[serde(default = "default_subagent_result_max_chars")]
    pub subagent_result_max_chars: u32,
    /// MCP stdio 客户端总开关(批次 6.2,L3 隔离;默认关闭):
    /// 开启后启动时装配 mcp_servers 列出的 stdio 服务器,工具以 mcp_ 前缀注册;
    /// v1 仅启动时装配,运行期改动重启后生效
    #[serde(default)]
    pub mcp_enabled: bool,
    /// MCP 服务器列表(默认空 = 不装配任何服务器)
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// 执行者人设完整开关(R3a):false = 精简(默认,旧配置缺省由 serde default 填 false,
    /// 零迁移),true = 完整(含 scenario+mes_example)。权威消费在任务模式人设拼装
    /// (persona_style);task 覆盖层可覆盖,None 沿用本扁平值。
    #[serde(default)]
    pub task_persona_full: bool,
    /// 任务工作台的按模式覆盖项。扁平字段即角色扮演(roleplay)的权威值——引擎直接读
    /// 扁平字段,故 roleplay 不设覆盖层;task 用此覆盖层替换扁平字段的差异项。
    /// 旧 settings.json 无此字段,serde default 为空 = task 沿用扁平值。
    #[serde(default)]
    pub task: ModeSettings,
}

/// 默认工具循环轮次上限
fn default_max_tool_rounds() -> u32 {
    32
}

/// 默认工具循环历史保留轮数(R3b):最近 4 轮完整,更早轮摘要化
fn default_tool_history_keep_rounds() -> u32 {
    4
}

/// 默认工具循环历史 token 预算(R3b):16K 估算 token,覆盖主流模型单轮工具结果规模;
/// 实测膨胀形态(3 步 26K+)在此预算内被收敛到最近几轮
fn default_tool_history_budget_tokens() -> u32 {
    16_384
}

/// 默认放行模式黑名单
fn default_bypass_blacklist() -> Vec<String> {
    vec![
        "delete_file".to_string(),
        "format_disk".to_string(),
        "modify_system".to_string(),
        "registry_write".to_string(),
    ]
}

/// 默认尾部注入角色(user;旧配置缺省保持 user 行为)
fn default_user_role() -> String {
    "user".to_string()
}

/// 默认上下文压缩模式(off = 不压缩,保持既有行为)
fn default_compaction_mode() -> String {
    "off".to_string()
}

/// 默认上下文压缩触发阈值(0.8 = 历史 token 达上下文窗口 80% 时自动压缩)
fn default_compaction_threshold() -> f32 {
    0.8
}

/// 默认压缩后保留的最近消息条数(缓存感知管线;沿用原 KEEP_RECENT_MESSAGES 常量值)
fn default_compaction_keep_recent() -> u32 {
    4
}

/// 默认 snip 零成本裁剪消息长度阈值(8KB)
fn default_compaction_snip_bytes() -> u32 {
    8192
}

/// 默认记忆槽注入条数上限(落地项 2)
fn default_memory_inject_limit() -> u32 {
    8
}

/// 默认开启技能渐进披露(落地项 3)
fn default_skill_progressive_disclosure() -> bool {
    true
}

/// 默认开启回退快照(批次 6.1)
fn default_undo_enabled() -> bool {
    true
}

/// 默认子智能体最大嵌套深度(落地项 3)
fn default_subagent_max_depth() -> u32 {
    2
}

/// 默认子智能体并发上限(落地项 3)
fn default_subagent_max_concurrency() -> u32 {
    6
}

/// 默认子智能体结果最大字符数(落地项 3)
fn default_subagent_result_max_chars() -> u32 {
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

/// 默认搜索端点(DuckDuckGo HTML 免费接口,无需 API Key)
pub const DEFAULT_SEARCH_ENDPOINT: &str = "https://html.duckduckgo.com/html/";

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

    /// API Key 脱敏展示(仅保留后 4 位)
    pub fn masked_api_key(&self) -> String {
        mask_key(&self.openai_api_key)
    }
}

/// 脱敏:非空时返回 `****xxxx`(保留后 4 位)
pub fn mask_key(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    let last = key.chars().rev().take(4).collect::<Vec<_>>();
    let last: String = last.into_iter().rev().collect();
    format!("****{last}")
}

/// 规范化 OpenAI 兼容 API 地址(自动补全格式):
/// - 去空白与尾部斜杠
/// - 无协议时补协议:本机地址(localhost/127.x/0.0.0.0/[::1])补 http://,其余补 https://
/// - 无路径时补 `/v1`(OpenAI 兼容服务普遍要求 /v1,缺它请求 /models 会 404)
pub fn normalize_base_url(input: &str) -> String {
    let mut s = input.trim().to_string();
    if s.is_empty() {
        return s;
    }
    // 去尾部斜杠
    while s.ends_with('/') {
        s.pop();
    }
    // 补协议
    if !s.starts_with("http://") && !s.starts_with("https://") {
        let lower = s.to_lowercase();
        let is_local = lower.starts_with("localhost")
            || lower.starts_with("127.")
            || lower.starts_with("0.0.0.0")
            || lower.starts_with("[::1]");
        s = if is_local {
            format!("http://{s}")
        } else {
            format!("https://{s}")
        };
    }
    // 补 /v1(仅当 host 后无任何路径时)
    let (_, rest) = s.split_once("://").unwrap_or(("", s.as_str()));
    let has_path = match rest.find('/') {
        None => false,
        Some(i) => !rest[i..].trim_matches('/').is_empty(),
    };
    if !has_path {
        s.push_str("/v1");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 临时数据目录(测试隔离用)
    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "kedai-settings-test-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 最小 AppConfig(仅供设置加载/保存测试;不读环境变量,避免受本机 .env 影响)
    fn test_cfg() -> AppConfig {
        AppConfig {
            host: "127.0.0.1".into(),
            port: 0,
            data_dir: std::env::temp_dir(),
            log_dir: std::env::temp_dir(),
            web_dist: None,
            connector: "mock".into(),
            openai_base_url: "https://example.com/v1".into(),
            openai_api_key: String::new(),
            openai_model: "test-model".into(),
            default_temperature: 0.8,
            default_top_p: 0.9,
            default_max_tokens: 1024,
            default_max_context_tokens: 65_536,
            log_level: "info".into(),
            api_token: "test-token".into(),
            auth_required: false,
            api_token_injected: true,
            allow_remote: false,
            bootstrap_enabled: true,
            strict_client_header: false,
        }
    }

    /// API Key 落盘应为密文(文件不含明文),load 后还原为明文
    #[test]
    fn api_key_is_encrypted_on_disk_and_restored_on_load() {
        let dir = tmp_dir("enc");
        let mut s = RuntimeSettings::from_config(&test_cfg());
        s.openai_api_key = "sk-secret-value-9999".to_string();
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧版明文 settings.json:可正常读取,且 load 时自动就地迁移为密文
    #[test]
    fn legacy_plaintext_settings_are_migrated_on_load() {
        let dir = tmp_dir("migrate");
        let mut s = RuntimeSettings::from_config(&test_cfg());
        s.openai_api_key = "sk-legacy-plain-1234".to_string();
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
        let raw = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !raw.contains("sk-legacy-plain-1234"),
            "load 后应已就地迁移为密文: {raw}"
        );
        // 迁移后再次 load 仍应得到同一明文
        let again = RuntimeSettings::load(&dir, &test_cfg());
        assert_eq!(again.openai_api_key, "sk-legacy-plain-1234");
        let _ = std::fs::remove_dir_all(&dir);
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
        let _ = std::fs::remove_dir_all(&dir);
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 记忆设置(落地项 2):默认 关闭蒸馏 / 注入上限 8;
    /// 旧版 settings.json 缺字段时 serde default 补齐;越界值钳回默认;保存往返还原。
    #[test]
    fn memory_settings_defaults_clamp_and_roundtrip() {
        let cfg = test_cfg();
        let s = RuntimeSettings::from_config(&cfg);
        assert!(!s.memory_distill_enabled, "蒸馏默认关闭");
        assert_eq!(s.memory_inject_limit, 8, "注入上限默认 8");

        // 旧版配置缺字段:serde default 补齐,行为与默认一致
        let dir = tmp_dir("memory-legacy");
        let mut json = serde_json::to_value(&s).unwrap();
        json.as_object_mut()
            .unwrap()
            .remove("memory_distill_enabled");
        json.as_object_mut().unwrap().remove("memory_inject_limit");
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
        let loaded = RuntimeSettings::load(&dir, &cfg);
        assert!(!loaded.memory_distill_enabled);
        assert_eq!(loaded.memory_inject_limit, 8);

        // 越界(>50)钳回默认;0 合法(等价关闭注入)
        let dir2 = tmp_dir("memory-clamp");
        let mut s2 = RuntimeSettings::from_config(&cfg);
        s2.memory_distill_enabled = true;
        s2.memory_inject_limit = 500;
        s2.save(&dir2).unwrap();
        let loaded2 = RuntimeSettings::load(&dir2, &cfg);
        assert!(loaded2.memory_distill_enabled, "开关应保存往返还原");
        assert_eq!(loaded2.memory_inject_limit, 8, "越界上限应钳回 8");

        let dir3 = tmp_dir("memory-zero");
        let mut s3 = RuntimeSettings::from_config(&cfg);
        s3.memory_inject_limit = 0;
        s3.save(&dir3).unwrap();
        assert_eq!(
            RuntimeSettings::load(&dir3, &cfg).memory_inject_limit,
            0,
            "0 是合法值(关闭注入)"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
        let _ = std::fs::remove_dir_all(&dir3);
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

        // task 模式覆盖层:Some 覆盖扁平值,None 沿用
        let mut s4 = RuntimeSettings::from_config(&cfg);
        s4.task.subagent_max_depth = Some(1);
        let task_view = s4.for_mode(AppMode::Task);
        assert_eq!(task_view.subagent_max_depth, 1, "task 覆盖应生效");
        assert_eq!(task_view.subagent_max_concurrency, 6, "未覆盖项沿用扁平值");
        let rp_view = s4.for_mode(AppMode::Roleplay);
        assert_eq!(rp_view.subagent_max_depth, 2, "roleplay 读扁平权威值");

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

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
        let _ = std::fs::remove_dir_all(&dir3);
    }

    /// MCP 设置(批次 6.2):默认 关/空列表;旧版 settings.json 缺字段时 serde default 补齐;
    /// 保存往返还原;缺名/缺命令条目被清理;task 覆盖层生效。
    #[test]
    fn mcp_settings_defaults_sanitize_roundtrip_and_mode_override() {
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

        // 缺省字段(旧客户端手写条目只给 name/command):args 默认空、enabled 默认 true
        let partial: McpServerConfig =
            serde_json::from_str(r#"{"name":"a","command":"b"}"#).unwrap();
        assert!(partial.args.is_empty());
        assert!(partial.enabled, "条目 enabled 缺省应为 true");

        // task 覆盖层:Some 覆盖扁平值,None 沿用
        let mut s3 = RuntimeSettings::from_config(&cfg);
        s3.task.mcp_enabled = Some(true);
        let task_view = s3.for_mode(AppMode::Task);
        assert!(task_view.mcp_enabled, "task 覆盖应生效");
        assert!(task_view.mcp_servers.is_empty(), "未覆盖项沿用扁平值");
        assert!(
            !s3.for_mode(AppMode::Roleplay).mcp_enabled,
            "roleplay 读扁平权威值"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
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
            serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                .unwrap();
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
        let _ = std::fs::remove_dir_all(&dir);
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

    /// R3a:task_persona_full 默认精简(None/false 同义),旧配置零迁移;
    /// 覆盖层 Some(true) = 完整人设(现状四段);serde 线格式 None 省略不落 null。
    /// 口径见 docs/模式提示词边界.md 第一节。
    #[test]
    fn task_persona_full_defaults_slim_and_roundtrips() {
        // 覆盖层 serde 三态:缺字段/null → None(沿用扁平);None 序列化省略字段
        let missing: ModeSettings = serde_json::from_str("{}").unwrap();
        assert!(
            missing.task_persona_full.is_none(),
            "缺字段应为 None(沿用扁平值)"
        );
        let null: ModeSettings = serde_json::from_str(r#"{"task_persona_full": null}"#).unwrap();
        assert!(null.task_persona_full.is_none(), "null 应为 None");
        let some: ModeSettings = serde_json::from_str(r#"{"task_persona_full": true}"#).unwrap();
        assert_eq!(some.task_persona_full, Some(true));
        assert!(
            !serde_json::to_string(&missing)
                .unwrap()
                .contains("task_persona_full"),
            "None 应省略字段而不是落 null"
        );

        let cfg = test_cfg();
        // 旧版 settings.json:无 task 覆盖层、无扁平字段 → 读入后默认精简(false)
        let dir = tmp_dir("persona-full");
        let mut legacy = serde_json::to_value(RuntimeSettings::from_config(&cfg)).unwrap();
        let obj = legacy.as_object_mut().unwrap();
        obj.remove("task");
        obj.remove("task_persona_full");
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();
        let loaded = RuntimeSettings::load(&dir, &cfg);
        assert!(!loaded.task_persona_full, "旧配置缺省应为精简(false)");
        assert!(
            !loaded.for_mode(AppMode::Task).task_persona_full,
            "None 覆盖层沿用扁平值 = 精简(旧配置兼容)"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // 覆盖层三分支:Some(true)=完整;Some(false)/None=精简;roleplay 读扁平权威值
        let mut s = RuntimeSettings::from_config(&cfg);
        s.task.task_persona_full = Some(true);
        assert!(
            s.for_mode(AppMode::Task).task_persona_full,
            "Some(true) = 完整人设"
        );
        assert!(
            !s.for_mode(AppMode::Roleplay).task_persona_full,
            "roleplay 读扁平权威值(false),task 覆盖层不得污染"
        );
        s.task.task_persona_full = Some(false);
        assert!(
            !s.for_mode(AppMode::Task).task_persona_full,
            "Some(false) = 精简"
        );
    }
}
