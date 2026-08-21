// API 路由组装 + health + 头像静态服务 + SPA 回退
// 路由表按域拆至 routes/ 子模块;静态文档与 SPA 回退在 static_files.rs;
// 响应工具(WithStatus/db_err)在 util.rs,经下方再导出保持 crate::api::* 路径不变;
// 结构化错误码(ErrorCode/err_with_code)在 errors.rs,同样经下方再导出。
pub mod agent;
pub mod agent_flows;
pub mod app_state;
pub mod audio;
pub mod characters;
pub mod chat;
pub mod contracts;
pub mod contract_history;
pub mod diagnostics;
pub mod import_export;
pub mod kaleido;
pub mod macros;
pub mod memory;
pub mod plugins;
pub mod prompt_inject;
pub mod quick_replies;
pub mod resource;
pub mod security;
pub mod sessions;
pub mod settings;
pub mod skills;
pub mod slash_commands;
pub mod tasks;
pub mod tokens;
pub mod tool_permissions;
pub mod user_scripts;
pub mod variables;
pub mod world_books;

pub(crate) mod routes;
pub(crate) mod static_files;
mod errors;
mod util;

pub use util::WithStatus;
pub(crate) use errors::{code_for_status, err_with_code, ErrorCode};
pub(crate) use util::db_err;

use crate::api::app_state::AppState;
use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use std::sync::Arc;

/// 组装全部路由(API + 头像 + 前端静态)
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        // 健康检查
        .route(
            "/api/health",
            get(|| async {
                Json(json!({
                    "ok": true,
                    "ts": chrono::Utc::now().timestamp_millis(),
                }))
            }),
        )
        // 同源页面仅在 loopback Host/Origin 下引导内存 token;响应禁止缓存。
        .route("/api/bootstrap", get(static_files::bootstrap))
        // 头像静态服务
        .route("/api/avatars/{file}", get(static_files::avatar_file))
        // 角色卡脚本沙箱文档(独立 CSP,绕过全站 script-src 'self' 对内联脚本的限制)
        .route("/sandbox.html", get(static_files::sandbox_document))
        // 角色卡远程资源界面宿主文档(独立 CSP:允许作者页面脚本/网络,断掉与宿主的一切共享)
        .route(
            "/resource-frame.html",
            get(static_files::resource_frame_document),
        )
        // 消息渲染面板宿主文档(TH-render 等价物:独立 CSP,断掉一切网络出口)
        .route("/render-frame.html", get(static_files::render_frame_document))
        // 分域 API 路由(实现见 routes/ 子模块;merge 后统一 with_state,与原单链语义一致)
        .merge(routes::chat::chat_routes())
        .merge(routes::settings::settings_routes())
        .merge(routes::agent::agent_routes())
        .merge(routes::content::content_routes())
        .merge(routes::misc::misc_routes())
        // 前端静态(生产:磁盘 dist 或内嵌);SPA 回退
        .fallback(static_files::spa_fallback)
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(state, security::guard))
        // 上传 body 限制:axum 默认 2MB,角色卡/世界书/预设 30MB 上限(MAX_UPLOAD/MAX_PRESET)
        // 会被先拦下,这里放行到 35MB(multipart 封装有少量 overhead),再由 handler 内精确校验;
        // agent 执行流程保存(JSON body)同样受此上限覆盖。
        .layer(DefaultBodyLimit::max(35 * 1024 * 1024))
        // CORS:仅放行本机来源(浏览器同源页面本不需要 CORS;此限制用于挡住恶意网页
        // 跨域读取/触发本地服务,同时兼容开发模式 Vite(5173)与桌面窗口)。
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::predicate(|origin, _| {
                    let host = origin.to_str().ok().and_then(|o| {
                        o.trim_start_matches("http://")
                            .trim_start_matches("https://")
                            .split('/')
                            .next()
                            .and_then(|h| h.split(':').next())
                    });
                    matches!(host, Some("127.0.0.1") | Some("localhost"))
                }))
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
}
