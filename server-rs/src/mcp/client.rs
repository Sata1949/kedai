// MCP stdio 客户端协议层(批次 6.2,L3 隔离):JSON-RPC 2.0 over stdio。
//
// 行帧说明:MCP stdio 传输是「换行分隔 JSON」(NDJSON)——每条消息一个完整 JSON 对象,
// 以 \n 结尾;不是 LSP 的 Content-Length 头帧。实现与测试都按行读写。
//
// 读写解耦:客户端只依赖 AsyncRead + AsyncWrite(trait object),测试用 tokio::io::duplex
// 双工内存管模拟服务器,不起真实进程;process.rs 负责把子进程 stdin/stdout 接到这里。
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

/// initialize 握手超时(冷启动;超时按「服务器不可用」处理,记 warn 并禁用,不 panic)
pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);
/// tools/list 超时(启动装配路径,与握手同档)
pub const LIST_TOOLS_TIMEOUT: Duration = Duration::from_secs(30);
/// tools/call 超时:110s,略小于注册表 MCP 档 120s——让客户端先返回可读错误,
/// 注册表 120s 硬超时只做兜底(错误文案带「下一步」指引)。
pub const CALL_TOOL_TIMEOUT: Duration = Duration::from_secs(110);

/// initialize 握手声明的协议版本(2025-06-18;PLGM 2.1,2026-10-07 由 2024-11-05 升级)。
/// 旧服务器在 initialize 响应里声明旧版本时**容忍并记录**(降级不拒连,见 [`McpClient::initialize`])。
const PROTOCOL_VERSION: &str = "2025-06-18";

/// tools/list 分页遍历上限(保护:防服务器给不完的游标把装配拖死)
const MAX_LIST_PAGES: usize = 50;
/// tools/list 工具总数上限(保护:超限按 Err 上报,不静默截断)
const MAX_LIST_TOOLS: usize = 1000;

/// initialize 协商结果(PLGM 2.1):服务器声明的版本 / 能力 / 标识,存档供管理器读取。
#[derive(Debug, Clone)]
pub struct InitializeOutcome {
    /// 服务器声明的协议版本(缺失为空串)
    pub protocol_version: String,
    /// 服务器能力对象(如 `{"tools":{"listChanged":true}}`)
    pub capabilities: Value,
    /// 服务器标识(serverInfo)
    pub server_info: Value,
}

impl InitializeOutcome {
    /// 服务器是否声明「工具列表可变更」通知(PLGM 2.3 的重列门控)
    pub fn supports_tools_list_changed(&self) -> bool {
        self.capabilities
            .pointer("/tools/listChanged")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// tools/list 返回的单个工具描述
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    /// JSON Schema(MCP 字段名 inputSchema;OpenAI 兼容工具定义直接复用)
    pub input_schema: Value,
}

type BoxedReader = BufReader<Box<dyn AsyncRead + Unpin + Send>>;
type BoxedWriter = Box<dyn AsyncWrite + Unpin + Send>;

/// 通知回调(服务器→客户端,如 `notifications/tools/list_changed`)
pub type NotificationHook = Arc<dyn Fn(&str, &Value) + Send + Sync>;
/// 读循环终止回调(EOF/读错误/行长超限;死亡注销用;主动 close 不触发)
pub type ExitHook = Arc<dyn Fn() + Send + Sync>;

/// 共享内部状态:reader 任务与 request() 都要访问(等待表按 id 派发响应)
struct Shared {
    writer: tokio::sync::Mutex<BoxedWriter>,
    /// 在飞请求:id → 响应通道。std Mutex 仅在同步代码块内取放,不跨 .await。
    pending: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    next_id: AtomicU64,
    /// 关停标志:close()/Drop 后新请求立即失败(执行器闭包可能仍持 Arc,
    /// 不能依赖「最后一个 Arc 析构」来表达关停语义)
    closed: AtomicBool,
    /// initialize 协商结果(PLGM 2.1;供管理器读取能力位与版本记录)
    info: Mutex<Option<InitializeOutcome>>,
    /// 通知回调(构造后可设,读循环运行期读取;见 [`McpClient::set_notification_hook`])
    notification_hook: Mutex<Option<NotificationHook>>,
    /// 读循环终止回调(见 [`McpClient::set_exit_hook`])
    exit_hook: Mutex<Option<ExitHook>>,
}

/// MCP stdio 客户端:一个后台 reader 任务按 id 把响应派发给在飞请求。
/// Drop 时 abort reader 任务并让所有在飞请求以「连接已关闭」失败。
pub struct McpClient {
    shared: Arc<Shared>,
    reader_task: tokio::task::JoinHandle<()>,
}

impl McpClient {
    /// 在给定字节流上启动客户端(立即 spawn reader 任务;初始化握手由 initialize() 显式发起)
    pub fn new(read: Box<dyn AsyncRead + Unpin + Send>, write: BoxedWriter) -> Self {
        let shared = Arc::new(Shared {
            writer: tokio::sync::Mutex::new(write),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            info: Mutex::new(None),
            notification_hook: Mutex::new(None),
            exit_hook: Mutex::new(None),
        });
        let reader_shared = shared.clone();
        let reader_task = tokio::spawn(async move {
            read_loop(Box::new(BufReader::new(read)), reader_shared).await;
        });
        McpClient {
            shared,
            reader_task,
        }
    }

    /// initialize 握手:发送 initialize(clientInfo: kedai/0.2.0,capabilities 空),
    /// 成功后回 notifications/initialized 通知(无 id,不等响应)。
    ///
    /// PLGM 2.1(2026-10-07):声明版本升级到 2025-06-18;响应里的 protocolVersion/
    /// capabilities/serverInfo 存档供管理器读取(能力位门控 list_changed 重列);
    /// 服务器声明旧版本时**容忍并警告**(降级不拒连——MCP 协商语义本就允许回退)。
    pub async fn initialize(&self) -> Result<(), String> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "kedai", "version": env!("CARGO_PKG_VERSION") },
        });
        let result = self
            .request("initialize", params, INITIALIZE_TIMEOUT)
            .await?;
        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if protocol_version != PROTOCOL_VERSION {
            tracing::warn!(
                declared = %protocol_version,
                requested = PROTOCOL_VERSION,
                "MCP 服务器声明的协议版本与请求不一致,按声明版本降级使用(容忍)"
            );
        }
        *self.shared.info.lock().unwrap_or_else(|e| e.into_inner()) = Some(InitializeOutcome {
            protocol_version,
            capabilities: result.get("capabilities").cloned().unwrap_or(json!({})),
            server_info: result.get("serverInfo").cloned().unwrap_or(Value::Null),
        });
        self.notify("notifications/initialized", json!({})).await
    }

    /// initialize 协商结果(先于 `initialize()` 完成时为 None)
    pub fn initialize_outcome(&self) -> Option<InitializeOutcome> {
        self.shared
            .info
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 注册通知回调(服务器→客户端通知,如 `notifications/tools/list_changed`);
    /// 构造后运行期可设,重复设置覆盖。回调在 reader 任务内同步调用,不得做重活。
    pub fn set_notification_hook(&self, hook: NotificationHook) {
        *self
            .shared
            .notification_hook
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(hook);
    }

    /// 注册读循环终止回调(EOF/读错误/行长超限触发;`close()`/Drop 主动关停**不触发**,
    /// 由 reader task abort 保证)。回调在 reader 任务内同步调用,不得做重活。
    pub fn set_exit_hook(&self, hook: ExitHook) {
        *self
            .shared
            .exit_hook
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(hook);
    }

    /// tools/list:列出服务器全部工具(**分页遍历**,PLGM 2.2)。
    ///
    /// 逐页跟随 `nextCursor` 直到缺席;超页数/超工具数上限按 `Err` 上报(不静默截断)。
    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>, String> {
        let mut tools: Vec<McpToolInfo> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_LIST_PAGES {
            let mut params = serde_json::Map::new();
            if let Some(c) = &cursor {
                params.insert("cursor".to_string(), Value::String(c.clone()));
            }
            let result = self
                .request("tools/list", Value::Object(params), LIST_TOOLS_TIMEOUT)
                .await?;
            if let Some(items) = result.get("tools").and_then(Value::as_array) {
                for t in items {
                    let Some(name) = t.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    tools.push(McpToolInfo {
                        name: name.to_string(),
                        description: t
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        input_schema: t
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({ "type": "object" })),
                    });
                }
            }
            if tools.len() > MAX_LIST_TOOLS {
                return Err(format!(
                    "tools/list 工具数超过上限 {MAX_LIST_TOOLS},疑似服务器分页异常,已中止装配"
                ));
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            match &cursor {
                None => return Ok(tools),
                Some(c) => {
                    tracing::debug!(cursor = %c, page_tools = tools.len(), "MCP tools/list 继续翻页")
                }
            }
        }
        Err(format!(
            "tools/list 翻页超过上限 {MAX_LIST_PAGES} 页,疑似游标不收敛,已中止装配"
        ))
    }

    /// tools/call:调用工具,把 result.content[] 中的 text 项拼接为 String 返回;
    /// result.isError = true 时按工具级失败返回 Err(文本同样取自 content)。
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String, String> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                CALL_TOOL_TIMEOUT,
            )
            .await?;
        let text = extract_content_text(&result);
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(format!("MCP 工具 \"{name}\" 返回错误:{text}"));
        }
        Ok(text)
    }

    /// 显式关停:标志位置位 → 新请求立即失败;abort reader 任务;
    /// 清空等待表,在飞请求以「连接已关闭」收场。幂等。
    /// (进程句柄的 kill 由 McpProcess 侧负责,本方法只管协议层)
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        self.reader_task.abort();
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// 发送通知(无 id,不等响应;写失败仅告警——通知不可重试也不影响在飞请求)
    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let frame = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let mut line = frame.to_string();
        line.push('\n');
        let mut w = self.shared.writer.lock().await;
        w.write_all(line.as_bytes())
            .await
            .map_err(|e| format!("MCP 通知 \"{method}\" 写入失败: {e}"))?;
        w.flush()
            .await
            .map_err(|e| format!("MCP 通知 \"{method}\" 刷写失败: {e}"))
    }

    /// 发送请求并等待响应:先在等待表登记 id,再写行帧,最后带超时等 oneshot。
    /// 超时/写失败都会清掉等待表项,避免泄漏;响应乱序或未知 id 由 reader 任务丢弃。
    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        if self.shared.closed.load(Ordering::Relaxed) {
            return Err(format!(
                "MCP 请求 \"{method}\" 失败:连接已关闭(客户端已关停)。下一步:可在设置 → MCP 服务 中重启该服务器"
            ));
        }
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);

        let frame = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut line = frame.to_string();
        line.push('\n');
        {
            let mut w = self.shared.writer.lock().await;
            let write_result = match w.write_all(line.as_bytes()).await {
                Ok(()) => w.flush().await,
                Err(e) => Err(e),
            };
            if let Err(e) = write_result {
                self.shared
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                return Err(format!("MCP 请求 \"{method}\" 写入失败: {e}"));
            }
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => {
                if let Some(err) = resp.get("error") {
                    let msg = err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("未知错误");
                    return Err(format!("MCP 请求 \"{method}\" 被服务器拒绝: {msg}"));
                }
                Ok(resp.get("result").cloned().unwrap_or(Value::Null))
            }
            // oneshot 发送端被 drop = reader 任务结束(连接关闭/进程退出)
            Ok(Err(_)) => Err(format!(
                "MCP 请求 \"{method}\" 失败:连接已关闭(服务器进程可能已退出)。下一步:可在设置 → MCP 服务 中重启该服务器"
            )),
            Err(_) => {
                self.shared
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                Err(format!(
                    "MCP 请求 \"{method}\" 超时({}s)。下一步:检查该 MCP 服务器是否正常运行,或在设置中禁用",
                    timeout.as_secs()
                ))
            }
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // 与 close() 同语义:停 reader、置关停标志;在飞请求的 oneshot 发送端
        // 随等待表清空/Shared 析构而 drop,等待方收到「连接已关闭」错误,不悬挂。
        self.close();
    }
}

/// 单行最大字节数:MCP 服务器(外部不可信)若输出超长行(无换行的海量数据),
/// `read_line` 会持续增长缓冲直至耗尽内存。超限即判定协议异常并结束读循环,
/// 由 close() 通知等待方(宁可断开也不 OOM)。
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// 后台读循环:逐行读取 NDJSON,按 id 派发给等待表;
/// 坏行跳过、通知派发给注册回调、未知 id 记警告(容错,不中断会话);
/// EOF / 读错误 / 行长超限即结束,结束时触发退出回调(死亡注销)。
async fn read_loop(reader: Box<BoxedReader>, shared: Arc<Shared>) {
    let mut reader = reader;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break, // EOF:服务器关闭 stdout(进程退出)
            Ok(_) => {}
            Err(_) => break,
        }
        // 行长上限:read_line 读到换行才返回,超长行会撑爆内存(见 MAX_LINE_BYTES 注释)
        if line.len() > MAX_LINE_BYTES {
            tracing::warn!(
                bytes = line.len(),
                limit = MAX_LINE_BYTES,
                "MCP 单行超出上限,判定协议异常并结束读循环"
            );
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
            tracing::warn!(
                line_preview = trimmed.chars().take(120).collect::<String>(),
                "MCP 收到无法解析的行,已跳过"
            );
            continue;
        };
        // 只关心带 id 的响应;通知(无 id)/请求(服务器→客户端)分别派发或记警告(PLGM 2.1/2.3)
        let Some(id) = msg.get("id").and_then(Value::as_u64) else {
            if let Some(method) = msg.get("method").and_then(Value::as_str) {
                let hook = shared
                    .notification_hook
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if let Some(hook) = hook {
                    hook(method, &msg);
                } else {
                    tracing::debug!(method, "MCP 收到通知(未注册回调,已忽略)");
                }
            } else {
                tracing::warn!(
                    line_preview = trimmed.chars().take(120).collect::<String>(),
                    "MCP 收到无 id 且无 method 的报文,已跳过"
                );
            }
            continue;
        };
        // 服务器→客户端**请求**(带 id 且有 method):v1 不支持,记警告日志(便于诊断)
        if msg.get("method").is_some() {
            let m = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
            tracing::warn!(id, method = m, "MCP 服务器发起了客户端未支持的请求,已忽略");
            continue;
        }
        let tx = shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        if let Some(tx) = tx {
            // 接收端可能已因超时被清走;发送失败忽略
            let _ = tx.send(msg);
        } else {
            // 未知/已超时 id:丢弃但留痕(PLGM 2.1 的「未知 id 记警告」)
            tracing::warn!(id, "MCP 收到未知或已超时的响应 id,已丢弃");
        }
    }
    // 读循环结束(EOF/错误/行长超限):清空等待表,让所有在飞请求以「连接已关闭」失败
    shared
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    // 终止回调(死亡注销;close()/Drop 走 abort 不走这里——两条路径语义不同)
    let hook = shared
        .exit_hook
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(hook) = hook {
        hook();
    }
}

/// 提取 tools/call 的文本结果(PLGM 2.1,2026-10-07 升级)。
///
/// 规则:
/// - `content[]` 逐项转文本:`text` 原样;`resource_link`/`resource` 给 URI 占位;
///   `image` 给「base64 已省略」占位;其余类型给类型占位——**不再静默丢弃非文本项**;
/// - 文本拼接为空时,**优先取 `structuredContent`**(JSON 文本;部分服务器只回结构化结果),
///   再退整段 JSON 兜底;
/// - 全空给占位文案,避免模型拿到空串困惑。
fn extract_content_text(result: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for item in content {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = item.get("text").and_then(Value::as_str) {
                        parts.push(t.to_string());
                    }
                }
                Some("resource_link") => {
                    let uri = item.get("uri").and_then(Value::as_str).unwrap_or("未知");
                    parts.push(format!("[resource_link: {uri}]"));
                }
                Some("resource") => {
                    let uri = item
                        .pointer("/resource/uri")
                        .and_then(Value::as_str)
                        .unwrap_or("未知");
                    parts.push(format!("[resource: {uri}]"));
                }
                Some("image") => {
                    let mime = item
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("未知类型");
                    parts.push(format!("[image: {mime}(base64 已省略)]"));
                }
                Some(other) => parts.push(format!("[{other} 内容]")),
                None => {}
            }
        }
    }
    let text = parts
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !text.is_empty() {
        return text;
    }
    if let Some(sc) = result.get("structuredContent").filter(|v| !v.is_null()) {
        return match sc {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
    }
    if result.is_null() {
        return "(MCP 工具返回空结果)".to_string();
    }
    result.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, DuplexStream};

    /// 内存双工管模拟 MCP 服务器:逐行读请求,按 method 回固定响应;
    /// respond 闭包可自定义(坏行/乱序 id 容错测试用)。
    fn mock_server(
        mut server: DuplexStream,
        respond: impl Fn(&Value) -> Vec<String> + Send + 'static,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut reader = BufReader::new(&mut server);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(req) = serde_json::from_str::<Value>(trimmed) else {
                    continue;
                };
                for resp in respond(&req) {
                    let mut out = resp;
                    out.push('\n');
                    if reader.get_mut().write_all(out.as_bytes()).await.is_err() {
                        return;
                    }
                }
                let _ = reader.get_mut().flush().await;
            }
        })
    }

    /// 标准响应器:initialize / tools/list / tools/call 各回固定结果,通知不回。
    fn standard_respond(req: &Value) -> Vec<String> {
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let Some(id) = id else {
            return Vec::new(); // 通知:无响应
        };
        let result = match method {
            "initialize" => {
                json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "serverInfo": { "name": "mock", "version": "0.0.1" } })
            }
            "tools/list" => json!({ "tools": [
                { "name": "echo", "description": "回显", "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } } },
                { "name": "fail", "description": "总是失败" },
            ]}),
            "tools/call" => {
                let name = req["params"]["name"].as_str().unwrap_or("");
                if name == "fail" {
                    json!({ "content": [{ "type": "text", "text": "炸了" }], "isError": true })
                } else {
                    let arg = req["params"]["arguments"]["text"].as_str().unwrap_or("");
                    json!({ "content": [
                        { "type": "text", "text": "第一行" },
                        { "type": "resource", "resource": { "uri": "x://y" } },
                        { "type": "text", "text": format!("回显:{arg}") },
                    ]})
                }
            }
            _ => json!({}),
        };
        vec![json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()]
    }

    fn client_pair() -> (McpClient, DuplexStream) {
        let (client_io, server_io) = duplex(64 * 1024);
        let (cr, cw) = tokio::io::split(client_io);
        (McpClient::new(Box::new(cr), Box::new(cw)), server_io)
    }

    #[tokio::test]
    async fn handshake_list_and_call_over_ndjson() {
        let (client, server) = client_pair();
        let server_task = mock_server(server, standard_respond);

        client.initialize().await.expect("握手应成功");
        let tools = client.list_tools().await.expect("tools/list 应成功");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].description, "回显");
        assert_eq!(
            tools[1].input_schema,
            json!({ "type": "object" }),
            "缺 inputSchema 应回退空对象"
        );

        let out = client
            .call_tool("echo", json!({ "text": "你好" }))
            .await
            .expect("tools/call 应成功");
        // 非文本项不再静默丢弃:resource 项给 URI 占位(PLGM 2.1)
        assert_eq!(out, "第一行\n[resource: x://y]\n回显:你好");

        let err = client.call_tool("fail", json!({})).await.unwrap_err();
        assert!(err.contains("炸了"), "isError 应按失败返回: {err}");

        drop(client);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn request_frames_are_jsonrpc2_ndjson() {
        let (client, server) = client_pair();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
        let server_task = tokio::spawn(async move {
            let mut reader = BufReader::new(server);
            let mut line = String::new();
            // 只抓第一帧(initialize 请求),回完继续吞帧到 EOF
            // (不回完就退出会让客户端的 initialized 通知写进已关管道,握手判失败)
            reader.read_line(&mut line).await.unwrap();
            tx.send(line.trim().to_string()).await.unwrap();
            let resp = json!({ "jsonrpc": "2.0", "id": 1, "result": { "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "serverInfo": { "name": "m", "version": "1" } } });
            reader
                .get_mut()
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
            let _ = reader.get_mut().flush().await;
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });

        client.initialize().await.expect("握手应成功");
        let frame: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(frame["jsonrpc"], "2.0");
        assert_eq!(frame["method"], "initialize");
        assert_eq!(frame["id"], 1, "首请求 id 从 1 开始");
        assert_eq!(frame["params"]["clientInfo"]["name"], "kedai");
        assert_eq!(
            frame["params"]["protocolVersion"], PROTOCOL_VERSION,
            "应声明协议版本"
        );

        drop(client);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn bad_lines_and_unknown_ids_are_tolerated() {
        let (client, server) = client_pair();
        // 响应器:先回一行垃圾 + 一个未知 id 响应,再回正常响应
        let server_task = mock_server(server, |req| {
            let mut out = vec![
                "这不是 JSON".to_string(),
                json!({ "jsonrpc": "2.0", "id": 9999, "result": {} }).to_string(),
            ];
            out.extend(standard_respond(req));
            out
        });

        client
            .initialize()
            .await
            .expect("坏行/未知 id 不应打断握手");
        let tools = client.list_tools().await.expect("后续请求应正常");
        assert_eq!(tools.len(), 2);

        drop(client);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn server_disconnect_fails_inflight_request() {
        let (client, server) = client_pair();
        // 服务器不回任何响应,直接关闭
        drop(server);
        let err = client
            .request("tools/list", json!({}), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(
            err.contains("连接已关闭") || err.contains("写入失败"),
            "断连应明确报错: {err}"
        );
    }

    /// PLGM 2.2:tools/list 跟随 nextCursor 翻页;第二页请求应带 cursor 参数。
    #[tokio::test]
    async fn tools_list_follows_pagination_cursor() {
        let (client, server) = client_pair();
        let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen_srv = seen.clone();
        let server_task = mock_server(server, move |req| {
            let Some(id) = req.get("id").cloned() else {
                return Vec::new();
            };
            let method = req.get("method").and_then(Value::as_str).unwrap_or("");
            let result = match method {
                "initialize" => {
                    json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "serverInfo": { "name": "m", "version": "1" } })
                }
                "tools/list" => {
                    seen_srv
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(req["params"].clone());
                    match req["params"].get("cursor").and_then(Value::as_str) {
                        None => json!({ "tools": [{ "name": "t1" }], "nextCursor": "page2" }),
                        Some("page2") => json!({ "tools": [{ "name": "t2" }] }),
                        _ => json!({ "tools": [] }),
                    }
                }
                _ => json!({}),
            };
            vec![json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()]
        });

        client.initialize().await.unwrap();
        let tools = client.list_tools().await.expect("分页遍历应成功");
        assert_eq!(
            tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["t1", "t2"],
            "两页工具应合并"
        );
        let calls = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(calls.len(), 2, "应发出两次 tools/list");
        assert_eq!(calls[1]["cursor"], json!("page2"), "第二页请求应带游标");

        drop(client);
        let _ = server_task.await;
    }

    /// PLGM 2.1:协商结果存档(版本/能力/serverInfo),旧版本声明容忍不拒连。
    #[tokio::test]
    async fn initialize_outcome_records_capabilities_and_tolerates_downgrade() {
        let (client, server) = client_pair();
        let server_task = mock_server(server, |req| {
            let Some(id) = req.get("id").cloned() else {
                return Vec::new();
            };
            let result = json!({
                "protocolVersion": "2024-11-05", // 旧版本声明:容忍
                "capabilities": { "tools": { "listChanged": true } },
                "serverInfo": { "name": "mock", "version": "0" }
            });
            vec![json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()]
        });

        client
            .initialize()
            .await
            .expect("旧版本声明应容忍(降级不拒连)");
        let info = client.initialize_outcome().expect("应存档协商结果");
        assert_eq!(info.protocol_version, "2024-11-05");
        assert!(info.supports_tools_list_changed(), "能力位应可读");
        assert_eq!(info.server_info["name"], json!("mock"));

        drop(client);
        let _ = server_task.await;
    }

    /// PLGM 2.3:服务器通知(无 id,带 method)派发给注册的回调。
    #[tokio::test]
    async fn notification_hook_receives_method() {
        let (client, server) = client_pair();
        let server_task = tokio::spawn(async move {
            let mut reader = BufReader::new(server);
            let mut line = String::new();
            // 抓 initialize 请求 → 回响应 → 主动推一条通知
            reader.read_line(&mut line).await.unwrap();
            let resp = json!({ "jsonrpc": "2.0", "id": 1, "result": { "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "serverInfo": { "name": "m", "version": "1" } } });
            reader
                .get_mut()
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
            let note = json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" });
            reader
                .get_mut()
                .write_all(format!("{note}\n").as_bytes())
                .await
                .unwrap();
            let _ = reader.get_mut().flush().await;
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(4);
        client.set_notification_hook(Arc::new(move |method, _params| {
            let _ = tx.try_send(method.to_string());
        }));
        client.initialize().await.unwrap();
        let method = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("2s 内应收到通知")
            .unwrap();
        assert_eq!(method, "notifications/tools/list_changed");

        drop(client);
        let _ = server_task.await;
    }

    /// PLGM 2.4:EOF 触发退出回调;close() 主动关停(reader abort)不触发。
    #[tokio::test]
    async fn exit_hook_fires_on_eof_but_not_on_close() {
        let (client, server) = client_pair();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(4);
        client.set_exit_hook(Arc::new(move || {
            let _ = tx.try_send(());
        }));
        drop(server); // EOF
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("EOF 应触发退出回调");

        let (client2, _server2) = client_pair();
        let (tx2, mut rx2) = tokio::sync::mpsc::channel::<()>(4);
        client2.set_exit_hook(Arc::new(move || {
            let _ = tx2.try_send(());
        }));
        client2.close();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            rx2.try_recv().is_err(),
            "close() 主动关停不应触发退出回调(仅死亡路径触发)"
        );
    }

    /// PLGM 2.1:结果提取——仅结构化结果时优先 structuredContent;非文本项给占位不静默丢。
    #[tokio::test]
    async fn call_tool_extracts_structured_and_placeholder_content() {
        let (client, server) = client_pair();
        let server_task = mock_server(server, |req| {
            let Some(id) = req.get("id").cloned() else {
                return Vec::new();
            };
            let method = req.get("method").and_then(Value::as_str).unwrap_or("");
            let result = match method {
                "initialize" => {
                    json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "serverInfo": { "name": "m", "version": "1" } })
                }
                "tools/call" => match req["params"]["name"].as_str().unwrap_or("") {
                    "structured" => json!({ "structuredContent": { "score": 7 } }),
                    "media" => json!({ "content": [
                        { "type": "resource_link", "uri": "https://x/y" },
                        { "type": "image", "mimeType": "image/png", "data": "AAAA" },
                    ]}),
                    _ => json!({}),
                },
                _ => json!({}),
            };
            vec![json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()]
        });

        client.initialize().await.unwrap();
        let out = client.call_tool("structured", json!({})).await.unwrap();
        assert_eq!(
            out, "{\"score\":7}",
            "仅结构化结果应优先取 structuredContent"
        );
        let out = client.call_tool("media", json!({})).await.unwrap();
        assert_eq!(
            out,
            "[resource_link: https://x/y]\n[image: image/png(base64 已省略)]"
        );

        drop(client);
        let _ = server_task.await;
    }
}
