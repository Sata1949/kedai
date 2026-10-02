// 截图 / 无障碍状态 API(移动端视觉能力包 A3)。
//
// 端点:
//   GET  /api/screen/status   截图能力状态(安卓 = 无障碍截图服务;带进程内缓存,
//                             ?refresh=1 清缓存强制重探)
//   POST /api/screen/settings 跳转系统「无障碍」设置页(设置面板「前往系统设置开启」按钮)
//
// 安全说明:纯状态读取与设置页跳转,随应用整体受 bootstrap token 鉴权保护(同其它 /api 端点)。
use crate::api::app_state::AppState;
use crate::api::errors::err_status;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// 状态查询参数(可选)。
#[derive(Debug, Deserialize)]
pub struct ScreenStatusQuery {
    /// `refresh=1` 强制重探(清无障碍服务状态缓存)。
    ///
    /// 为什么必须显式区分:Android 侧状态探测带进程内缓存,用户刚在系统设置里开启/关闭
    /// 无障碍服务后,不强制重探会一直拿到缓存旧值(设置页「刷新」按钮走这里;语义与
    /// /api/exec/tier 的 refresh 一致)。
    #[serde(default)]
    pub refresh: Option<String>,
}

/// GET /api/screen/status:截图能力状态。
/// 安卓侧为无障碍截图服务状态;非安卓返回 `platform:"other"` 且 `available:false`
/// (桌面截图走 Windows 原生 GDI,不经本通道;本端点主要供安卓设置面板使用)。
pub async fn status(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ScreenStatusQuery>,
) -> Response {
    let force = matches!(q.refresh.as_deref(), Some("1") | Some("true"));
    let vision_screenshot_enabled = state.settings_snapshot().vision_screenshot_enabled;
    let (accessibility_enabled, reason) =
        crate::services::screen_capture_android::service_enabled(force);
    Json(json!({
        // available 语义:安卓无障碍截图通道是否可用(含系统服务启用状态);
        // 非安卓恒 false(本端点不描述桌面取屏)。
        "available": cfg!(target_os = "android") && accessibility_enabled,
        "reason": reason,
        "platform": if cfg!(target_os = "android") { "android" } else { "other" },
        "vision_screenshot_enabled": vision_screenshot_enabled,
        "android": { "accessibility_enabled": accessibility_enabled },
    }))
    .into_response()
}

/// POST /api/screen/settings:跳系统「无障碍」设置页(需用户手动开启服务)。
pub async fn open_settings(State(_state): State<Arc<AppState>>) -> Response {
    match crate::services::screen_capture_android::open_settings() {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err_status(format!("打开系统无障碍设置失败:{e}"), StatusCode::BAD_REQUEST),
    }
}
