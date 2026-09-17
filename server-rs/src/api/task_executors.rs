// 任务执行者库 API:GET/POST /api/task-executors + DELETE /api/task-executors/{id}
// (全局执行者库,持久化 data/task_executors.json)。
//
// 与 agent_flows 的路由形态一致(三端点、同步落盘整体挪阻塞线程池);
// 差异:无「当前选中」概念(执行者按任务选用,见 tasks.executor_id),
// 因此 GET 只回列表,POST 为 upsert(id 空 = 新建)。
use crate::api::app_state::AppState;
use crate::api::json_body::JsonBody;
use crate::api::{internal, validation};
use crate::services::executor_service::TaskExecutorConfig;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// GET 响应体:执行者列表(按库内顺序;前端下拉直用)
fn executors_payload(list: &[TaskExecutorConfig]) -> serde_json::Value {
    json!({ "ok": true, "executors": list })
}

pub async fn list(State(state): State<Arc<AppState>>) -> Response {
    let list = state
        .executors
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .list()
        .to_vec();
    Json(executors_payload(&list)).into_response()
}

#[derive(Deserialize)]
pub struct SaveExecutorBody {
    pub config: TaskExecutorConfig,
}

/// POST /api/task-executors:保存(创建或更新)执行者;
/// config.id 为空时视为新建(后端分配 id)。校验失败 400。
pub async fn save(
    State(state): State<Arc<AppState>>,
    JsonBody(body): JsonBody<SaveExecutorBody>,
) -> Response {
    // 保存内含同步 JSON 落盘,整体(持锁 + 校验 + 写文件)挪阻塞线程池
    let executors = state.executors.clone();
    let result = state
        .db_call(move || {
            let mut svc = executors.lock().unwrap_or_else(|e| e.into_inner());
            svc.save(body.config).map(|saved| (saved, svc.list().to_vec()))
        })
        .await;
    match result {
        Ok(Ok((saved, list))) => Json(json!({
            "ok": true,
            "saved": saved,
            "executors": list,
        }))
        .into_response(),
        Ok(Err(e)) => validation(e),
        Err(e) => internal(e),
    }
}

/// DELETE /api/task-executors/{id}:删除执行者(引用它的任务不受影响,
/// 执行期查不到配置即回退通用执行者)
pub async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let executors = state.executors.clone();
    let result = state
        .db_call(move || {
            let mut svc = executors.lock().unwrap_or_else(|e| e.into_inner());
            svc.remove(&id).map(|()| svc.list().to_vec())
        })
        .await;
    match result {
        Ok(Ok(list)) => Json(executors_payload(&list)).into_response(),
        Ok(Err(e)) => validation(e),
        Err(e) => internal(e),
    }
}
