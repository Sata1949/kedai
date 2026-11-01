// 任务执行轨迹读取端点(TMSHOW-1,2026-11-01)。
//
// 从 `api/tasks.rs` 拆出:`tasks.rs` 已 1157 行,再加 42 行会顶破只降不升的
// 文件大小基线(`tools/check-file-size.mjs`)。本端点自成一块(查询体 + 单个 handler),
// 拆出后 `tasks.rs` 侧只需一行 `mod trace;` 与路由登记,不动既有代码。
use crate::api::app_state::AppState;
use crate::api::{db_err, not_found};
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// `GET /api/tasks/{id}/trace` 查询体(TMSHOW-1)。
#[derive(Deserialize)]
pub struct TaskTraceQuery {
    /// 只要 `seq > after` 的轨迹;缺省 0 = 从窗口内最早一条起。
    #[serde(default)]
    pub after: Option<i64>,
    /// 返回条数上限;缺省 500,夹取 `1..=1000`(与事件补拉同口径,防一次拉爆)。
    #[serde(default)]
    pub limit: Option<usize>,
}

/// GET /api/tasks/{id}/trace:任务执行轨迹读取(TMSHOW-1)。
///
/// 为什么独立端点而不并入任务详情:轨迹正文可达数 MB(思考 + 工具入出参完整文本),
/// 而任务详情由 `agent_status` 等高频事件驱动反复重拉(`loadTaskDetail`)——
/// 把轨迹塞进详情会让每次进展刷新都传一遍全量轨迹。
///
/// 语义与 `GET /api/tasks/{id}/events` 逐条对齐:`seq` 升序、`after` 增量、
/// `truncated` 标记窗口外缺项、`after` 超过最新返回空数组(不是 404)。
pub async fn list_trace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<TaskTraceQuery>,
) -> Response {
    if state.tasks.get(&id).is_none() {
        return not_found("任务不存在");
    }
    let svc = state.tasks.clone();
    let after = q.after.unwrap_or(0);
    let limit = q.limit.unwrap_or(500).clamp(1, 1000);
    match state
        .db_call(move || svc.list_trace(&id, after, limit))
        .await
    {
        Err(e) => db_err(&e),
        // 读取失败回 500,不得回 200 空数组让客户端把缺失当权威(2026-10-25 审计 P1)。
        Ok(Err(e)) => db_err(&e),
        Ok(Ok(page)) => Json(json!({
            "trace": page.rows,
            "truncated": page.truncated,
        }))
        .into_response(),
    }
}
