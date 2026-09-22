// 任务模式路由:/api/tasks(列表/新建/详情/执行/批准/停止/删除/事件 SSE 流)
use crate::api::app_state::AppState;
use crate::api::json_body::JsonBody;
use crate::api::{db_err, err_with_code, not_found, validation, ErrorCode, WithStatus};
use crate::models::types::{
    TaskApproveExecMode, TaskFollowupMode, TaskRunMode, TaskStatus, TaskStep,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreateTaskBody {
    pub title: String,
    /// 执行者库 id(可选;缺省 = 通用执行者)。指向 data/task_executors.json,
    /// 不存在的 id 由服务层静默丢弃(执行者属可选增强,不该让创建失败)。
    #[serde(default)]
    pub executor_id: Option<String>,
    /// 兼容字段:旧客户端的角色卡执行者。服务端**一律忽略**(执行者已与角色卡解耦,
    /// 落库时 character_id 显式置 NULL)——保留字段只为旧客户端请求不 400。
    #[serde(default)]
    pub character_id: Option<String>,
    /// 执行模式(批次 4 六模式):缺省 legacy;未知值严格拒绝(400),
    /// DB 读取侧的容错回退(from_str_lossy)不用于 API 入参。
    #[serde(default)]
    pub task_mode: Option<String>,
    /// **绑定的自定义流程 id**(二维批次 5a;仅 custom 模式可用,缺省 = 跟随当前流程)。
    ///
    /// 可选 = 旧客户端行为不变。给了就「绑定即冻结」:创建时按该流程落一份快照
    /// (存在 + 启用 + 结构 + 可达引用链校验,失败 400 且**不建行**),此后改流程 /
    /// 换当前流程都不影响这个任务。非 custom 模式给了该字段 → 400(不接受
    /// 「字段存在但静默无效」)。
    #[serde(default)]
    pub flow_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ApproveTaskBody {
    /// 可携修改后计划(整体替换 tasks.plan);None = 按已产出计划原样批准
    #[serde(default)]
    pub plan: Option<Vec<TaskStep>>,
    /// 本次批准续跑的执行方式(2026-09-17):缺省 = approved_plan(按计划逐步执行,
    /// 即改造前行为);可选 solo/multi/team/custom。未知值 400。
    /// 不改 tasks.task_mode(仍为 plan,记录任务当初怎么产出计划)。
    #[serde(default)]
    pub exec_mode: Option<String>,
}

/// followup 追加指令请求体(批次 R2a):content 为追加指令原文。
/// mode(批次 R2b+):append 缺省 | replace(整体替换 result);未知值 400。
#[derive(Deserialize)]
pub struct FollowupTaskBody {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub mode: Option<String>,
}

/// plan-chat 规划对话请求体(批次 R2b):message 为本轮反馈原文。
#[derive(Deserialize)]
pub struct PlanChatBody {
    #[serde(default)]
    pub message: String,
}

pub async fn list(State(state): State<Arc<AppState>>) -> Response {
    let svc = state.tasks.clone();
    match state.db_call(move || svc.list()).await {
        Err(e) => db_err(&e),
        Ok(tasks) => Json(json!({ "tasks": tasks })).into_response(),
    }
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    JsonBody(body): JsonBody<CreateTaskBody>,
) -> Response {
    // 任务模式严格解析:未知值 400「未知任务模式」(from_str_lossy 仅用于 DB 读侧容错)
    let mode = match body.task_mode.as_deref() {
        None => TaskRunMode::Legacy,
        Some(s) => match TaskRunMode::from_str_strict(s) {
            Some(m) => m,
            None => {
                return validation(format!(
                    "未知任务模式:{s}(可选:legacy/solo/multi/plan/team/custom)"
                ));
            }
        },
    };
    let svc = state.tasks.clone();
    let executor_id = body.executor_id.clone();
    let character_id = body.character_id.clone();
    let flow_id = body.flow_id.clone();
    match state
        .db_call(move || {
            svc.create(
                &body.title,
                executor_id.as_deref(),
                character_id.as_deref(),
                mode,
                flow_id.as_deref(),
            )
        })
        .await
    {
        Err(e) => db_err(&e),
        Ok(Ok(task)) => Json(json!({ "ok": true, "task": task }))
            .into_response()
            .with_status(StatusCode::CREATED),
        Ok(Err(e)) => validation(e),
    }
}

pub async fn get(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let svc = state.tasks.clone();
    match state
        .db_call(move || {
            let found = svc.get_with_subtasks(&id);
            found.map(|(task, subtasks)| {
                let (p, c, r) = svc.usage_total(&task.id);
                // 批次 R2:用户指令历史(followup 追加 / plan_chat 规划对话),
                // created_at 升序;无消息任务为空数组(前端零特判)
                let messages = svc.list_task_messages(&task.id);
                // 二维批次 5a:本任务的流程快照(入口 + 可达子流程闭包)。
                // 只在**详情**下发(列表接口不带——快照是 O(流程库) 体积);
                // 未绑定且未跑过的任务为 None(前端按 optional 容错,零噪音)。
                // 运行态徽标读它而不是「当前流程库」,于是改流程/换当前流程都不再让徽标漂移。
                let flow_snapshot = svc.flow_snapshot(&task.id);
                (task, subtasks, p, c, r, messages, flow_snapshot)
            })
        })
        .await
    {
        Err(e) => db_err(&e),
        Ok(Some((task, subtasks, p, c, r, messages, flow_snapshot))) => Json(json!({
            "task": task,
            "subtasks": subtasks,
            "usage_total": { "prompt_tokens": p, "completion_tokens": c, "reasoning_tokens": r },
            "messages": messages,
            "flow_snapshot": flow_snapshot,
        }))
        .into_response(),
        Ok(None) => not_found("任务不存在"),
    }
}

/// GET /api/tasks/events:任务事件 SSE 流(WP4 任务模式实时化,取代前端轮询)。
/// 订阅 TaskService 的 broadcast 通道并逐条转发为 data 帧(与 /api/chat/send 同款
/// KeepAlive 30s + CACHE_CONTROL no-cache);接收滞后(Lagged)记 warn 后跳过积压
/// 继续,通道关闭(Closed)结束流。
pub async fn events(State(state): State<Arc<AppState>>) -> Response {
    let mut rx = state.tasks.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let json_str = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
                    yield Ok::<Event, Infallible>(Event::default().data(json_str));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    // 滞后意味着前端已丢失事件(面板会短暂与后端不一致),属真实可观测
                    // 异常而非调试噪声,故记 warn;前端依 5s 兜底轮询/重连补拉恢复一致。
                    tracing::warn!(skipped = skipped, "任务事件 SSE 接收滞后,跳过积压事件");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    // 装配单点在 api::util::sse_response(KeepAlive 30s + no-transform),
    // 与 /api/chat/send 共用——两处必须一致,见该函数注释。
    super::sse_response(stream)
}

/// GET /api/tasks/{id}/calls:任务 LLM 调用追踪全量列表(批次 3「调用情况」面板;
/// 面板打开/事件重连时经本端点补拉,实时增量走 kind=llm_call 事件)。
pub async fn list_calls(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let svc = state.tasks.clone();
    match state.db_call(move || svc.list_llm_calls(&id)).await {
        Err(e) => db_err(&e),
        Ok(calls) => Json(json!({ "calls": calls })).into_response(),
    }
}

/// 全部任务 token 累计(侧栏任务模式「全局累计」)。
pub async fn usage_total(State(state): State<Arc<AppState>>) -> Response {
    let svc = state.tasks.clone();
    match state.db_call(move || svc.all_usage_total()).await {
        Err(e) => db_err(&e),
        Ok((p, c, r)) => Json(json!({
            "usage_total": { "prompt_tokens": p, "completion_tokens": c, "reasoning_tokens": r },
        }))
        .into_response(),
    }
}

pub async fn run(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let svc = state.tasks.clone();
    match state.db_call(move || svc.run(&id)).await {
        Err(e) => db_err(&e),
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => validation(e),
    }
}

/// POST /api/tasks/{id}/approve:批准计划(plan 模式特有)。
/// 仅 planned 态合法(其余 400);body 可携修改后 plan(整体替换);
/// `exec_mode` 决定续跑执行方式(缺省 approved_plan = 按计划逐步执行,即改造前行为)。
pub async fn approve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<ApproveTaskBody>,
) -> Response {
    // 严格解析执行方式:未知值 400(与 create 的 task_mode 同款口径,
    // 不用容错回退——批准是显式用户动作,静默回退会让用户以为选了别的模式)
    let exec_mode = match body.exec_mode.as_deref() {
        None => TaskApproveExecMode::ApprovedPlan,
        Some(s) => match TaskApproveExecMode::from_str_strict(s) {
            Some(m) => m,
            None => {
                return validation(format!(
                    "未知执行方式:{s}(可选:approved_plan/solo/multi/team/custom)"
                ));
            }
        },
    };
    let svc = state.tasks.clone();
    match state
        .db_call(move || svc.approve(&id, body.plan, exec_mode))
        .await
    {
        Err(e) => db_err(&e),
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => validation(e),
    }
}

/// POST /api/tasks/{id}/followup:终态任务追加指令(批次 R2a;R2b+ 扩 mode)。
/// 校验顺序:空指令 400(VALIDATION,先于状态门禁)→ 非法 mode 400 → 不存在
/// 404(NOT_FOUND)→ 非终态(running/planning/planned/pending)409(CONFLICT)→
/// service 层复核(预检后状态竞态变化,同样 409)。
/// body.mode:append(缺省/空,历史行为,产出追加进 result)|replace(产出整体替换
/// result,用于「压缩/重写/改前面」类指令);未知值 400。
/// 成功 200:用户指令已落 task_messages 并 spawn solo 续跑,前端经
/// status 事件(running → done/partial)感知生命周期,详情重拉携带 messages。
pub async fn followup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<FollowupTaskBody>,
) -> Response {
    let content = body.content.trim().to_string();
    if content.is_empty() {
        return err_with_code(
            ErrorCode::Validation,
            "追加指令不能为空",
            StatusCode::BAD_REQUEST,
        );
    }
    // mode 严格解析:缺省/空 = append(旧客户端零变化);未知值 400(与 task_mode 同口径)
    let mode = match body.mode.as_deref() {
        None | Some("") => TaskFollowupMode::Append,
        Some(s) => match TaskFollowupMode::from_str_strict(s) {
            Some(m) => m,
            None => {
                return err_with_code(
                    ErrorCode::Validation,
                    format!("未知追加模式:{s}(可选:append/replace)"),
                    StatusCode::BAD_REQUEST,
                )
            }
        },
    };
    let svc = state.tasks.clone();
    let id_probe = id.clone();
    let found = match state.db_call(move || svc.get(&id_probe)).await {
        Err(e) => return db_err(&e),
        Ok(t) => t,
    };
    let Some(task) = found else {
        return err_with_code(ErrorCode::NotFound, "任务不存在", StatusCode::NOT_FOUND);
    };
    if !matches!(
        task.status,
        TaskStatus::Done | TaskStatus::Partial | TaskStatus::Error | TaskStatus::Ended
    ) {
        return err_with_code(
            ErrorCode::Conflict,
            format!(
                "仅终态任务可追加指令(当前:{});执行中请等待或 stop,待批准请走批准/规划对话",
                task.status.as_str()
            ),
            StatusCode::CONFLICT,
        );
    }
    let svc = state.tasks.clone();
    match state
        .db_call(move || svc.followup(&id, &content, mode))
        .await
    {
        Err(e) => db_err(&e),
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        // 复核失败(竞态:预检通过后状态被 stop/重跑改变)按 409 语义返回
        Ok(Err(e)) => err_with_code(ErrorCode::Conflict, e, StatusCode::CONFLICT),
    }
}

/// POST /api/tasks/{id}/plan-chat:批准环节规划对话(批次 R2b)。
/// 仅 planned 态合法;同步等待规划器修订(LLM 调用,数秒~数十秒),
/// 响应携带修订后计划。校验顺序:空反馈 400(VALIDATION)→ 不存在
/// 404(NOT_FOUND)→ 非 planned 409(CONFLICT);LLM 上游失败 502(UPSTREAM);
/// 修订期间 stop 竞态中断按 409 返回。
/// 成功后前端经重发的 plan + approval_required 事件刷新批准区与对话记录。
pub async fn plan_chat(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<PlanChatBody>,
) -> Response {
    let message = body.message.trim().to_string();
    if message.is_empty() {
        return err_with_code(
            ErrorCode::Validation,
            "反馈内容不能为空",
            StatusCode::BAD_REQUEST,
        );
    }
    let svc = state.tasks.clone();
    let id_probe = id.clone();
    let found = match state.db_call(move || svc.get(&id_probe)).await {
        Err(e) => return db_err(&e),
        Ok(t) => t,
    };
    let Some(task) = found else {
        return err_with_code(ErrorCode::NotFound, "任务不存在", StatusCode::NOT_FOUND);
    };
    if task.status != TaskStatus::Planned {
        return err_with_code(
            ErrorCode::Conflict,
            format!(
                "仅待批准(planned)状态的任务可进行规划对话(当前:{})",
                task.status.as_str()
            ),
            StatusCode::CONFLICT,
        );
    }
    // 同步等待规划修订:planned 态无其他执行;plan_chat 内部经 register_cancel
    // 登记,stop 可中断;不经 db_call(主体是 async LLM 调用,非阻塞 DB 闭包)
    match state.tasks.plan_chat(&id, &message).await {
        Ok(steps) => Json(json!({ "ok": true, "plan": steps })).into_response(),
        // 状态类失败(stop 竞态中断 / 复核门禁)按 409;LLM 上游/解析失败按 502
        Err(e) if e.contains("任务已停止") || e.contains("仅待批准") => {
            err_with_code(ErrorCode::Conflict, e, StatusCode::CONFLICT)
        }
        Err(e) => err_with_code(ErrorCode::Upstream, e, StatusCode::BAD_GATEWAY),
    }
}

pub async fn stop(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let svc = state.tasks.clone();
    match state.db_call(move || svc.stop(&id)).await {
        Err(e) => db_err(&e),
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => not_found("任务不存在"),
    }
}

pub async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let svc = state.tasks.clone();
    match state.db_call(move || svc.delete(&id)).await {
        Err(e) => db_err(&e),
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found("任务不存在"),
    }
}
