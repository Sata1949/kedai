// 运行期设置服务:API 连接(Base URL / Key / 模型)与生成参数(temperature/top_p/max_tokens/上下文窗口)
// 持久化到 data/settings.json;存在则优先于 .env,支持前端直接编辑(PUT /api/settings)。
//
// 凭据安全:openai_api_key 字段在内存中始终是明文(供连接器直接使用),但落盘前经
// secret_store::protect 加密(Windows DPAPI,绑定当前用户),load 时解密。旧版明文
// settings.json 可直接读取,并在下次保存时自动升级为密文。
//
// 目录化拆分(纯代码移动,逻辑不变):
//   connection.rs 连接设置:Base URL 规范化、API Key 脱敏展示、默认搜索端点
//   params.rs     生成参数:模式隔离类型、serde 默认值函数、from_config / for_mode 覆盖合并
//   secret.rs     密钥迁移(DPAPI):settings.json 加载/保存,API Key 加密落盘与旧明文就地迁移
// 本文件保留跨域共享的核心类型(AppMode / RuntimeSettings)并做再导出,
// 保证 `services::settings_service::*` 对外路径不变。
use serde::{Deserialize, Serialize};

// 授权档位词汇已下沉 L1(2026-09-14):L2 直连 models,不经 tools 转发。
use crate::models::tool_policy::AuthorizationMode;

mod connection;
mod connector_pool;
mod params;
mod prompts_literary;
mod secret;

pub use connection::{
    mask_key, normalize_base_url, resolve_connector_target, strip_endpoint_suffix,
    task_mode_default_connection, vision_enabled, ConnectionProfile, CONNECTOR_TYPE_MOCK,
    CONNECTOR_TYPE_OPENAI, DEFAULT_SEARCH_ENDPOINT, MAX_CONNECTIONS,
};
pub use connector_pool::{connection_label, ConnectorPool};
// 推荐档数值类型(定义随提示词常量迁入 prompts_literary;在父模块再导出保证公开可达)
pub use params::{
    default_coding_task_agent_prompt, default_literary_reflect_prompt,
    default_literary_roleplay_agent_prompt, default_literary_task_agent_prompt,
    default_reflect_prompt, default_roleplay_agent_prompt, default_task_agent_prompt,
    is_valid_literary_recommend_preset, is_valid_literary_style_preset, literary_recommend_values,
    literary_style_text, task_idle_floor_secs, LiteraryRecommendSnapshot, McpServerConfig,
    ModeSettings, RoleplayPromptConfig, TaskPromptConfig, TASK_DEFAULT_OUTPUT_TOKENS,
    TASK_DEFAULT_TEMPERATURE, TASK_DEFAULT_TOP_P,
};
pub use prompts_literary::LiteraryRecommendValues;

/// 团队总预算的可调下限(秒,设置页口径,2026-10-26):低于 10 分钟连一个子目标的
/// 「读→改→自测」闭环都跑不完,截断只会制造半成品,故不允许更小的有限值。
pub const TASK_TEAM_BUDGET_MIN_SECS: u32 = 600;

/// 团队总预算的可调上限(秒,设置页口径,2026-10-26):超过该值不再接受为有限预算,
/// 一律归一为 `0`(不限)——「想设得更久」在产品语义里就是「别限时」。
/// **默认值自 TPROF-1(2026-10-27)起改为 `0`(不限)**;1200 仍是有限档的最大值,
/// 需要硬性控成本的用户可显式选 600-1200。
pub const TASK_TEAM_BUDGET_MAX_SECS: u32 = 1200;

// RuntimeSettings 字段的 serde(default = "...") 按名字在本模块作用域解析;
// 默认值函数集中在 params.rs,在此引入保持属性文本不变。
use params::{
    default_authorization_mode, default_bypass_blacklist, default_compaction_keep_recent,
    default_compaction_mode, default_compaction_snip_bytes, default_compaction_threshold,
    default_loop_guard_semantic_max_distinct, default_loop_guard_semantic_min_calls,
    default_loop_guard_semantic_window, default_max_flow_call_depth,
    default_max_flow_calls_per_task, default_max_tool_rounds, default_memory_inject_char_budget,
    default_memory_inject_limit, default_memory_max_entries, default_node_max_context,
    default_session_budget_action, default_session_token_budget,
    default_skill_progressive_disclosure, default_subagent_max_concurrency,
    default_subagent_max_depth, default_subagent_result_max_chars, default_task_idle_timeout_secs,
    default_task_step_budget_secs, default_task_team_budget_secs, default_task_tool_policy,
    default_task_total_budget_secs, default_tool_authorization_timeout_secs,
    default_tool_history_budget_tokens, default_tool_history_keep_rounds, default_undo_enabled,
    default_user_role, default_worldbook_sync_character_enabled,
};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSettings {
    pub openai_base_url: String,
    /// API Key:内存中为明文;持久化时由 save() 加密、load() 解密(见 secret_store)
    pub openai_api_key: String,
    pub model: String,
    /// 多套连接配置(2026-09-22 多套连接批次):**真源**。
    /// 上面的 openai_base_url / openai_api_key / model 自本批次起降为
    /// 「默认连接(active_connection_id)的派生视图」,由 normalize_connections 投影,
    /// 旧客户端、embedding、启动装配、任务记账仍读扁平字段,行为不变。
    /// 空数组 = 尚未播种(load 时按扁平字段播种一条),故**必须** serde default:
    /// 缺键时若整文件反序列化失败,load 会整体回退环境配置(用户设置静默丢失)。
    #[serde(default)]
    pub connections: Vec<ConnectionProfile>,
    /// 默认连接 id;指向不存在或已停用的连接时回退「第一个启用的连接」,全停用则空投影
    #[serde(default)]
    pub active_connection_id: Option<String>,
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
    /// 授权模式(三档,批次授权改造):strict/loose/bypass。
    /// - strict:读/写/删文件都需授权;
    /// - loose:读/写文件放行,删文件需授权(默认);
    /// - bypass:除「系统路径(C 盘)写/删」外一律放行。
    ///
    /// 旧配置无此字段时由 serde default 填 loose,再由 load 侧迁移按 bypass_mode 修正
    /// (见 settings_service::migrate_authorization_mode)。
    #[serde(default = "default_authorization_mode")]
    pub authorization_mode: AuthorizationMode,
    /// 旧版放行模式开关(deprecated):仅用于旧配置迁移,新代码读 authorization_mode。
    /// 保留字段以保证旧 settings.json 与旧 API 调用方可解析(不落 null)。
    #[serde(default)]
    pub bypass_mode: bool,
    /// 「始终需授权」清单(重定义原放行模式黑名单):名单内工具在三档模式下都需授权,
    /// 用于把高危工具钉死在授权之后。默认空(不额外拦截)。
    #[serde(default = "default_bypass_blacklist")]
    pub bypass_blacklist: Vec<String>,
    /// 授权等待超时(秒;默认 300,钳 30..=1800):仅对需要授权的聊天路径生效,
    /// 超时后回灌 authorization_timeout 错误并发出可见事件。
    #[serde(default = "default_tool_authorization_timeout_secs")]
    pub tool_authorization_timeout_secs: u32,
    /// 任务模式工具策略(批次授权改造):all / deny_dangerous / allowlist。
    /// 任务模式无 UI 授权上下文,未放行工具直接拒绝而非等待授权。
    #[serde(default = "default_task_tool_policy")]
    pub task_tool_policy: String,
    /// 任务模式 allowlist 策略下的工具白名单(task_tool_policy=allowlist 时生效)
    #[serde(default)]
    pub task_tool_allowlist: Vec<String>,
    /// 聊天 AGENT 模式工具循环轮次上限(默认 32;每轮可执行多个工具调用,
    /// 达到上限后停止调用工具并输出当前结果)。任务模式已取消默认轮次限制,
    /// 不再消费本字段(2026-10-19)。
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: u32,
    /// 流程**动态调用**的嵌套深度上限(A 批 A3;TPROF-1 起默认 3,钳 1..=5)。
    ///
    /// 口径同 7b:只对「宽松档节点调 `run_flow`」生效;静态子图(`sub_flow_id`)与注册期
    /// 的嵌套校验仍按 `MAX_SUB_FLOW_DEPTH = 3`(那是**结构**上限,不是运行期成本闸)。
    #[serde(default = "default_max_flow_call_depth")]
    pub max_flow_call_depth: u32,
    /// 单个任务内的流程调用**次数**上限(A 批 A3;TPROF-1 起默认 24,钳 1..=64)。
    #[serde(default = "default_max_flow_calls_per_task")]
    pub max_flow_calls_per_task: u32,
    /// 节点默认上下文上限(A 批 A4;默认 **0 = 不裁剪**,否则钳 256..=1048576)。
    ///
    /// 与节点级 `PlanStep.max_context` 的关系:节点值优先,缺省才用本设置;两者皆空
    /// 即与批次 8 之前逐字节一致。缺省关闭是刻意的——默认套预算会截断当前正常工作的
    /// 长输入,属可观测的行为变化(见 `计划.md` 裁定 26 口径 2)。
    #[serde(default = "default_node_max_context")]
    pub default_node_max_context: u32,
    /// 工具循环历史保留的最近完整轮数(R3b;TPROF-1 起默认 12,钳 1..=32):
    /// 超出后最老轮的 tool 结果原地替换为短摘要(配对不破坏),防止全量回灌无界膨胀
    #[serde(default = "default_tool_history_keep_rounds")]
    pub tool_history_keep_rounds: u32,
    /// 工具循环历史 token 预算(R3b;TPROF-1 起默认 65536,钳 1024..=1M;0 = 禁用预算闸门,
    /// 仅 keep_rounds 生效):估算超预算时从最老完整轮起继续摘要,保底最近 1 轮完整
    #[serde(default = "default_tool_history_budget_tokens")]
    pub tool_history_budget_tokens: u32,
    /// 单次生成的 token 预算(HB-1 成本护栏;0 = 关闭,默认 0;否则钳 1024..=1e9):
    /// 工具循环累计 prompt+completion 达到该值即按 session_budget_action 处置。
    /// 与 tool_history_budget_tokens 不同——后者管上下文裁剪,本项管成本上限。
    #[serde(default = "default_session_token_budget")]
    pub session_token_budget: u32,
    /// 预算超限动作(HB-1;warn = 只提示一次后继续(默认),stop = 提示并停止本轮工具循环)
    #[serde(default = "default_session_budget_action")]
    pub session_budget_action: String,
    /// 语义熔断窗口(HB-2;默认 16,钳 4..=64):近 W 次工具调用内做「同工具 + 输出无变化」判定
    #[serde(default = "default_loop_guard_semantic_window")]
    pub loop_guard_semantic_window: u32,
    /// 语义熔断同工具调用次数下限(HB-2;默认 12;0 = 关闭本闸门,否则钳 4..=64):
    /// 窗口内同工具累计达到该次数且输出去重不超过上限即判空转
    #[serde(default = "default_loop_guard_semantic_min_calls")]
    pub loop_guard_semantic_min_calls: u32,
    /// 语义熔断输出指纹去重上限(HB-2;默认 2,钳 1..=8):去重后不超过该值即判空转
    #[serde(default = "default_loop_guard_semantic_max_distinct")]
    pub loop_guard_semantic_max_distinct: u32,
    /// 任务步骤墙钟预算秒数(提交 3 · D3;默认 3600 = 开,0 = 关,否则钳 1..=86400):
    /// 单次工具循环的墙钟上限,到点带着已有产出收尾(步骤记 done,不制造失败)。
    /// **扁平字段、只被任务侧消费**(`task_engine::task_loop_limits` 装配给
    /// `GenerationParams.step_budget`);聊天路径不读、不传。
    /// 不设 60s 之类下限是刻意的:1 秒合法(测试靠它触发预算路径),文档注「建议 ≥300」。
    #[serde(default = "default_task_step_budget_secs")]
    pub task_step_budget_secs: u32,
    /// 任务级总预算秒数(PRODCAP-2;默认 **0 = 关**,否则钳 1..=86400):
    /// 一次任务执行(含多步/多节点/多子目标)的墙钟上限——到点后多步执行器**不再
    /// 启动新步骤/节点/子目标**,在跑步由 `min(步骤预算, 剩余)` 收窄后**带产出收尾**
    /// (不制造失败)。**扁平字段、只被任务侧消费**(`task_engine::spawn_run` 起算
    /// `TaskRunContext.deadline`);聊天路径不读、不传。
    /// 默认关的判据:单步预算已覆盖主要失控场景,总预算防「每步都不超、整任务失控」。
    #[serde(default = "default_task_total_budget_secs")]
    pub task_total_budget_secs: u32,
    /// 团队任务执行阶段总预算秒数(TMR3-4;TPROF-1 起默认 **0 = 不限**,否则
    /// `600..=1200`,超过 1200 在写入时归一为 `0`):
    /// 只被 team 执行器消费,从**规划完成后**起算;到点不再启动新主/子目标,
    /// 跑步由 `min(步骤预算, 剩余)` 收窄后带产出收尾(不制造失败)。
    /// 为什么单独一档:`task_step_budget_secs` 在 team 里每个子目标重新起算,
    /// 4 主 × 4 子目标可跑到小时级;2026-10-21 实测单主 26 分钟后被外部 driver stop。
    /// 与 `task_total_budget_secs` **取更早者**;后者不再「显式优先」。
    #[serde(default = "default_task_team_budget_secs")]
    pub task_team_budget_secs: u32,
    /// 任务空闲超时秒数(提交 3 · D7;默认 2400,0 = 关,否则钳
    /// `task_idle_floor_secs()`..=86400):运行中任务若连续该时长既无模型调用行、也无事件心跳,
    /// 由看守以 stop 同源路径收尾并标注「空闲超时自动收尾」。
    /// **下限不写死数字**——它由「单条命令上限(`exec::MAX_TIMEOUT_MS`,现 1800s)」与
    /// 「单次模型调用上限(`executor::TASK_TOOL_LOOP_CALL_TIMEOUT`,300s)」再加 1 派生,
    /// 单一出处在 `params::task_idle_floor_secs`(批次 2 之前这里与 PUT 校验各写一份 601,
    /// 抬 bash 上限就会两处漂移并把合法长命令误杀)。
    /// **扁平字段、只被任务侧消费**(`task_service/idle.rs` 的看守)。
    #[serde(default = "default_task_idle_timeout_secs")]
    pub task_idle_timeout_secs: u32,
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
    /// 记忆槽字符预算(升级工作流 B2;通道 1+2 合计,默认 2000,钳 0..=20000;
    /// 0 = 不限制;超预算截断并在槽内加显式提示)
    #[serde(default = "default_memory_inject_char_budget")]
    pub memory_inject_char_budget: u32,
    /// 每角色记忆容量上限(升级工作流 B3;默认 200,钳 0..=10000;0 = 不淘汰):
    /// 超限时把最低分条目置 selected=0 归档(只归档不删除)
    #[serde(default = "default_memory_max_entries")]
    pub memory_max_entries: u32,
    /// 剧情推演词条同步·角色档(RPFLOW-2):deep/agent 归档步的 `worldbook_update`
    /// 可更新「绑定到当前角色的世界书 + 角色卡内嵌条目」中的绿灯(触发)词条;
    /// 命中蓝灯(常驻)条目时不改写,改为新建绿灯条目。默认开启。
    #[serde(default = "default_worldbook_sync_character_enabled")]
    pub worldbook_sync_character_enabled: bool,
    /// 剧情推演词条同步·全局档(RPFLOW-2):同上,作用范围为全局世界书(未绑定角色)。
    /// 默认关闭——全局书常服务多个角色,改动影响面更大。
    #[serde(default)]
    pub worldbook_sync_global_enabled: bool,
    /// 向量化(embedding)开关:开启且凭据齐全时,记忆写入同步生成向量、召回走混合打分
    #[serde(default)]
    pub embedding_enabled: bool,
    /// embedding 服务地址(OpenAI 兼容 /embeddings;独立于聊天 Base URL)
    #[serde(default)]
    pub embedding_base_url: String,
    /// embedding API Key:内存明文,落盘经 secret_store 加密(与 openai_api_key 同策略)
    #[serde(default)]
    pub embedding_api_key: String,
    /// embedding 模型名(如 text-embedding-3-small / embedding-3 / bge-m3)
    #[serde(default)]
    pub embedding_model: String,
    /// 向量维度(0 = 由首次调用探测并回填;非 0 时校验实际返回维度是否一致)
    #[serde(default)]
    pub embedding_dim: u32,
    /// 技能渐进披露开关(落地项 3;默认开启):system 只注入技能 name+description
    /// 紧凑清单,正文按需 read(type=skill);关闭回退旧行为(不注入清单)
    #[serde(default = "default_skill_progressive_disclosure")]
    pub skill_progressive_disclosure: bool,
    /// 回退快照开关(批次 6.1;默认开启):写工具执行前把逆操作负载落 undo_snapshots 表,
    /// Agent 面板「回退到此处」按快照逆序恢复;关闭后不再产生新快照(存量快照仍可恢复)
    #[serde(default = "default_undo_enabled")]
    pub undo_enabled: bool,
    /// 子智能体最大嵌套深度(落地项 3;TPROF-1 起默认 3,钳 1..=4;主 Agent 为第 0 层)
    #[serde(default = "default_subagent_max_depth")]
    pub subagent_max_depth: u32,
    /// 子智能体并发上限(落地项 3;TPROF-1 起默认 8,钳 1..=16;当前顺序派发,作为在飞计数守卫)
    #[serde(default = "default_subagent_max_concurrency")]
    pub subagent_max_concurrency: u32,
    /// 子智能体结果最大字符数(落地项 3;TPROF-1 起默认 8000 = 上限,钳 500..=8000;
    /// 超出截断并附尾注)
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
    /// 命令执行总开关(阶段 E;默认**关闭**,与「root 能力默认关闭、用户显式开启」一致)。
    /// 关闭时 bash 工具与 Android 执行层直接拒绝执行(工具仍注册,便于用户在授权面板看到)。
    #[serde(default)]
    pub exec_enabled: bool,
    /// Android 执行层:允许 ROOT 档(su 提权)。默认关闭(合规要求,不适合上架)。
    #[serde(default)]
    pub exec_allow_root: bool,
    /// Android 执行层:允许 Shizuku 档(ADB shell 权限,无需 root)。默认关闭。
    #[serde(default)]
    pub exec_allow_shizuku: bool,
    /// Android 执行层:允许沙箱档(应用自身 UID)。默认关闭(统一由 exec_enabled 控制)。
    #[serde(default)]
    pub exec_allow_sandbox: bool,
    /// 「视觉与截图」总开关(2026-10-02 视觉能力包 D5 起消费):true = 允许截图工具
    /// 在此设备取屏(Windows 原生 GDI;安卓无障碍见移动端批次)。默认**关闭**,
    /// 须用户显式开启——隐私敏感:截图内容会经所配置的连接发往模型端点。
    /// 全局扁平字段(与 exec_* 同口径:能力开关是进程级事实,不进模式覆盖层,
    /// 故 roleplay/task 共用一份)。
    #[serde(default)]
    pub vision_screenshot_enabled: bool,
    /// 编码能力包开关(2026-09-28;默认 false = 不启用,旧配置缺省由 serde default 填 false,
    /// 零迁移)。启用后任务模式执行者/汇总者的**缺省**默认词改用编码执行者模板
    /// (`default_coding_task_agent_prompt`);用户在提示词框自定义过的值逐字优先。
    /// 只影响提示词缺省值,不改变工具面、不影响角色扮演模式。task 覆盖层可覆盖,
    /// None 沿用本扁平值。
    #[serde(default)]
    pub task_coding_bundle_enabled: bool,
    /// 文学能力包开关(2026-10-05,LIT-1;默认 false = 不启用,旧配置缺省由 serde default
    /// 填 false,零迁移)。启用后角色扮演侧三件事生效(自 LIT-2/LIT-3 起):缺省提示词取
    /// 文学增强版、system 尾追加文学增强段、最新用户消息尾追加 Author's Note 段;
    /// 用户自定义提示词逐字优先。**纯扁平字段**:角色扮演侧没有覆盖层
    /// (`for_mode` 对 Roleplay 直接 clone),故不加覆盖层、不进任务设置面。
    #[serde(default)]
    pub literary_bundle_enabled: bool,
    /// 任务模式文学能力包开关(2026-10-05;默认 false = 不启用)。完全对称
    /// `task_coding_bundle_enabled`:扁平 + task 覆盖层,仅影响任务执行者/汇总者的
    /// **缺省**默认词选择(限文学向任务),不改变工具面、不影响角色扮演模式。
    #[serde(default)]
    pub task_literary_bundle_enabled: bool,
    /// 文学包**文风预设**(LIT-6,2026-10-06;默认空 = 不注入)。纯扁平字段(该侧无覆盖层)。
    /// 取值集合见 `params::LITERARY_STYLE_PRESETS`(`plain` / `classical` / `lightnovel` /
    /// `hardboiled`);**未知取值在新写入时 400、load 时归一化为空串**(不静默注入未知档)。
    /// 生效面:开关开且取值非空时,在位置 0(最新消息尾)追加一段经 untrusted 包裹的文风素材段。
    #[serde(default)]
    pub literary_style_preset: String,
    /// 文学包**长程一致性推荐档**(LIT-7,2026-10-06;默认空 = 不改变任何既有值)。
    /// 取值 `""` / `medium`(中篇)/ `long`(长篇);用户**显式选档**才把该档的一组值写入
    /// `compaction_mode` / `compaction_threshold` / `compaction_keep_recent` 三项,写入前原值
    /// 存进 [`Self::literary_recommend_snapshot`],选回空串即按快照恢复。
    /// 该档**不随文学包开关门控**——它只写通用字段,门控会让「回退」入口不可达。
    #[serde(default)]
    pub literary_recommend_preset: String,
    /// 采纳推荐档**之前**的压缩三项快照(LIT-7):仅在用户选档时拍摄,选回「不改变」时恢复并清空。
    /// `#[serde(default)]` = 旧配置零迁移;为空时序列化省略(不给空配置引入新键)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literary_recommend_snapshot: Option<LiteraryRecommendSnapshot>,
    /// 任务模式默认连接(TM-SET-1;默认空 = 跟随默认连接 active_connection,零迁移)。
    /// 逐任务显式 `connection_id`(任务级/节点级)之后的一级回退,仅任务模式消费;
    /// 指向的连接被删/停用时**软回退**默认连接(便利设置不该让任务失败,见
    /// `task_mode_default_connection`)。task 覆盖层可覆盖,None 沿用本扁平值。
    #[serde(default)]
    pub task_default_connection_id: String,
    /// 任务工作台的按模式覆盖项。扁平字段即角色扮演(roleplay)的权威值——引擎直接读
    /// 扁平字段,故 roleplay 不设覆盖层;task 用此覆盖层替换扁平字段的差异项。
    /// 旧 settings.json 无此字段,serde default 为空 = task 沿用扁平值。
    #[serde(default)]
    pub task: ModeSettings,
}
#[cfg(test)]
mod tests;
