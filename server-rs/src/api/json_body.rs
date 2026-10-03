use crate::api::WithStatus;
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
use axum::body::to_bytes;
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
                tracing::warn!(error = %rejection, "JSON 请求体解析失败");
                Err(invalid_json_response())
            }
        }
    }
}

/// 统一的「非法 JSON」响应:400 + `{error, code: VALIDATION}`。
fn invalid_json_response() -> Response {
    Json(json!({
        "error": INVALID_JSON_MESSAGE,
        "code": crate::api::ErrorCode::Validation.as_str(),
    }))
    .into_response()
    .with_status(StatusCode::BAD_REQUEST)
}

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
            .map_err(|_| invalid_json_response())?;
        // 无体/纯空白体 = 未提供参数(区别于「提供了但畸形」)
        if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(JsonBodyOpt(None));
        }
        match serde_json::from_slice::<T>(&bytes) {
            Ok(value) => Ok(JsonBodyOpt(Some(value))),
            Err(e) => {
                tracing::warn!(error = %e, "JSON 请求体解析失败(可选体)");
                Err(invalid_json_response())
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
}
