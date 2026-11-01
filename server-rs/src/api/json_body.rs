// JSON 请求体提取器统一收口(批次 1 · 错误面收口)。
//
// 背景(实测):axum 内建 `Json<T>` 的 `JsonRejection` 直接 `IntoResponse` 时,
// 畸形 JSON 返回 **400 text/plain**(如 `Failed to deserialize the JSON body into the
// target type: ...`),前端 `client.ts` 的 `request()` 走 `res.json()` 解析会抛错,
// 只能拿到「请求失败 400」——丢失了「哪个字段不对」这一用户可纠正的信息。
//
// 本模块提供薄包装 `JsonBody<T>`:把任何 `JsonRejection` 映射为
// `{ error, code: "VALIDATION" }` + 400,与 `api/errors.rs` 的错误契约一致。
//
// 接入方式(增量):handler 签名 `Json(body): Json<T>` → `JsonBody(body): JsonBody<T>`,
// 路由 `body` 类型不变(仍是 `T`)。未接入的 handler 保持 axum 内建行为,不影响功能。
//
// **已知例外:413 请求体过大**。`DefaultBodyLimit`(api/mod.rs 的 35MB 层)在
// **提取器被调用前**就由 tower 中间件拒绝,产生的是 `LengthLimitError`,本模块的
// `FromRequest` 根本不执行——要统一 413 的 JSON 形状必须自定义 tower 层拦截 body
// `Frame` 错误,成本(改动全站 body 流)远高于收益(体积超限本就罕见且前端上传前已校验),
// 故本轮登记为已知例外,不做改造。

use crate::api::WithStatus;
use axum::body::{to_bytes, Body};
use axum::extract::rejection::JsonRejection;
use axum::extract::FromRequest;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::de::DeserializeOwned;
use serde_json::json;

/// 畸形/不可解析 JSON 体的用户文案:不含 serde 内部类型名与行列细节
/// (那些进 `tracing::warn!` 日志),只说明「哪里出错了 + 怎么办」。
const INVALID_JSON_MESSAGE: &str = "请求体不是合法 JSON,请检查字段格式";

/// 未知字段的文案模板(字段名由 serde 报错里提取;提不到则退回通用文案)。
///
/// 为什么单独给一条:未知字段是**唯一**一类「请求体语法完全合法、用户能自己修好、
/// 且极可能是结构性误解」的失败。笼统的「不是合法 JSON」会把人引向检查引号与逗号,
/// 而真正的问题是字段名或层次错了。**本模板必须保持中性**——`JsonBody` 是全站通用
/// 提取器,不能写死某个端点的语义(端点特有的指引由各自的 handler 提供)。
const UNKNOWN_FIELD_TEMPLATE: &str =
    "请求体含未知字段「{}」:请核对该字段名与所在的层次(本接口不接受未定义的额外字段)";

/// 从 serde 的 `unknown field \`xxx\`` 报错里取出字段名。
///
/// 只认这一个稳定子串:serde 未承诺消息格式,取不到就回 None 走通用文案——
/// 宁可文案差一点,也不要为了猜格式而误报(把畸形 JSON 说成「未知字段」更误导)。
fn unknown_field_of(rejection_text: &str) -> Option<&str> {
    let rest = rejection_text.split("unknown field `").nth(1)?;
    let name = rest.split('`').next()?;
    (!name.is_empty()).then_some(name)
}

/// JSON 体提取器:`Json<T>` 的收口包装,拒绝时输出 `{error, code}` + 400(JSON)。
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonBody<T>(pub T);

impl<S, T> FromRequest<S> for JsonBody<T>
where
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(JsonBody(value)),
            Err(rejection) => {
                // 原始 rejection 只进日志:它带 serde 类型名/字节偏移,对用户无行动价值
                let detail = rejection.to_string();
                tracing::warn!(error = %detail, "JSON 请求体解析失败");
                Err(invalid_json_response_for(&detail))
            }
        }
    }
}

/// 构造「非法 JSON」响应;能识别出未知字段时给出**点名**的文案。
fn invalid_json_response_for(rejection_text: &str) -> Response {
    let message = match unknown_field_of(rejection_text) {
        Some(field) => UNKNOWN_FIELD_TEMPLATE.replace("{}", field),
        None => INVALID_JSON_MESSAGE.to_string(),
    };
    Json(json!({
        "error": message,
        "code": crate::api::ErrorCode::Validation.as_str(),
    }))
    .into_response()
    .with_status(StatusCode::BAD_REQUEST)
}

/// 拒绝「把整体多包一层 `settings` 信封」的 JSON 体提取器(2026-11-01)。
///
/// **为什么需要它(而不是给结构体加 `deny_unknown_fields`)**:
/// `PUT /api/settings` 必须保留 TM-SET-2 的「退役字段静默忽略」兼容语义——旧客户端带
/// 已下线键时得回 200,否则老前端在新服务端上直接不可用。全局拒绝未知键会把这个面打掉。
/// 而「信封形状」是**另一种**失败:请求体语法合法、字段名也对,只是外面多了层壳,
/// serde 把它当未知键丢掉 → 回 200 却一个字段都没改(2026-11-01 实测踩到)。
///
/// 故这里只针对**那一个形状**做判定:顶层若是独占的 `settings` 对象,直接 400 并点名,
/// 其余未知键照旧忽略。判定在反序列化**之前**做,靠解析成 `Value` 看顶层键集合。
///
/// 只在明确有这种误用风险的端点上用,不做全站默认——否则就是变相 `deny_unknown_fields`。
pub struct JsonBodyRejectingEnvelope<T>(pub T);

/// 信封键名(与前端/脚本误用形态一致)
const ENVELOPE_KEY: &str = "settings";

impl<S, T> FromRequest<S> for JsonBodyRejectingEnvelope<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let bytes = match to_bytes(body, JSON_ENVELOPE_BODY_LIMIT).await {
            Ok(b) => b,
            Err(_) => return Err(invalid_json_response_for("")),
        };
        // 顶层是对象且含 settings 键 = 信封误用。用 Value 只看键集合,不看值形状:
        // 即使对方传的是 `{"settings": null}` 也同样是误解契约。
        if let Ok(serde_json::Value::Object(map)) =
            serde_json::from_slice::<serde_json::Value>(&bytes)
        {
            if map.contains_key(ENVELOPE_KEY) {
                tracing::warn!(
                    keys = ?map.keys().collect::<Vec<_>>(),
                    "PUT 请求体顶层含 settings 信封,已拒绝"
                );
                return Err(Json(json!({
                    "error": SETTINGS_ENVELOPE_MESSAGE,
                    "code": crate::api::ErrorCode::Validation.as_str(),
                }))
                .into_response()
                .with_status(StatusCode::BAD_REQUEST));
            }
        }
        // 信封判定通过后走标准提取(沿用同一套拒绝文案)
        let req = axum::extract::Request::from_parts(parts, Body::from(bytes));
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(JsonBodyRejectingEnvelope(value)),
            Err(rejection) => {
                let detail = rejection.to_string();
                tracing::warn!(error = %detail, "JSON 请求体解析失败");
                Err(invalid_json_response_for(&detail))
            }
        }
    }
}

/// 信封判定需要先读全 body,用自己的上限(与全站 35MB 同量级;settings 体本身很小)
const JSON_ENVELOPE_BODY_LIMIT: usize = 35 * 1024 * 1024;

/// 顶层被多包一层 `settings` 信封时的用户文案。
///
/// 必须点名「顶层就是字段本身」:只说「格式不对」的话,调用方第一反应是去检查字段拼写,
/// 而真正的问题是外边多了一层壳。
const SETTINGS_ENVELOPE_MESSAGE: &str =
    "请求体顶层不接受 settings 信封:设置字段本身要直接放在顶层(例如 {\"exec_enabled\": true}),而不是 {\"settings\": {\"exec_enabled\": true}}";

/// 可选 JSON 体提取器(2026-10-03 API 设置补全):请求体缺失/空白时得到 `None`,
/// 畸形 JSON 仍按本模块口径 400 收口。
///
/// 服务于「新旧客户端共存」的端点(如连接探测):旧客户端发 `{}` 或不带体,
/// 新客户端带探测参数,同一 handler 内统一处理。注意 `Some("")` 与字段缺省
/// 依然可区分(serde 层面),调用方据此实现「显式空值 ≠ 未提供」的语义。
pub struct JsonBodyOpt<T>(pub Option<T>);

/// 探测参数体积极小(几十字节),给个远小于全站 35MB 的本地上限防误用
const JSON_BODY_OPT_LIMIT: usize = 1024 * 1024;

impl<S, T> FromRequest<S> for JsonBodyOpt<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(
        req: axum::extract::Request,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let bytes = to_bytes(req.into_body(), JSON_BODY_OPT_LIMIT)
            .await
            .map_err(|_| invalid_json_response_for(""))?;
        // 无体/纯空白体 = 未提供参数(区别于「提供了但畸形」)
        if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(JsonBodyOpt(None));
        }
        match serde_json::from_slice::<T>(&bytes) {
            Ok(value) => Ok(JsonBodyOpt(Some(value))),
            Err(e) => {
                let detail = e.to_string();
                tracing::warn!(error = %detail, "JSON 请求体解析失败(可选体)");
                Err(invalid_json_response_for(&detail))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::post;
    use axum::Router;
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Deserialize)]
    struct Payload {
        name: String,
    }

    fn app() -> Router {
        Router::new().route(
            "/echo",
            post(|JsonBody(p): JsonBody<Payload>| async move { p.name }),
        )
    }

    /// 读取响应 (状态码, content-type, JSON/文本 body)
    async fn call(body: &str) -> (StatusCode, String, String) {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        let status = resp.status();
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, ct, String::from_utf8_lossy(&bytes).to_string())
    }

    #[tokio::test]
    async fn valid_body_passes_through() {
        let (status, _, body) = call(r#"{"name":"kedai"}"#).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "kedai");
    }

    #[tokio::test]
    async fn malformed_json_is_json_400_with_validation_code() {
        let (status, ct, body) = call("{不是合法 JSON").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // 关键:不再返回 text/plain,前端 request() 可解析出 code
        assert!(
            ct.starts_with("application/json"),
            "content-type 应为 JSON:{ct}"
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["code"], "VALIDATION");
        assert!(v["error"].as_str().unwrap().contains("合法 JSON"));
        // 泄露守卫:响应体不得回显 serde 的解析细节
        assert!(
            !body.contains("Failed to deserialize"),
            "泄露 serde 原文:{body}"
        );
    }

    #[tokio::test]
    async fn missing_required_field_is_json_400_with_validation_code() {
        // 字段缺失(结构合法但不符合目标类型)同样收口为 JSON + VALIDATION
        let (status, ct, body) = call(r#"{"other":1}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            ct.starts_with("application/json"),
            "content-type 应为 JSON:{ct}"
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["code"], "VALIDATION");
        assert!(!body.contains("missing field"), "泄露 serde 原文:{body}");
    }

    #[tokio::test]
    async fn wrong_content_type_is_json_400_with_validation_code() {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "text/plain")
            .body(Body::from(r#"{"name":"kedai"}"#))
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(
            ct.starts_with("application/json"),
            "content-type 应为 JSON:{ct}"
        );
    }

    // ---------- JsonBodyOpt(可选体提取器) ----------

    fn opt_app() -> Router {
        Router::new().route(
            "/probe",
            post(|JsonBodyOpt(p): JsonBodyOpt<Payload>| async move {
                match p {
                    Some(p) => p.name,
                    None => "no-body".to_string(),
                }
            }),
        )
    }

    async fn call_opt(body: Option<&str>) -> (StatusCode, String) {
        let mut builder = axum::http::Request::builder().method("POST").uri("/probe");
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let req = builder
            .body(Body::from(body.unwrap_or("").to_string()))
            .unwrap();
        let resp = opt_app().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    #[tokio::test]
    async fn opt_missing_body_yields_none() {
        let (status, body) = call_opt(None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "no-body");
    }

    #[tokio::test]
    async fn opt_empty_and_whitespace_body_yield_none() {
        for probe in ["", "   ", "\n\t"] {
            let (status, body) = call_opt(Some(probe)).await;
            assert_eq!(status, StatusCode::OK, "空白体 {probe:?} 应视为未提供");
            assert_eq!(body, "no-body");
        }
    }

    #[tokio::test]
    async fn opt_valid_body_parses_some() {
        let (status, body) = call_opt(Some(r#"{"name":"kedai"}"#)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "kedai");
    }

    #[tokio::test]
    async fn opt_provided_but_invalid_for_type_is_400() {
        // `{}` 是合法 JSON 但缺必填字段:与「未提供」不同,必须 400 收口
        let (status, body) = call_opt(Some("{}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["code"], "VALIDATION");
    }

    #[tokio::test]
    async fn opt_malformed_body_is_json_400() {
        let (status, body) = call_opt(Some("{不是合法 JSON")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["code"], "VALIDATION");
    }
    // ===== 未知字段点名(2026-11-01)=====
    //
    // 背景:`PUT /api/settings` 的 body 全是 Option + serde(default),未知键会被静默丢弃
    // ——多包一层 `{"settings": {...}}` 时接口回 200 却一个字段都没改。结构体加
    // `deny_unknown_fields` 后需要一条**可操作**的文案,而不是笼统的「不是合法 JSON」。

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Strict {
        name: String,
    }

    async fn call_strict(body: &str) -> (StatusCode, String) {
        let app = Router::new().route(
            "/strict",
            post(|JsonBody(p): JsonBody<Strict>| async move { p.name }),
        );
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/strict")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    #[tokio::test]
    async fn unknown_field_is_named_in_error() {
        let (status, body) = call_strict(r#"{"settings":{"name":"x"}}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["code"], "VALIDATION");
        let msg = v["error"].as_str().unwrap_or_default();
        // 点名收到的那个未知键(用户据此能立刻看出是哪一层多了东西)
        assert!(msg.contains("settings"), "错误文案应点名未知字段:{msg}");
        // 且提示核对「层次」——只说"格式不对"会把用户引向检查引号逗号
        assert!(msg.contains("层次"), "错误文案应提示核对层次:{msg}");
        // 本模板必须保持中性:JsonBody 是全站通用提取器,不能写死某个端点的语义
        assert!(
            !msg.contains("设置") && !msg.contains("exec_enabled"),
            "通用提取器的文案不应硬编码 settings 端点语义:{msg}"
        );
    }

    #[tokio::test]
    async fn malformed_json_keeps_generic_message() {
        // 畸形 JSON 不能被误报成「未知字段」——那会把更严重的问题说小
        let (status, body) = call_strict("{不是 JSON").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let msg = v["error"].as_str().unwrap_or_default();
        assert!(
            !msg.contains("未知字段"),
            "畸形 JSON 不应被说成未知字段:{msg}"
        );
    }

    #[test]
    fn unknown_field_extractor_handles_absent_and_empty() {
        assert_eq!(unknown_field_of("some other error"), None);
        assert_eq!(unknown_field_of("unknown field ``"), None);
    }
}
