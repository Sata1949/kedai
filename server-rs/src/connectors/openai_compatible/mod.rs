// OpenAI 兼容连接器(与 Node 版 connectors/openai-compatible.ts 对齐)
// 重试助手在 retry.rs;SSE 解析在 sse_parser.rs;单测在 tests.rs。
mod retry;
mod sse_parser;
#[cfg(test)]
mod tests;

use crate::models::llm_error::{LlmError, TransportFailure};
use crate::models::types::{GenerationParams, LlmMessage, LlmStreamChunk};
use futures::StreamExt;
use reqwest::Client;
use retry::{log_retry, retry_delay, wait_retry, MAX_ATTEMPTS};
use serde_json::{json, Value};
use sse_parser::SseParser;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

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
    client: Client,
}

impl OpenAiCompatibleConnector {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        OpenAiCompatibleConnector {
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            client: make_client(),
        }
    }

    /// 当前模型名
    pub fn model(&self) -> &str {
        &self.model
    }

    /// 切换模型(保留 base_url / api_key)
    pub fn with_model(&self, model: &str) -> Self {
        OpenAiCompatibleConnector {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: model.to_string(),
            client: make_client(),
        }
    }

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        if !self.api_key.is_empty() {
            if let Ok(v) =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.api_key))
            {
                h.insert(reqwest::header::AUTHORIZATION, v);
            }
        }
        h
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
        let url = format!("{}/chat/completions", self.base_url);
        let tool_names: Vec<Value> = params
            .tools
            .iter()
            .map(|tool| Value::String(tool.name.clone()))
            .collect();
        tracing::info!(mode = "stream", model = self.model.clone(), provider_host = provider_host(&self.base_url), step = "generate", tool_names = %crate::utils::logging::JsonField(serde_json::Value::Array(tool_names)), tool_count = params.tools.len(), tool_choice = format!("{:?}", params.tool_choice), "provider_round_start");
        let mut body = json!({
            "model": self.model,
            "messages": to_openai_messages(messages),
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
                wait_retry(retry_delay(attempt, retry_after), &mut abort).await?;
                continue;
            }
            return Err(LlmError::from_http_status(
                status.as_u16(),
                format!("OpenAI 兼容接口返回 {status}: {truncated}"),
            ));
        };

        // 流读取收口到独立函数(便于用合成流 + 毫秒级空闲阈值单测,不必真等 120s)
        read_sse_stream(resp.bytes_stream(), abort, tx, STREAM_IDLE_TIMEOUT).await
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
pub(super) async fn read_sse_stream<S, B, E>(
    stream: S,
    mut abort: watch::Receiver<bool>,
    tx: mpsc::UnboundedSender<LlmStreamChunk>,
    idle_timeout: Duration,
) -> Result<(), LlmError>
where
    S: futures::Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    // 就地 pin:不为调用方附加 Unpin 约束(组合流如 then/chain 常非 Unpin)
    tokio::pin!(stream);
    let mut parser = SseParser::default();
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

/// LlmMessage → OpenAI 兼容 messages 数组(处理 assistant.tool_calls 与 role=tool)
fn to_openai_messages(messages: &[LlmMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            let mut obj = serde_json::Map::new();
            obj.insert("role".into(), json!(m.role));
            if m.role == "tool" {
                obj.insert(
                    "tool_call_id".into(),
                    json!(m.tool_call_id.clone().unwrap_or_default()),
                );
                obj.insert("content".into(), json!(m.content));
            } else if let Some(calls) = &m.tool_calls {
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
                obj.insert("content".into(), json!(m.content));
            }
            Value::Object(obj)
        })
        .collect()
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
