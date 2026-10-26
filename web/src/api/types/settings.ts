// 运行期设置 / 连接 / 提示词注入 / 技能域类型。

import type { AuthorizationMode } from './tools';

export interface ConnectorInfo {
  connector: string;
  model: string;
  models: string[];
  availableConnectors: Array<{ type: string; label: string }>;
}

/** 连接器类型(取值域与后端 `connectors::available_connector_types()` 一致) */
export type ConnectorType = 'openai-compatible' | 'mock';

/** 接口方言(取值域与后端 `connectors::openai_compatible` 的 API_STYLE_* 一致) */
export type ApiStyle = 'chat-completions' | 'responses' | 'anthropic';

/** 一套 API 连接配置(GET /api/settings 的 connections 元素);api_key 只回传掩码 */
export interface ConnectionProfile {
  /** 稳定 id:保存时按它命中已有连接(缺省/未命中 = 新建) */
  id: string;
  /** 展示名 */
  name: string;
  connector_type: ConnectorType;
  base_url: string;
  model: string;
  /** 接口方言;旧数据(无此键)按 chat-completions 处理 */
  api_style: ApiStyle;
  /** 停用的连接保留配置,但不会成为默认连接,也不出现在后续批次的节点选择器里 */
  enabled: boolean;
  /** 支持视觉输入(2026-10-02 视觉能力包):图像随请求发送;聊天贴图与视觉工具的门控 */
  supports_vision: boolean;
  /** 支持结构化输出(2026-10-03 VISION-L6):引擎的结构化产物调用(如变量状态更新)会随请求要求 JSON 模式 */
  supports_structured_output: boolean;
  /** 支持前缀续写(预留:已接入连接构建与指纹,引擎暂无前缀续写生成路径,勾选暂不改变行为) */
  supports_prefix_completion: boolean;
  /** 支持中途系统插入(2026-10-03 VISION-L6):Anthropic/Responses 方言下中途系统内容就地以 user 消息保序,而非上提拼顶 */
  supports_mid_conversation_system: boolean;
  /** 大图自动拆分(DeepSeek 等尺寸限制较严的端点):超阈值大图在发送前自动切块 */
  image_auto_split: boolean;
  /** 密钥掩码(仅保留后 4 位) */
  api_key_masked: string;
  /** 该连接是否已配置密钥 */
  has_api_key: boolean;
}

/** 连接配置的写入项:各字段可选,按 id 命中已有连接;**api_key 空/缺省 = 保持该连接原密钥**;
 *  要把已配置的密钥改为空,须显式传 `clear_api_key: true`(与 api_key 互斥,新输入优先) */
export interface ConnectionProfilePatch {
  id?: string;
  name?: string;
  connector_type?: string;
  base_url?: string;
  api_key?: string;
  /** 显式清空该连接已保存的密钥(2026-10-03 API 设置补全);仅命中已有 id 时生效,新建连接忽略 */
  clear_api_key?: boolean;
  model?: string;
  /** 接口方言显式覆盖;缺省 = 沿用已有值并按 URL 端点后缀推断(仅改写默认档) */
  api_style?: string;
  enabled?: boolean;
  /** 模型能力位(2026-10-02 视觉能力包):缺省 = 沿用该 id 的现有值;显式 false 才清空 */
  supports_vision?: boolean;
  supports_structured_output?: boolean;
  supports_prefix_completion?: boolean;
  supports_mid_conversation_system?: boolean;
  image_auto_split?: boolean;
}

/** 运行期设置(API 连接 + 生成参数);api_key 仅回传脱敏值 */
export interface RuntimeSettings {
  openai_base_url: string;
  api_key_masked: string;
  has_api_key: boolean;
  model: string;
  /** 多套连接配置(**真源**);上面的 openai_base_url / model 是默认连接的派生视图 */
  connections: ConnectionProfile[];
  /** 默认连接 id;null = 没有可用连接(全停用/删空 → 连接器回退 mock) */
  active_connection_id: string | null;
  default_temperature: number;
  default_top_p: number;
  default_max_tokens: number;
  max_context_tokens: number;
  /** Agent 系统提示词(空 = 内置默认),支持 {{character_name}} 等占位符 */
  agent_system_prompt: string;
  /** 联网搜索端点(空 = 默认 DuckDuckGo HTML 接口) */
  search_endpoint: string;
  /** mvu 变量状态注入位置:system(默认,世界书并入 system)/ user_tail(追加到最新用户消息尾,前缀缓存友好) */
  mvu_vars_position: string;
  /** 反思提示词(空 = 机械规则检查;非空 = 反思步骤调用 LLM 判定) */
  reflect_prompt: string;
  /** 复杂模式预设尾部提示词(空 = 禁用;位置0 尾部) */
  preset_tail_prompt: string;
  /** 预设尾部提示词注入角色(user / assistant) */
  preset_tail_role: string;
  /** 反思失败建议的可选补充说明(主体建议由引擎自动生成 ≤200 token,注入位置0 内预设尾部之前;空 = 仅自动建议) */
  reflect_advice_prompt: string;
  /** 反思失败建议提示词注入角色(user / assistant) */
  reflect_advice_role: string;
  /** 授权模式(三档):strict=读/写/删文件都需授权;loose=读/写放行、删需授权;bypass=除系统路径(C 盘)写/删外全放行 */
  authorization_mode: AuthorizationMode;
  /** @deprecated 旧放行模式开关;读 authorization_mode 代替。true 等价 bypass,false 等价 strict */
  bypass_mode: boolean;
  /** 「始终需授权」清单:名单内工具在三档模式下都需授权 */
  bypass_blacklist: string[];
  /** 授权等待超时(秒;30..=1800,默认 300) */
  tool_authorization_timeout_secs: number;
  /** 任务模式工具策略:all=全量、deny_dangerous=拒绝危险工具但保留 bash(默认)、allowlist=白名单 */
  task_tool_policy: 'all' | 'deny_dangerous' | 'allowlist';
  /** 任务模式工具白名单(task_tool_policy=allowlist 时生效) */
  task_tool_allowlist: string[];
  /** 聊天 AGENT 模式工具循环轮次上限(默认 32);任务模式已取消默认轮次限制 */
  max_tool_rounds: number;
  /** 流程动态调用嵌套深度上限(**任务侧**设置;A 批 A3;默认 2,1..=5):
   *  节点用 `run_flow` 工具逐层调用流程时的深度闸(与静态子图 `sub_flow_id` 的嵌套上限是两回事) */
  max_flow_call_depth: number;
  /** 单任务内流程调用次数上限(**任务侧**设置;A 批 A3;默认 8,1..=64) */
  max_flow_calls_per_task: number;
  /** 节点默认上下文上限(**任务侧**设置;A 批 A4;默认 0 = **不裁剪**,否则 256..=1048576):
   *  节点自己写了 `max_context` 即按节点的,只有未写时才用本项 */
  default_node_max_context: number;
  /** 工具历史保留的最近完整轮数(1..=32;默认 4;超出后最老轮摘要化) */
  tool_history_keep_rounds: number;
  /** 工具历史 token 预算(0 = 禁用预算闸门;否则 1024..=1M,默认 16384) */
  tool_history_budget_tokens: number;
  /** 单次生成的 token 预算(0 = 关闭,默认 0;否则 1024..=1e9):
   *  工具循环累计 prompt+completion 达到该值即按 session_budget_action 处置 */
  session_token_budget: number;
  /** 预算超限动作:warn(只提示一次,默认)/ stop(提示并停止本轮工具循环) */
  session_budget_action: 'warn' | 'stop';
  /** 语义熔断窗口(默认 16,4..=64):近 W 次工具调用内判定「同工具 + 输出无变化」 */
  loop_guard_semantic_window: number;
  /** 语义熔断同工具调用次数下限(默认 12;0 = 关闭,否则 4..=64) */
  loop_guard_semantic_min_calls: number;
  /** 语义熔断输出指纹去重上限(默认 2,1..=8) */
  loop_guard_semantic_max_distinct: number;
  /** 任务步骤墙钟预算秒数(**任务侧**设置;默认 3600 = 开,0 = 关,否则 1..=86400):
   *  单次工具循环的墙钟上限,到点带着已有产出收尾(步骤照常 done);文档注「建议 ≥300」 */
  task_step_budget_secs: number;
  /** 任务级总预算秒数(**任务侧**设置;默认 0 = 关,否则 1..=86400):
   *  一次任务执行的墙钟上限——到点后不再启动新步骤/节点/子目标,以已完成部分收尾 */
  task_total_budget_secs: number;
  /** 团队任务执行阶段总预算秒数(**仅 team**;默认 1200,0 = 不限,否则 600..=1200):
   *  从规划完成后起算;到点不再启动新主/子目标,以已完成部分收尾。
   *  超过 1200 由后端归一为 0(不限)——「想设更久」即「别限时」 */
  task_team_budget_secs: number;
  /** 任务空闲超时秒数(**任务侧**设置;默认 2400,0 = 关,否则 2101..=86400):
   *  运行中任务连续该时长无模型调用也无事件 → 看守自动收尾(与 stop 同源),
   *  原因写进 error「空闲超时自动收尾」;下限 = 单命令 1800s + 单次调用 300s + 1 */
  task_idle_timeout_secs: number;
  /** 变量两步生成独立模型(HB-7;null = 与正文共用同一模型) */
  mvu_model: string | null;
  /** 变量两步生成独立温度(HB-7;null = 内置 0.3) */
  mvu_temperature: number | null;
  /** HTML 渲染开关(状态栏脚本执行前置条件):true = 开启安全 HTML 渲染 */
  render_html: boolean;
  /** 上下文压缩模式:off(不压缩)/ manual(手动触发)/ auto(token 超阈值自动压缩) */
  compaction_mode: string;
  /** 上下文压缩触发阈值(0.5..=0.95,默认 0.8):auto 模式下历史 token 占比达到该值即压缩 */
  compaction_threshold: number;
  /** 压缩后保留的最近消息条数(2..=200,默认 4;缓存感知管线) */
  compaction_keep_recent: number;
  /** snip 零成本裁剪的超长消息阈值(字节;0 = 禁用,上限 1MB,默认 8192) */
  compaction_snip_bytes: number;
  /** LLM 请求快照开关(第四点·主题 A):true = 每次下发前把完整消息数组落库供回放调试 */
  llm_request_log: boolean;
  /** 跨会话记忆蒸馏开关:true = 允许 POST /api/memory/distill 蒸馏会话为角色记忆 */
  memory_distill_enabled: boolean;
  /** 每次注入提示词的记忆条数上限(0..=50;0 = 不注入) */
  memory_inject_limit: number;
  /** 记忆槽字符预算(0..=20000;0 = 不限制,默认 2000):按精选排序累积到预算即停 */
  memory_inject_char_budget: number;
  /** 每角色记忆容量上限(0..=10000;0 = 不淘汰,默认 200):超出后最低分条目置 selected=0 */
  memory_max_entries: number;
  /** 剧情推演词条同步·角色档(RPFLOW-2;默认 true):deep/agent 归档步可更新
   *  「绑定到当前角色的世界书 + 角色卡内嵌条目」中的绿灯(关键词触发)词条;
   *  命中蓝灯(常驻 constant)条目时不改写,改为新建一条绿灯条目 */
  worldbook_sync_character_enabled: boolean;
  /** 剧情推演词条同步·全局档(RPFLOW-2;默认 false):同上,作用范围为全局世界书
   *  (未绑定角色的世界书);默认关——全局书常服务多个角色,改动影响面更大 */
  worldbook_sync_global_enabled: boolean;
  /** 向量化(embedding)开关:开启后记忆写入生成向量、召回走「向量+Jaccard」混合打分 */
  embedding_enabled: boolean;
  /** embedding 服务地址(OpenAI 兼容 /embeddings;独立于聊天 Base URL) */
  embedding_base_url: string;
  /** embedding API Key 脱敏展示(仅后 4 位;不回显明文) */
  embedding_api_key_masked: string;
  /** 是否已配置 embedding API Key */
  has_embedding_api_key: boolean;
  /** embedding 模型名(如 text-embedding-3-small / embedding-3 / bge-m3) */
  embedding_model: string;
  /** 向量维度(0 = 由测试连接自动探测并回填) */
  embedding_dim: number;
  /** 技能渐进披露开关(true = system 只注入「名称:用途」清单,正文按需 read;默认 true) */
  skill_progressive_disclosure: boolean;
  /** 子智能体最大嵌套深度(1..=4;默认 2) */
  subagent_max_depth: number;
  /** 子智能体最大并发数(1..=16;默认 6) */
  subagent_max_concurrency: number;
  /** 子智能体结果最大字符数(500..=8000;默认 2000,超出截断带尾注) */
  subagent_result_max_chars: number;
  /** 回退快照(undo)开关(批次 6.1b;默认 true):开时写工具执行前自动存档,可回退 */
  undo_enabled: boolean;
  /** MCP stdio 客户端总开关(批次 6.2;默认 false;仅启动时装配,改后重启生效) */
  mcp_enabled: boolean;
  /** MCP 服务器列表(stdio 托管子进程;默认空) */
  mcp_servers: McpServerConfig[];
  /** 命令执行总开关(阶段 E;默认 false = 关闭,须用户显式开启) */
  exec_enabled: boolean;
  /** Android 执行层:允许 ROOT 档(su 提权;默认 false) */
  exec_allow_root: boolean;
  /** Android 执行层:允许 Shizuku 档(ADB shell 权限,无需 root;默认 false) */
  exec_allow_shizuku: boolean;
  /** Android 执行层:允许沙箱档(应用自身 UID;默认 false) */
  exec_allow_sandbox: boolean;
  /** 「视觉与截图」总开关(视觉能力包;默认 false = 关闭,须用户显式开启):
   *  true = 允许截图工具取屏(Windows 原生 GDI;安卓无障碍见移动端批次)。
   *  隐私敏感:截图内容会经所配置的连接发往模型端点。全局字段(不分模式)。 */
  vision_screenshot_enabled: boolean;
  /** 任务模式编码能力包显式启用开关(默认 false = 关闭,须用户显式开启)。仅任务模式生效:
   *  开启后,任务执行者**未自定义**系统提示词时,默认值改用「编码执行者模板」
   *  (强调先读后写 / 遵循既有风格 / 改完跑验证 / 最小改动);用户若在提示词框里自定义过,
   *  则其自定义值优先,本开关只影响默认值。**不影响角色扮演模式** */
  task_coding_bundle_enabled: boolean;
  /** 文学能力包开关(2026-10-05,LIT-1;默认 false = 关闭)。仅角色扮演模式生效:
   *  开启后角色扮演侧缺省提示词取文学增强版、并在 system 尾与最新用户消息尾追加两段
   *  文学注入;用户自定义提示词逐字优先。**扁平字段**(该侧无覆盖层,两模式读到同一值) */
  literary_bundle_enabled: boolean;
  /** 任务模式文学能力包开关(2026-10-05;默认 false)。对称 task_coding_bundle_enabled:
   *  扁平 + task 覆盖层,仅影响任务执行者/汇总者的**缺省**提示词(文学向变体) */
  task_literary_bundle_enabled: boolean;
  /** 文学包文风预设(LIT-6;空串 = 不注入)。取值 `plain` / `classical` / `lightnovel` /
   *  `hardboiled`;未知取值服务端 400。纯扁平字段,仅角色扮演模式生效 */
  literary_style_preset: string;
  /** 文学包长程一致性推荐档(LIT-7;空串 = 不改变)。取值 `medium`(中篇)/ `long`(长篇);
   *  选档会写入压缩三项数值,选回空串按服务端快照恢复写入前原值 */
  literary_recommend_preset: string;
  /** 任务模式默认连接(TM-SET-1;空串 = 跟随默认连接)。任务连接回退链:
   *  逐任务/节点显式连接 → 本项 → 默认连接;指向的连接被删/停用时运行期软回退默认连接 */
  task_default_connection_id: string;
}

/** MCP 服务器配置(批次 6.2):name 会 sanitize 为工具名前缀段([a-z0-9_]) */
export interface McpServerConfig {
  /** 服务器名(工具注册为 mcp_{name}_{tool}) */
  name: string;
  /** 可执行命令(如 npx / node / 某个 exe) */
  command: string;
  /** 命令行参数 */
  args: string[];
  /** 是否启用(默认 true;false = 保留配置但不装配) */
  enabled: boolean;
}

export interface PromptPreviewLayer {
  source: string;
  role: string;
  layer: number;
  order: number;
  content: string;
}

export interface PromptPreview {
  ok: boolean;
  note: string;
  layers: PromptPreviewLayer[];
  /** 结构化告警码(WB-E;文案映射见 utils/promptWarnings.ts)。后端恒返回数组,空集表示无告警。 */
  warnings: string[];
}

/** GET 响应独有、PUT 不接收的派生字段(密钥掩码/布尔)。
 *  保持文件内常量(不导出):类型 barrel 的运行期导出必须为空(`index.test.ts` 钉死)。 */
const SETTINGS_GET_ONLY_FIELDS = [
  'api_key_masked',
  'has_api_key',
  'embedding_api_key_masked',
  'has_embedding_api_key',
] as const;

/** PUT 独有、GET 响应不返回的写入字段(明文密钥只在写入侧出现)。 */
const SETTINGS_PATCH_ONLY_FIELDS = ['openai_api_key', 'embedding_api_key'] as const;

/** GET/PUT 同名但线格式不同的字段:patch 侧需重定义(见下方 Overrides)。 */
const SETTINGS_PATCH_OVERRIDE_FIELDS = [
  'connections',
  'active_connection_id',
  'mvu_model',
  'mvu_temperature',
] as const;

type SettingsGetOnlyField = (typeof SETTINGS_GET_ONLY_FIELDS)[number];
type SettingsPatchOverrideField = (typeof SETTINGS_PATCH_OVERRIDE_FIELDS)[number];

interface RuntimeSettingsPatchOverrides {
  /** 明文密钥只在写入侧出现;值留空 = 保持现有密钥不变 */
  openai_api_key?: string;
  /** 明文密钥只在写入侧出现;值留空 = 保持现有密钥不变 */
  embedding_api_key?: string;
  /** 多套连接(**全量数组语义**:数组里没有的 id 即被删除) */
  connections?: ConnectionProfilePatch[];
  /** 默认连接 id(空串 = 清除,由后端回退到第一个启用连接) */
  active_connection_id?: string;
  /** 变量两步生成独立模型(空串 = 清除回到与正文共用) */
  mvu_model?: string;
  /** 变量两步生成独立温度(0..=2 设置;负值 = 清除回到内置 0.3) */
  mvu_temperature?: number;
}

/**
 * PUT /api/settings 的 patch 类型(C-2B 起派生自 `RuntimeSettings`)。
 *
 * 不再手写 75 个可选字段:新增设置只在 `RuntimeSettings` 与后端 `UpdateSettingsBody`
 * 声明,两侧字段集合由 `tools/check-settings-parity.mjs` 双向对拍。三处固有差异
 * (GET 独有 / PUT 独有 / 同名不同形)由上方常量显式列出,不靠注释约定。
 */
export type RuntimeSettingsPatch = Partial<
  Omit<RuntimeSettings, SettingsGetOnlyField | SettingsPatchOverrideField>
> &
  RuntimeSettingsPatchOverrides;

/** 提示词注入模式:简单模式 / 复杂模式(楼层系统) */
export type InjectMode = 'simple' | 'complex';
export type FloorPosition = 'system' | 'before' | 'after' | 'depth';
export type FloorRole = 'system' | 'user' | 'assistant';

/** 禁词条目:输出中出现 word 时注入自省提示词(所有模式),deep/agent/custom 另由引擎工具替换为 replacement */
export interface BannedWordEntry {
  /** 禁词(精确子串匹配) */
  word: string;
  /** 替换词(含义相近、更得体;空串 = 直接删除) */
  replacement: string;
}

/** 简单模式:字数/转述/对话/视角 四项 + 禁词库 */
export interface SimplePromptConfig {
  word_count_enabled: boolean;
  word_count: number;
  paraphrase_enabled: boolean;
  dialogue_enabled: boolean;
  perspective_enabled: boolean;
  perspective: string;
  /** 注入顺序(合成文本按此顺序拼接;空 = 默认 字数→转述→对话→视角) */
  order?: string[];
  /** 禁词库总开关 */
  banned_words_enabled?: boolean;
  /** 禁词提示词(纯文本,发给 AI 的自省提示;新格式) */
  banned_prompt?: string;
  /** 禁词 → 替换词 映射表(旧格式兼容,仅保留字段不再用于注入) */
  banned_words?: BannedWordEntry[];
}

/** 单条提示词楼层(仿 SillyTavern Prompt Manager) */
export interface PromptFloor {
  id: string;
  name: string;
  content: string;
  role: FloorRole;
  position: FloorPosition;
  /** 深度:从历史末尾往前数第 N 条之后插入(0 = 最新消息后;仅 position=depth 生效) */
  depth: number;
  enabled: boolean;
  order: number;
}

/** 提示词注入整体配置(全局,所有会话生效) */
export interface PromptInjectConfig {
  mode: InjectMode;
  simple: SimplePromptConfig;
  floors: PromptFloor[];
}

/** 导入结果(替换现有楼层) */
export interface PromptPresetImportResult {
  ok: boolean;
  imported: number;
  config: PromptInjectConfig;
}

/** 技能记录(提示词技能) */
export interface SkillRecord {
  id: string;
  name: string;
  description: string;
  content: string;
  enabled: boolean;
  created_at: string;
  /** 技能工具白名单(空 = 不限);经 read(type=skill) 加载该技能时可用的工具集合 */
  allowed_tools: string[];
  /** 是否允许作为子智能体派发(agentgo 链路) */
  run_as_subagent: boolean;
  /** 技能级模型覆盖(空 = 沿用当前连接器模型) */
  model: string;
  /** 虚拟文件夹路径(IMPHUB 附件管理;空串 = 未分组) */
  folder: string;
}

export interface SkillImportItem {
  name: string;
  description?: string;
  content?: string;
  /** 工具白名单(数组形态;后端 SkillImport 同名口径) */
  allowed_tools?: string[];
  run_as_subagent?: boolean;
  model?: string;
  /** 虚拟文件夹路径(同名覆盖时以导入值为准) */
  folder?: string;
}
