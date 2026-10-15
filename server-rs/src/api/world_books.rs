// 世界书路由:/api/world-books(列表/上传/详情/更新/删除/条目编辑/自检)+ 角色卡内嵌条目
//
// 缓存失效口径(2026-10-14 修复批):条目缓存与契约缓存统一在 WorldBookService 的
// 写路径内失效(见 services/world_book_service.rs 的 `write_hook`),本文件不再显式
// 清契约缓存——API 之外还有工具/脚本写路径,挂在服务层唯一汇聚点才不会漏。
use crate::api::app_state::AppState;
use crate::api::characters::{parse_boundary, parse_multipart};
use crate::api::{err_status, not_found, validation, WithStatus};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let books = state.world_books.list();
    Json(json!({ "world_books": books }))
}

/// GET /api/world-books/auto-assign-check — 自动分配属性机制自检
/// (可选 ?character_id= 指定角色:额外校验该角色/全局世界书条目解析链路)
#[derive(Deserialize)]
pub struct AutoAssignQuery {
    #[serde(default)]
    pub character_id: Option<String>,
}

pub async fn auto_assign_check(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AutoAssignQuery>,
) -> Json<serde_json::Value> {
    let result = state
        .world_books
        .auto_assign_check(query.character_id.as_deref());
    Json(result)
}

const MAX_UPLOAD: usize = 30 * 1024 * 1024;

/// 上传独立世界书(multipart:file + 可选 character_id 字段)
pub async fn upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(boundary) = parse_boundary(content_type) else {
        return validation("缺少文件字段(file)");
    };
    let parts = parse_multipart(&body, &boundary);
    let Some(file) = parts.iter().find(|p| p.filename.is_some()) else {
        return validation("缺少文件字段(file)");
    };
    if file.content.len() > MAX_UPLOAD {
        return validation("文件超过 30MB 上限");
    }
    let character_id = parts
        .iter()
        .find(|p| p.field == "character_id")
        .map(|p| String::from_utf8_lossy(&p.content).trim().to_string())
        .filter(|s| !s.is_empty());
    let file_name = file
        .filename
        .clone()
        .unwrap_or_else(|| "worldbook.json".to_string());
    match state
        .world_books
        .upload(&file.content, &file_name, character_id.as_deref())
    {
        Ok(b) => Json(b).into_response().with_status(StatusCode::CREATED),
        Err(e) => validation(e),
    }
}

#[derive(Deserialize)]
pub struct UpdateBody {
    #[serde(default)]
    pub enabled: Option<bool>,
    /// 绑定角色 id;传空串表示清除绑定(转全局);缺省表示不变
    #[serde(default)]
    pub character_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Response {
    let cid = body.character_id.map(|s| {
        if s.trim().is_empty() {
            None
        } else {
            Some(s.trim().to_string())
        }
    });
    match state.world_books.update(
        &id,
        body.enabled,
        cid.as_ref().map(|c| c.as_deref()),
        body.name.as_deref(),
    ) {
        Some(b) => Json(b).into_response(),
        None => not_found("世界书不存在"),
    }
}

pub async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if state.world_books.delete(&id) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        not_found("世界书不存在")
    }
}

pub async fn entries(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.world_books.entries(&id) {
        Some(entries) => Json(json!({ "id": id, "entries": entries })).into_response(),
        None => not_found("世界书不存在"),
    }
}

/// PUT /api/world-books/{id}/entries — 全量回写条目(内容/关键词/正则/常驻/激发/位置)
#[derive(Deserialize)]
pub struct SaveEntriesBody {
    pub entries: Vec<crate::models::types::WorldBookEntryView>,
}

pub async fn save_entries(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SaveEntriesBody>,
) -> Response {
    // 预检只为区分 404/422 文案;真正的读改写已下沉到服务层写锁事务
    // (2026-10-15 并发修复批:原实现「get → merge → save_data_raw」三步分离,
    // 与工具/脚本的并发写互不感知,后写整份覆盖先写)。
    let rec = match state.world_books.get(&id) {
        Some(r) => r,
        None => return not_found("世界书不存在"),
    };
    if rec.data_raw.is_none() {
        return validation("世界书缺少原始数据");
    }
    match state.world_books.save_entries_merged(&id, &body.entries) {
        Ok(true) => Json(json!({ "ok": true, "id": id, "entries": body.entries })).into_response(),
        // 预检与提交之间书被并发删除:与预检同文案(极少见的竞态窗口)
        Ok(false) => not_found("世界书不存在"),
        // "保存失败"是给用户的固定文案(非内部错误原文),故走 err_status 保留原文,
        // 只补 code;若改成 internal() 会把文案降级为通用提示,反而丢信息。
        Err(_) => err_status("保存失败", StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/world-books/{id}/entries — 新增一条空条目
pub async fn add_entry(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.world_books.add_entry(&id) {
        Some(view) => Json(json!({ "ok": true, "entry": view })).into_response(),
        None => not_found("世界书不存在或保存失败"),
    }
}

/// GET /api/characters/{id}/world-entries — 角色卡内嵌世界书(character_book)条目编辑视图
pub async fn character_entries(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    match state.world_books.character_book_views(&id) {
        Some(entries) => Json(json!({ "character_id": id, "entries": entries })).into_response(),
        None => Json(json!({ "character_id": id, "entries": [] })).into_response(),
    }
}

/// PUT /api/characters/{id}/world-entries — 回写角色卡内嵌世界书条目(写回 data_raw.character_book.entries)
pub async fn save_character_entries(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SaveEntriesBody>,
) -> Response {
    match state.world_books.save_character_book(&id, &body.entries) {
        Some(()) => {
            // 无缓存失效动作:内嵌书每轮现读 characters.data_raw(条目缓存只覆盖独立书),
            // 且契约注册表的条目源只读独立书(collect_entries_for_character)——
            // 内嵌书条目不是 [nlkaleido_contract] 来源,不存在需要失效的缓存。
            Json(json!({ "ok": true, "character_id": id, "entries": body.entries })).into_response()
        }
        None => validation("角色卡无内嵌世界书,或保存失败"),
    }
}
