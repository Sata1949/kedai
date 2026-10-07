// MCP stdio 客户端管理(批次 6.2;L3 隔离;docs/契约-架构与数据.md §2.5)。
//
// 边界:v1 只做 stdio transport + tools(不做 SSE/HTTP,不做 resources/prompts);
// 仅在启动时装配(mcp_enabled=false 时完全跳过:零进程、零注册),运行期改设置
// 不回溯重连,重启后生效。
//
// 权限:MCP 工具名带 mcp_ 前缀,不命中任何内建分类——permissions.rs default_risk
// 的「未知工具按 Dangerous 处理」自动覆盖全部 MCP 工具,无需额外分类工作。
//
// ## 代际纪律(L3 青层·活;2026-09-14 依赖倒置)
//
// 本模块**只依赖 L1**:配置词汇(`models::tool_policy::McpServerConfig`)、
// 工具定义与注册窄接口(`models::types::{ToolDefinition, ToolExecutor, ToolRegistrar}`)。
// 它**不再** `use crate::services::` 或 `use crate::tools::`——L3 不得依赖 L2。
//
// 配置的获取方式随之改变:宿主(组合根)读设置后,把 `Vec<McpServerConfig>` **传参**进来,
// 而不是让本模块自己去读 `RuntimeSettings`（后者是 L2 类型）。注册工具则经
// `&dyn ToolRegistrar` 由宿主注入,而非直接持有 L2 的 `ToolRegistry`。
// 这与 `task_core::TaskBackend` 断开 `task_engine→task_service` 是同一手法。
pub mod client;
pub mod process;

use crate::models::tool_policy::McpServerConfig;
use crate::models::types::{ToolDefinition, ToolExecutor, ToolRegistrar};
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use client::McpClient;
use process::McpProcess;

/// MCP 工具执行超时(注册表单工具档):外部服务器冷调用/大数据可能慢于内建工具,
/// 放宽到 120s;客户端内部 tools/call 超时 110s 略小于本档,保证先拿到可读错误。
pub const MCP_TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// 单个已装配服务器(运行句柄):client 供执行器转发,process 句柄 Drop 即杀(防孤儿)。
struct ServerHandle {
    /// 句柄表持有一份 client Arc(执行器闭包另持 Arc 转发调用);
    /// shutdown/stop 经此 Arc 主动 close——执行器持有的 Arc 会让 client 存活,
    /// 没有主动 close 的话,关停后的残留注册项调用会悬挂而非快速失败。
    client: Arc<McpClient>,
    /// None 仅出现于内存管测试(无真实进程)
    _process: Option<McpProcess>,
}

/// 单台服务器运行态(管理器内部三态;`disabled` 由 API 层按设置快照派生,不属内部状态)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerState {
    Running,
    Failed,
    Stopped,
}

impl McpServerState {
    /// 线格式(running/failed/stopped;GET /api/mcp/servers 直接透出)
    pub fn as_str(&self) -> &'static str {
        match self {
            McpServerState::Running => "running",
            McpServerState::Failed => "failed",
            McpServerState::Stopped => "stopped",
        }
    }
}

/// 单台服务器台账:配置 + 状态 + 工具名 + 最近错误(注销/重启/重列 diff 的单一事实源)。
struct ServerRuntime {
    raw_name: String,
    sanitized: String,
    state: McpServerState,
    /// 本台已注册工具的完整名(死亡注销/停止注销/重列 diff 都以此为准)
    tool_names: Vec<String>,
    last_error: Option<String>,
    protocol_version: Option<String>,
    handle: Option<ServerHandle>,
    /// `list_changed` 重列单飞标记(裁定 9:单飞 + 尾随)
    relist_running: bool,
    relist_pending: bool,
}

/// GET 面快照(不泄漏内部结构与锁)
#[derive(Debug, Clone)]
pub struct ServerSnapshot {
    pub name: String,
    pub state: &'static str,
    pub tool_names: Vec<String>,
    pub last_error: Option<String>,
    pub protocol_version: Option<String>,
}

/// 管理器内部态(整个管理器以 Arc 共享:异步回调需 Weak 引用,防 Arc 环)。
struct Inner {
    /// key = 原始服务器名(展示与 API 寻址用);工具前缀用 sanitized。
    servers: Mutex<HashMap<String, ServerRuntime>>,
    /// 注册表窄接口(首次 start 注入;死亡注销/重列回调复用)
    registry: Mutex<Option<Arc<dyn ToolRegistrar>>>,
}

/// MCP 管理器:服务器句柄与台账。内部可变,锁内不跨 .await(仅同步取放);
/// 任何位置**不同时持两把锁**(registry 与 servers 一律顺序取放,防锁序死锁)。
pub struct McpManager {
    inner: Arc<Inner>,
}

impl McpManager {
    /// 空管理器(mcp_enabled=false 或缺省路径:零进程、零注册)
    pub fn empty() -> Self {
        McpManager {
            inner: Arc::new(Inner {
                servers: Mutex::new(HashMap::new()),
                registry: Mutex::new(None),
            }),
        }
    }

    fn lock_servers(&self) -> std::sync::MutexGuard<'_, HashMap<String, ServerRuntime>> {
        self.inner.servers.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn registry(&self) -> Option<Arc<dyn ToolRegistrar>> {
        self.inner
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 运行中(running)的服务器数(启动日志/测试断言用)
    pub fn server_count(&self) -> usize {
        self.lock_servers()
            .values()
            .filter(|r| matches!(r.state, McpServerState::Running))
            .count()
    }

    /// 全量快照(API 层 GET /api/mcp/servers 用;按名字排序确定)
    pub fn snapshot(&self) -> Vec<ServerSnapshot> {
        let mut out: Vec<ServerSnapshot> = self
            .lock_servers()
            .values()
            .map(|r| ServerSnapshot {
                name: r.raw_name.clone(),
                state: r.state.as_str(),
                tool_names: r.tool_names.clone(),
                last_error: r.last_error.clone(),
                protocol_version: r.protocol_version.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// 启动装配:遍历给定服务器,逐个 spawn → 握手 → 分页 tools/list → 注册工具。
    ///
    /// **入参而非自读设置**:`servers` 由宿主(组合根)从 `RuntimeSettings` 过滤 `enabled`
    /// 后传入;`registry` 是 L1 的窄接口 `Arc<dyn ToolRegistrar>`(Arc 供死亡/重列异步回调复用)。
    ///
    /// 单台失败(spawn 失败/握手失败/协议错误)记 `state=failed` + `last_error`,不影响其余
    /// 服务器;服务器名 sanitize 冲突**先到者生效、后到者跳过并记错**(不覆盖,裁定 7)。
    pub async fn start(&self, servers: Vec<McpServerConfig>, registry: Arc<dyn ToolRegistrar>) {
        *self
            .inner
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(registry);
        // 冲突预检(不起进程):sanitize 名唯一,后到者跳过并记 last_error
        let mut seen: HashMap<String, String> = HashMap::new();
        for rt in self.lock_servers().values() {
            seen.insert(rt.sanitized.clone(), rt.raw_name.clone());
        }
        let mut accepted: Vec<(McpServerConfig, String)> = Vec::new();
        for cfg in servers {
            let sanitized = sanitize_server_name(&cfg.name);
            if let Some(first) = seen.get(&sanitized) {
                let msg =
                    format!("服务器名与「{first}」sanitize 后冲突(同为 {sanitized}),已跳过装配");
                tracing::warn!(server = cfg.name, error = %msg, "MCP 服务器装配跳过");
                self.record_skipped(&cfg, &sanitized, msg);
                continue;
            }
            seen.insert(sanitized.clone(), cfg.name.clone());
            accepted.push((cfg, sanitized));
        }
        for (cfg, sanitized) in accepted {
            self.start_one(cfg, sanitized).await;
        }
    }

    /// 跳过条目记台账(failed + last_error),供 GET 面如实展示
    fn record_skipped(&self, cfg: &McpServerConfig, sanitized: &str, error: String) {
        self.lock_servers().insert(
            cfg.name.clone(),
            ServerRuntime {
                raw_name: cfg.name.clone(),
                sanitized: sanitized.to_string(),
                state: McpServerState::Failed,
                tool_names: Vec::new(),
                last_error: Some(error),
                protocol_version: None,
                handle: None,
                relist_running: false,
                relist_pending: false,
            },
        );
    }

    /// 失败记台账(spawn/装配失败路径;幂等覆盖)
    fn set_failed(&self, raw_name: &str, sanitized: &str, error: String) {
        let mut g = self.lock_servers();
        let entry = g
            .entry(raw_name.to_string())
            .or_insert_with(|| ServerRuntime {
                raw_name: raw_name.to_string(),
                sanitized: sanitized.to_string(),
                state: McpServerState::Failed,
                tool_names: Vec::new(),
                last_error: None,
                protocol_version: None,
                handle: None,
                relist_running: false,
                relist_pending: false,
            });
        entry.state = McpServerState::Failed;
        entry.last_error = Some(error);
        entry.handle = None;
        entry.tool_names.clear();
    }

    /// 启动单台:spawn → attach;失败隔离(不影响其余服务器)。
    /// 启动装配、手动 start、restart 共用本入口。
    async fn start_one(&self, cfg: McpServerConfig, sanitized: String) {
        let tag = cfg.name.clone();
        match McpProcess::spawn(&cfg) {
            Ok((process, client)) => {
                match self
                    .attach(cfg, sanitized.clone(), client, Some(process))
                    .await
                {
                    Ok(n) => tracing::info!(server = tag, tools = n, "MCP 服务器已装配"),
                    Err(e) => {
                        tracing::warn!(server = tag, error = %e, "MCP 服务器装配失败,已禁用该服务器");
                        self.set_failed(&tag, &sanitized, e);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(server = tag, error = %e, "MCP 服务器启动失败,已禁用该服务器");
                self.set_failed(&tag, &sanitized, e);
            }
        }
    }

    /// 停止单台:主动 close + kill + 注销其工具,state=stopped(保留配置供重启)。
    /// 幂等;未知服务器返回 false(API 层转 404)。
    pub async fn stop(&self, raw_name: &str) -> bool {
        let (handle, tool_names) = {
            let mut g = self.lock_servers();
            let Some(rt) = g.get_mut(raw_name) else {
                return false;
            };
            rt.state = McpServerState::Stopped;
            rt.last_error = None;
            (rt.handle.take(), std::mem::take(&mut rt.tool_names))
        };
        if let Some(h) = &handle {
            h.client.close();
        }
        if let Some(registry) = self.registry() {
            for name in &tool_names {
                registry.unregister(name);
            }
        }
        drop(handle); // kill_on_drop 收进程
        tracing::info!(server = raw_name, "MCP 服务器已停止,工具已注销");
        true
    }

    /// 重启单台(裁定 8):stop(注销旧工具)→ 以**传入的新配置** start。
    /// 未知服务器等价于首次启动;restart 读「当前扁平设置快照」由 API 层负责传入。
    pub async fn restart(&self, cfg: McpServerConfig) {
        let _ = self.stop(&cfg.name).await;
        let sanitized = sanitize_server_name(&cfg.name);
        self.start_one(cfg, sanitized).await;
    }

    /// 手动启动(裁定 8):已在运行则幂等无操作;否则以传入配置启动。
    /// 是否受该条 `enabled` 字段阻断由调用方(API 层)决定——显式动作不受设置字段阻断。
    pub async fn start_manual(&self, cfg: McpServerConfig) {
        let running = self
            .lock_servers()
            .get(&cfg.name)
            .map(|r| matches!(r.state, McpServerState::Running))
            .unwrap_or(false);
        if running {
            return;
        }
        let sanitized = sanitize_server_name(&cfg.name);
        self.start_one(cfg, sanitized).await;
    }

    /// 装配单台(含内存管测试路径):退出/通知回调 → 握手 → 分页 tools/list →
    /// 工具名冲突预检 → 注册 → 台账入库。任何一步失败即 kill 进程并 Err。
    async fn attach(
        &self,
        cfg: McpServerConfig,
        sanitized: String,
        client: McpClient,
        process: Option<McpProcess>,
    ) -> Result<usize, String> {
        let Some(registry) = self.registry() else {
            return Err("MCP 管理器尚未注入注册表(应由 start 首参注入)".into());
        };
        let client = Arc::new(client);
        // 死亡回调(PLGM 2.4):注销本台全部工具 + state=failed(Weak 防 Arc 环)
        let weak = Arc::downgrade(&self.inner);
        let sanitized_exit = sanitized.clone();
        client.set_exit_hook(Arc::new(move || {
            if let Some(inner) = weak.upgrade() {
                handle_server_exit(&inner, &sanitized_exit);
            }
        }));
        // 握手;失败即回收进程
        if let Err(e) = client.initialize().await {
            if let Some(mut p) = process {
                p.kill().await;
            }
            return Err(e);
        }
        let outcome = client.initialize_outcome();
        // 通知通道(PLGM 2.3):能力门控——仅在服务器声明 tools.listChanged 时安装;
        // 装在 tools/list 之前,避免「首轮列完后立刻来的通知」落空
        if outcome
            .as_ref()
            .map(|o| o.supports_tools_list_changed())
            .unwrap_or(false)
        {
            let weak = Arc::downgrade(&self.inner);
            let sanitized_note = sanitized.clone();
            client.set_notification_hook(Arc::new(move |method, _params| {
                if method == "notifications/tools/list_changed" {
                    if let Some(inner) = weak.upgrade() {
                        spawn_relist(inner, sanitized_note.clone());
                    }
                }
            }));
        }
        let tools = match client.list_tools().await {
            Ok(t) => t,
            Err(e) => {
                // 装配失败:显式 kill(Drop 的 kill_on_drop 是兜底,这里立即回收)
                if let Some(mut p) = process {
                    p.kill().await;
                }
                return Err(e);
            }
        };
        let protocol_version = outcome.as_ref().map(|o| o.protocol_version.clone());
        // 工具名冲突预检(裁定 7):同服务器内 sanitize/截断后重名 → 只跳过该工具并记错
        let mut tool_errors: Vec<String> = Vec::new();
        let mut registered_names: Vec<String> = Vec::new();
        for tool in &tools {
            let full_name = prefixed_tool_name(&sanitized, &tool.name);
            if registered_names.contains(&full_name) {
                tool_errors.push(format!(
                    "工具 {} 的注册名 {full_name} 与同服务器另一工具冲突,已跳过",
                    tool.name
                ));
                continue;
            }
            register_mcp_tool(&registry, &sanitized, &client, tool);
            registered_names.push(full_name);
        }
        if !tool_errors.is_empty() {
            tracing::warn!(
                server = %cfg.name,
                errors = %tool_errors.join("; "),
                "MCP 工具存在命名冲突,已部分注册"
            );
        }
        let registered = registered_names.len();
        self.lock_servers().insert(
            cfg.name.clone(),
            ServerRuntime {
                raw_name: cfg.name.clone(),
                sanitized,
                state: McpServerState::Running,
                tool_names: registered_names,
                last_error: if tool_errors.is_empty() {
                    None
                } else {
                    Some(tool_errors.join("; "))
                },
                protocol_version,
                handle: Some(ServerHandle {
                    client,
                    _process: process,
                }),
                relist_running: false,
                relist_pending: false,
            },
        );
        Ok(registered)
    }

    /// 关闭全部服务器(进程退出路径):先主动 close 协议层(在飞/后续请求快速失败,不悬挂),
    /// 再清空台账——各 ServerHandle Drop 时 kill_on_drop 杀进程。
    /// 与 stop 的区别:不注销已注册工具(进程即将整体退出,残留注册项的调用会以
    /// 「连接已关闭」快速失败,不会悬挂)。
    pub async fn shutdown(&self) {
        let handles: Vec<ServerHandle> = {
            let mut g = self.lock_servers();
            g.drain().filter_map(|(_, rt)| rt.handle).collect()
        };
        for h in &handles {
            h.client.close();
        }
        drop(handles); // 显式落 Drop 语义(kill 进程、abort stderr 任务)
    }
}

/// 构造并注册单个 MCP 工具(attach 与 relist 共用);调用方负责名字去重。
/// MCP 工具标记为外部来源:参数对引擎不透明,三档授权模式按系统路径扫描处理。
fn register_mcp_tool(
    registry: &Arc<dyn ToolRegistrar>,
    sanitized: &str,
    client: &Arc<McpClient>,
    tool: &client::McpToolInfo,
) {
    let definition = ToolDefinition {
        name: prefixed_tool_name(sanitized, &tool.name),
        description: if tool.description.is_empty() {
            format!("[MCP·{sanitized}] {}", tool.name)
        } else {
            format!("[MCP·{sanitized}] {}", tool.description)
        },
        parameters: tool.input_schema.clone(),
    };
    let call_client = client.clone();
    let remote_name = tool.name.clone();
    let execute: ToolExecutor = Arc::new(
        move |args: Value, _ctx| -> BoxFuture<'static, Result<String, String>> {
            let call_client = call_client.clone();
            let remote_name = remote_name.clone();
            Box::pin(async move { call_client.call_tool(&remote_name, args).await })
        },
    );
    registry.register_external(
        definition,
        execute,
        Some(MCP_TOOL_TIMEOUT),
        crate::models::tool_policy::ToolOrigin::Mcp,
    );
}

/// 服务器死亡(读循环 EOF/错误/超限):注销其全部工具、state=failed、留 last_error。
/// 由 client 的退出回调触发(Weak 升级成功才执行;管理器已销毁则忽略)。幂等。
fn handle_server_exit(inner: &Inner, sanitized: &str) {
    let registry = inner
        .registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let mut g = inner.servers.lock().unwrap_or_else(|e| e.into_inner());
    let Some(rt) = g
        .values_mut()
        .find(|r| r.sanitized == sanitized && matches!(r.state, McpServerState::Running))
    else {
        return;
    };
    if let Some(registry) = &registry {
        for name in rt.tool_names.drain(..) {
            registry.unregister(&name);
        }
    }
    rt.state = McpServerState::Failed;
    rt.last_error = Some(
        "服务器进程已退出(stdout EOF 或读错误)。下一步:可在设置 → MCP 服务 中重启该服务器".into(),
    );
    rt.handle = None; // 句柄 Drop:进程已死,kill 幂等
    tracing::warn!(server = %rt.raw_name, "MCP 服务器进程退出,其工具已从注册表注销");
}

/// `list_changed` 重列(裁定 9):单飞 + 尾随——重列进行中再来通知 → 记 pending,收尾重跑一次。
///
/// 通知可能**紧跟首轮 tools/list 响应**到达(此时装配尚未把台账入库):先短轮询等待台账
/// (最多约 2s)再动作;装配失败/管理器销毁则静默放弃(该窗口内首轮列表已含最新状态,
/// 丢弃无害)。
fn spawn_relist(inner: Arc<Inner>, sanitized: String) {
    tokio::spawn(async move {
        let mut armed = false;
        for _ in 0..200 {
            {
                let mut g = inner.servers.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(rt) = g.values_mut().find(|r| r.sanitized == sanitized) {
                    if !matches!(rt.state, McpServerState::Running) {
                        return;
                    }
                    if rt.relist_running {
                        rt.relist_pending = true;
                        return;
                    }
                    rt.relist_running = true;
                    armed = true;
                }
            }
            if armed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if !armed {
            return;
        }
        loop {
            relist_tools(&inner, &sanitized).await;
            let mut g = inner.servers.lock().unwrap_or_else(|e| e.into_inner());
            let Some(rt) = g.values_mut().find(|r| r.sanitized == sanitized) else {
                return;
            };
            if rt.relist_pending {
                rt.relist_pending = false;
                drop(g);
                continue;
            }
            rt.relist_running = false;
            return;
        }
    });
}

/// 重列单台工具并做 diff(新增注册 / 消失注销 / 同名刷新定义);重列失败保留现状(裁定 9)。
async fn relist_tools(inner: &Arc<Inner>, sanitized: &str) {
    // 1) 取 client 与注册表(顺序短锁,不同时持两把)
    let client = {
        let g = inner.servers.lock().unwrap_or_else(|e| e.into_inner());
        let Some(rt) = g.values().find(|r| r.sanitized == sanitized) else {
            return;
        };
        if !matches!(rt.state, McpServerState::Running) {
            return;
        }
        rt.handle.as_ref().map(|h| h.client.clone())
    };
    let Some(client) = client else { return };
    let Some(registry) = inner
        .registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    else {
        return;
    };
    // 2) 重列(网络往返,不持锁)
    let tools = match client.list_tools().await {
        Ok(t) => t,
        Err(e) => {
            let mut g = inner.servers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(rt) = g.values_mut().find(|r| r.sanitized == sanitized) {
                rt.last_error = Some(format!("工具列表重列失败:{e}(保留现状)"));
            }
            tracing::warn!(server = sanitized, error = %e, "MCP tools/list_changed 重列失败,保留现状");
            return;
        }
    };
    // 3) 先注册(新增与同名刷新;同批重名先到者生效),再注销消失,最后更新台账
    let mut new_names: Vec<String> = Vec::new();
    let mut diff_errors: Vec<String> = Vec::new();
    for tool in &tools {
        let full_name = prefixed_tool_name(sanitized, &tool.name);
        if new_names.contains(&full_name) {
            diff_errors.push(format!(
                "工具 {} 的注册名 {full_name} 与其他工具冲突,已跳过",
                tool.name
            ));
            continue;
        }
        register_mcp_tool(&registry, sanitized, &client, tool);
        new_names.push(full_name);
    }
    let (added, removed) = {
        let mut g = inner.servers.lock().unwrap_or_else(|e| e.into_inner());
        let Some(rt) = g.values_mut().find(|r| r.sanitized == sanitized) else {
            return;
        };
        let old = std::mem::replace(&mut rt.tool_names, new_names.clone());
        let added = new_names.iter().filter(|n| !old.contains(n)).count();
        let gone: Vec<String> = old.into_iter().filter(|n| !new_names.contains(n)).collect();
        for name in &gone {
            registry.unregister(name);
        }
        if !diff_errors.is_empty() {
            rt.last_error = Some(diff_errors.join("; "));
        }
        (added, gone.len())
    };
    tracing::info!(
        server = sanitized,
        added,
        removed,
        "MCP 工具列表已按 list_changed 重列"
    );
}

/// 服务器名 sanitize 为 [a-z0-9_](小写;非法字符归并为单个 _):
/// 作为工具名前缀段,需满足 function calling 工具名约束且不与内建/插件冲突。
pub fn sanitize_server_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_underscore = false;
    for ch in name.trim().chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            last_underscore = false;
        } else if !last_underscore && !out.is_empty() {
            out.push('_');
            last_underscore = true;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "server".to_string() // 全非法字符的兜底名(如纯中文名)
    } else {
        out
    }
}

/// MCP 侧工具名净化:function calling 允许 [a-zA-Z0-9_-],其余字符替换为 _。
fn sanitize_tool_part(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "tool".to_string()
    } else {
        out
    }
}

/// 完整注册名:mcp_{server}_{tool},总长钳 64(OpenAI function 名上限)。
fn prefixed_tool_name(sanitized_server: &str, tool: &str) -> String {
    let mut name = format!("mcp_{}_{}", sanitized_server, sanitize_tool_part(tool));
    if name.len() > 64 {
        name.truncate(64);
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::tool_policy::ToolRisk;
    use crate::models::types::ToolContext;
    // 测试自建真实注册表(McpManager::start 的装配行为需断言真实注册表状态);
    // 生产代码不依赖 tools —— 见本文件头部「代际纪律」。
    use crate::services::settings_service::RuntimeSettings;
    use crate::tools::permissions::PermissionDecision;
    use crate::tools::registry::ToolRegistry;
    use serde_json::json;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::{duplex, AsyncWriteExt, BufReader, DuplexStream};

    #[test]
    fn sanitize_server_name_normalizes_to_prefix_charset() {
        assert_eq!(sanitize_server_name("File System"), "file_system");
        assert_eq!(sanitize_server_name("fs-2.0"), "fs_2_0");
        assert_eq!(sanitize_server_name("  EDGE--case  "), "edge_case");
        assert_eq!(sanitize_server_name("文件系统"), "server", "全非法字符兜底");
        assert_eq!(sanitize_server_name(""), "server");
    }

    #[test]
    fn prefixed_tool_name_has_mcp_prefix_and_length_cap() {
        assert_eq!(prefixed_tool_name("fs", "read_file"), "mcp_fs_read_file");
        let long = prefixed_tool_name("s", &"x".repeat(100));
        assert!(long.len() <= 64, "工具名应钳到 64 字符: {}", long.len());
        assert!(long.starts_with("mcp_s_"));
    }

    fn ctx() -> ToolContext {
        ToolContext {
            session_id: "s".into(),
            character_id: "c".into(),
            agent_depth: 0,
            scope: None,
            budget: None,
        }
    }

    fn allow() -> PermissionDecision {
        PermissionDecision {
            allowed: true,
            risk: ToolRisk::Dangerous,
            reason: "测试放行".into(),
        }
    }

    /// 内存管测试用的配置(不 spawn:直接走 attach)
    fn cfg_mem(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            command: "kedai-mem-test-no-spawn".into(),
            args: vec![],
            enabled: true,
        }
    }

    /// 内存管假服务器:initialize/tools/list/tools/call 固定响应(与 client.rs 测试同范式)
    fn mock_server(mut io: DuplexStream) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut reader = BufReader::new(&mut io);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let Ok(req) = serde_json::from_str::<Value>(line.trim()) else {
                    continue;
                };
                let Some(id) = req.get("id").cloned() else {
                    continue;
                };
                let result = match req["method"].as_str().unwrap_or("") {
                    "initialize" => {
                        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": { "name": "mock", "version": "1" } })
                    }
                    "tools/list" => json!({ "tools": [
                        { "name": "read_file", "description": "读文件", "inputSchema": { "type": "object" } },
                    ]}),
                    "tools/call" => {
                        json!({ "content": [{ "type": "text", "text": "文件内容-甲乙丙" }] })
                    }
                    _ => json!({}),
                };
                let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
                if reader
                    .get_mut()
                    .write_all(format!("{resp}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
                let _ = reader.get_mut().flush().await;
            }
        })
    }

    fn client_pair() -> (McpClient, DuplexStream) {
        let (a, b) = duplex(64 * 1024);
        let (ar, aw) = tokio::io::split(a);
        (McpClient::new(Box::new(ar), Box::new(aw)), b)
    }

    /// 可编程假服务器(list_changed 测试用):声明 `tools.listChanged`,
    /// 按调用序号返回不同工具页,并在前两页响应后各推一条 list_changed 通知。
    fn mock_server_list_changed(
        mut io: DuplexStream,
        pages: Vec<Vec<&'static str>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut reader = BufReader::new(&mut io);
            let mut line = String::new();
            let mut list_calls = 0usize;
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let Ok(req) = serde_json::from_str::<Value>(line.trim()) else {
                    continue;
                };
                let Some(id) = req.get("id").cloned() else {
                    continue;
                };
                let method = req["method"].as_str().unwrap_or("").to_string();
                let result = match method.as_str() {
                    "initialize" => json!({
                        "protocolVersion": "2025-06-18",
                        "capabilities": { "tools": { "listChanged": true } },
                        "serverInfo": { "name": "mock", "version": "1" }
                    }),
                    "tools/list" => {
                        let idx = list_calls.min(pages.len().saturating_sub(1));
                        list_calls += 1;
                        let tools: Vec<Value> = pages[idx]
                            .iter()
                            .map(|n| json!({ "name": n, "description": "d" }))
                            .collect();
                        json!({ "tools": tools })
                    }
                    _ => json!({}),
                };
                let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
                if reader
                    .get_mut()
                    .write_all(format!("{resp}\n").as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
                // 前两页响应后各推一条 list_changed(驱动重列与 diff)
                if method == "tools/list" && list_calls <= 2 {
                    let note =
                        json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" })
                            .to_string();
                    if reader
                        .get_mut()
                        .write_all(format!("{note}\n").as_bytes())
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = reader.get_mut().flush().await;
            }
        })
    }

    /// 轮询等待条件(后台重列任务/异步回调不保证同步完成)
    async fn wait_until(what: &str, cond: impl Fn() -> bool) {
        for _ in 0..300 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("等待超时: {what}");
    }

    /// 空配置/全禁用:start 零注册、零服务器(默认关路径的零副作用保证)
    ///
    /// 代际倒置后 `start` 收「已过滤的服务器列表」,故本测试分两段断言:
    /// ① 空列表(等价于 `mcp_enabled=false` 或设置里无服务器)→ 零副作用;
    /// ② **宿主过滤语义**:`enabled=false` 的条目必须被宿主滤掉,不得进入 `start`。
    ///    过滤动作在组合根(`api/app_state.rs`),此处以同一规则复现并锁定契约。
    #[tokio::test]
    async fn start_with_empty_or_disabled_config_is_noop() {
        let reg = Arc::new(ToolRegistry::new());
        let mgr = McpManager::empty();
        let mut settings = RuntimeSettings::from_config(&crate::config::AppConfig::from_env());
        settings.mcp_servers = vec![McpServerConfig {
            name: "off".into(),
            command: "cmd".into(),
            args: vec![],
            enabled: false, // 显式禁用:不应起进程
        }];
        // 宿主过滤规则(与 api/app_state.rs 的启动装配一致)
        let servers: Vec<McpServerConfig> = settings
            .mcp_servers
            .iter()
            .filter(|s| s.enabled)
            .cloned()
            .collect();
        assert!(servers.is_empty(), "enabled=false 的服务器应被宿主滤除");
        mgr.start(servers, reg.clone()).await;
        assert_eq!(mgr.server_count(), 0);
        assert!(reg.list_definitions().is_empty(), "不应注册任何工具");
        assert!(mgr.snapshot().is_empty(), "无服务器时不产生台账条目");
    }

    /// 装配链路:握手 + tools/list → 工具以 mcp_{server}_{tool} 注册,
    /// 执行器经注册表(120s 档)转发 tools/call 并拼接 text 结果;台账记工具名与协商版本。
    #[tokio::test]
    async fn attach_registers_prefixed_tools_that_forward_calls() {
        let reg = Arc::new(ToolRegistry::new());
        let mgr = McpManager::empty();
        mgr.start(vec![], reg.clone()).await; // 注入注册表(内存管路径)
        let (client, server_io) = client_pair();
        let server_task = mock_server(server_io);

        let n = mgr
            .attach(cfg_mem("File System"), "file_system".into(), client, None)
            .await
            .expect("装配应成功");
        assert_eq!(n, 1);
        assert_eq!(mgr.server_count(), 1);
        let snap = mgr.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].state, "running");
        assert_eq!(
            snap[0].tool_names,
            vec!["mcp_file_system_read_file".to_string()],
            "台账应记录工具名"
        );
        assert_eq!(
            snap[0].protocol_version.as_deref(),
            Some("2025-06-18"),
            "协商版本应记录"
        );

        let defs = reg.list_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "mcp_file_system_read_file", "工具名应带前缀");
        assert!(defs[0].description.contains("[MCP·file_system]"));
        // MCP 档超时 120s(register_with_timeout 单工具档)
        let tool = reg.get("mcp_file_system_read_file").unwrap();
        assert_eq!(tool.timeout, Some(MCP_TOOL_TIMEOUT));

        let out = reg
            .execute_with_decision(
                "mcp_file_system_read_file",
                r#"{"path":"a.txt"}"#,
                ctx(),
                &allow(),
            )
            .await
            .expect("转发调用应成功");
        assert_eq!(out, "文件内容-甲乙丙");

        // shutdown(进程退出路径):句柄清空,残留注册项调用报「连接已关闭」而不是悬挂
        mgr.shutdown().await;
        assert_eq!(mgr.server_count(), 0);
        let err = reg
            .execute_with_decision(
                "mcp_file_system_read_file",
                r#"{"path":"a.txt"}"#,
                ctx(),
                &allow(),
            )
            .await
            .unwrap_err();
        assert!(err.contains("连接已关闭"), "关闭后调用应明确失败: {err}");

        // 注册表执行器仍持有 client Arc(写端未关),假服务器等不到 EOF——直接 abort
        server_task.abort();
    }

    /// 握手失败的台:attach 返回 Err 且不残留台账/注册项(调用方据此记 failed)
    #[tokio::test]
    async fn attach_failure_leaves_no_trace() {
        let reg = Arc::new(ToolRegistry::new());
        let mgr = McpManager::empty();
        mgr.start(vec![], reg.clone()).await;
        let (client, server_io) = client_pair();
        drop(server_io); // 对端直接断开 → initialize 必失败
        let err = mgr
            .attach(cfg_mem("broken"), "broken".into(), client, None)
            .await
            .unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(mgr.server_count(), 0);
        assert!(
            mgr.snapshot().is_empty(),
            "失败不留台账(由调用方 set_failed 记录)"
        );
        assert!(reg.list_definitions().is_empty());
    }

    /// 停止(PLGM 2.5):注销工具、state=stopped、未知服务器返回 false;
    /// 停止后的服务器侧关闭**不得翻回 failed**(主动 stop 的 reader abort 不触发死亡回调)。
    #[tokio::test]
    async fn stop_unregisters_tools_and_reports_state() {
        let reg = Arc::new(ToolRegistry::new());
        let mgr = McpManager::empty();
        mgr.start(vec![], reg.clone()).await;
        let (client, server_io) = client_pair();
        let server_task = mock_server(server_io);
        mgr.attach(cfg_mem("fs"), "fs".into(), client, None)
            .await
            .expect("装配应成功");
        assert_eq!(mgr.server_count(), 1);

        assert!(mgr.stop("fs").await, "命中服务器应返回 true");
        assert_eq!(mgr.server_count(), 0);
        assert_eq!(mgr.snapshot()[0].state, "stopped");
        assert!(reg.get("mcp_fs_read_file").is_none(), "停止应注销工具");
        assert!(!mgr.stop("nope").await, "未知服务器应返回 false");

        server_task.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            mgr.snapshot()[0].state,
            "stopped",
            "停止态不得被死亡回调翻改"
        );
    }

    /// 死亡注销(L16①,PLGM 2.4):读循环 EOF → 工具注销 + state=failed + last_error 留痕
    #[tokio::test]
    async fn server_death_unregisters_tools_and_marks_failed() {
        let reg = Arc::new(ToolRegistry::new());
        let mgr = McpManager::empty();
        mgr.start(vec![], reg.clone()).await;
        let (client, server_io) = client_pair();
        let server_task = mock_server(server_io);
        mgr.attach(cfg_mem("fs"), "fs".into(), client, None)
            .await
            .expect("装配应成功");
        assert!(reg.get("mcp_fs_read_file").is_some());

        // 服务器死亡(stdout EOF):abort 假服务器任务 → 其持有的对端被 drop
        server_task.abort();
        wait_until("死亡注销生效", || {
            mgr.snapshot().first().map(|s| s.state) == Some("failed")
                && reg.get("mcp_fs_read_file").is_none()
        })
        .await;
        let snap = &mgr.snapshot()[0];
        assert!(
            snap.last_error.as_deref().unwrap_or("").contains("重启"),
            "last_error 应含重启指引: {snap:?}"
        );
        assert!(snap.tool_names.is_empty(), "工具台账应清空");
        assert_eq!(mgr.server_count(), 0);
        // 注销后调用报「未注册」(不再悬挂)
        let err = reg
            .execute_with_decision("mcp_fs_read_file", "{}", ctx(), &allow())
            .await
            .unwrap_err();
        assert!(err.contains("未注册"), "{err}");
    }

    /// list_changed 重列(PLGM 2.3):声明能力位 → 通知驱动 diff——先增后减,单飞 + 尾随收尾
    #[tokio::test]
    async fn list_changed_drives_tool_diff() {
        let reg = Arc::new(ToolRegistry::new());
        let mgr = McpManager::empty();
        mgr.start(vec![], reg.clone()).await;
        let (client, server_io) = client_pair();
        let server_task =
            mock_server_list_changed(server_io, vec![vec!["t1"], vec!["t1", "t2"], vec!["t2"]]);
        let n = mgr
            .attach(cfg_mem("mock"), "mock".into(), client, None)
            .await
            .expect("装配应成功");
        assert_eq!(n, 1, "首次列出 1 个工具");
        assert!(reg.get("mcp_mock_t1").is_some());

        // 第一轮重列:新增 t2
        wait_until("重列新增 t2", || reg.get("mcp_mock_t2").is_some()).await;
        // 第二轮重列:移除 t1(单飞 + 尾随收尾)
        wait_until("重列移除 t1", || reg.get("mcp_mock_t1").is_none()).await;
        assert!(reg.get("mcp_mock_t2").is_some(), "t2 应保留");
        assert_eq!(
            mgr.snapshot()[0].tool_names,
            vec!["mcp_mock_t2".to_string()],
            "台账应反映最终工具集"
        );
        mgr.shutdown().await;
        server_task.abort();
    }

    /// 冲突预检(裁定 7):服务器 sanitize 名碰撞 → 后到者跳过并记 last_error,不起进程
    #[tokio::test]
    async fn start_conflict_precheck_skips_later_server() {
        let reg = Arc::new(ToolRegistry::new());
        let mgr = McpManager::empty();
        let mk = |name: &str| McpServerConfig {
            name: name.into(),
            // 不存在的命令:首台 spawn 快速失败(不遗留长驻进程)
            command: "kedai-no-such-command-9z8y7x".into(),
            args: vec![],
            enabled: true,
        };
        mgr.start(vec![mk("File System"), mk("file-system")], reg.clone())
            .await;
        let snap = mgr.snapshot();
        assert_eq!(snap.len(), 2, "两台都应留台账");
        let first = snap.iter().find(|s| s.name == "File System").unwrap();
        let second = snap.iter().find(|s| s.name == "file-system").unwrap();
        assert_eq!(first.state, "failed");
        assert!(
            first
                .last_error
                .as_deref()
                .unwrap_or("")
                .contains("启动失败"),
            "首台应记 spawn 失败: {first:?}"
        );
        assert_eq!(second.state, "failed");
        assert!(
            second.last_error.as_deref().unwrap_or("").contains("冲突"),
            "后到者应记冲突跳过: {second:?}"
        );
    }
}
