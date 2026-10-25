// 设置路由:/api/settings(连接测试/模型列表/信息/模型切换/运行期设置读写)
//
// C-2B 起按域拆分:`patch/` 承载 PUT 的 75 字段补丁(每域一个 apply 函数),
// `projection.rs` 承载 GET/PUT 的响应投影,`probes.rs` / `prompt.rs` 承载连接探测与
// 提示词预览端点。本文件只保留请求体、`get_settings` 与 `update_settings` 的事务骨架。
use crate::api::app_state::AppState;
use crate::api::json_body::JsonBody;
use crate::api::{err_with_code, validation, ErrorCode};
use crate::connectors::openai_compatible::API_STYLE_CHAT;
use crate::services::settings_service::{
    resolve_connector_target, AppMode, ConnectionProfile, McpServerConfig,
};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

mod patch;
mod probes;
mod projection;
mod prompt;

pub use probes::{connect, get_model, info, models, refresh_models, switch_model, test_embedding};
pub use prompt::{get_agent_prompt, prompt_preview, save_agent_prompt};

use projection::settings_json;

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
    /// mvu 变量状态注入位置(system / user_tail;非法值 400)
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
    /// 聊天 AGENT 模式工具循环轮次上限(1..=200;缺省保持不变)。
    /// 任务模式自 2026-10-19 起取消默认轮次限制,不再消费本设置。
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
    /// 上下文压缩模式(off / manual / auto;非法值 400)
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
    /// 团队任务执行阶段总预算秒数(TMR3-4;0 = 关,否则 1..=86400;缺省保持不变)
    #[serde(default)]
    pub task_team_budget_secs: Option<u32>,
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

/// GET /api/settings:当前运行期设置(供前端表单回填)。?mode= 返回该模式合并后的有效值。
pub async fn get_settings(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ModeQuery>,
) -> Json<serde_json::Value> {
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
    // 逐域应用:连接(含显式清空密钥名单)→ 生成 → Agent → 工具/权限 → 记忆 → 任务 → 能力包。
    // 任一字段非法即返回 400,候选副本丢弃(整批不落库、不替换内存)。
    let outcome = match patch::apply(&mut candidate, &body, mode) {
        Ok(outcome) => outcome,
        Err(error) => return validation(error.message),
    };
    let cleared_key_ids = outcome.cleared_key_ids;

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
