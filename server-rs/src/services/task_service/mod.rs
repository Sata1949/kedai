// 任务模式(task 工作台)服务:任务主表 CRUD + 后台执行引擎。
// 执行流程:planning(LLM 规划拆解)→ running(逐步派子智能体执行)→ done(LLM 汇总);
// 出错 → error;stop → ended。子任务存 task_subtasks 表(独立于 agent_subtasks,
// 因后者的 session_id 外键指向 sessions 表,而任务 id 不在其中);
// multi/team 经 agentgo 排出的子 agent 记录走 agent_subtask_service 的 task: 前缀
// 内存覆盖层(批次 4.3b,FK 守卫),list_subtasks 读路径合并两者(可观测性问题⑤;
// 覆盖层随进程存活,重启后仅 DB 行可见——既定取舍,见 db.rs::list_subtasks)。
// 模块地图(巨型文件拆分后,纯代码移动,逻辑不变):
//   本文件      TaskService 结构体、任务 CRUD、usage 累计
//   db.rs       任务主表/子任务表读写与 usage 落库
//   events.rs   任务事件 broadcast 通道与发射(WP4 SSE 实时化)
//   cancel.rs   取消信号与执行 token 登记
//   prompt.rs   提示词组装(任务设置/世界书/注入/Agent 系统提示词/人设)
//   executor.rs 后台执行引擎(run/stop、LLM 单次生成原语、后台主体)
//   idle.rs     任务级空闲看守(提交 3 · D7:活动心跳判据 + 自动收尾 + 常驻 tick)
use super::log_query_failure;
use crate::models::db::{now_iso, Db, PooledRead};
use crate::models::types::{
    CharacterRecord, GenerationParams, LlmMessage, LlmStreamChunk, SseEvent, TaskEventKind,
    TaskFollowupMode, TaskLlmCallRecord, TaskMessageRecord, TaskRecord, TaskRunMode, TaskStatus,
    TaskStep, TaskSubtaskRecord, TaskSubtaskStatus, ToolCallArgs, ToolChoice, ToolContext,
    ToolDefinition,
};
use crate::services::agent_flow_service::{AgentFlowService, FlowSnapshot};
use crate::services::agent_subtask_service::AgentSubtaskService;
use crate::services::character_service::CharacterService;
use crate::services::settings_service::{connection_label, AppMode, RuntimeSettings};
use crate::services::world_book_service::WorldBookService;
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

// 按职责拆分的子模块(纯代码移动):DB 读写 / 取消信号 / 提示词组装 / 后台执行
pub(crate) mod backend_impl;
pub(crate) mod cancel;
pub(crate) mod db;
pub(crate) mod events;
pub(crate) mod executor;
pub(crate) mod idle;
pub(crate) mod prompt;

use self::db::{row_to_task, TASK_COLS};

/// 任务模式单次 LLM 调用总时长上限(涵盖共享读锁排队 + 整个流式生成)。
/// 超时报错让任务进入 error 终态,防止上游停滞或锁队列饿死导致任务永久停在
/// planning/步骤 running(层内另有 SSE 流空闲看门狗,此为兜底;正常规划/生成
/// 均在数十秒量级,5 分钟上限足够宽裕)。
const TASK_LLM_TOTAL_TIMEOUT: Duration = Duration::from_secs(300);

// 空输出分级重试与规划解析的算法/常量(EMPTY_RETRY_BACKOFF / RETRY_MAX_TOKENS_CAP /
// PLAN_INITIAL_MAX_TOKENS / PLAN_MAX_ATTEMPTS / parse_plan)已于批次 4.2 上移
// task_engine::retry + task_engine::parse(任务引擎职责,非宿主能力)。

/// 规划器只读侦察白名单(问题②,2026-08-31 实测:计划模式下模型只写计划、
/// 不调用工具收集信息,对「测试 agent 框架能力」这类目标只能凭空编造步骤):
/// 规划阶段允许模型先调用只读工具收集与目标相关的信息,再产出计划 JSON。
/// 严禁写操作(write/replace/create/memory_write/update_variables)与编排类
/// (agentgo/agentend)——写操作违背 plan 模式「只规划不执行」零副作用纪律,
/// 编排类会把规划阶段变成实际执行。
///
/// 常量本体在 `tools::tool_sets`(单一出处):无作用域 = `READONLY_SCOUT`;
/// 任务绑定了作用域时由 `tool_sets::scout_tools(true)` 追加工作区只读三件(D5 修复,
/// 见 `plan_scout_loop` 的文档注释)。此处不再留本地别名,避免「改了一处、另一处没改」。
///
/// 规划器侦察轮上限:工具调用最多 2 轮,随后最终轮不带工具强制产出计划 JSON
/// (计划契约不变);侦察是增强环节,轮数封底防模型沉迷收集迟迟不出计划。
const PLANNER_SCOUT_MAX_ROUNDS: usize = 2;

/// 任务模式单次 LLM 调用的完整产出:正文 + 诊断信息(批次 B.3 搬迁至 task_core,
/// 本处再导出保持既有调用方零改动)。
pub(crate) use crate::services::task_core::TaskGenOutput;

pub struct TaskService {
    db: Arc<Db>,
    characters: Arc<CharacterService>,
    /// 任务 scratch 工作区根(任务模式 D1):未绑定工作区的任务在其下按任务 id 建
    /// 子目录,作为执行侧路径闸门根与 bash 缺省 cwd。来源 `AppConfig::task_scratch_dir`
    /// (与数据目录同级;`KEDAI_TASK_SCRATCH_DIR` 可覆盖),只在首次使用时建目录。
    scratch_root: std::path::PathBuf,
    /// 聊天引擎(solo/plan 等六模式执行器复用 execute_generation/run_tool_loop;
    /// 批次 4.2 注入,engine 不依赖 tasks,无循环)。**连接器也从这里取**——二维批次 5b
    /// 起本服务不再自持连接器:默认连接由 `engine.connector` 提供、节点级连接由
    /// `engine.resolve_connector` 单点解析(计划改动点 1:不在 TaskService 另建缓存,
    /// 否则「改设置 → 谁生效」会有两个数据源)。
    engine: Arc<crate::agents::engine::AgentEngine>,
    /// 自定义 Agent 执行流程库(custom 模式读取当前启用流程;批次 4.3b 注入,
    /// 与 AppState 共享同一实例,克隆配置后即释放锁,不持引用跨 .await)
    flow: Arc<Mutex<AgentFlowService>>,
    /// 子智能体任务(stop 时按 task: 前缀结束任务模式子 agent,防孤儿后台任务)
    agent_subtasks: Arc<AgentSubtaskService>,
    /// 运行期设置(读任务模式的生成参数);与 AppState 共享同一实例
    settings: Arc<Mutex<RuntimeSettings>>,
    /// 世界书(注入执行者人设的世界观设定)
    world_books: Arc<WorldBookService>,
    /// 执行者库(任务创建时校验 executor_id 是否存在;执行期按 id 取执行者指令
    /// 注入 system 提示词;与 AppState 共享同一实例)
    executors: Arc<Mutex<crate::services::executor_service::ExecutorService>>,
    /// 任务 id → (取消信号, 本次执行 token)。token 用于区分同一任务的先后执行:
    /// stop 后立即重跑时,旧后台任务退出不得删除/覆盖新任务的取消条目与状态。
    cancels: Mutex<HashMap<String, (watch::Sender<bool>, u64)>>,
    /// 任务 id → **最近活动时刻**(提交 3 · D7,空闲看守的活动心跳)。
    ///
    /// 为什么需要内存心跳而不是只看库:任务侧的 `task_llm_calls` 行是**整轮工具循环
    /// 结束后**才落的(`task_engine/solo.rs` 的统一出口),一次十分钟的工具循环期间库里
    /// 没有任何新行——只看库会把正在干活的任务判成空闲并收掉。心跳在 run 起点登记
    /// (`register_cancel`),并由全部事件发射点刷新(`emit_event` / `emit_llm_call`:
    /// 工具调用、工具结果、步骤、Finish 都经此),故「心跳停了」= 真的什么都没发生。
    /// 表由看守每轮按运行中任务裁剪,不随任务数无限增长。
    activity: Mutex<HashMap<String, Instant>>,
    /// 任务事件广播通道(WP4):DB 写入成功后发射 SseEvent::Task,
    /// GET /api/tasks/events 订阅转发为 SSE
    events: tokio::sync::broadcast::Sender<SseEvent>,
}

impl TaskService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<Db>,
        characters: Arc<CharacterService>,
        settings: Arc<Mutex<RuntimeSettings>>,
        world_books: Arc<WorldBookService>,
        engine: Arc<crate::agents::engine::AgentEngine>,
        flow: Arc<Mutex<AgentFlowService>>,
        agent_subtasks: Arc<AgentSubtaskService>,
        executors: Arc<Mutex<crate::services::executor_service::ExecutorService>>,
        scratch_root: std::path::PathBuf,
    ) -> Self {
        let svc = TaskService {
            db,
            characters,
            scratch_root,
            engine,
            flow,
            agent_subtasks,
            settings,
            world_books,
            executors,
            cancels: Mutex::new(HashMap::new()),
            activity: Mutex::new(HashMap::new()),
            events: tokio::sync::broadcast::channel(events::EVENTS_CAPACITY).0,
        };
        // 孤儿任务启动恢复(问题④):遗留 running/planning 任务置 ended 终态
        // + error 文本;DB 写成功后逐个发射 status 事件(WP4 纪律:先写库后发射)
        for id in db::recover_orphan_tasks(&svc.db) {
            svc.emit_event(
                TaskEventKind::Status,
                &id,
                None,
                Some(TaskStatus::Ended),
                Some("服务重启,任务中断".into()),
            );
        }
        svc
    }

    /// 任务 scratch 目录解析(任务模式 D1):`<scratch_root>/<task_id>`,首次使用时创建。
    ///
    /// 未绑定工作区的任务由 `task_engine::run_inner` 取本目录作为执行作用域(路径闸门根
    /// 与 bash 缺省 cwd),使「产出一份文档」类任务不再把数据目录当草稿纸。
    /// **失败不降级**:建不出来就让任务以明确错误终止——降级回数据目录去写用户真实数据
    /// 是更坏的结果(口径同 run_inner 的「工作区目录失效不降级」)。
    pub(crate) fn scratch_dir_for(&self, task_id: &str) -> Result<std::path::PathBuf, String> {
        // 任务 id 由服务端生成(UUID),此处只做廉价防御:带分隔符/上溯段的 id 绝不拼进路径
        if task_id.is_empty()
            || task_id.contains(['/', '\\'])
            || task_id.contains("..")
            || task_id.contains(':')
        {
            return Err(format!("任务 id 不能作为目录名使用:{task_id}"));
        }
        let dir = self.scratch_root.join(task_id);
        std::fs::create_dir_all(&dir).map_err(|e| {
            format!(
                "任务临时工作区创建失败({}):{e};可用 KEDAI_TASK_SCRATCH_DIR 指定可写目录",
                dir.display()
            )
        })?;
        Ok(dir)
    }

    // ===== CRUD =====

    /// 创建任务。
    ///
    /// 两个执行者入参的兼容关系(2026-09-17 执行者库批次):
    ///   - `executor_id`:独立执行者库 id(前端唯一的绑定入口);
    ///   - `character_id`:**兼容入参**,前端已不再发送,但 API 层面继续接受,避免
    ///     破坏既有客户端。接受后**原样落库**——旧任务语义(世界书按角色过滤 /
    ///     占位符渲染 / `character_prompt` 工具读取)仍以它为准;不做执行者绑定。
    ///     (「执行者人设档位」注入已随 TM-SET-2 退役,本字段不再影响提示词人设段。)
    ///
    /// 两者同时给出时执行期以 executor_id 为准(见 prompt.rs 的分支顺序)。
    ///
    /// `workspace`(编码通道批次)由 API 层**校验并 canonicalize 后**传入,本函数只做
    /// 空白归一(不重复解析文件系统:创建期与运行期的判定口径必须只有一个出处,
    /// 见 `tools::workspace_guard`)。
    #[allow(clippy::too_many_arguments)] // 参数即创建请求体的字段面,拆 struct 只会多一层无人消费的中间类型
    pub fn create(
        &self,
        title: &str,
        executor_id: Option<&str>,
        character_id: Option<&str>,
        mode: TaskRunMode,
        flow_id: Option<&str>,
        flow_ids: Option<&[String]>,
        connection_id: Option<&str>,
        workspace: Option<&str>,
    ) -> Result<TaskRecord, String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("任务目标不能为空".into());
        }
        // 流程绑定(二维批次 5a):只对 custom 模式有意义。其余模式给了就明确拒绝
        // ——「字段存在但静默无效」是本仓反复点名要避免的形态(裁定 15 差异 2)。
        let flow_id = flow_id.map(str::trim).filter(|s| !s.is_empty());
        if flow_id.is_some() && mode != TaskRunMode::Custom {
            return Err("只有自定义流程模式可以绑定流程(flow_id)".into());
        }
        // 对比模式名单(二维批次 7b):口径与 flow_id 逐条对齐——非 custom 给了即 400;
        // custom 下**给了空清单**(含全空白项)也 400:名存实亡的字段不如不传
        // (要跑强制模式就别带 flow_ids,而不是带一个空数组)。
        let flow_ids_raw: Option<Vec<String>> = flow_ids.map(|ids| {
            ids.iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        });
        if flow_ids_raw.is_some() && mode != TaskRunMode::Custom {
            return Err("只有自定义流程模式可以给出可调用流程名单(flow_ids)".into());
        }
        let flow_ids: Option<Vec<String>> = match flow_ids_raw {
            None => None,
            Some(v) if v.is_empty() => {
                return Err(
                    "对比模式的流程名单为空:请至少选择一个流程,或不要传 flow_ids(强制模式)".into(),
                )
            }
            Some(v) => Some(v),
        };
        // 命名用途:名单成员(下面两次校验都要)
        let extras: Vec<String> = flow_ids.clone().unwrap_or_default();
        // 绑定即冻结:创建时按 id 解析一份快照(存在 + 启用 + 结构 + 可达引用链校验),
        // 之后的流程编辑/换当前流程都不影响这个任务。校验在写库**之前**做,失败不留行;
        // 流程锁在此作用域内取用并释放(与下方 db.write 不同时持有,避免锁序纠缠)。
        // 二维批次 7b:快照的**起点集**扩为 {根流程} ∪ 名单,被调流程因此同样在冻结域内;
        // 名单成员严格校验(存在 + 启用 + 结构合法),根流程出现在名单里也在这里被拒。
        let frozen = match flow_id {
            None => {
                // 未绑定根流程:不冻结快照(执行时才捕获),但名单成员仍要**严格**校验
                // ——「字段存在但静默无效」的边界不能因为「跟随当前流程」而放宽
                if !extras.is_empty() {
                    let flow = self.agent_flow();
                    let guard = flow.lock().unwrap_or_else(|e| e.into_inner());
                    guard.validate_members(&extras)?;
                }
                None
            }
            Some(id) => {
                let snap = {
                    let flow = self.agent_flow();
                    let guard = flow.lock().unwrap_or_else(|e| e.into_inner());
                    guard.snapshot_for(id, &extras)?
                };
                Some(serde_json::to_string(&snap).map_err(|e| format!("流程快照序列化失败: {e}"))?)
            }
        };
        let flow_ids_json = match &flow_ids {
            None => None,
            Some(v) => {
                Some(serde_json::to_string(v).map_err(|e| format!("流程名单序列化失败: {e}"))?)
            }
        };
        // 任务级连接(A 批 B1):创建期校验「引用存在且启用」,失败 400 点名连接。
        // 为什么与流程库的「保存期不校验引用」不同:流程可导出/跨机导入,保存期拒绝会把
        // 可移植流程变成本机绑定;任务**不可搬运**(搬运不迁移任务,IFW-11 边界 3),
        // 故即时校验在这里只有好处——用户不必等到跑起来才发现选错了连接。
        // 运行期仍按 5b 口径兜底:引用被删/停用即明确报错,不静默回退默认连接。
        let connection_id = connection_id.map(str::trim).filter(|s| !s.is_empty());
        if let Some(cid) = connection_id {
            let settings = self.task_settings();
            let Some(profile) = settings.connections.iter().find(|p| p.id == cid) else {
                return Err(
                    "选择的连接不存在(可能已被删除);请在设置里恢复该连接,或改用默认连接".into(),
                );
            };
            if !profile.enabled {
                return Err(format!(
                    "选择的连接「{}」已停用;请启用它,或改用默认连接",
                    connection_label(profile)
                ));
            }
        }
        let id = Uuid::new_v4().to_string();
        let now = now_iso();
        // 工作区(编码通道批次):创建期已冻结的 canonical 绝对路径,原样落库;
        // 空白/None = 未绑定(旧客户端不带该字段 → 零变化)
        let workspace = workspace.map(str::trim).filter(|s| !s.is_empty());
        // 执行者库命中校验:引用了不存在的执行者时静默丢弃而非报错——执行者属可选增强,
        // 不该因一次删除让引用它的任务建不出来(与执行期「查不到配置即回退通用执行者」
        // 同口径,避免创建期与执行期语义分叉)。
        let eid = executor_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter(|id| {
                self.executors
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(id)
                    .is_some()
            })
            .map(str::to_string);
        let cid = character_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        {
            // 写锁作用域:INSERT 完成后立即释放——下方 add_task_message 会再次取
            // db.write(),若仍持锁则自死锁(非重入锁)
            let conn = self.db.write();
            conn.execute(
                "INSERT INTO tasks (id, title, status, plan, result, error, character_id, created_at, updated_at, task_mode, executor_id, flow_id, flow_snapshot, flow_ids, connection_id, workspace) \
                 VALUES (?1, ?2, 'pending', '[]', '', '', ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    id,
                    title,
                    cid,
                    now,
                    mode.as_str(),
                    eid,
                    flow_id,
                    frozen,
                    flow_ids_json,
                    connection_id,
                    workspace
                ],
            )
            .map_err(|e| format!("创建任务失败: {e}"))?;
        }
        // 用户目标落 task_messages(role=user,kind=goal):任务模式的对话记录区
        // 按 user/assistant 逐轮气泡呈现,目标作为首条用户发言(实跑问题 1)。
        // 落库失败不阻断创建(消息是展示层增强,任务本身已建好)。
        let _ = self.add_task_message(&id, "user", "goal", title);
        self.emit_event(
            TaskEventKind::Created,
            &id,
            Some(title.to_string()),
            Some(TaskStatus::Pending),
            Some("任务已创建".into()),
        );
        Ok(TaskRecord {
            id,
            title: title.to_string(),
            status: TaskStatus::Pending,
            plan: Vec::new(),
            result: String::new(),
            error: String::new(),
            character_id: cid,
            created_at: now.clone(),
            updated_at: now,
            // 创建入口由调用方(API)严格解析用户所选模式;缺省 legacy(行为与旧版一致)
            task_mode: mode,
            executor_id: eid,
            flow_id: flow_id.map(str::to_string),
            // 对比模式名单(二维批次 7b):原样回带(前端据此显示「可调用 N 个流程」)
            flow_ids,
            // 任务级连接(A 批 B1):原样回带(前端据此显示「本任务用哪条连接」)
            connection_id: connection_id.map(str::to_string),
            // 工作区(编码通道批次):原样回带回冻结值(前端据此显示「已绑定工作区」)
            workspace: workspace.map(str::to_string),
        })
    }

    /// 解析本任务**本轮要跑的流程快照**(二维批次 5a 的单一出处)。
    ///
    /// 规则(任务侧与 approve 前置校验共用同一份,避免两条分叉的判断):
    ///  - **绑定任务**(`flow_id` 有值):读 `tasks.flow_snapshot` 并用
    ///    [`validate_snapshot`](crate::services::agent_flow_service::validate_snapshot)
    ///    校验它**自身**——不看流程库,被冻结的任务不该因为库里被改/被删而失效。
    ///    快照缺失(只可能来自手改 DB)即报错,不静默回退到别的流程。
    ///  - **未绑定任务**(或任务行已消失):按当时的**当前流程**解析(要求存在且启用),
    ///    语义与 5a 之前完全一致。
    pub(crate) fn resolve_task_flow(
        &self,
        task: Option<&TaskRecord>,
    ) -> Result<FlowSnapshot, String> {
        let flow = self.agent_flow();
        let guard = flow.lock().unwrap_or_else(|e| e.into_inner());
        // 对比模式名单(二维批次 7b):未绑定任务走「当时的当前流程 + 名单」,
        // 名单成员此刻按宽松口径解析(见 `current_snapshot` 的说明);绑定任务用冻结快照,
        // 快照里本就含名单闭包,这里取出的 extra_ids 只为保持两个分支入口一致。
        let extra_ids: Vec<String> = task.and_then(|t| t.flow_ids.clone()).unwrap_or_default();
        let Some(flow_id) = task.and_then(|t| t.flow_id.clone()) else {
            return guard.current_snapshot(&extra_ids);
        };
        let task_id = task.map(|t| t.id.as_str()).unwrap_or_default();
        let snapshot = self
            .flow_snapshot(task_id)
            .ok_or_else(|| format!("任务绑定的流程快照缺失(flow_id={flow_id}),无法执行"))?;
        guard.validate_task_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    /// 改绑编排(A 批 B3):**全量替换** `flow_id` / `flow_ids`,并重新冻结快照。
    ///
    /// 口径(见交接稿 R7;撤销原「绑定不可改绑」,理由见 `遗留.md` IFW-8 边界 2 的溯源):
    ///  - 只有 custom 模式任务可改绑(编排绑定对其它模式无意义);
    ///  - `planning` / `running` / `planned` 态**拒绝**——进行中改绑会撕裂本轮快照;
    ///  - 全量替换语义:`flow_id = None` = 跟随当前流程(快照清空,下次执行重新捕获),
    ///    `flow_ids = []` = 强制模式(清空名单);缺键不再有「不改」的第三种含义;
    ///  - 校验与 `create` 逐条同源(存在+启用+结构+可达引用链),**在写库之前**做,
    ///    失败不留痕;历史 plan 行不动(编排徽标按新快照解析,对不上即不显示,IFW-5 口径)。
    pub fn bind(
        &self,
        id: &str,
        flow_id: Option<&str>,
        flow_ids: &[String],
    ) -> Result<Option<TaskRecord>, String> {
        let Some(task) = self.get(id) else {
            // 不存在 → `Ok(None)`(API 层转 404,与 followup 同款;服务层只报语义错误)
            return Ok(None);
        };
        if task.task_mode != TaskRunMode::Custom {
            return Err("只有自定义流程模式可以改绑流程".into());
        }
        if matches!(
            task.status,
            TaskStatus::Planning | TaskStatus::Running | TaskStatus::Planned
        ) {
            return Err(format!(
                "任务正在执行({}),不能改绑流程;请先停止任务",
                task.status.as_str()
            ));
        }
        let flow_id = flow_id.map(str::trim).filter(|s| !s.is_empty());
        let extras: Vec<String> = flow_ids
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        // 快照捕获与成员严格校验:与 create 同一处原语(不得在两侧各写一份判定)
        let frozen = match flow_id {
            None => {
                if !extras.is_empty() {
                    let flow = self.agent_flow();
                    let guard = flow.lock().unwrap_or_else(|e| e.into_inner());
                    guard.validate_members(&extras)?;
                }
                None
            }
            Some(fid) => {
                let snap = {
                    let flow = self.agent_flow();
                    let guard = flow.lock().unwrap_or_else(|e| e.into_inner());
                    guard.snapshot_for(fid, &extras)?
                };
                Some(serde_json::to_string(&snap).map_err(|e| format!("流程快照序列化失败: {e}"))?)
            }
        };
        let flow_ids_json = if extras.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&extras).map_err(|e| format!("流程名单序列化失败: {e}"))?)
        };
        let now = now_iso();
        {
            let conn = self.db.write();
            conn.execute(
                "UPDATE tasks SET flow_id = ?1, flow_snapshot = ?2, flow_ids = ?3, updated_at = ?4 WHERE id = ?5",
                params![flow_id, frozen, flow_ids_json, now, id],
            )
            .map_err(|e| format!("改绑流程失败: {e}"))?;
        }
        self.emit_event(
            TaskEventKind::FlowBound,
            id,
            None,
            Some(task.status),
            Some("编排绑定已更新".into()),
        );
        Ok(self.get(id))
    }

    pub fn list(&self) -> Vec<TaskRecord> {
        let Some(conn) = read_or_log(&self.db, "任务列表 read") else {
            return Vec::new();
        };
        let mut stmt = match conn.prepare_cached(&format!(
            "SELECT {TASK_COLS} FROM tasks ORDER BY created_at DESC"
        )) {
            Ok(s) => s,
            Err(e) => return log_query_failure("任务列表 prepare", e),
        };
        let query = stmt.query_map([], row_to_task);
        match query {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => log_query_failure("任务列表 query_map", e),
        }
    }

    pub fn get(&self, id: &str) -> Option<TaskRecord> {
        self.db
            .read()
            .ok()?
            .query_row(
                &format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"),
                params![id],
                row_to_task,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn get_with_subtasks(&self, id: &str) -> Option<(TaskRecord, Vec<TaskSubtaskRecord>)> {
        let task = self.get(id)?;
        let subtasks = self.list_subtasks(id);
        Some((task, subtasks))
    }

    /// 删除任务(子任务经外键 ON DELETE CASCADE 一并删除);运行中则先取消。
    pub fn delete(&self, id: &str) -> bool {
        self.signal_cancel(id);
        self.remove_cancel_entry(id);
        let deleted = self
            .db
            .write()
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])
            .map(|n| n > 0)
            .unwrap_or(false);
        if deleted {
            self.emit_event(
                TaskEventKind::Deleted,
                id,
                None,
                None,
                Some("任务已删除".into()),
            );
        }
        deleted
    }

    /// 单任务 token 累计(prompt / completion / reasoning)。
    pub fn usage_total(&self, task_id: &str) -> (i64, i64, i64) {
        self.db
            .read()
            .ok()
            .and_then(|conn| {
                conn.query_row(
                    "SELECT COALESCE(SUM(prompt_tokens),0), COALESCE(SUM(completion_tokens),0), COALESCE(SUM(reasoning_tokens),0)                      FROM task_usage WHERE task_id = ?1",
                    params![task_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .ok()
                .flatten()
            })
            .unwrap_or((0, 0, 0))
    }

    /// 自定义流程库句柄(custom 执行器读取当前启用配置用;批次 4.3b)。
    /// 调用方须 lock 后立即克隆配置并释放 guard,禁持引用跨 .await。
    pub(crate) fn agent_flow(&self) -> Arc<Mutex<AgentFlowService>> {
        self.flow.clone()
    }

    /// 全部任务的 token 累计(侧栏「全局累计」)。
    pub fn all_usage_total(&self) -> (i64, i64, i64) {
        self.db
            .read()
            .ok()
            .and_then(|conn| {
                conn.query_row(
                    "SELECT COALESCE(SUM(prompt_tokens),0), COALESCE(SUM(completion_tokens),0), COALESCE(SUM(reasoning_tokens),0)                      FROM task_usage",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .ok()
                .flatten()
            })
            .unwrap_or((0, 0, 0))
    }
}

/// 只读连接获取失败兜底(与 log_query_failure 同款语义,2026-08 裸 unwrap 审计纪律):
/// 记 warn 并返回 None,调用方回退空列表。Db::read 的错误为 String(连接池层),
/// 与 log_query_failure 的 rusqlite::Error 不同源,故单列本 helper。
fn read_or_log(db: &Db, op: &str) -> Option<PooledRead> {
    match db.read() {
        Ok(conn) => Some(conn),
        Err(e) => {
            tracing::warn!(op = op, error = e, "DB 只读连接获取失败,回退空列表");
            None
        }
    }
}
