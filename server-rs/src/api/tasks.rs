// 任务模式路由:/api/tasks(列表/新建/详情/执行/停止/删除)
use crate::api::app_state::AppState;
use crate::api::WithStatus;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreateTaskBody {
    pub title: String,
    #[serde(default)]
    pub character_id: Option<String>,
}

pub async fn list(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({ "tasks": state.tasks.list() }))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTaskBody>,
) -> Response {
    match state.tasks.create(&body.title, body.character_id.as_deref()) {
        Ok(task) => Json(json!({ "ok": true, "task": task }))
            .into_response()
            .with_status(StatusCode::CREATED),
        Err(e) => Json(json!({ "error": e }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST),
    }
}

pub async fn get(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.tasks.get_with_subtasks(&id) {
        Some((task, subtasks)) => Json(json!({ "task": task, "subtasks": subtasks }))
            .into_response(),
        None => Json(json!({ "error": "任务不存在" }))
            .into_response()
            .with_status(StatusCode::NOT_FOUND),
    }
}

pub async fn run(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.tasks.run(&id) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => Json(json!({ "error": e }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST),
    }
}

pub async fn stop(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if state.tasks.stop(&id) {
        Json(json!({ "ok": true })).into_response()
    } else {
        Json(json!({ "error": "任务不存在" }))
            .into_response()
            .with_status(StatusCode::NOT_FOUND)
    }
}

pub async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if state.tasks.delete(&id) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Json(json!({ "error": "任务不存在" }))
            .into_response()
            .with_status(StatusCode::NOT_FOUND)
    }
}
