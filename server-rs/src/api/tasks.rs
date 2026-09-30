// 任务模式路由:/api/tasks(列表/新建/详情/执行/批准/停止/删除/事件 SSE 流)
use crate::api::app_state::AppState;
use crate::api::json_body::JsonBody;
use crate::api::workspace::validate_workspace;
use crate::api::{
    conflict, db_err, err_with_code, internal, not_found, validation, ErrorCode, WithStatus,
};
use crate::models::types::{
    TaskApproveExecMode, TaskFollowupMode, TaskRunMode, TaskStatus, TaskStep,
};
use axum::extract::{Path, Query, State};
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
    /// **对比模式的可调用流程名单**(二维批次 7b;仅 custom 模式可用)。
    ///
    /// 非空即「根流程照常执行 + 名单内流程作为 `run_flow` 工具释放给宽松节点」;
    /// 给了空数组(含全空白项)→ 400(空名单 = 名存实亡,要跑强制模式就别带这个键);
    /// 非 custom 给了 → 400。成员须存在且启用,成员结构不合法同样 400(逐项点名)。
    #[serde(default)]
    pub flow_ids: Option<Vec<String>>,
    /// **任务级连接**(A 批 B1;缺省 = 跟随设置的默认连接)。
    ///
    /// 语义:该任务**所有** LLM 调用的缺省连接(指向 `settings.json` 的 `connections[].id`),
    /// 与 `task_mode` 无关(绑的是 provider,不是编排);节点级 `PlanStep.connection_id`
    /// 仍然优先。创建期校验「存在且启用」,失败 400 并点名连接——任务不可跨机搬运,
    /// 故不像流程库那样把引用校验推迟到运行期。运行期引用失效即明确报错,不静默回退。
    #[serde(default)]
    pub connection_id: Option<String>,
    /// **绑定的工作区**(编码通道批次 1;缺省/空串 = 不绑定,旧客户端零变化)。
    ///
    /// 非空时必须是**已存在的目录**:创建期 canonicalize 后**冻结**落库(此后目录被移走
    /// 或删除,任务会以明确错误终止,不静默降级)。工作区是 `fs_*` 工具族的路径闸门根,
    /// 也是 `bash` 的 cwd 缺省与 jail 边界。与数据目录的双向包含关系一律拒绝
    /// (不许把工作区指进数据目录,也不许让数据目录落在工作区内)。
    #[serde(default)]
    pub workspace: Option<String>,
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

/// 校验创建请求里的工作区并**冻结**为规范化绝对路径(编码通道批次 1)。
///
/// 2026-09-30(CODE-4):实现与单测已搬到 `api::workspace`——探测端点与创建期必须共用
/// 同一把尺,故它不再是 tasks.rs 的私有函数;此处保留调用点,不复制比较逻辑。
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
    let flow_ids = body.flow_ids.clone();
    let connection_id = body.connection_id.clone();
    // 工作区(编码通道批次 1):创建期校验 + canonicalize 冻结;数据目录包含关系双向拒绝
    let workspace = match validate_workspace(body.workspace.as_deref(), &state.config.data_dir) {
        Ok(w) => w,
        Err(e) => return validation(e),
    };
    match state
        .db_call(move || {
            svc.create(
                &body.title,
                executor_id.as_deref(),
                character_id.as_deref(),
                mode,
                flow_id.as_deref(),
                flow_ids.as_deref(),
                connection_id.as_deref(),
                workspace.as_deref(),
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

/// POST /api/tasks/{id}/bind:改绑编排(A 批 B3)——全量替换 `flow_id` / `flow_ids`
/// 并**重新冻结**快照(改绑即重冻结;历史 plan 行不动)。
///
/// 体语义:缺省/`null` 的 `flow_id` = 跟随当前流程(等价于解绑);`flow_ids: []` = 强制模式
/// (清空名单)。两者都是**全量替换**——不设「缺键 = 不改」的第三种含义,避免歧义。
#[derive(Deserialize)]
pub struct BindTaskBody {
    #[serde(default)]
    pub flow_id: Option<String>,
    #[serde(default)]
    pub flow_ids: Vec<String>,
}

/// 改绑编排:仅 custom 模式、且非 planning/running/planned 态(进行中改绑会撕裂本轮快照)。
pub async fn bind(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<BindTaskBody>,
) -> Response {
    let svc = state.tasks.clone();
    match state
        .db_call(move || svc.bind(&id, body.flow_id.as_deref(), &body.flow_ids))
        .await
    {
        Err(e) => db_err(&e),
        // 不存在 → 404(与 followup 同款:服务层只报语义错误,「查不到」由 API 定状态码)
        Ok(Ok(None)) => not_found("任务不存在"),
        Ok(Ok(Some(task))) => Json(json!({ "ok": true, "task": task })).into_response(),
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
                // 工作区画像(CODE-4,2026-09-30):详情读取时**实时**探测,不落库——
                // 目录内容会变,落库就与「创建期冻结」的 workspace 语义混淆。
                // 未绑定工作区 → None(前端不渲染该行,零 IO)。
                let workspace_profile = task.workspace.as_deref().map(|ws| {
                    crate::services::workspace_profile::probe(std::path::Path::new(ws))
                });
                (task, subtasks, p, c, r, messages, flow_snapshot, workspace_profile)
            })
        })
        .await
    {
        Err(e) => db_err(&e),
        Ok(Some((task, subtasks, p, c, r, messages, flow_snapshot, workspace_profile))) => Json(json!({
            "task": task,
            "subtasks": subtasks,
            "usage_total": { "prompt_tokens": p, "completion_tokens": c, "reasoning_tokens": r },
            "messages": messages,
            "flow_snapshot": flow_snapshot,
            "workspace_profile": workspace_profile,
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

// ==================== 文件变更台账(2026-09-30 批次 4,PRODCAP-4「交付可审计」)====================

/// `?path=` 查询体(工作区内**相对路径**;绝对路径与越界由 `safe_workspace_path` 拒绝)。
#[derive(Deserialize)]
pub struct ChangePathQuery {
    pub path: String,
}

/// GET /api/tasks/{id}/changes:任务文件变更清单(按发生顺序)。
///
/// **空清单返回空数组**,端点不猜语义 —— 「本轮没改文件」与「没记账」由前端显式区分展示。
/// **扫描缺项(`undected`)是顶层字段,不是清单里的行**(2026-09-30 批次 4b):
/// bash 侧树扫描没扫完时记一条 `task_scan_marks`,这里聚合成本字段下发——
/// 缺项不是「一个文件改动」,混进 `changes` 会同时污染清单长度语义与回滚定位。
/// 本批不发 `file_changed` 事件(实时推送留待 PRODCAP-1 的 `task_events` 回放落地,
/// 理由见 `计划.md` PRODCAP-4 条目):前端在打开详情与任务进终态时各拉一次。
pub async fn file_changes(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let db = state.db.clone();
    let list_id = id.clone();
    match state
        .db_call(move || crate::services::task_change_service::list(&db, &list_id))
        .await
    {
        Err(e) => db_err(&e),
        Ok(changes) => {
            let mark_db = state.db.clone();
            let undected_reason = state
                .db_call(move || {
                    crate::services::task_change_service::latest_scan_mark(&mark_db, &id)
                })
                .await
                .ok()
                .flatten();
            Json(json!({
                "changes": changes,
                "undected": undected_reason.is_some(),
                "undected_reason": undected_reason,
            }))
            .into_response()
        }
    }
}

/// 把 `?path=` 解析成工作区内绝对路径(与工具侧同一个闸门,不搞第二份路径策略)。
/// 返回 `(绝对路径, 任务是否进行中)`;闸门拒绝时返回端点可直接下发的 400 响应。
fn resolve_change_path(
    state: &Arc<AppState>,
    id: &str,
    raw: &str,
) -> Result<std::path::PathBuf, Box<Response>> {
    let Some(task) = state.tasks.get(id) else {
        return Err(Box::new(not_found("任务不存在")));
    };
    let Some(workspace) = task.workspace.as_ref() else {
        return Err(Box::new(validation(
            "本任务未绑定工作区,没有可比对的文件变更基线",
        )));
    };
    let data_dir = state.config.data_dir.clone();
    match crate::tools::workspace_guard::safe_workspace_path(
        std::path::Path::new(workspace),
        raw,
        Some(&data_dir),
    ) {
        Ok(p) => Ok(p),
        // 闸门拒绝是**用户可纠正**的输入问题(越界/符号链接/NUL),走 400 + VALIDATION,
        // 原文里不含本机绝对路径(闸门文案只回显相对路径与原因)
        Err(e) => Err(Box::new(validation(e))),
    }
}

/// GET /api/tasks/{id}/changes/diff?path=:单文件 unified diff(改动前基线 vs 当前正文)。
///
/// 三种「拿不到基线」的形态**都返回可读说明,不返回空 diff**(空 diff 会被前端和
/// 用户一起读成「没改动」,而那恰恰是本端点要区分掉的情形):
/// - 该文件不在台账里 → `available:false` + 原因;
/// - 新建 → `available:true` + note,正文全部按新增行给出(改动前本就没有该文件);
/// - 基线超留存上限(256KB)或当时读取失败 → `available:false` + 「基线不可用」。
pub async fn file_change_diff(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<ChangePathQuery>,
) -> Response {
    let abs = match resolve_change_path(&state, &id, &q.path) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    use crate::services::task_change_service as tcs;
    let baseline = tcs::baseline_state(&state.db, &id, &q.path);
    // 当前正文:文件可能已被后续步骤删除,按空处理(于是 diff 全是删除行,仍然可读)
    let after = std::fs::read(&abs)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    match baseline {
        tcs::BaselineState::Unavailable(reason) => Json(json!({
            "available": false,
            "reason": reason,
            "path": q.path,
        }))
        .into_response(),
        tcs::BaselineState::NewFile => Json(json!({
            "available": true,
            "path": q.path,
            "note": "改动前不存在该文件(本次为新建),以下按全量新增行给出",
            "diff": tcs::unified_diff("", &after, &q.path),
        }))
        .into_response(),
        tcs::BaselineState::Ready(bytes) => {
            let before = String::from_utf8_lossy(&bytes).into_owned();
            Json(json!({
                "available": true,
                "path": q.path,
                "diff": tcs::unified_diff(&before, &after, &q.path),
            }))
            .into_response()
        }
    }
}

/// POST /api/tasks/{id}/changes/rollback?path=:把单个文件恢复到它**本任务内最近一次改动前**
/// 的状态(台账 Q4 取 (a))。
///
/// 三条纪律:
/// 1. **基线不可用就不动手**(`available:false` + 原因),绝不「恢复成空文件」装作案发现场;
/// 2. 若该条记录是 `create`,回滚 = 删除该文件(而不是写空文件);
/// 3. **回滚本身再记一条 `op=rollback`** —— 它同样是一次改动,必须在清单里看得见、不隐身。
///
/// 任务进行中拒绝:模型还在写同一个文件时回滚会互相覆盖,那种竞态不靠「尽力而为」兜。
pub async fn file_change_rollback(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<ChangePathQuery>,
) -> Response {
    let abs = match resolve_change_path(&state, &id, &q.path) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let task_running = state
        .tasks
        .get(&id)
        .map(|t| {
            matches!(
                t.status,
                crate::models::types::TaskStatus::Running
                    | crate::models::types::TaskStatus::Planning
            )
        })
        .unwrap_or(false);
    if task_running {
        return conflict("任务进行中,请先停止再回滚(避免与模型写入互相覆盖)");
    }
    use crate::services::task_change_service as tcs;
    let outcome = tcs::rollback(&state.db, &id, &q.path, &abs);
    match outcome {
        tcs::RollbackOutcome::Restored(bytes) => Json(json!({
            "ok": true,
            "path": q.path,
            "restored_bytes": bytes,
            "note": "已恢复到改动前状态,并另记一条 rollback 变更",
        }))
        .into_response(),
        tcs::RollbackOutcome::Removed => Json(json!({
            "ok": true,
            "path": q.path,
            "removed": true,
            "note": "该文件由本任务新建,回滚即删除",
        }))
        .into_response(),
        tcs::RollbackOutcome::Unavailable(reason) => Json(json!({
            "ok": false,
            "available": false,
            "path": q.path,
            "reason": reason,
        }))
        .into_response(),
        // 落盘失败原文只进日志(可能含本机绝对路径),响应给固定文案
        tcs::RollbackOutcome::Failed(e) => internal(e),
    }
}

/// POST /api/tasks/{id}/changes/rollback-all:整任务回滚(CODE-2;计划 Q2=(a))。
///
/// 对**本任务台账里的每个路径**逐项执行与单文件端点完全相同的语义:
/// 取该路径最新一行;create → 删除;无基线 → 跳过(不动手)。三条纪律:
/// 1. **逐项报告、不整体否决**:某一项无基线/落盘失败不拦住其余项(`results` 逐项列出);
/// 2. running/planning → 409 **整单拒绝**(与单文件同口径:避免与模型写入互相覆盖);
/// 3. 每项成功都再记 `op=rollback`(复用单文件同一核心函数,不写第二份)。
///
/// 可逆性:对同一任务再调一次 = 逐项切回「改动后」状态(取最新一行的自然结果,非新语义)。
pub async fn file_change_rollback_all(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let Some(task) = state.tasks.get(&id) else {
        return not_found("任务不存在");
    };
    let Some(workspace) = task.workspace.clone() else {
        return validation("本任务未绑定工作区,没有可比对的文件变更基线");
    };
    if matches!(task.status, TaskStatus::Running | TaskStatus::Planning) {
        return conflict("任务进行中,请先停止再回滚(避免与模型写入互相覆盖)");
    }
    let data_dir = state.config.data_dir.clone();
    let db = state.db.clone();
    let all_id = id.clone();
    match state
        .db_call(move || {
            let root = std::path::Path::new(&workspace);
            crate::services::task_change_service::rollback_all(&db, &all_id, |rel| {
                crate::tools::workspace_guard::safe_workspace_path(root, rel, Some(&data_dir))
            })
        })
        .await
    {
        Err(e) => db_err(&e),
        Ok(items) => {
            use crate::services::task_change_service::BulkRollbackResult as R;
            let (mut restored, mut removed, mut skipped, mut failed) =
                (0usize, 0usize, 0usize, 0usize);
            let results: Vec<serde_json::Value> = items
                .iter()
                .map(|it| match &it.result {
                    R::Restored(bytes) => {
                        restored += 1;
                        json!({ "path": it.rel_path, "result": "restored", "restored_bytes": bytes })
                    }
                    R::Removed => {
                        removed += 1;
                        json!({ "path": it.rel_path, "result": "removed", "note": "该文件由本任务新建,回滚即删除" })
                    }
                    R::Skipped(reason) => {
                        skipped += 1;
                        json!({ "path": it.rel_path, "result": "skipped", "reason": reason })
                    }
                    // 落盘失败原文可能含本机路径 → 只进日志,响应给固定文案(与单文件同口径)
                    R::Failed(e) => {
                        failed += 1;
                        tracing::warn!(path = %it.rel_path, error = %e, "整任务回滚:单项落盘失败");
                        json!({ "path": it.rel_path, "result": "failed", "reason": "落盘失败,详情见日志" })
                    }
                })
                .collect();
            Json(json!({
                "ok": failed == 0,
                "restored": restored,
                "removed": removed,
                "skipped": skipped,
                "failed": failed,
                "results": results,
            }))
            .into_response()
        }
    }
}

/// 组装整任务 patch 正文(纯函数,可单测;由 patch 端点调用)。
///
/// 由**可 diff 行**逐文件拼装:修改 = `diff --git` + 头 + 单 hunk(`git_hunk`,严格计数);
/// 新建 = `new file mode` + `--- /dev/null` + 全量新增行。拿不到基线的(超留存上限/
/// 读取失败/已删除)与「改动过大 / 无逐行变化 / 路径含空格或引号」不进正文,
/// 统一进头部**注释区**列明——半截 patch 在 git 眼里是坏补丁,而静默丢掉的文件
/// 在执行者眼里又会被读成「没改过」,两头都不能接受。
fn build_task_patch(
    db: &Arc<crate::models::db::Db>,
    task_id: &str,
    workspace_root: &std::path::Path,
    data_dir: &std::path::Path,
) -> String {
    use crate::services::task_change_service as tcs;
    use std::fmt::Write as _;
    let mut included = String::new();
    let mut notes: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in tcs::list(db, task_id) {
        if !seen.insert(row.path.clone()) {
            continue;
        }
        let rel = row.path.clone();
        // 路径含空格/引号时 git 补丁需要 C 风格引用,本批不处理 → 明说未包含,
        // 不产出一个会被 `git apply` 拒绝的坏条目
        if rel.contains(' ') || rel.contains('"') {
            notes.push(format!("{rel}(路径含空格或引号,patch 暂不包含)"));
            continue;
        }
        let abs = match crate::tools::workspace_guard::safe_workspace_path(
            workspace_root,
            &rel,
            Some(data_dir),
        ) {
            Ok(p) => p,
            Err(e) => {
                notes.push(format!("{rel}(路径被工作区闸门拒绝:{e})"));
                continue;
            }
        };
        match tcs::baseline_state(db, task_id, &rel) {
            tcs::BaselineState::Unavailable(reason) => notes.push(format!("{rel}({reason})")),
            tcs::BaselineState::NewFile => {
                let after = std::fs::read(&abs)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                let lines: Vec<&str> = after.lines().collect();
                let _ = writeln!(included, "diff --git a/{rel} b/{rel}");
                let _ = writeln!(included, "new file mode 100644");
                let _ = writeln!(included, "--- /dev/null");
                let _ = writeln!(included, "+++ b/{rel}");
                let _ = writeln!(included, "@@ -0,0 +1,{} @@", lines.len());
                for l in lines {
                    let _ = writeln!(included, "+{l}");
                }
            }
            tcs::BaselineState::Ready(bytes) => {
                let before = String::from_utf8_lossy(&bytes).into_owned();
                let after = std::fs::read(&abs)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                match tcs::git_hunk(&before, &after) {
                    Ok(hunk) => {
                        let _ = writeln!(included, "diff --git a/{rel} b/{rel}");
                        let _ = writeln!(included, "--- a/{rel}");
                        let _ = writeln!(included, "+++ b/{rel}");
                        included.push_str(&hunk);
                    }
                    Err(reason) => notes.push(format!("{rel}({reason})")),
                }
            }
        }
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Kedai 任务变更 patch(自动生成;可用 `git apply` 应用)"
    );
    if !notes.is_empty() {
        let _ = writeln!(out, "# 未包含 {} 个文件:", notes.len());
        for n in &notes {
            let _ = writeln!(out, "#   - {n}");
        }
    }
    out.push_str(&included);
    out
}

/// GET /api/tasks/{id}/changes/patch:整任务 unified patch(CODE-2)。
///
/// 文本响应(`text/plain`),由可 diff 行拼成 **git 可 apply** 的补丁(组装规则见
/// `build_task_patch`)。前端取回走文本通道(不套 JSON 形状闸门),经保存对话框导出。
/// 诚实边界:按行文本生成(行尾统一 LF),不含 `\ No newline at end of file` 处理;
/// 空台账 → 只有头部注释的文本(不是 404、也不是空响应——那两者会被读成别的意思)。
pub async fn file_changes_patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let Some(task) = state.tasks.get(&id) else {
        return not_found("任务不存在");
    };
    let Some(workspace) = task.workspace.clone() else {
        return validation("本任务未绑定工作区,没有可比对的文件变更基线");
    };
    let data_dir = state.config.data_dir.clone();
    let db = state.db.clone();
    let patch_id = id.clone();
    match state
        .db_call(move || {
            build_task_patch(&db, &patch_id, std::path::Path::new(&workspace), &data_dir)
        })
        .await
    {
        Err(e) => db_err(&e),
        Ok(body) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            )],
            body,
        )
            .into_response(),
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
