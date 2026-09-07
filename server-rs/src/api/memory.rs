// 记忆库路由(跨会话记忆蒸馏·落地项 2):
//   POST   /api/memory/distill   蒸馏指定会话(需开启 memory_distill_enabled)
//   GET    /api/memory           按角色列出全部记忆(全字段)
//   POST   /api/memory           手动添加(kind='manual')
//   PATCH  /api/memory/:id       编辑 content / selected
//   DELETE /api/memory/:id       删除
use crate::api::app_state::AppState;
use crate::api::{db_err, WithStatus};
use crate::models::types::{GenerationParams, LlmMessage, ToolChoice};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct DistillBody {
    pub session_id: String,
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub character_id: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateBody {
    pub character_id: String,
    pub content: String,
}

#[derive(Deserialize, Default)]
pub struct UpdateBody {
    pub content: Option<String>,
    pub selected: Option<bool>,
}

/// POST /api/memory/distill:对指定会话蒸馏记忆。
/// LLM 走 engine 当前连接器(mock 可测);历史为空时直接返回 inserted=0 不调模型。
pub async fn distill(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DistillBody>,
) -> Response {
    // 设置快照:不留锁跨 await
    let enabled = state.settings_snapshot().memory_distill_enabled;
    if !enabled {
        return Json(json!({
            "error": "跨会话记忆蒸馏未开启,请先在设置中打开 memory_distill_enabled"
        }))
        .into_response()
        .with_status(StatusCode::BAD_REQUEST);
    }
    let session_id = body.session_id.trim().to_string();
    if session_id.is_empty() {
        return Json(json!({ "error": "缺少 session_id" }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST);
    }
    // 蒸馏生成参数(与 compaction 同取向:低温度、无工具、非流式)
    let params = GenerationParams {
        temperature: 0.3,
        top_p: 1.0,
        max_tokens: 1024,
        stop: None,
        tools: Vec::new(),
        max_tool_rounds: None,
        tool_choice: ToolChoice::Auto,
        parallel_tool_calls: None,
    };
    // 中止通道:HTTP 端点无 SSE 取消路径,恒 false(不中断)
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let engine = state.engine.clone();
    let sessions = state.sessions.clone();
    let memory = state.memory.clone();
    let llm = move |messages: Vec<LlmMessage>| async move {
        let (text, _usage) = engine.generate_text(&messages, params, abort_rx).await?;
        Ok(text)
    };
    match memory.distill_session(&sessions, &session_id, llm).await {
        Ok(outcome) => Json(json!({
            "ok": true,
            "inserted": outcome.inserted,
            "character_id": outcome.character_id,
        }))
        .into_response(),
        Err(e) => Json(json!({ "error": e }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST),
    }
}

/// GET /api/memory?character_id=:角色全部记忆(最新在前,含未选中条目与计数)
pub async fn list(State(state): State<Arc<AppState>>, Query(query): Query<ListQuery>) -> Response {
    let Some(character_id) = query.character_id.map(|c| c.trim().to_string()) else {
        return Json(json!({ "error": "缺少 character_id" }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST);
    };
    if character_id.is_empty() {
        return Json(json!({ "error": "缺少 character_id" }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST);
    }
    let svc = state.memory.clone();
    let memories = match state.db_call(move || svc.list(&character_id)).await {
        Ok(v) => v,
        Err(e) => return db_err(&e),
    };
    Json(json!({ "memories": memories })).into_response()
}

/// POST /api/memory:手动添加一条记忆(kind='manual')
pub async fn create(State(state): State<Arc<AppState>>, Json(body): Json<CreateBody>) -> Response {
    let character_id = body.character_id.trim().to_string();
    if character_id.is_empty() {
        return Json(json!({ "error": "缺少 character_id" }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST);
    }
    if body.content.trim().is_empty() {
        return Json(json!({ "error": "记忆内容不能为空" }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST);
    }
    let svc = state.memory.clone();
    let content = body.content.clone();
    match state
        .db_call(move || svc.create_manual(&character_id, &content))
        .await
    {
        Err(e) => db_err(&e),
        Ok(Ok(entry)) => Json(json!({ "ok": true, "memory": entry }))
            .into_response()
            .with_status(StatusCode::CREATED),
        Ok(Err(e)) => Json(json!({ "error": e }))
            .into_response()
            .with_status(StatusCode::BAD_REQUEST),
    }
}

/// PATCH /api/memory/:id:编辑 content 与/或 selected(空白 content 拒绝)
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateBody>,
) -> Response {
    if let Some(c) = &body.content {
        if c.trim().is_empty() {
            return Json(json!({ "error": "记忆内容不能为空" }))
                .into_response()
                .with_status(StatusCode::BAD_REQUEST);
        }
    }
    let svc = state.memory.clone();
    let updated = state
        .db_call(move || svc.update(id, body.content.as_deref(), body.selected))
        .await;
    match updated {
        Err(e) => db_err(&e),
        Ok(Some(entry)) => Json(json!({ "ok": true, "memory": entry })).into_response(),
        Ok(None) => Json(json!({ "error": format!("记忆 {id} 不存在或更新被拒绝") }))
            .into_response()
            .with_status(StatusCode::NOT_FOUND),
    }
}

/// DELETE /api/memory/:id
pub async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> Response {
    let svc = state.memory.clone();
    match state.db_call(move || svc.delete(id)).await {
        Err(e) => db_err(&e),
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => Json(json!({ "error": format!("记忆 {id} 不存在") }))
            .into_response()
            .with_status(StatusCode::NOT_FOUND),
    }
}
