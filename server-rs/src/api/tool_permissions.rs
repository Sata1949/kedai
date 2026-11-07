// Agent 工具授权 API：查询当前会话/角色裁决结果，授予或撤销显式权限。
use crate::api::app_state::AppState;
use crate::api::{conflict, db_err, not_found, validation};
use crate::models::types::ToolContext;
use crate::tools::permissions::PendingAuthorizationDecision;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct PermissionQuery {
    pub session_id: String,
    /// 旧客户端仍可传入，但服务端始终以会话记录中的角色为准。
    #[serde(default)]
    pub character_id: Option<String>,
}

#[derive(Deserialize)]
pub struct PermissionBody {
    pub session_id: String,
    pub tool: String,
    pub scope: String,
    /// 恢复当前调用时必填;旧的预授权/撤销接口可不传。
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub call_id: Option<String>,
    /// 仅为兼容旧协议保留,不参与授权目标推导。
    #[serde(default)]
    pub scope_id: Option<String>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PermissionQuery>,
) -> Response {
    let svc = state.sessions.clone();
    let sid = query.session_id.clone();
    let session = match state.db_call(move || svc.get(&sid)).await {
        Err(e) => return db_err(&e),
        Ok(Some(s)) => s,
        Ok(None) => return not_found("会话不存在"),
    };
    let ctx = ToolContext {
        session_id: session.id.clone(),
        character_id: session.character_id.clone(),
        agent_depth: 0,
        // 本处只借 ctx 的会话身份列工具清单,与工作区绑定无关
        scope: None,
        budget: None,
    };
    let tools: Vec<_> = state
        .tool_registry
        .list_definitions()
        .into_iter()
        .map(|definition| {
            let decision = state
                .tool_registry
                .permissions()
                .decide(&definition.name, &ctx);
            json!({
                "name": definition.name,
                "description": definition.description,
                "risk": decision.risk,
                "allowed": decision.allowed,
                "reason": decision.reason,
            })
        })
        .collect();
    Json(json!({
        "tools": tools,
        "grants": state.tool_registry.permissions().grants_for(&session.id, &session.character_id),
    }))
    .into_response()
}

pub async fn authorize(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PermissionBody>,
) -> Response {
    mutate_permission(&state, &body, true).await
}

pub async fn revoke(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PermissionBody>,
) -> Response {
    mutate_permission(&state, &body, false).await
}

pub async fn resolve(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PermissionBody>,
) -> Response {
    let Some(run_id) = body.run_id.as_deref() else {
        return validation("缺少 run_id");
    };
    let Some(call_id) = body.call_id.as_deref() else {
        return validation("缺少 call_id");
    };
    // 会话归属校验(2026-11-06 三档授权):任务模式用的是**虚拟**会话 id
    // (`task:{id}` / `task:{id}:main:{n}` / `task:{id}:{phase}:{step}`),它们不在
    // `sessions` 表里。此前一律查 sessions 表,任务侧的确认卡永远 404。
    // 判据:带 `task:` 前缀且首个冒号段是存在的任务 id → 合法;否则回落原会话校验。
    let task_sid: Option<String> = body
        .session_id
        .strip_prefix("task:")
        .and_then(|rest| rest.split(':').next())
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let is_task_session = match &task_sid {
        Some(task_id) => {
            let svc = state.tasks.clone();
            let tid = task_id.clone();
            match state.db_call(move || svc.get(&tid)).await {
                Err(e) => return db_err(&e),
                Ok(None) => return not_found("任务不存在"),
                Ok(Some(_)) => true,
            }
        }
        None => {
            let svc = state.sessions.clone();
            let sid = body.session_id.clone();
            match state.db_call(move || svc.get(&sid)).await {
                Err(e) => return db_err(&e),
                Ok(None) => return not_found("会话不存在"),
                Ok(Some(_)) => false,
            }
        }
    };
    if state.tool_registry.get(&body.tool).is_none() {
        return validation("工具未注册");
    }
    let decision = match body.scope.as_str() {
        "once" => PendingAuthorizationDecision::AllowOnce,
        "session" => PendingAuthorizationDecision::AllowSession,
        "role" => PendingAuthorizationDecision::AllowRole,
        "deny" => PendingAuthorizationDecision::Deny,
        _ => return validation("decision 仅支持 once/session/role/deny"),
    };
    // 任务会话没有角色维度(`character_id` 是任务执行者,不是角色授权对象),
    // 「当前会话」授权也只对一个虚拟子会话生效(team 各主各一份),两者都会让用户
    // 误解授权范围。任务侧只接受 once/deny,其余明确 400 而不是静默降级。
    if is_task_session
        && !matches!(
            decision,
            PendingAuthorizationDecision::AllowOnce | PendingAuthorizationDecision::Deny
        )
    {
        return validation("任务模式仅支持 once(仅本次)/ deny(拒绝)两种决议");
    }
    match state.tool_registry.permissions().resolve_wait(
        run_id,
        call_id,
        &body.session_id,
        &body.tool,
        decision,
    ) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(error) => conflict(&error),
    }
}

async fn mutate_permission(state: &AppState, body: &PermissionBody, authorize: bool) -> Response {
    let svc = state.sessions.clone();
    let sid = body.session_id.clone();
    let session = match state.db_call(move || svc.get(&sid)).await {
        Err(e) => return db_err(&e),
        Ok(Some(s)) => s,
        Ok(None) => return not_found("会话不存在"),
    };
    if state.tool_registry.get(&body.tool).is_none() {
        return validation("工具未注册");
    }
    let scope_id = match body.scope.as_str() {
        "session" => session.id.as_str(),
        "role" => session.character_id.as_str(),
        _ => return validation("scope 仅支持 session 或 role"),
    };
    // B-1:authorize/revoke 内含同步 JSON 原子落盘(tool_permissions.json),挪阻塞线程池
    let registry = state.tool_registry.clone();
    let tool = body.tool.clone();
    let scope = body.scope.clone();
    let scope_id = scope_id.to_string();
    let result = state
        .db_call(move || {
            if authorize {
                registry.permissions().authorize(&tool, &scope, &scope_id)
            } else {
                registry.permissions().revoke(&tool, &scope, &scope_id)
            }
        })
        .await;
    match result {
        Err(e) => db_err(&e),
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(error)) => validation(&error),
    }
}
