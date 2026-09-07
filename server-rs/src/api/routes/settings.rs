// 设置 / 跨会话记忆 / Token / 诊断路由(自 api/mod.rs build_router 迁入)
use crate::api::app_state::AppState;
use crate::api::{diagnostics, memory, settings, tokens};
use axum::routing::{get, post, put};
use axum::Router;
use std::sync::Arc;

pub(crate) fn settings_routes() -> Router<Arc<AppState>> {
    Router::new()
        // 设置
        .route(
            "/api/settings",
            get(settings::get_settings).put(settings::update_settings),
        )
        .route("/api/settings/connect", post(settings::connect))
        .route(
            "/api/settings/refresh-models",
            post(settings::refresh_models),
        )
        .route("/api/settings/models", get(settings::models))
        .route("/api/settings/info", get(settings::info))
        .route(
            "/api/settings/model",
            put(settings::switch_model).get(settings::get_model),
        )
        .route(
            "/api/settings/agent-prompt",
            get(settings::get_agent_prompt).put(settings::save_agent_prompt),
        )
        .route(
            "/api/settings/prompt-preview",
            get(settings::prompt_preview),
        )
        // 跨会话记忆蒸馏(落地项 2):蒸馏 / 列表 / 手动添加 / 编辑 / 删除
        .route("/api/memory/distill", post(memory::distill))
        .route("/api/memory", get(memory::list).post(memory::create))
        .route(
            "/api/memory/{id}",
            axum::routing::patch(memory::update).delete(memory::delete),
        )
        // Token 统计
        .route("/api/token/count", post(tokens::count))
        .route("/api/token/session-total", get(tokens::session_total))
        .route("/api/token/global-total", get(tokens::global_total))
        // 缓存诊断(缓存感知管线):命中率/费用节省/四级水位
        .route("/api/diagnostics/cache", get(diagnostics::cache))
}
