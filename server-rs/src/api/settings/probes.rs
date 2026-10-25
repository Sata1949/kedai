//! 连接探测、模型列表与模型切换端点。
use crate::api::app_state::AppState;
use crate::api::json_body::{JsonBody, JsonBodyOpt};
use crate::connectors::openai_compatible::{normalize_api_style, API_STYLE_CHAT};
use crate::services::settings_service::{
    normalize_base_url, CONNECTOR_TYPE_MOCK, CONNECTOR_TYPE_OPENAI,
};
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use super::ConnectionProbeBody;

#[derive(Deserialize)]
pub struct SwitchModelBody {
    pub model: String,
}

/// 解析逐连接探测的目标连接器(2026-10-03 API 设置补全):
/// 显式 `connection` 草稿参数(字段级覆盖)> `connection_id` 命中已存连接(缺省字段取
/// 已存值,密钥仅服务端持有)> 两者皆缺省 = 当前生效连接器快照(旧客户端行为不变)。
/// 探测用一次性临时连接器:不落库、不改动 `engine.connector`、不进连接器池缓存。
async fn resolve_probe_connector(
    state: &Arc<AppState>,
    body: Option<ConnectionProbeBody>,
) -> crate::connectors::Connector {
    let Some(body) = body else {
        return state.engine.connector.read().await.clone();
    };
    let saved = body
        .connection_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .and_then(|id| {
            state
                .settings_snapshot()
                .connections
                .into_iter()
                .find(|p| p.id == id)
        });
    if body.connection.is_none() && saved.is_none() {
        return state.engine.connector.read().await.clone();
    }
    let explicit = body.connection.unwrap_or_default();
    let trim_non_empty =
        |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let connector_type = trim_non_empty(explicit.connector_type)
        .or_else(|| saved.as_ref().map(|p| p.connector_type.clone()))
        .unwrap_or_else(|| CONNECTOR_TYPE_OPENAI.to_string());
    // 显式空串 = 按空值探测(draft 清空地址后探测应如实报不可达,而非拿旧地址测试);
    // normalize 与保存路径同口径,「host:1234」这类裸地址在探测时也能按补全后的形态访问
    let base_url = explicit
        .base_url
        .map(|v| normalize_base_url(&v))
        .or_else(|| saved.as_ref().map(|p| p.base_url.clone()))
        .unwrap_or_default();
    // api_key 空/缺省 = 回退已存密钥(与「空 Key 忽略」同口径:已存连接不要求重填 Key)
    let api_key = trim_non_empty(explicit.api_key)
        .or_else(|| saved.as_ref().map(|p| p.api_key.clone()))
        .unwrap_or_default();
    let api_style = trim_non_empty(explicit.api_style)
        .map(|s| normalize_api_style(&s).to_string())
        .or_else(|| saved.as_ref().map(|p| p.api_style.clone()))
        .unwrap_or_else(|| API_STYLE_CHAT.to_string());
    let model = explicit
        .model
        .or_else(|| saved.as_ref().map(|p| p.model.clone()))
        .unwrap_or_default();
    // 类型白名单:探测不走「未知类型回退 mock」的兜底(那会把手滑的类型测成假绿),
    // 未知/空一律按 openai-compatible
    let connector_type = if connector_type == CONNECTOR_TYPE_MOCK {
        CONNECTOR_TYPE_MOCK.to_string()
    } else {
        CONNECTOR_TYPE_OPENAI.to_string()
    };
    crate::connectors::build_connector(
        &connector_type,
        &base_url,
        &api_key,
        &model,
        &api_style,
        crate::connectors::ConnectorCapabilities::default(),
    )
}

/// POST /api/settings/refresh-models:向已保存的 API 请求可用模型列表(立即生效,不保存)。
/// 支持逐连接探测(可选 body,见 [`ConnectionProbeBody`]);缺省行为不变(测当前生效连接器)。
pub async fn refresh_models(
    State(state): State<Arc<AppState>>,
    JsonBodyOpt(body): JsonBodyOpt<ConnectionProbeBody>,
) -> Json<serde_json::Value> {
    let connector = resolve_probe_connector(&state, body).await;
    let models = connector.available_models().await;
    // openai-compatible 下若只拿到回退的 1 个当前模型,大概率是服务不支持 /models 接口
    // 或 Base URL / 接口格式与真实端点不匹配(后者会让 /models 也 404/400)
    let message = if connector.type_name() == "openai-compatible" && models.len() <= 1 {
        Some(format!(
            "API 未返回完整模型列表(该服务可能不支持 /models 接口,或 Base URL 与接口格式不匹配);已保留当前模型「{}」",
            models.first().cloned().unwrap_or_default()
        ))
    } else {
        None
    };
    Json(json!({ "ok": true, "models": models, "message": message }))
}

/// POST /api/settings/connect:测试连接(支持逐连接探测,可选 body 见 [`ConnectionProbeBody`])
pub async fn connect(
    State(state): State<Arc<AppState>>,
    JsonBodyOpt(body): JsonBodyOpt<ConnectionProbeBody>,
) -> Json<serde_json::Value> {
    let connector = resolve_probe_connector(&state, body).await;
    Json(connector.test().await)
}

/// GET /api/settings/models:可用模型列表
pub async fn models(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let models = state
        .engine
        .connector
        .read()
        .await
        .clone()
        .available_models()
        .await;
    Json(json!({ "models": models }))
}

/// POST /api/settings/embedding/test:测试向量化连接(嵌入一条固定文本)。
/// 成功回传实际维度与耗时;失败回传错误原因。不写库、不改配置。
///
/// 形状(批次 1):保留 200 + `ok:false`,`ok:false` 是**正常业务上报**(测试结果),
/// 不是 HTTP 错误——前端 settings.ts 的 testEmbedding 把该对象直接渲染到设置页,
/// 改成非 2xx 会让 request() 抛 ApiError,把「测试未通过」变成异常弹窗(破坏既有 UX)。
/// 这里补 `code`/`error` 供程序化分支;`message` 保留原文(用户需要据此修正自己的配置)。
pub async fn test_embedding(State(state): State<Arc<AppState>>) -> axum::response::Response {
    use crate::api::ErrorCode;
    use axum::response::IntoResponse;

    let settings = state.settings_snapshot();
    let svc = crate::services::embedding_service::EmbeddingService::new();
    match svc.test(&settings).await {
        Ok((dim, ms)) => Json(json!({
            "ok": true,
            "dim": dim,
            "latency_ms": ms,
            "message": format!("连接成功:向量维度 {dim},耗时 {ms} ms"),
        }))
        .into_response(),
        Err(e) => {
            // 未配置 → VALIDATION(用户可自行修正);已配置但调用失败 → UPSTREAM(上游)
            let code = match &e {
                crate::services::embedding_service::EmbedError::NotConfigured(_) => {
                    ErrorCode::Validation
                }
                crate::services::embedding_service::EmbedError::Failed(_) => ErrorCode::Upstream,
            };
            let message = e.to_string();
            Json(
                json!({ "ok": false, "code": code.as_str(), "error": message, "message": message }),
            )
            .into_response()
        }
    }
}

/// GET /api/settings/info:连接器信息 + 模型列表 + 可用连接器
pub async fn info(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let connector = state.engine.connector.read().await.clone();
    let type_name = connector.type_name();
    let models = connector.available_models().await;
    let model = connector.model().to_string();
    Json(json!({
        "connector": type_name,
        "model": model,
        "models": models,
        "availableConnectors": crate::connectors::available_connector_types(),
    }))
}

/// PUT /api/settings/model:切换模型(立即生效)
pub async fn switch_model(
    State(state): State<Arc<AppState>>,
    JsonBody(body): JsonBody<SwitchModelBody>,
) -> axum::response::Response {
    use crate::api::validation;
    use axum::response::IntoResponse;

    let model = body.model.trim().to_string();
    if model.is_empty() {
        return validation("缺少 model");
    }
    let changed = state.engine.model() != model;
    if changed {
        state.engine.switch_model(&model).await;
        *state.model.lock().unwrap_or_else(|e| e.into_inner()) = model.clone();
    }
    Json(json!({ "ok": true, "model": model, "changed": changed })).into_response()
}

/// GET /api/settings/model:当前模型
pub async fn get_model(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({ "model": state.engine.model() }))
}
