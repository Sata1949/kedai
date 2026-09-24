// 自定义 Agent 执行流程配置:GET/PUT /api/agent-flows + POST /api/agent-flows/select
// + DELETE /api/agent-flows/{id}(全局流程库,持久化 data/agent_flows.json)
// + GET /api/agent-flows/export 与 POST /api/agent-flows/import(二维批次 7a 流程搬运)。
// 响应统一携带 library{current_flow_id, flows} 与 config(当前选中流程,兼容旧调用方)。
use crate::api::app_state::AppState;
use crate::api::json_body::JsonBody;
use crate::api::{internal, validation};
use crate::services::agent_flow_service::{AgentFlowConfig, AgentFlowLibrary, ImportConflictMode};
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// 统一返回体:library(流程库)+ config(当前选中流程,可能为 null)
fn flow_payload(lib: &AgentFlowLibrary) -> serde_json::Value {
    let config = lib
        .current_flow_id
        .as_ref()
        .and_then(|id| lib.flows.iter().find(|f| &f.id == id))
        .cloned();
    json!({ "ok": true, "library": lib, "config": config })
}

pub async fn get_agent_flows(State(state): State<Arc<AppState>>) -> Response {
    let lib = state
        .flow
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_library()
        .clone();
    Json(flow_payload(&lib)).into_response()
}

#[derive(Deserialize)]
pub struct UpdateFlowBody {
    pub config: AgentFlowConfig,
}

/// PUT /api/agent-flows:保存(创建或更新)流程并设为当前选中;
/// config.id 为空时视为新建(后端分配 id)。校验失败 400。
pub async fn update_agent_flows(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateFlowBody>,
) -> Response {
    // B-1:set 内含同步 JSON 落盘,整体(持锁 + 校验 + 写文件)挪阻塞线程池;
    // 显式持锁在闭包内一次完成(set 后直接读库),避免 std Mutex 重入死锁
    let flow = state.flow.clone();
    let result = state
        .db_call(move || {
            let mut svc = flow.lock().unwrap_or_else(|e| e.into_inner());
            svc.set(body.config).map(|()| svc.get_library().clone())
        })
        .await;
    match result {
        Ok(Ok(lib)) => Json(flow_payload(&lib)).into_response(),
        Ok(Err(e)) => validation(e),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct SelectFlowBody {
    pub id: String,
}

/// POST /api/agent-flows/select:切换当前选中流程
pub async fn select_agent_flow(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SelectFlowBody>,
) -> Response {
    // B-1:select 内含同步落盘,持锁 + 写文件整体挪阻塞线程池
    let flow = state.flow.clone();
    let result = state
        .db_call(move || {
            let mut svc = flow.lock().unwrap_or_else(|e| e.into_inner());
            svc.select(&body.id).map(|()| svc.get_library().clone())
        })
        .await;
    match result {
        Ok(Ok(lib)) => Json(flow_payload(&lib)).into_response(),
        Ok(Err(e)) => validation(e),
        Err(e) => internal(e),
    }
}

/// DELETE /api/agent-flows/{id}:删除流程(删除当前流程时回退到第一个)
pub async fn delete_agent_flow(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    // B-1:remove 内含同步落盘,持锁 + 写文件整体挪阻塞线程池
    let flow = state.flow.clone();
    let result = state
        .db_call(move || {
            let mut svc = flow.lock().unwrap_or_else(|e| e.into_inner());
            svc.remove(&id).map(|()| svc.get_library().clone())
        })
        .await;
    match result {
        Ok(Ok(lib)) => Json(flow_payload(&lib)).into_response(),
        Ok(Err(e)) => validation(e),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct ExportFlowQuery {
    /// 入口流程 id(缺省 = 导出**全库**);给了就导出该流程 + 其可达子流程闭包
    #[serde(default)]
    pub id: Option<String>,
}

/// GET /api/agent-flows/export?id=…:导出**流程搬运包**(二维批次 7a)。
/// 给 id 只校验该流程自身的结构 + 可达引用链(不看 `enabled`);不给 id 则只读不校验。
/// 校验失败 400(错误文案来自校验器本身)。
pub async fn export_agent_flows(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ExportFlowQuery>,
) -> Response {
    // 持锁只读 + 序列化,无落盘;仍走 db_call 以免与保存路径抢锁阻塞运行时
    let flow = state.flow.clone();
    let result = state
        .db_call(move || {
            let svc = flow.lock().unwrap_or_else(|e| e.into_inner());
            svc.export_bundle(q.id.as_deref())
        })
        .await;
    match result {
        Ok(Ok(bundle)) => Json(json!({ "ok": true, "bundle": bundle })).into_response(),
        Ok(Err(e)) => validation(e),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct ImportFlowsBody {
    /// 要导入的流程(来自搬运包或旧格式文件的归一结果)
    pub flows: Vec<AgentFlowConfig>,
    /// 搬运包里的入口流程 id(可选):导入后选中它(经重映射解析)
    #[serde(default)]
    pub root_id: Option<String>,
    /// id 冲突处理(A 批 B4):`rename`(缺省,= 7a 行为,新增副本)/ `replace`(覆盖本库那份)。
    /// 未知值**严格拒绝**(400)——覆盖是不可逆动作,静默回退到 rename 会让用户以为已覆盖。
    #[serde(default)]
    pub on_conflict: Option<String>,
}

/// POST /api/agent-flows/import:导入流程搬运包(二维批次 7a;A 批 B4 加 `on_conflict`)。
///
/// 语义:缺省**只新增**——同 id 内容相同则跳过,同 id 内容不同则分配新 id 并改写批内引用,
/// 绝不覆盖库里既有流程;`on_conflict=replace` 时同 id 内容不同者**覆盖本库那一份**
/// (id 不变,库中其它流程一律不动)。校验在候选库上一次性完成,
/// **失败 400 且库逐字节不变**(原子)。
/// 成功 200:`{ok, report, library, config}`(report 供前端给出「导入 N / 跳过 M / 新增 id K /
/// 覆盖 J」)。
pub async fn import_agent_flows(
    State(state): State<Arc<AppState>>,
    JsonBody(body): JsonBody<ImportFlowsBody>,
) -> Response {
    let mode = match body.on_conflict.as_deref().map(str::trim) {
        None | Some("") | Some("rename") => ImportConflictMode::Rename,
        Some("replace") => ImportConflictMode::Replace,
        Some(other) => {
            return validation(format!("未知的冲突处理方式:{other}(可选:rename/replace)"))
        }
    };
    // B-1:import 内含全库校验 + 同步落盘,持锁 + 写文件整体挪阻塞线程池
    let flow = state.flow.clone();
    let result = state
        .db_call(move || {
            let mut svc = flow.lock().unwrap_or_else(|e| e.into_inner());
            svc.import_bundle(body.flows, body.root_id.as_deref(), mode)
                .map(|report| (report, svc.get_library().clone()))
        })
        .await;
    match result {
        Ok(Ok((report, lib))) => {
            let mut payload = flow_payload(&lib);
            payload["report"] = json!(report);
            Json(payload).into_response()
        }
        Ok(Err(e)) => validation(e),
        Err(e) => internal(e),
    }
}
