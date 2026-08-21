// 任务模式路由:/api/tasks(列表/新建/详情/执行/停止/删除)
use crate::api::app_state::AppState;
use crate::api::{db_err, WithStatus};
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
    let svc = state.tasks.clone();
    let tasks = state
        .db_call(move || svc.list())
        .await
        .expect("读取任务列表任务失败");
    Json(json!({ "tasks": tasks }))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTaskBody>,
) -> Response {
    let svc = state.tasks.clone();
    match state
        .db_call(move || svc.create(&body.title, body.character_id.as_deref()))
        .await
    {
        Err(e) => db_err(&e),
        Ok(Ok(task)) => Json(json!({ "ok": true, "task": task }))
            .into_response()
            .with_status(StatusCode::CREATED),
        Ok(Err(e)) => Json(json!({ "error": e }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST),
    }
}

pub async fn get(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let svc = state.tasks.clone();
    match state.db_call(move || svc.get_with_subtasks(&id)).await {
        Err(e) => db_err(&e),
        Ok(Some((task, subtasks))) => Json(json!({ "task": task, "subtasks": subtasks }))
            .into_response(),
        Ok(None) => Json(json!({ "error": "任务不存在" }))
            .into_response()
            .with_status(StatusCode::NOT_FOUND),
    }
}

pub async fn run(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let svc = state.tasks.clone();
    match state.db_call(move || svc.run(&id)).await {
        Err(e) => db_err(&e),
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => Json(json!({ "error": e }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST),
    }
}

pub async fn stop(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let svc = state.tasks.clone();
    match state.db_call(move || svc.stop(&id)).await {
        Err(e) => db_err(&e),
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => Json(json!({ "error": "任务不存在" }))
            .into_response()
            .with_status(StatusCode::NOT_FOUND),
    }
}

pub async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let svc = state.tasks.clone();
    match state.db_call(move || svc.delete(&id)).await {
        Err(e) => db_err(&e),
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => Json(json!({ "error": "任务不存在" }))
            .into_response()
            .with_status(StatusCode::NOT_FOUND),
    }
}
