// OpenAI 兼容连接器(与 Node 版 connectors/openai-compatible.ts 对齐)
// 重试助手在 retry.rs;SSE 解析在 sse_parser.rs;单测在 tests.rs。
// 三方言(chat-completions / responses / anthropic)共用本连接器的重试、看门狗与
// 错误分类边界;差异面只有:端点路径、请求体映射、鉴权头、流式事件解析。
mod anthropic_parser;
mod responses_parser;
mod retry;
mod sse_parser;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_dialects;
mod tool_accum;

use crate::models::llm_error::{LlmError, TransportFailure};
use crate::models::types::{GenerationParams, LlmMessage, LlmStreamChunk};
use anthropic_parser::AnthropicParser;
use futures::StreamExt;
use reqwest::Client;
use responses_parser::ResponsesParser;
use retry::{log_retry, notify_retry, retry_delay, wait_retry, MAX_ATTEMPTS};
use serde_json::{json, Value};
use sse_parser::SseParser;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// 接口方言取值(settings 层 `ConnectionProfile.api_style` 的线格式,serde 字符串):
/// - `chat-completions`:OpenAI Chat Completions(既有默认,`{base}/chat/completions`);
/// - `responses`:OpenAI Responses(`{base}/responses`);
/// - `anthropic`:Anthropic Messages(`{base}/v1/messages`,base 已以 /v1 结尾时用 `{base}/messages`)。
pub const API_STYLE_CHAT: &str = "chat-completions";
pub const API_STYLE_RESPONSES: &str = "responses";
pub const API_STYLE_ANTHROPIC: &str = "anthropic";

/// 方言枚举(解析口径与 `normalize_connector_type` 同纪律:未知值回退默认档,不报错)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiStyle {
    ChatCompletions,
    Responses,
    Anthropic,
}

impl ApiStyle {
    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim() {
            API_STYLE_RESPONSES => ApiStyle::Responses,
            API_STYLE_ANTHROPIC => ApiStyle::Anthropic,
            _ => ApiStyle::ChatCompletions,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ApiStyle::ChatCompletions => API_STYLE_CHAT,
            ApiStyle::Responses => API_STYLE_RESPONSES,
            ApiStyle::Anthropic => API_STYLE_ANTHROPIC,
        }
    }
}

/// settings 字段规范化(空/未知回退 chat-completions;与 TS 侧取值域一一对应)
pub fn normalize_api_style(s: &str) -> &'static str {
    ApiStyle::from_str_lossy(s).as_str()
}

/// 连接/响应头阶段超时(SSE 流开始前;流开始后读取不受此限制)
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// 建连超时
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// SSE 流空闲超时:流中超过此时长未**产出有效数据事件**则报错中止。
/// 判定口径是「解析出合法 JSON 的 data: 或 [DONE]」,不是「收到任何字节」——
/// 上游建流后只发 SSE 注释心跳(`: ping`)时,字节在来、正文不出,按字节判活会让
/// 读取循环永久挂起(任务卡在 running、聊天卡在生成中)。2026-09-18 修正;
/// 取较大值以容忍推理模型的慢速出段。
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// 构造带超时的 HTTP 客户端(连接阶段超时;整体请求超时由调用方按流式语义控制)
fn make_client() -> Client {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .unwrap_or_else(|_| Client::new())
}

/// reqwest 错误 → 传输失败形态(L1 不感知 reqwest 类型,折算在边界处完成)。
/// 判定顺序:超时 → 建连 → 响应体 → 请求构造;均不成立按其他传输失败处理。
fn transport_failure_of(e: &reqwest::Error) -> TransportFailure {
    if e.is_timeout() {
        TransportFailure::Timeout
    } else if e.is_connect() {
        TransportFailure::Connect
    } else if e.is_body() {
        TransportFailure::Body
    } else if e.is_request() {
        TransportFailure::Request
    } else {
        TransportFailure::Other
    }
}

/// 提取 provider host(仅用于日志,不发起请求)
fn provider_host(base_url: &str) -> String {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "invalid-host".into())
}

#[derive(Clone)]
pub struct OpenAiCompatibleConnector {
    base_url: String,
    api_key: String,
    model: String,
    style: ApiStyle,
    /// 连接能力位(视觉能力包 D1):序列化层据此决定图像 parts 是否发送 / 大图是否拆分
    capabilities: crate::connectors::ConnectorCapabilities,
    client: Client,
}

impl OpenAiCompatibleConnector {
    /// 默认 Chat Completions 方言(既有构造口径;新增方言走 `new_with_style`)
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        Self::new_with_style(base_url, api_key, model, ApiStyle::ChatCompletions)
    }

    pub fn new_with_style(base_url: &str, api_key: &str, model: &str, style: ApiStyle) -> Self {
        OpenAiCompatibleConnector {
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            style,
            capabilities: crate::connectors::ConnectorCapabilities::default(),
            client: make_client(),
        }
    }

    /// 附加连接能力位(视觉能力包 D1;`build_connector` 装配路径使用)
    pub fn with_capabilities(
        mut self,
        capabilities: crate::connectors::ConnectorCapabilities,
    ) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// 当前能力位(序列化层与测试读取)
    pub fn capabilities(&self) -> crate::connectors::ConnectorCapabilities {
        self.capabilities
    }

    /// 当前模型名
    pub fn model(&self) -> &str {
        &self.model
    }

    /// 当前接口方言
    pub fn style(&self) -> ApiStyle {
        self.style
    }

    /// 切换模型(保留 base_url / api_key / 方言 / 能力位)
    pub fn with_model(&self, model: &str) -> Self {
        OpenAiCompatibleConnector {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: model.to_string(),
            style: self.style,
            capabilities: self.capabilities,
            client: make_client(),
        }
    }

    /// 鉴权头。Chat / Responses 用 Bearer;Anthropic 官方用 `x-api-key` + `anthropic-version`,
    /// 兼容网关(百炼 claude-code-proxy 等)两种都收——故三者一齐携带,任一网关放行即可。
    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        if self.api_key.is_empty() {
            return h;
        }
        if self.style == ApiStyle::Anthropic {
            if let Ok(v) = reqwest::header::HeaderValue::from_str(&self.api_key) {
                h.insert("x-api-key", v);
            }
            h.insert(
                "anthropic-version",
                reqwest::header::HeaderValue::from_static("2023-06-01"),
            );
        }
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.api_key)) {
            h.insert(reqwest::header::AUTHORIZATION, v);
        }
        h
    }

    /// 各方言的生成端点路径(base 由 settings 层规范化,保证无路径时已补 /v1):
    /// - chat:`{base}/chat/completions`(既有口径);
    /// - responses:`{base}/responses`;
    /// - anthropic:`{base}/v1/messages`;base 已以 `/v1` 结尾时用 `{base}/messages`
    ///   (官方 Anthropic SDK 的 base 约定是不含 /v1,而用户手填的官方地址常是 …/v1)。
    fn generate_url(&self) -> String {
        match self.style {
            ApiStyle::ChatCompletions => format!("{}/chat/completions", self.base_url),
            ApiStyle::Responses => format!("{}/responses", self.base_url),
            ApiStyle::Anthropic => {
                if self.base_url.ends_with("/v1") {
                    format!("{}/messages", self.base_url)
                } else {
                    format!("{}/v1/messages", self.base_url)
                }
            }
        }
    }

    /// 拉取模型列表;失败或格式不符时回退当前模型(不致命)。
    /// 对缺 /v1 的旧配置做一次补 /v1 的兜底请求,避免「{base}/models → 404」。
    pub async fn list_models_async(&self) -> Vec<String> {
        let mut attempts = vec![format!("{}/models", self.base_url)];
        if self.base_url.ends_with("/v1") {
            attempts.push(format!("{}/models", self.base_url.trim_end_matches("/v1")));
        } else {
            attempts.push(format!("{}/v1/models", self.base_url));
        }
        for url in attempts {
            let resp = match self
                .client
                .get(&url)
                .headers(self.auth_headers())
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => r,
                _ => continue,
            };
            match resp.json::<Value>().await {
                Ok(v) => {
                    let models: Vec<String> = v
                        .get("data")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| {
                                    m.get("id")
                                        .and_then(|id| id.as_str())
                                        .map(|s| s.to_string())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if !models.is_empty() {
                        return models;
                    }
                }
                Err(_) => continue,
            }
        }
        vec![self.model.clone()]
    }

    /// 真实探测 `/models`,区分端点不可达、认证失败与模型列表回退。
    pub async fn test_connection(&self) -> Value {
        let url = format!("{}/models", self.base_url);
        let response = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await;
        let Ok(response) = response else {
            return json!({
                "ok": false, "message": "API 端点不可达", "endpoint_reachable": false,
                "authenticated": false, "fallback_used": false, "http_status": null, "models": []
            });
        };
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403) {
            return json!({
                "ok": false, "message": "API 端点可达,但认证失败", "endpoint_reachable": true,
                "authenticated": false, "fallback_used": false, "http_status": status.as_u16(), "models": []
            });
        }
        if !status.is_success() {
            return json!({
                "ok": false, "message": format!("API 端点可达,返回 HTTP {status}"), "endpoint_reachable": true,
                "authenticated": true, "fallback_used": false, "http_status": status.as_u16(), "models": []
            });
        }
        let body = response.json::<Value>().await.ok();
        let models: Vec<String> = body
            .as_ref()
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let fallback_used = models.is_empty();
        let models = if fallback_used {
            vec![self.model.clone()]
        } else {
            models
        };
        json!({
            "ok": true,
            "message": if fallback_used { "连接成功,但模型列表无效,已回退当前模型".to_string() } else { format!("连接成功,可用模型 {} 个", models.len()) },
            "endpoint_reachable": true, "authenticated": true, "fallback_used": fallback_used,
            "http_status": status.as_u16(), "models": models
        })
    }

    /// 流式生成:POST {baseUrl}/chat/completions,SSE 逐行解析
    /// 支持 function calling:params.tools 非空时下发 tools;delta.tool_calls 按 index 聚合
    pub async fn generate(
        &self,
        messages: &[LlmMessage],
        params: GenerationParams,
        abort: watch::Receiver<bool>,
    ) -> Result<Vec<LlmStreamChunk>, LlmError> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.generate_stream(messages, params, abort, tx).await?;
        let mut chunks = Vec::new();
        while let Some(chunk) = rx.recv().await {
            chunks.push(chunk);
        }
        Ok(chunks)
    }

    /// 流式生成:网络块到达后立即增量解析并发送,等待读取期间也监听中断。
    /// 自动重试:对 408/429/5xx 与连接错误做指数退避(尊重 Retry-After),仅在
    /// 尚未收到任何可见 token/工具调用前重试(避免重复输出);最大 MAX_ATTEMPTS 次。
    /// 错误带分类(超时/限流/鉴权/上游/生成),供上层直接映射错误码而无需解析文案。
    pub async fn generate_stream(
        &self,
        messages: &[LlmMessage],
        params: GenerationParams,
        mut abort: watch::Receiver<bool>,
        tx: mpsc::UnboundedSender<LlmStreamChunk>,
    ) -> Result<(), LlmError> {
        let url = self.generate_url();
        let tool_names: Vec<Value> = params
            .tools
            .iter()
            .map(|tool| Value::String(tool.name.clone()))
            .collect();
        tracing::info!(mode = "stream", model = self.model.clone(), style = self.style.as_str(), provider_host = provider_host(&self.base_url), step = "generate", tool_names = %crate::utils::logging::JsonField(serde_json::Value::Array(tool_names)), tool_count = params.tools.len(), tool_choice = format!("{:?}", params.tool_choice), "provider_round_start");
        let body = self.build_request_body(messages, &params);

        // 请求 + 读响应头阶段:可重试。一旦进入 SSE 读取阶段(收到首个字节)即不再重试。
        let mut attempt = 0usize;
        let resp = loop {
            attempt += 1;
            if *abort.borrow() {
                return Err(LlmError::generation("生成已中断"));
            }
            let req = self
                .client
                .post(&url)
                .headers(self.auth_headers())
                .json(&body);
            // 发送 + 等待响应头;连接/整体超时由 timeout 兜底(流开始后不再受此限制)
            let sent = tokio::time::timeout(REQUEST_TIMEOUT, req.send()).await;
            let resp = match sent {
                Err(_) => {
                    // 超时无 reqwest 错误可折算,直接判为超时分类(无字符串猜测路径)
                    let e = LlmError::timeout(format!(
                        "请求 OpenAI 兼容接口超时({}s)",
                        REQUEST_TIMEOUT.as_secs()
                    ));
                    if attempt < MAX_ATTEMPTS {
                        log_retry(&e, attempt);
                        notify_retry(&tx, attempt, &e);
                        wait_retry(retry_delay(attempt, None), &mut abort).await?;
                        continue;
                    }
                    return Err(e);
                }
                Ok(Err(e)) => {
                    // reqwest 错误在边界处折算为传输形态,分类由 L1 纯映射决定
                    let e = LlmError::from_transport(
                        transport_failure_of(&e),
                        format!("请求 OpenAI 兼容接口失败: {e}"),
                    );
                    if attempt < MAX_ATTEMPTS {
                        log_retry(&e, attempt);
                        notify_retry(&tx, attempt, &e);
                        wait_retry(retry_delay(attempt, None), &mut abort).await?;
                        continue;
                    }
                    return Err(e);
                }
                Ok(Ok(r)) => r,
            };
            let status = resp.status();
            if status.is_success() {
                break resp;
            }
            // 可重试状态:408/429/5xx(尊重 Retry-After);其余直接失败
            let retryable = matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504);
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            let text = resp.text().await.unwrap_or_default();
            let truncated: String = text.chars().take(300).collect();
            if retryable && attempt < MAX_ATTEMPTS {
                log_retry(&format!("上游返回 {status}"), attempt);
                notify_retry(&tx, attempt, &format!("上游返回 {status}"));
                wait_retry(retry_delay(attempt, retry_after), &mut abort).await?;
                continue;
            }
            return Err(LlmError::from_http_status(
                status.as_u16(),
                format!("OpenAI 兼容接口返回 {status}: {truncated}"),
            ));
        };

        // 流读取收口到独立函数(便于用合成流 + 毫秒级空闲阈值单测,不必真等 120s)
        let parser = match self.style {
            ApiStyle::ChatCompletions => StreamParser::Chat(SseParser::default()),
            ApiStyle::Responses => StreamParser::Responses(ResponsesParser::default()),
            ApiStyle::Anthropic => StreamParser::Anthropic(AnthropicParser::default()),
        };
        read_sse_stream(resp.bytes_stream(), abort, tx, STREAM_IDLE_TIMEOUT, parser).await
    }

    /// 按方言构建请求体(chat / responses / anthropic 的映射差异全部收口在此)。
    /// 能力位(视觉能力包 D2)随构筑下传:仅 `supports_vision` 打开且有可用图像时
    /// 才把 content 变成 parts 数组;**无图路径的请求体逐字节不变**。
    fn build_request_body(&self, messages: &[LlmMessage], params: &GenerationParams) -> Value {
        match self.style {
            ApiStyle::ChatCompletions => {
                build_chat_body(&self.model, messages, params, self.capabilities)
            }
            ApiStyle::Responses => {
                build_responses_body(&self.model, messages, params, self.capabilities)
            }
            ApiStyle::Anthropic => {
                build_anthropic_body(&self.model, messages, params, self.capabilities)
            }
        }
    }
}

/// 本消息可下发的图像:能力位打开 + data_url 已解析(空 data_url 是解析失败的兜底剔除)。
fn usable_images(
    m: &LlmMessage,
    caps: crate::connectors::ConnectorCapabilities,
) -> Vec<&crate::models::types::ImageRef> {
    if !caps.supports_vision {
        return Vec::new();
    }
    m.images.iter().filter(|i| !i.data_url.is_empty()).collect()
}

/// 拆 `data:<mime>;base64,<payload>`(仅 anthropic 需要裸 base64 段;失败返回 None)
fn split_data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mime = meta.strip_suffix(";base64")?;
    Some((mime, payload))
}

/// chat 方言的 content 落位:无图 = 纯字符串(与既有行为逐字节一致);
/// 有图 = parts 数组(text 仅非空时出现 + image_url data URL)。
fn insert_chat_content(
    obj: &mut serde_json::Map<String, Value>,
    content: &str,
    images: &[&crate::models::types::ImageRef],
) {
    if images.is_empty() {
        obj.insert("content".into(), json!(content));
        return;
    }
    let mut parts: Vec<Value> = Vec::new();
    if !content.is_empty() {
        parts.push(json!({ "type": "text", "text": content }));
    }
    for img in images {
        // 拆分产物带标注(大图总览/行-列):以 text part 紧贴图像之前输出(D3)
        if let Some(label) = &img.label {
            parts.push(json!({ "type": "text", "text": label }));
        }
        parts.push(json!({ "type": "image_url", "image_url": { "url": img.data_url } }));
    }
    obj.insert("content".into(), Value::Array(parts));
}

/// 工具图像 → chat content parts(修复批次):label 紧贴图像之前;词表为 chat 的
/// `text` / `image_url`。仅在能力位打开且有 data_url 时调用(调用方已过滤)。
fn push_tool_image_parts(parts: &mut Vec<Value>, images: &[&crate::models::types::ImageRef]) {
    for img in images {
        if let Some(label) = &img.label {
            parts.push(json!({ "type": "text", "text": label }));
        }
        parts.push(json!({ "type": "image_url", "image_url": { "url": img.data_url } }));
    }
}

/// 工具组结束:如本轮工具返回了图像,追加一条 user 消息承载(chat 契约的 tool
/// 消息 content 只接受字符串/文本 part,图像必须落在 user 消息——修复批次)。
fn flush_tool_images(out: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    let mut parts = vec![json!({ "type": "text", "text": "（本轮工具调用返回的图像）" })];
    parts.append(pending);
    out.push(json!({ "role": "user", "content": Value::Array(parts) }));
}

/// 工具图像 → responses input 项(词表为 `input_text` / `input_image`)
fn push_tool_image_items(parts: &mut Vec<Value>, images: &[&crate::models::types::ImageRef]) {
    for img in images {
        if let Some(label) = &img.label {
            parts.push(json!({ "type": "input_text", "text": label }));
        }
        parts.push(json!({ "type": "input_image", "image_url": img.data_url }));
    }
}

/// 工具组结束:responses 侧同款兜底——`function_call_output.output` 只承载文本,
/// 图像累积后追加一条 user 输入项(与 chat 同策略,避免依赖各端对输出内图像的支持)
fn flush_tool_image_items(input: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    let mut parts = vec![json!({ "type": "input_text", "text": "（本轮工具调用返回的图像）" })];
    parts.append(pending);
    input.push(json!({ "role": "user", "content": Value::Array(parts) }));
}

/// chat-completions 请求体(自 generate_stream 原样提取,行为逐字节不变)
fn build_chat_body(
    model: &str,
    messages: &[LlmMessage],
    params: &GenerationParams,
    caps: crate::connectors::ConnectorCapabilities,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": to_openai_messages(messages, caps),
        "stream": true,
        "temperature": params.temperature,
        "top_p": params.top_p,
        "max_tokens": params.max_tokens,
        // 让上游在流末尾下发 usage(OpenAI 需要;DeepSeek 默认下发,重复声明无副作用)。
        // 无此字段时部分提供商流式不返回 usage,任务模式诊断/落库将拿不到 token 数
        "stream_options": { "include_usage": true },
    });
    if let Some(stop) = &params.stop {
        if !stop.is_empty() {
            body["stop"] = json!(stop);
        }
    }
    if !params.tools.is_empty() {
        // OpenAI function calling 标准格式:{"type":"function","function":{name,description,parameters}}
        // 部分兼容后端严格要求 type 字段,缺省会 400
        body["tools"] = Value::Array(
            params
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect(),
        );
        // tool_choice:按 GenerationParams 策略序列化(默认 auto,保持原行为);
        // 支持 none(禁止调用)/ required(强制调用至少一个)/ function(name)(指定工具)
        body["tool_choice"] = match &params.tool_choice {
            crate::models::types::ToolChoice::Auto => Value::from("auto"),
            crate::models::types::ToolChoice::None => Value::from("none"),
            crate::models::types::ToolChoice::Required => Value::from("required"),
            crate::models::types::ToolChoice::Function(name) => json!({
                "type": "function",
                "function": { "name": name }
            }),
        };
        // 并行工具调用开关:仅在显式指定时下发(缺省让后端自行决定)
        if let Some(parallel) = params.parallel_tool_calls {
            body["parallel_tool_calls"] = Value::from(parallel);
        }
    }
    body
}

/// responses 请求体映射:
/// - system 消息 → 顶层 `instructions`(多条以空行拼接;Responses 无 system 角色输入项);
/// - user/assistant 文本 → `{role, content}` 输入项;assistant 的工具调用 → `function_call`
///   输入项(带 call_id);tool 结果 → `function_call_output`(带 call_id);
/// - tools 为扁平格式 `{type:"function", name, description, parameters}`(无嵌套 function);
/// - `max_tokens` → `max_output_tokens`;
/// - **不下发 stop**:Responses 契约无停用序列参数。
fn build_responses_body(
    model: &str,
    messages: &[LlmMessage],
    params: &GenerationParams,
    caps: crate::connectors::ConnectorCapabilities,
) -> Value {
    let mut instructions: Vec<String> = Vec::new();
    let mut input: Vec<Value> = Vec::new();
    let mut pending_tool_images: Vec<Value> = Vec::new();
    for m in messages {
        if m.role == "tool" {
            // 工具输出恒为纯文本(修复批次):图像由 `flush_tool_image_items` 在工具组
            // 末尾以 user 输入项承载——不塞 `function_call_output.output`,避免依赖
            // 各端对「函数输出内图像项」的支持差异(不支持即静默丢图)。
            input.push(json!({
                "type": "function_call_output",
                "call_id": m.tool_call_id.clone().unwrap_or_default(),
                "output": m.content,
            }));
            push_tool_image_items(&mut pending_tool_images, &usable_images(m, caps));
            continue;
        }
        flush_tool_image_items(&mut input, &mut pending_tool_images);
        match m.role.as_str() {
            "system" => {
                if !m.content.is_empty() {
                    instructions.push(m.content.clone());
                }
            }
            "assistant" => {
                if !m.content.is_empty() {
                    input.push(json!({ "role": "assistant", "content": m.content }));
                }
                if let Some(calls) = &m.tool_calls {
                    for c in calls {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": c.id,
                            "name": c.name,
                            "arguments": c.arguments,
                        }));
                    }
                }
            }
            _ => {
                // user 消息:有图 → content parts(input_text + input_image data URL);
                // 无图 → 纯字符串(与既有行为逐字节一致)
                let images = usable_images(m, caps);
                if images.is_empty() {
                    input.push(json!({ "role": "user", "content": m.content }));
                } else {
                    let mut parts: Vec<Value> = Vec::new();
                    if !m.content.is_empty() {
                        parts.push(json!({ "type": "input_text", "text": m.content }));
                    }
                    for img in images {
                        // 拆分产物带标注(大图总览/行-列):text 项紧贴图像之前(D3)
                        if let Some(label) = &img.label {
                            parts.push(json!({ "type": "input_text", "text": label }));
                        }
                        parts.push(json!({ "type": "input_image", "image_url": img.data_url }));
                    }
                    input.push(json!({ "role": "user", "content": parts }));
                }
            }
        }
    }
    flush_tool_image_items(&mut input, &mut pending_tool_images);
    let mut body = json!({
        "model": model,
        "input": input,
        "stream": true,
        "temperature": params.temperature,
        "top_p": params.top_p,
        "max_output_tokens": params.max_tokens,
    });
    if !instructions.is_empty() {
        body["instructions"] = Value::String(instructions.join("\n\n"));
    }
    if !params.tools.is_empty() {
        body["tools"] = Value::Array(
            params
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect(),
        );
        body["tool_choice"] = match &params.tool_choice {
            crate::models::types::ToolChoice::Auto => Value::from("auto"),
            crate::models::types::ToolChoice::None => Value::from("none"),
            crate::models::types::ToolChoice::Required => Value::from("required"),
            crate::models::types::ToolChoice::Function(name) => json!({
                "type": "function",
                "name": name
            }),
        };
        if let Some(parallel) = params.parallel_tool_calls {
            body["parallel_tool_calls"] = Value::from(parallel);
        }
    }
    body
}

/// anthropic 请求体映射:
/// - system 消息 → 顶层 `system`(字符串,多条空行拼接;Anthropic 无 system 角色消息);
/// - tool 结果 → user 消息的 `tool_result` 块,**连续多条合并进同一条 user 消息**
///   (Anthropic 要求 user/assistant 交替,多个结果必须是同一消息的多个块);
/// - assistant 工具调用 → `tool_use` 块(input 为解析后的 JSON 对象);
/// - 连续的同角色纯文本消息合并(避免角色不交替被 400);
/// - `max_tokens` 必填;temperature 钳制到官方区间 0..=1(超界时向合法值收敛,不静默丢弃);
/// - stop → `stop_sequences`;tools 为 `{name, description, input_schema}`;
/// - tool_choice:none 档 Anthropic 无对应表达 → **不下发 tools**(模型无从调用,语义等价);
/// - reasoning_content 不回传(Anthropic thinking 块需签名,原样拼接必被 400)。
fn build_anthropic_body(
    model: &str,
    messages: &[LlmMessage],
    params: &GenerationParams,
    caps: crate::connectors::ConnectorCapabilities,
) -> Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    /// data URL → Anthropic image 块(base64 段不带前缀);形状不符返回 None(该图跳过)
    fn image_block(img: &crate::models::types::ImageRef) -> Option<Value> {
        let (media_type, data) = split_data_url(&img.data_url)?;
        Some(json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data }
        }))
    }

    fn flush_tool_results(out: &mut Vec<Value>, pending: &mut Vec<Value>) {
        if !pending.is_empty() {
            out.push(json!({ "role": "user", "content": Value::Array(std::mem::take(pending)) }));
        }
    }

    for m in messages {
        if m.role == "tool" {
            // 工具图像(修复批次):放进 tool_result 的 content 块数组——Anthropic
            // 官方允许 tool_result 内嵌 image 块;此前只取文本会静默丢图。
            let images = usable_images(m, caps);
            let content = if images.is_empty() {
                json!(m.content)
            } else {
                let mut blocks: Vec<Value> = Vec::new();
                if !m.content.is_empty() {
                    blocks.push(json!({ "type": "text", "text": m.content }));
                }
                for img in &images {
                    if let Some(label) = &img.label {
                        blocks.push(json!({ "type": "text", "text": label }));
                    }
                    if let Some(block) = image_block(img) {
                        blocks.push(block);
                    }
                }
                if blocks.is_empty() {
                    json!(m.content)
                } else {
                    Value::Array(blocks)
                }
            };
            pending_tool_results.push(json!({
                "type": "tool_result",
                "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                "content": content,
            }));
            continue;
        }
        flush_tool_results(&mut out, &mut pending_tool_results);
        match m.role.as_str() {
            "system" => {
                if !m.content.is_empty() {
                    system_parts.push(m.content.clone());
                }
            }
            "assistant" => {
                let mut blocks: Vec<Value> = Vec::new();
                if !m.content.is_empty() {
                    blocks.push(json!({ "type": "text", "text": m.content }));
                }
                if let Some(calls) = &m.tool_calls {
                    for c in calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": c.id,
                            "name": c.name,
                            "input": serde_json::from_str::<Value>(&c.arguments)
                                .unwrap_or_else(|_| json!({})),
                        }));
                    }
                }
                if !blocks.is_empty() {
                    out.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
            _ => {
                // user 消息:有图 → 块数组(text + image 块);无图 → 既有字符串/合并路径
                // (连续同角色消息合并(Anthropic 要求 user/assistant 交替,连发两条
                // user 会被 400):纯文本并入文本;tool_result 块消息后跟文本时,
                // 文本并作同一 user 消息的 text 块(块数组本就允许多块混排))
                let images = usable_images(m, caps);
                let mut image_blocks: Vec<Value> = Vec::new();
                for img in &images {
                    // 拆分产物带标注(大图总览/行-列):text 块紧贴 image 块之前(D3)
                    if let Some(label) = &img.label {
                        image_blocks.push(json!({ "type": "text", "text": label }));
                    }
                    if let Some(block) = image_block(img) {
                        image_blocks.push(block);
                    }
                }
                if image_blocks.is_empty() {
                    if let Some(last) = out.last_mut() {
                        if last.get("role").and_then(Value::as_str) == Some("user") {
                            match last.get_mut("content") {
                                Some(Value::String(s)) => {
                                    *s = format!("{s}\n\n{}", m.content);
                                    continue;
                                }
                                Some(Value::Array(blocks)) => {
                                    blocks.push(json!({ "type": "text", "text": m.content }));
                                    continue;
                                }
                                _ => {}
                            }
                        }
                    }
                    out.push(json!({ "role": "user", "content": m.content }));
                } else {
                    let mut blocks: Vec<Value> = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(json!({ "type": "text", "text": m.content }));
                    }
                    blocks.append(&mut image_blocks);
                    // 合并规则与纯文本路径一致:上一条也是 user 时并入其内容
                    // (修复批次:字符串 content 先升级为块数组再并入——此前只并块数组,
                    // 上一条 user 是纯文本时会产生连续 user 消息,Anthropic 直接 400)
                    if let Some(last) = out.last_mut() {
                        if last.get("role").and_then(Value::as_str) == Some("user") {
                            if let Some(slot) = last.get_mut("content") {
                                match slot {
                                    Value::Array(prev) => {
                                        prev.append(&mut blocks);
                                        continue;
                                    }
                                    Value::String(prev_text) => {
                                        let mut merged = vec![json!({
                                            "type": "text",
                                            "text": prev_text.as_str()
                                        })];
                                        merged.append(&mut blocks);
                                        *slot = Value::Array(merged);
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    out.push(json!({ "role": "user", "content": blocks }));
                }
            }
        }
    }
    flush_tool_results(&mut out, &mut pending_tool_results);

    let mut body = json!({
        "model": model,
        "messages": out,
        "stream": true,
        "max_tokens": params.max_tokens.max(1),
        "temperature": params.temperature.clamp(0.0, 1.0),
        "top_p": params.top_p,
    });
    if !system_parts.is_empty() {
        body["system"] = Value::String(system_parts.join("\n\n"));
    }
    if let Some(stop) = &params.stop {
        if !stop.is_empty() {
            body["stop_sequences"] = json!(stop);
        }
    }
    if !params.tools.is_empty() && params.tool_choice != crate::models::types::ToolChoice::None {
        body["tools"] = Value::Array(
            params
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect(),
        );
        body["tool_choice"] = match &params.tool_choice {
            crate::models::types::ToolChoice::Auto => json!({ "type": "auto" }),
            crate::models::types::ToolChoice::None => json!({ "type": "auto" }),
            crate::models::types::ToolChoice::Required => json!({ "type": "any" }),
            crate::models::types::ToolChoice::Function(name) => {
                json!({ "type": "tool", "name": name })
            }
        };
    }
    body
}

/// 三方言解析器统一门面(读取循环与空闲看门狗只依赖 push/finish/is_done 三件事)。
/// 与 `read_sse_stream` 同为模块内私有(crate 内无其它调用者),可见性对齐避免告警。
enum StreamParser {
    Chat(SseParser),
    Responses(ResponsesParser),
    Anthropic(AnthropicParser),
}

impl StreamParser {
    fn push(&mut self, bytes: &[u8], out: &mut Vec<LlmStreamChunk>) -> Result<usize, LlmError> {
        match self {
            StreamParser::Chat(p) => p.push(bytes, out),
            StreamParser::Responses(p) => p.push(bytes, out),
            StreamParser::Anthropic(p) => p.push(bytes, out),
        }
    }

    fn finish(&mut self, out: &mut Vec<LlmStreamChunk>) -> Result<(), LlmError> {
        match self {
            StreamParser::Chat(p) => p.finish(out),
            StreamParser::Responses(p) => p.finish(out),
            StreamParser::Anthropic(p) => p.finish(out),
        }
    }

    fn is_done(&self) -> bool {
        match self {
            StreamParser::Chat(p) => p.is_done(),
            StreamParser::Responses(p) => p.is_done(),
            StreamParser::Anthropic(p) => p.is_done(),
        }
    }
}

/// 读取 SSE 响应流直到 `[DONE]` / EOF / 出错,边解析边把块推给 `tx`。
///
/// 空闲看门狗口径(2026-09-18 修正,`docs/经验.md` E45):只有**解析出有效数据事件**
/// (合法 JSON 的 `data:`、或 `[DONE]`)才算上游有推进并按此重置计时。此前按「收到任何
/// 字节」重置——上游建流后只发 SSE 注释心跳(`: ping`)同样产生字节,看门狗永不触发,
/// 读取循环可无限挂起(task 状态永久停在 running)。注释行不是产出,不得为停滞续命。
///
/// `abort` 优先于读取(`biased`):用户停止时立即返回,不被上游的连续字节拖住。
/// 不用 `Sleep::reset`:经 Pin 包装 reset 在 select 循环中会立即完成(已实测踩坑),
/// 故改为每轮按「剩余额度」新建 sleep。
async fn read_sse_stream<S, B, E>(
    stream: S,
    mut abort: watch::Receiver<bool>,
    tx: mpsc::UnboundedSender<LlmStreamChunk>,
    idle_timeout: Duration,
    mut parser: StreamParser,
) -> Result<(), LlmError>
where
    S: futures::Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    // 就地 pin:不为调用方附加 Unpin 约束(组合流如 then/chain 常非 Unpin)
    tokio::pin!(stream);
    // 最近一次「有效数据事件」的时刻;空闲计时以它为基准
    let mut last_progress = std::time::Instant::now();
    loop {
        // 先在循环顶部判定期限,再进 select。
        //
        // 为什么不能只靠 select 里的 sleep 分支(2026-09-18 实现时踩到并修):
        // `biased` 让分支按书写顺序优先轮询,而心跳刷屏时 `stream.next()` 恒就绪——
        // 每轮循环都在 stream 分支返回、sleep 分支被新建后立刻丢弃,计时器永远走不到
        // 完成,看门狗失效(上游狂发注释即可让读取无限循环)。顶部判定不依赖分支被轮到,
        // 只要时间真的过去了就必然收敛。
        if last_progress.elapsed() >= idle_timeout {
            let waited_ms = last_progress.elapsed().as_millis() as u64;
            tracing::warn!(
                waited_ms = waited_ms,
                timeout_s = idle_timeout.as_secs(),
                "SSE 流空闲超时触发(阈值内未收到有效数据事件,注释心跳不计)"
            );
            return Err(LlmError::timeout(format!(
                "上游流停滞超时({}s 未产出任何数据),已中止本次生成",
                idle_timeout.as_secs()
            )));
        }
        let remaining = idle_timeout.saturating_sub(last_progress.elapsed());
        tokio::select! {
            biased;
            changed = abort.changed() => {
                if changed.is_err() || *abort.borrow() {
                    return Err(LlmError::generation("生成已中断"));
                }
            }
            next = stream.next() => match next {
                Some(Ok(bytes)) => {
                    let mut chunks = Vec::new();
                    // 解析失败自带分类(非法 UTF-8/JSON、坏工具参数 → 生成层失败;
                    // 流内上游错误对象按结构化字段分类)
                    let progressed = parser.push(bytes.as_ref(), &mut chunks)? > 0;
                    if progressed {
                        last_progress = std::time::Instant::now();
                    }
                    for chunk in chunks {
                        // 有意丢弃 SendError:接收端关闭即原因本身,文案已等价表达
                        tx.send(chunk)
                            .map_err(|_| LlmError::generation("生成接收端已关闭"))?;
                    }
                    if parser.is_done() {
                        break;
                    }
                    // 未产出数据时让出一次(2026-09-18 实测踩到):上游高频发注释心跳时
                    // 本分支恒就绪,循环将不再有 await 让点,独占 worker 线程并饿死同线程
                    // 的其它任务(单线程运行时下表现为中断信号都送不进来)。让出后由顶部
                    // 的时限判定收敛,不靠 sleep 分支被轮到。
                    if !progressed {
                        tokio::task::yield_now().await;
                    }
                }
                Some(Err(e)) => {
                    // 响应体读取中断(流中途断开)→ 可重试的上游故障
                    return Err(LlmError::from_transport(
                        TransportFailure::Body,
                        format!("读取响应失败: {e}"),
                    ));
                }
                None => break,
            },
            // 长时间无任何字节:此分支负责让等待可中断且不空转(到点即判停滞)
            _ = tokio::time::sleep(remaining) => {
                let waited_ms = last_progress.elapsed().as_millis() as u64;
                tracing::warn!(
                    waited_ms = waited_ms,
                    timeout_s = idle_timeout.as_secs(),
                    "SSE 流空闲超时触发(阈值内未收到有效数据事件,注释心跳不计)"
                );
                return Err(LlmError::timeout(format!(
                    "上游流停滞超时({}s 未产出任何数据),已中止本次生成",
                    idle_timeout.as_secs()
                )));
            }
        }
    }
    let mut chunks = Vec::new();
    parser.finish(&mut chunks)?;
    for chunk in chunks {
        // 有意丢弃 SendError:接收端关闭即原因本身,文案已等价表达
        tx.send(chunk)
            .map_err(|_| LlmError::generation("生成接收端已关闭"))?;
    }
    Ok(())
}

/// LlmMessage → OpenAI 兼容 messages 数组(处理 assistant.tool_calls 与 role=tool;
/// 图像仅在能力位打开且有 data_url 时以 parts 数组下发)。
///
/// 工具图像落位(2026-10-02 修复批次):chat 契约的 `tool` 消息 content 只接受
/// 字符串/文本 part,带 `image_url` part 是非法输入(严格后端整轮 400)。故 tool
/// 消息恒为纯文本,图像(含拆分标注)累积后在本轮工具组**末尾**追加一条 user
/// 消息承载——中间不得插入其它角色:契约要求 assistant(tool_calls) 之后紧跟全部
/// tool 结果。无工具图像的形状与改造前逐字节一致。
fn to_openai_messages(
    messages: &[LlmMessage],
    caps: crate::connectors::ConnectorCapabilities,
) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    let mut pending_tool_images: Vec<Value> = Vec::new();
    for m in messages {
        if m.role == "tool" {
            let mut obj = serde_json::Map::new();
            obj.insert("role".into(), json!("tool"));
            obj.insert(
                "tool_call_id".into(),
                json!(m.tool_call_id.clone().unwrap_or_default()),
            );
            obj.insert("content".into(), json!(m.content));
            out.push(Value::Object(obj));
            push_tool_image_parts(&mut pending_tool_images, &usable_images(m, caps));
            continue;
        }
        flush_tool_images(&mut out, &mut pending_tool_images);
        let mut obj = serde_json::Map::new();
        obj.insert("role".into(), json!(m.role));
        let images = usable_images(m, caps);
        if let Some(calls) = &m.tool_calls {
            obj.insert("content".into(), Value::Null);
            // DeepSeek 系后端要求思考模式多轮调用时回传 reasoning_content,否则 400
            if let Some(rc) = &m.reasoning_content {
                if !rc.is_empty() {
                    obj.insert("reasoning_content".into(), json!(rc));
                }
            }
            obj.insert(
                "tool_calls".into(),
                Value::Array(
                    calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": { "name": tc.name, "arguments": tc.arguments }
                            })
                        })
                        .collect(),
                ),
            );
        } else {
            insert_chat_content(&mut obj, &m.content, &images);
        }
        out.push(Value::Object(obj));
    }
    flush_tool_images(&mut out, &mut pending_tool_images);
    out
}

/// 连接器统一枚举(与 Node 版 connectors/index.ts 对齐)
pub enum Connector {
    Mock(super::mock::MockConnector),
    OpenAi(super::openai_compatible::OpenAiCompatibleConnector),
}

pub fn connector_type(config_connector: &str) -> String {
    if config_connector == "openai-compatible" {
        "openai-compatible".into()
    } else {
        "mock".into()
    }
}
