// 静态资源与自包含文档:bootstrap / 头像 / iframe 宿主文档 / SPA 回退 / 内嵌 dist
// (自 api/mod.rs 迁入,纯代码移动,行为不变)
use crate::api::app_state::AppState;
use crate::api::util::WithStatus;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

pub(crate) async fn bootstrap(State(state): State<Arc<AppState>>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(
            json!({ "token": state.config.api_token }).to_string(),
        ))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// GET /api/avatars/{file}:从 DATA_DIR/avatars 读取
pub(crate) async fn avatar_file(
    State(state): State<Arc<AppState>>,
    Path(file): Path<String>,
) -> Response {
    // 防目录穿越
    let safe: String = file
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .collect();
    if safe != file {
        return Json(json!({ "error": "Not Found" })).into_response();
    }
    let path = state.config.data_dir.join("avatars").join(&file);
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mime = guess_mime(&file);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::NOT_FOUND.into_response())
        }
        Err(_) => Json(json!({ "error": "Not Found" })).into_response(),
    }
}

/// GET /sandbox.html — 角色卡脚本沙箱 bootstrap 文档。
///
/// 本身不含业务逻辑:只监听 postMessage({type:'boot', script}),把脚本体作为内联
/// `<script>` 注入自身执行。之所以需要独立文档而不用 srcdoc:srcdoc iframe 会继承
/// 父页面的 CSP,而全站 CSP 是 `script-src 'self'`(无 'unsafe-inline'),内联脚本
/// 会被直接拒绝;用 src 加载的文档则按自身响应头的 CSP 计算(见 security::sandbox_headers)。
/// 动态注入 `<script>` 走的是 script-src,不需要 'unsafe-eval'。
pub(crate) async fn sandbox_document() -> Response {
    const HTML: &str = include_str!("sandbox_template.html");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(
            header::CACHE_CONTROL,
            "no-store, no-cache, must-revalidate, max-age=0",
        )
        .body(axum::body::Body::from(HTML))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// GET /resource-frame.html — 角色卡远程资源界面宿主文档。
///
/// srcdoc/blob iframe 会继承父页面 CSP(script-src 'self'),作者页面(Vite 打包的
/// 内联 module script)无法执行;本独立文档用 src 加载,按自身宽松 CSP
/// (见 security::resource_frame_headers)计算,允许作者页面的脚本/样式/图片/网络,
/// 但不与宿主共享 origin(iframe 无 allow-same-origin),拿不到宿主
/// DOM/localStorage/token。宿主经后端代理抓取作者页面 HTML 后 postMessage 投递,
/// 本页用 DOM 重建(appendChild)触发脚本执行(document.write 会丢失 module script)。
pub(crate) async fn resource_frame_document() -> Response {
    const HTML: &str = include_str!("resource_frame_template.html");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(
            header::CACHE_CONTROL,
            "no-store, no-cache, must-revalidate, max-age=0",
        )
        .body(axum::body::Body::from(HTML))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// GET /render-frame.html — 消息渲染面板宿主文档(TH-render 等价物)。
///
/// 消息正文中约定式识别的 HTML 代码块(含 `<html`/`<head`/`<body` 标记)渲染为
/// 面板 iframe,本文档用 src 加载(不继承父页面 CSP,见 security::render_frame_headers),
/// 宿主经 postMessage 投递面板 HTML,本页 appendChild 注入执行。面板脚本运行在
/// 无 same-origin 的沙箱 iframe 里,断掉一切网络出口,与宿主完全隔离;面板内
/// parent.postMessage 上报高度(kd-panel-resize)与事件(kd-panel-event)。
pub(crate) async fn render_frame_document() -> Response {
    const HTML: &str = include_str!("render_frame_template.html");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(
            header::CACHE_CONTROL,
            "no-store, no-cache, must-revalidate, max-age=0",
        )
        .body(axum::body::Body::from(HTML))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// SPA 回退:先匹配实际静态文件,未命中时 GET 非 /api/ 返回 index.html;否则 404 {"error":"Not Found"}
pub(crate) async fn spa_fallback(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().clone();

    if method == axum::http::Method::GET && !path.starts_with("/api/") {
        // 归一化静态路径:丢弃 ".." / 空段 / 点段,防止目录穿越读到 web/dist 之外的文件
        let rel: String = path
            .split('/')
            .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
            .collect::<Vec<_>>()
            .join("/");
        let rel = rel.trim_start_matches('/');
        // 根路径或空路径 → 直接 index.html
        let target = if rel.is_empty() { "index.html" } else { rel };
        // 1) 实际静态文件(磁盘优先,回退内嵌)
        if let Some(bytes) = read_file_or_embedded(&state, target) {
            let mime = guess_mime(target);
            // index.html 禁止缓存(no-cache):WebView2/浏览器会缓存旧的 index.html,
            // 导致加载旧 hash 的 JS/CSS,表现为「改了代码重启后还是旧界面」。
            // 其余静态资源(带 hash 的 assets/*)同样禁止缓存,保证部署后立即生效。
            let cache_control = if target == "index.html" {
                "no-store, no-cache, must-revalidate, max-age=0"
            } else {
                "no-cache, must-revalidate"
            };
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .header(header::CACHE_CONTROL, cache_control)
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
        // 2) SPA 回退:未命中一律返回 index.html(与 Node 版 setNotFoundHandler 一致)
        if let Some(index) = read_file_or_embedded(&state, "index.html") {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(
                    header::CACHE_CONTROL,
                    "no-store, no-cache, must-revalidate, max-age=0",
                )
                .body(axum::body::Body::from(index))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    }
    // 其余一律 404 {"error":"Not Found"}
    Json(json!({ "error": "Not Found" }))
        .into_response()
        .with_status(StatusCode::NOT_FOUND)
}

/// 读取前端资源。发布版默认只使用内嵌 dist；仅显式设置 KEDAI_WEB_DIST 时启用磁盘覆盖。
fn read_file_or_embedded(state: &AppState, rel: &str) -> Option<Vec<u8>> {
    if let Some(web_dist) = &state.config.web_dist {
        let disk = web_dist.join(rel);
        if let Ok(bytes) = std::fs::read(&disk) {
            return Some(bytes);
        }
    }
    embedded::get(rel)
}

/// 内嵌 web/dist(编译时嵌入;要求目录存在)
mod embedded {
    use include_dir::{include_dir, Dir};
    pub static DIST: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../web/dist");

    pub fn get(rel: &str) -> Option<Vec<u8>> {
        let norm = rel.trim_start_matches('/');
        if norm.is_empty() || norm == "index.html" {
            return DIST.get_file("index.html").map(|f| f.contents().to_vec());
        }
        DIST.get_file(norm).map(|f| f.contents().to_vec())
    }
}

fn guess_mime(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if lower.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if lower.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".json") {
        "application/json"
    } else if lower.ends_with(".woff2") {
        "font/woff2"
    } else if lower.ends_with(".ico") {
        "image/x-icon"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod resource_frame_tests {
    /// 资源界面宿主文档须为沙箱 iframe(无 allow-same-origin)提供 Cache API 兼容层。
    ///
    /// 背景:角色卡作者页面(Vite 打包 SPA,如「干物吸血鬼少女与夜间工作」v2.1)初始化时
    /// 读取 `globalThis.caches` 做资源缓存与更新检查;沙箱 iframe 是 opaque origin,
    /// Cache Storage 被禁用,读取该属性直接抛 SecurityError,作者页面落入「重试」错误态,
    /// 资源界面(下载/协议/角色选择)无法显示。修复:宿主文档注入内存 `__kdCaches` shim,
    /// 并把注入作者脚本中的 `globalThis.caches` 引用替换为 shim(与 localStorage 同款策略)。
    const TEMPLATE: &str = include_str!("resource_frame_template.html");

    #[test]
    fn template_defines_kd_caches_shim() {
        assert!(
            TEMPLATE.contains("__kdCaches"),
            "宿主文档缺少 __kdCaches 内存 shim(Cache API 兼容),沙箱内作者脚本读取 globalThis.caches 会抛 SecurityError"
        );
        // shim 至少支持作者页面用到的 open(),否则界面仍走错误分支
        assert!(
            TEMPLATE.contains("open:") || TEMPLATE.contains("function open"),
            "__kdCaches shim 应暴露 open() 供作者页面缓存/更新检查使用"
        );
    }

    #[test]
    fn template_rewrites_author_caches_access() {
        assert!(
            TEMPLATE.contains(r"globalThis\.caches"),
            "宿主文档应包含把 globalThis.caches 引用重写为 __kdCaches 的脚本重写规则(防沙箱 Cache API SecurityError)"
        );
    }

    #[test]
    fn template_caches_shim_replayable_body() {
        // 作者页面 put 后再次 match 需能重复 arrayBuffer() 读 body;
        // 直接存/返回原 Response 会因 body 已消费走「Cache Missed」分支反复下载(无限循环)。
        assert!(
            TEMPLATE.contains("arrayBuffer()") && TEMPLATE.contains("bodyPromise"),
            "__kdCaches 应把响应体快照为 ArrayBuffer 并在 match 时重建 Response,保证缓存可重复命中,否则作者页面反复重新下载"
        );
    }

    #[test]
    fn template_keeps_existing_storage_shims() {
        assert!(TEMPLATE.contains("__kdMemoryStorage"), "localStorage shim 不应被移除");
        assert!(TEMPLATE.contains("__kdSessionStorage"), "sessionStorage shim 不应被移除");
    }
}
