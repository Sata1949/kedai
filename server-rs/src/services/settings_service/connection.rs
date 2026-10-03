// 连接设置:OpenAI 兼容 Base URL 规范化、API Key 脱敏展示、默认搜索端点、
// 多套连接配置(ConnectionProfile)的规范化与「默认连接 → 扁平字段」投影。
use serde::{Deserialize, Serialize};

use crate::connectors::openai_compatible::{
    normalize_api_style, API_STYLE_ANTHROPIC, API_STYLE_CHAT, API_STYLE_RESPONSES,
};

use super::RuntimeSettings;

/// 默认搜索端点(DuckDuckGo HTML 免费接口,无需 API Key)
pub const DEFAULT_SEARCH_ENDPOINT: &str = "https://html.duckduckgo.com/html/";

/// 连接器类型取值域(与 `connectors::available_connector_types()` 的展示项一一对应)
pub const CONNECTOR_TYPE_OPENAI: &str = "openai-compatible";
pub const CONNECTOR_TYPE_MOCK: &str = "mock";

/// 播种那条默认连接的固定 id:固定值让「未落盘前的重复读入」也拿到同一个 id
/// (uuid 会导致每次 load 都换 id,幂等性只能在落盘后才成立)
pub const DEFAULT_CONNECTION_ID: &str = "default";
pub const DEFAULT_CONNECTION_NAME: &str = "默认连接";

/// 连接套数上限(PUT 校验;手改 JSON 超出不报错,照常读入)
pub const MAX_CONNECTIONS: usize = 20;
const MAX_NAME_CHARS: usize = 50;
const MAX_BASE_URL_CHARS: usize = 500;
const MAX_MODEL_CHARS: usize = 200;

/// 一套 API 连接配置(多套连接批次)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionProfile {
    /// 稳定 id:保存时与磁盘密文配对的键(重排/删除后仍能对上),故已有 id 一律不改写
    pub id: String,
    pub name: String,
    /// 取值域见 `available_connector_types()`;非法值在 sanitize 时回退 openai-compatible
    pub connector_type: String,
    pub base_url: String,
    /// API Key:内存明文 / 落盘密文(与 openai_api_key 同策略,见 secret.rs)
    pub api_key: String,
    pub model: String,
    /// 接口方言(取值域 `chat-completions` | `responses` | `anthropic`,定义见
    /// `connectors::openai_compatible`)。旧 settings.json 无此键 → 默认 chat-completions
    /// (既有行为);粘贴带端点后缀的 URL 时在 sanitize 里按后缀推断(见 `strip_endpoint_suffix`)。
    #[serde(default = "default_api_style")]
    pub api_style: String,
    /// 停用的连接保留配置但不再是默认连接的候选,也不出现在后续批次的节点选择器里
    pub enabled: bool,
    /// 模型能力位(2026-10-02 视觉能力包 D1;连接级声明)。五个新键**必须带 serde
    /// default** —— 旧 settings.json 的连接条目缺键时按 false 读入,漏写会让整文件
    /// 反序列化失败、用户设置静默丢失(契约「新增键必须 serde default」同一纪律)。
    /// 消费状态(2026-10-03 VISION-L6 收口:五项全部进连接器构建与连接器池指纹):
    /// `supports_vision` 门控聊天贴图与视觉工具、`image_auto_split` 触发大图拆分、
    /// `supports_structured_output` 配合生成意图下发结构化输出参数、
    /// `supports_mid_conversation_system` 决定 responses/anthropic 对中途 system 的
    /// 处置;`supports_prefix_completion` 效果面暂无消费(引擎无前缀续写路径,如实预留)。
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub supports_structured_output: bool,
    #[serde(default)]
    pub supports_prefix_completion: bool,
    #[serde(default)]
    pub supports_mid_conversation_system: bool,
    #[serde(default)]
    pub image_auto_split: bool,
}

fn default_api_style() -> String {
    API_STYLE_CHAT.to_string()
}

/// 已知端点后缀 → (剥离后的地址, 推断出的方言;`None` = 不改变方言)。
/// 用户在设置里粘贴的往往是**完整端点**(控制台文档的形态),而连接器会把 base 当
/// 目录再拼端点——不剥离就会拼出 `…/chat/completions/models` 一类双段路径(404/400)。
/// 只精确匹配这些固定尾段;剥离后由 `normalize_base_url` 继续补 /v1 等常规规范化。
/// 顺序敏感:`/chat/completions` 必须先于 `/completions`、`/v1/messages` 先于 `/messages`。
pub fn strip_endpoint_suffix(url: &str) -> (String, Option<&'static str>) {
    const SUFFIXES: [(&str, Option<&str>); 6] = [
        ("/chat/completions", Some(API_STYLE_CHAT)),
        ("/completions", Some(API_STYLE_CHAT)),
        ("/responses", Some(API_STYLE_RESPONSES)),
        ("/v1/messages", Some(API_STYLE_ANTHROPIC)),
        ("/messages", Some(API_STYLE_ANTHROPIC)),
        // 只剥 /models(保留 /v1),使 base 仍以 /v1 结尾、各方言端点拼接规则不变
        ("/models", None),
    ];
    let trimmed = url.trim().trim_end_matches('/');
    for (suffix, style) in SUFFIXES {
        if let Some(stripped) = trimmed.strip_suffix(suffix) {
            // 防误伤空主机:https://host/chat/completions 剥到 https://host 是正确形态;
            // 但若剥离结果只剩协议头(异常输入)则放弃剥离
            if stripped.ends_with("://") {
                continue;
            }
            return (stripped.to_string(), style);
        }
    }
    (trimmed.to_string(), None)
}

impl ConnectionProfile {
    /// 连接器构建消费的能力位(2026-10-03 VISION-L6 收口:五项全投影)
    pub fn connector_capabilities(&self) -> crate::connectors::ConnectorCapabilities {
        crate::connectors::ConnectorCapabilities {
            supports_vision: self.supports_vision,
            image_auto_split: self.image_auto_split,
            supports_structured_output: self.supports_structured_output,
            supports_prefix_completion: self.supports_prefix_completion,
            supports_mid_conversation_system: self.supports_mid_conversation_system,
        }
    }

    /// API Key 脱敏展示(仅保留后 4 位)
    pub fn masked_api_key(&self) -> String {
        mask_key(&self.api_key)
    }

    pub fn has_api_key(&self) -> bool {
        !self.api_key.is_empty()
    }

    /// 卫生清理(load 与 PUT 共用):trim、URL 规范化(含端点后缀剥离)、类型回退、
    /// 方言推断与回退、空名补默认名、超长截断。
    pub fn sanitize(&mut self, index: usize) {
        self.name = truncate_chars(self.name.trim(), MAX_NAME_CHARS);
        if self.name.is_empty() {
            self.name = format!("连接 {}", index + 1);
        }
        self.connector_type = normalize_connector_type(&self.connector_type);
        let (stripped, inferred) = strip_endpoint_suffix(&self.base_url);
        self.base_url = truncate_chars(&normalize_base_url(&stripped), MAX_BASE_URL_CHARS);
        // 方言:非法值回退默认;后缀推断只填补默认档——显式选择的非默认档不被覆盖
        // (想用非默认档又不想被推断改写的场景:把 URL 的清到尾段即可,推断是幂等的)
        let current = normalize_api_style(&self.api_style);
        self.api_style = match inferred {
            Some(s) if current == API_STYLE_CHAT => s.to_string(),
            _ => current.to_string(),
        };
        self.model = truncate_chars(self.model.trim(), MAX_MODEL_CHARS);
    }
}

/// 连接器类型规范化:空白/非法值回退 openai-compatible。
/// 刻意不走 `build_connector` 的「未知类型回退 mock」——用户填了地址却被静默降级成演示最难排查。
pub fn normalize_connector_type(t: &str) -> String {
    match t.trim() {
        CONNECTOR_TYPE_MOCK => CONNECTOR_TYPE_MOCK.to_string(),
        _ => CONNECTOR_TYPE_OPENAI.to_string(),
    }
}

/// 解析连接器目标类型(纯函数;PUT 重建与启动装配共用同一口径):
/// - 无可用默认连接(全禁用 / 删空)→ mock,与「未配置」的既有语义一致
/// - 连接显式声明 mock → mock(演示模式是用户的明确选择)
/// - 地址与密钥**没配齐**时保持当前类型(当前是 mock 就继续 mock)
/// - 其余 → openai-compatible(今天唯一的真实连接器类型)
///
/// 判据用「地址 + 密钥都非空」而不是只看地址:`openai_base_url` 的环境默认值本身非空
/// (`config.rs` 的 from_env 缺省 `https://api.openai.com/v1`),只看地址会让全新安装与测试环境
/// 一启动就切到必然失败的真实连接器。旧实现里启动装配用「都非空」、PUT 重建用「任一非空」两套口径,
/// 本批次统一为前者 —— 那次不一致会让无密钥的本地服务在重启后悄悄回落到 mock。
pub fn resolve_connector_target(
    active: Option<&ConnectionProfile>,
    current_type: &str,
) -> &'static str {
    let Some(p) = active else {
        return CONNECTOR_TYPE_MOCK;
    };
    if p.connector_type == CONNECTOR_TYPE_MOCK {
        return CONNECTOR_TYPE_MOCK;
    }
    if p.base_url.is_empty() || p.api_key.is_empty() {
        return if current_type == CONNECTOR_TYPE_MOCK {
            CONNECTOR_TYPE_MOCK
        } else {
            CONNECTOR_TYPE_OPENAI
        };
    }
    CONNECTOR_TYPE_OPENAI
}

/// 视觉能力位闸门的取值口径(视觉能力包 D2/D4;**单一出处**):默认连接是否开启
/// 「视觉输入」。聊天贴图闸门(api/chat.rs)、任务工具策略(vision_gate)与角色扮演
/// 视觉纪律提示词共用本判据。
/// 已知简化:任务的工具清单按运行编译一次,跨节点不重编,故节点级显式连接的能力位
/// 差异不细判(按默认连接口径;登记于 docs/遗留.md)。
pub fn vision_enabled(settings: &RuntimeSettings) -> bool {
    settings
        .active_connection()
        .map(|p| p.supports_vision)
        .unwrap_or(false)
}

/// 任务模式默认连接(TM-SET-1)的**有效解析**——任务连接回退链的第三级
/// (逐任务/节点显式 `connection_id` → 本项 → 默认连接)。传入值为任务模式合并视图
/// (`for_mode(Task)`)。
///
/// 语义:
/// - 未配置(空串)→ `None`(调用方回退默认连接,与改造前逐字节一致);
/// - 指向存在且启用的连接 → `Some(id)`;
/// - 指向的连接已被删除 / 已停用 → 记 warn 并回退 `None`(默认连接)。
///
/// **软回退是有意的**:本项是任务模式的跨任务便利偏好,用户后来删掉那条连接时任务应照常
/// 可跑;与逐任务/逐节点**显式引用**「失效即报错,不静默回退」刻意不同——显式引用是用户
/// 对**本任务**的直接指定,静默换连接会打出与预期不同的账单,故失败优于猜。
pub fn task_mode_default_connection(settings: &RuntimeSettings) -> Option<String> {
    let id = settings.task_default_connection_id.trim();
    if id.is_empty() {
        return None;
    }
    if settings.connections.iter().any(|p| p.id == id && p.enabled) {
        return Some(id.to_string());
    }
    tracing::warn!(
        connection_id = id,
        "任务模式默认连接不存在或已停用,回退默认连接"
    );
    None
}

impl RuntimeSettings {
    /// API Key 脱敏展示(仅保留后 4 位)
    pub fn masked_api_key(&self) -> String {
        mask_key(&self.openai_api_key)
    }

    /// embedding API Key 脱敏展示(仅保留后 4 位;与聊天 Key 同策略)
    pub fn masked_embedding_api_key(&self) -> String {
        mask_key(&self.embedding_api_key)
    }

    /// 播种(幂等):连接数组为空时,用扁平字段建一条默认连接。
    /// 判据只有「数组为空」——第二次读入时数组非空,id 与内容都不再变化。
    pub fn seed_connections_from_flat(&mut self) {
        if !self.connections.is_empty() {
            return;
        }
        self.connections.push(ConnectionProfile {
            id: DEFAULT_CONNECTION_ID.to_string(),
            name: DEFAULT_CONNECTION_NAME.to_string(),
            connector_type: CONNECTOR_TYPE_OPENAI.to_string(),
            base_url: self.openai_base_url.clone(),
            api_key: self.openai_api_key.clone(),
            model: self.model.clone(),
            api_style: default_api_style(),
            enabled: true,
            supports_vision: false,
            supports_structured_output: false,
            supports_prefix_completion: false,
            supports_mid_conversation_system: false,
            image_auto_split: false,
        });
        self.active_connection_id = Some(DEFAULT_CONNECTION_ID.to_string());
    }

    /// 默认连接:active id 命中且启用 → 它;否则第一个启用的连接;一个都没有 → None
    pub fn active_connection(&self) -> Option<&ConnectionProfile> {
        if let Some(id) = self.active_connection_id.as_deref() {
            if let Some(p) = self.connections.iter().find(|p| p.id == id && p.enabled) {
                return Some(p);
            }
        }
        self.connections.iter().find(|p| p.enabled)
    }

    /// 连接数组规范化(load / save / PUT 共用,幂等):
    /// 逐条卫生清理 → id 去重 → 默认连接回退 → 扁平字段投影。
    pub fn normalize_connections(&mut self) {
        let mut seen: Vec<String> = Vec::with_capacity(self.connections.len());
        for i in 0..self.connections.len() {
            self.connections[i].sanitize(i);
            // id 是保存时与磁盘密文配对的键:只给空 id 与撞车的重复 id 重新分配
            // (手改 JSON、跨库合并都会产生重复),已有 id 一律不改写。
            let id_ok =
                !self.connections[i].id.is_empty() && !seen.contains(&self.connections[i].id);
            if !id_ok {
                self.connections[i].id = uuid::Uuid::new_v4().to_string();
            }
            seen.push(self.connections[i].id.clone());
        }
        // 默认连接回退:指向不存在/已禁用 → 第一个启用的;一个都没有 → None
        self.active_connection_id = self.active_connection().map(|p| p.id.clone());
        self.apply_active_projection();
    }

    /// 扁平三字段 = 默认连接的派生视图(connections 才是真源)。
    /// **无条件覆写**:手改 settings.json 里扁平字段不再生效,改连接请改 connections。
    pub fn apply_active_projection(&mut self) {
        let (base_url, api_key, model) = match self.active_connection() {
            Some(p) => (p.base_url.clone(), p.api_key.clone(), p.model.clone()),
            None => (String::new(), String::new(), String::new()),
        };
        self.openai_base_url = base_url;
        self.openai_api_key = api_key;
        self.model = model;
    }

    /// 取可变默认连接(供 PUT 把扁平字段写进默认连接,即「API 连接」区与旧客户端的写入路径)。
    /// 没有可用连接(全禁用/删空)时新建一条启用连接承载本次写入 —— 用户填的配置总要有地方生效。
    pub fn ensure_active_connection_mut(&mut self) -> &mut ConnectionProfile {
        let active_id = self.active_connection_id.clone();
        let found = active_id
            .as_deref()
            .and_then(|id| {
                self.connections
                    .iter()
                    .position(|p| p.id == id && p.enabled)
            })
            .or_else(|| self.connections.iter().position(|p| p.enabled));
        let index = match found {
            Some(i) => i,
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                self.connections.push(ConnectionProfile {
                    id: id.clone(),
                    name: format!("连接 {}", self.connections.len() + 1),
                    connector_type: CONNECTOR_TYPE_OPENAI.to_string(),
                    base_url: String::new(),
                    api_key: String::new(),
                    model: String::new(),
                    api_style: default_api_style(),
                    enabled: true,
                    supports_vision: false,
                    supports_structured_output: false,
                    supports_prefix_completion: false,
                    supports_mid_conversation_system: false,
                    image_auto_split: false,
                });
                self.active_connection_id = Some(id);
                self.connections.len() - 1
            }
        };
        &mut self.connections[index]
    }
}

/// 按字符数截断(不切断 UTF-8;超长视为手改配置的异常输入)
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
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
/// - 先去空白与端点后缀(`strip_endpoint_suffix`:用户粘贴的完整端点 URL 会被剥回 base)
/// - 去尾部斜杠
/// - 无协议时补协议:本机地址(localhost/127.x/0.0.0.0/[::1])补 http://,其余补 https://
/// - 无路径时补 `/v1`(OpenAI 兼容服务普遍要求 /v1,缺它请求 /models 会 404)
pub fn normalize_base_url(input: &str) -> String {
    let (stripped, _) = strip_endpoint_suffix(input);
    let mut s = stripped;
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
