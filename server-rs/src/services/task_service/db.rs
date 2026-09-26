// 任务主表/子任务表读写:行映射(row_to_task/row_to_subtask)、状态写入
// (reset_task/set_status/set_plan/set_result/set_error)、usage 落库(record_usage)、
// 子任务 CRUD(create_subtask/set_subtask_status/list_subtasks)。
// 自 task_service.rs 拆分迁入,纯代码移动,逻辑不变;依赖经 `use super::*` 取自 mod.rs。
use super::*;
use crate::services::task_core::assemble_from_plan;

/// 孤儿任务启动恢复(问题④,2026-08-31 实测:服务进程被停后,任务永远停在
/// running 态):上次进程退出时遗留在 running/planning 的任务,执行上下文已随
/// 进程消亡、永不再推进,启动时统一置 ended 终态 + error 文本「服务重启,任务中断」。
/// planned 不动:计划已产出待批准,approve 续跑语义可跨重启存活;
/// pending(未启动)与终态不动。
/// **部分成果兜底(提交 2)**:置 ended 时若该任务已完成若干步骤,把它们的产出拼装
/// 写回 result(与 finalize_run 的 ended 分支同一口径:状态与错误文本不变,只补一个
/// 可看的成果列,不伪造成果——无已完成步骤的任务 result 保持原样)。
/// 返回被恢复任务的 id 列表(供 TaskService 在 DB 写成功后逐个发射 status 事件);
/// 幂等:重复执行零命中(状态已是 ended,不再匹配 IN 条件)。
pub(super) fn recover_orphan_tasks(db: &Db) -> Vec<String> {
    let conn = db.write();
    let rows: Vec<(String, String)> = match conn
        .prepare_cached("SELECT id, plan FROM tasks WHERE status IN ('running', 'planning') ORDER BY created_at ASC, rowid ASC")
        .map(|mut stmt| {
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
        }) {
        Ok(Ok(rows)) => rows,
        Ok(Err(e)) => {
            tracing::warn!(error = e.to_string(), "孤儿任务恢复查询失败");
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!(error = e.to_string(), "孤儿任务恢复 prepare 失败");
            return Vec::new();
        }
    };
    let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    if ids.is_empty() {
        return ids;
    }
    let changed = conn
        .execute(
            "UPDATE tasks SET status = 'ended', error = '服务重启,任务中断', updated_at = ?1 WHERE status IN ('running', 'planning')",
            params![now_iso()],
        )
        .unwrap_or(0);
    if changed > 0 {
        tracing::info!(
            count = changed,
            "服务启动:遗留执行中任务已标记中断(孤儿恢复)"
        );
    }
    // 部分成果兜底:只在 result 尚为空时写入(WHERE result = '' 保证不覆盖既有成果)
    let mut salvaged = 0usize;
    for (id, plan_str) in &rows {
        let Ok(plan) = serde_json::from_str::<Vec<TaskStep>>(plan_str) else {
            continue;
        };
        let Some(text) = assemble_from_plan(&plan) else {
            continue;
        };
        if conn
            .execute(
                "UPDATE tasks SET result = ?1, updated_at = ?2 WHERE id = ?3 AND result = ''",
                params![text, now_iso(), id],
            )
            .unwrap_or(0)
            > 0
        {
            salvaged += 1;
        }
    }
    if salvaged > 0 {
        tracing::info!(
            count = salvaged,
            "服务启动:中断任务已补写已完成步骤的产出(部分成果兜底)"
        );
    }
    ids
}

/// 任务主表列:0 id, 1 title, 2 status, 3 plan, 4 result, 5 error, 6 character_id,
/// 7 created_at, 8 updated_at, 9 task_mode, 10 executor_id, 11 flow_id, 12 flow_ids,
/// 13 connection_id, 14 workspace (新增列一律追加在表尾,与 TASK_COLS 及建表顺序一致)
///
/// **有意不含 flow_snapshot**:它是 O(流程库) 体积的 JSON,而 TASK_COLS 被列表与详情
/// 每次事件刷新都用;要读快照走 [`TaskService::flow_snapshot`](本文件的单列查询)。
pub(super) fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<TaskRecord> {
    let plan_str: String = row.get(3)?;
    let plan = serde_json::from_str(&plan_str).unwrap_or_default();
    let status: String = row.get(2)?;
    let task_mode: String = row.get(9)?;
    // 对比模式名单(二维批次 7b):JSON 数组列,读取容错为空(None)——「列有脏数据」
    // 不是「任务跑不起来」的理由,与 status/task_mode 的 from_str_lossy 同款约定。
    let flow_ids_raw: Option<String> = row.get(12)?;
    let flow_ids = flow_ids_raw
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .filter(|v| !v.is_empty());
    Ok(TaskRecord {
        id: row.get(0)?,
        title: row.get(1)?,
        // DB 读取容错:未知值记 warn 回退 Pending,不 panic 不丢行
        status: TaskStatus::from_str_lossy(&status),
        plan,
        result: row.get(4)?,
        error: row.get(5)?,
        character_id: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        // DB 读取容错:未知模式记 warn 回退 Legacy(与 status 同款约定)
        task_mode: TaskRunMode::from_str_lossy(&task_mode),
        executor_id: row.get(10)?,
        flow_id: row.get(11)?,
        flow_ids,
        // 任务级连接(A 批 B1):纯 id 列,读取不做容错——引用失效由运行期按 5b 口径
        // **明确报错**(不静默回退默认连接),与 flow_ids 的 JSON 解析容错不同源。
        connection_id: row.get(13)?,
        // 工作区(编码通道批次):创建期冻结的 canonical 绝对路径;NULL/空串都视为未绑定
        // (空串只会来自手改 DB,一并归一为 None,免得下游把空路径当「绑定了一个根目录」)
        workspace: row
            .get::<_, Option<String>>(14)?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    })
}

pub(super) const TASK_COLS: &str = "id, title, status, plan, result, error, character_id, \
     created_at, updated_at, task_mode, executor_id, flow_id, flow_ids, connection_id, workspace";

/// 按字符截断(中文安全,不切 char 边界;调用追踪摘要用)
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 任务消息行映射(批次 R2):role/kind 原样透传(CHECK 约束由建表保证;
/// 读取侧宽容,旧行默认 kind='normal' 由迁移 DEFAULT 兜底)。
fn row_to_task_message(row: &rusqlite::Row) -> rusqlite::Result<TaskMessageRecord> {
    Ok(TaskMessageRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        role: row.get(2)?,
        kind: row.get(3)?,
        content: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn row_to_subtask(row: &rusqlite::Row) -> rusqlite::Result<TaskSubtaskRecord> {
    let status: String = row.get(4)?;
    Ok(TaskSubtaskRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        name: row.get(2)?,
        instruction: row.get(3)?,
        // DB 读取容错:未知值记 warn 回退 Pending,不 panic 不丢行
        status: TaskSubtaskStatus::from_str_lossy(&status),
        result: row.get(5)?,
        error: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        finished_at: row.get(9)?,
    })
}

impl TaskService {
    /// 同步落库统一让出点(2026-09-16 性能批次 P-6)。
    ///
    /// 任务引擎经 `tokio::spawn` 跑在 async worker 上,而这些方法做的是同步 rusqlite
    /// 写(唯一写连接 + busy_timeout 最长 5s + fsync),直接执行会卡住整个 worker,
    /// 连带该 worker 上排队的其他请求一起停摆。语义与判定规则见 `utils::blocking`。
    /// 全部写方法都经此包装,是「任务侧与聊天侧落库纪律一致」的单点保证
    /// (聊天侧同类写入早用 spawn_blocking,见 engine/run_finish.rs:94)。
    ///
    /// 注意:`record_self_heals` 不包——它不直接取写锁,而是委托给下方已包装的
    /// `record_llm_call`/`record_usage`;包了只会形成多余的嵌套。
    #[inline]
    fn blocking<T>(f: impl FnOnce() -> T) -> T {
        crate::utils::blocking::park_worker(f)
    }

    // ===== 状态写入(内部) =====

    pub(super) fn reset_task(&self, id: &str) -> bool {
        Self::blocking(|| {
            let conn = self.db.write();
            let changed = conn
                .execute(
                    "UPDATE tasks SET status = 'planning', plan = '[]', result = '', error = '', updated_at = ?1 WHERE id = ?2",
                    params![now_iso(), id],
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            if changed {
                // 重跑入口即进入规划态,与 set_status 同款 kind="status" 事件
                self.emit_event(
                    TaskEventKind::Status,
                    id,
                    None,
                    Some(TaskStatus::Planning),
                    Some("任务进入规划阶段".into()),
                );
            }
            changed
        })
    }

    /// pub(crate):任务引擎(task_engine)solo/plan 执行器推进任务状态用。
    pub(crate) fn set_status(&self, id: &str, status: TaskStatus) -> bool {
        Self::blocking(|| {
            let conn = self.db.write();
            let changed = conn
                .execute(
                    "UPDATE tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
                    params![status.as_str(), now_iso(), id],
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            if changed {
                self.emit_event(
                    TaskEventKind::Status,
                    id,
                    None,
                    Some(status),
                    Some(format!("任务状态更新为 {}", status.as_str())),
                );
            }
            changed
        })
    }

    /// pub(crate):任务引擎 plan 执行器落库计划用(写库成功后发射 kind=plan 事件)。
    pub(crate) fn set_plan(&self, id: &str, plan: &[TaskStep]) -> bool {
        Self::blocking(|| {
            let json = serde_json::to_string(plan).unwrap_or_else(|_| "[]".into());
            let conn = self.db.write();
            let changed = conn
                .execute(
                    "UPDATE tasks SET plan = ?1, updated_at = ?2 WHERE id = ?3",
                    params![json, now_iso(), id],
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            if changed {
                self.emit_event(
                    TaskEventKind::Plan,
                    id,
                    None,
                    None,
                    Some(format!("执行计划已更新(共 {} 步)", plan.len())),
                );
            }
            changed
        })
    }

    /// 记录本任务**本轮实际使用的流程编排**(二维批次 5a):绑定任务在创建时已冻结,
    /// 未绑定任务在**执行开始时**由执行器回写(重跑按新的当前流程覆写,语义与旧版一致)。
    ///
    /// 有意**不发事件**:快照是任务行的元数据(徽标/追溯的数据源),前端在详情加载时
    /// 读取即可——为它新增一种 SSE kind 要三处同步,收益为零。
    /// 落盘失败记 error 日志并返回 false(不阻断任务执行:快照缺失只影响徽标)。
    pub(crate) fn set_flow_snapshot(&self, id: &str, snapshot: &FlowSnapshot) -> bool {
        let json = match serde_json::to_string(snapshot) {
            Ok(text) => text,
            Err(e) => {
                tracing::error!(task_id = id, error = e.to_string(), "流程快照序列化失败");
                return false;
            }
        };
        let changed = self
            .db
            .write()
            .execute(
                "UPDATE tasks SET flow_snapshot = ?1, updated_at = ?2 WHERE id = ?3",
                params![json, now_iso(), id],
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !changed {
            tracing::error!(task_id = id, "流程快照写入未命中任何任务行");
        }
        changed
    }

    /// 读任务的流程快照(二维批次 5a;未绑定且未跑过 = None)。
    ///
    /// 单列查询(不进 `TASK_COLS`,见其文档):快照是 O(流程库) 体积,列表接口
    /// 每次事件刷新都要用,不能顺带拉它。损坏的快照记 warn 后按「无快照」处理——
    /// 详情接口不该因为一列脏数据整条失败。
    pub fn flow_snapshot(&self, id: &str) -> Option<FlowSnapshot> {
        let raw: Option<String> = self
            .db
            .read()
            .ok()?
            .query_row(
                "SELECT flow_snapshot FROM tasks WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let raw = raw?;
        match serde_json::from_str::<FlowSnapshot>(&raw) {
            Ok(snapshot) => Some(snapshot),
            Err(e) => {
                tracing::warn!(task_id = id, error = e.to_string(), "流程快照解析失败");
                None
            }
        }
    }

    /// 写入最终结果并置终态:全部步骤成功为 done;含 error 步骤为 partial(部分完成)。
    pub(super) fn set_result(&self, id: &str, result: &str, status: TaskStatus) -> bool {
        self.set_result_with_error(id, result, status, None)
    }

    /// set_result 的可解释版本:partial 终态可携带原因文本写入 tasks.error(供前端
    /// 状态行展示「为什么不是完成」);error 为 None 时清空旧 error(重跑成功后不残留
    /// 上一轮的错误提示)。其余语义与 set_result 一致。
    pub(super) fn set_result_with_error(
        &self,
        id: &str,
        result: &str,
        status: TaskStatus,
        error: Option<&str>,
    ) -> bool {
        Self::blocking(|| {
            let conn = self.db.write();
            let changed = conn
                .execute(
                    "UPDATE tasks SET result = ?1, status = ?2, error = ?3, updated_at = ?4 WHERE id = ?5",
                    params![
                        result,
                        status.as_str(),
                        error.unwrap_or(""),
                        now_iso(),
                        id
                    ],
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            drop(conn);
            if changed {
                self.emit_event(
                    TaskEventKind::Status,
                    id,
                    None,
                    Some(status),
                    Some("任务已产出最终结果".into()),
                );
            }
            changed
        })
    }

    /// planned 态写入计划清单文本(批次 R1,plan 模式):只更新 result 列,不动状态
    /// (planned 终态由 PlanExecutor 先行写入,本方法随其后)。
    /// planned 态 result 的语义是「待批准的计划清单」:供用户在批准前预览完整计划;
    /// 批准续跑完成后由终态 result(**汇总文本**,提交 2 起不再拼「## 最终计划」段)
    /// 整体覆盖。
    /// 写库成功后发射既有 kind="status" 事件(detail 与终态 set_result 文案区分;
    /// 不引入新事件 kind;WP4 纪律:仅 DB 写成功后发射)。
    pub(crate) fn set_planned_result(&self, id: &str, result: &str) -> bool {
        Self::blocking(|| {
            let conn = self.db.write();
            let changed = conn
                .execute(
                    "UPDATE tasks SET result = ?1, updated_at = ?2 WHERE id = ?3",
                    params![result, now_iso(), id],
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            if changed {
                self.emit_event(
                    TaskEventKind::Status,
                    id,
                    None,
                    Some(TaskStatus::Planned),
                    Some("计划清单已写入,待批准".into()),
                );
            }
            changed
        })
    }

    /// 兜底写 result(提交 2 部分成果兜底,用于取消 ended / 失败 error / 重启中断三类
    /// 「任务未完成但已有已完成步骤」的收尾):**只更新 result 列**,不动 status 与
    /// error——终态与错误文本由调用方既有的 set_status / set_error 决定,本方法只为
    /// 「已完成步骤的产出」留一个可见出口,不改变任何终态语义。
    /// 写库成功后发射既有 kind="status" 事件(detail 与终态文案区分;不引入新事件 kind;
    /// WP4 纪律:仅 DB 写成功后发射)。
    pub(super) fn set_result_only(&self, id: &str, result: &str) -> bool {
        Self::blocking(|| {
            let conn = self.db.write();
            let changed = conn
                .execute(
                    "UPDATE tasks SET result = ?1, updated_at = ?2 WHERE id = ?3",
                    params![result, now_iso(), id],
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            drop(conn);
            if changed {
                self.emit_event(
                    TaskEventKind::Status,
                    id,
                    None,
                    None,
                    Some("已保存已完成步骤的产出(任务未完成)".into()),
                );
            }
            changed
        })
    }

    /// 清空 result(提交 2,plan 批准续跑入口用):planned 态写入的计划清单是「待批准预览」,
    /// 批准后即过期——留着会让续跑的失败/取消兜底(判据是「result 为空才补写」)整条跳过,
    /// 成果卡里就只剩一份计划被当成「已完成部分的成果」。
    /// 只更新 result 列,不动 status/error;**不发事件**:调用点(approve)紧随其后就会写
    /// status(planning)/plan,前端由那些事件刷新,多一帧无信息量的事件反而无益。
    pub(super) fn clear_result(&self, id: &str) -> bool {
        Self::blocking(|| {
            let conn = self.db.write();
            conn.execute(
                "UPDATE tasks SET result = '', updated_at = ?1 WHERE id = ?2",
                params![now_iso(), id],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
        })
    }

    /// 只写 error 列的终态原因(提交 3 · D7,空闲看守用):**不动 status**——
    /// 终态(ended)由 `finish_ended` 里的 set_status 决定,本方法只为「为什么结束」留痕
    /// (与 `set_result_only` 同形:一个只写 result、一个只写 error,都不串终态)。
    /// 为什么不用 `set_error`:那个会把状态置成 error,而空闲收尾的语义是**正常收尾**
    /// (与用户点 stop 同源),status 必须是 ended。
    /// 「ended + 非空 error」不是新形态:`recover_orphan_tasks` 的重启中断行就是这么落的。
    /// 写库成功后发既有 kind="status" 事件(status 传 None,细节在 detail),不引新 kind。
    pub(super) fn set_error_only(&self, id: &str, error: &str) -> bool {
        Self::blocking(|| {
            let conn = self.db.write();
            let changed = conn
                .execute(
                    "UPDATE tasks SET error = ?1, updated_at = ?2 WHERE id = ?3",
                    params![error, now_iso(), id],
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            drop(conn);
            if changed {
                // detail 截断防超长文本撑大事件帧(口径同 set_error)
                let detail: String = error.chars().take(120).collect();
                self.emit_event(TaskEventKind::Status, id, None, None, Some(detail));
            }
            changed
        })
    }

    pub(super) fn set_error(&self, id: &str, error: &str) -> bool {
        Self::blocking(|| {
            let conn = self.db.write();
            let changed = conn
                .execute(
                    "UPDATE tasks SET error = ?1, status = 'error', updated_at = ?2 WHERE id = ?3",
                    params![error, now_iso(), id],
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            if changed {
                // detail 截断防超长错误文本撑大事件帧
                let detail: String = error.chars().take(120).collect();
                self.emit_event(
                    TaskEventKind::Status,
                    id,
                    None,
                    Some(TaskStatus::Error),
                    Some(detail),
                );
            }
            changed
        })
    }

    /// 运行中(或规划中)任务的**最近活动时刻**(提交 3 · D7):返回
    /// `(任务 id, 最近活动 ISO 时间戳)`。
    ///
    /// 口径:`COALESCE(MAX(task_llm_calls.created_at), tasks.updated_at)`——
    /// 有调用行就用最近一行,没有(刚进 running、或在长工具循环里还没落行)就回退
    /// `tasks.updated_at`(状态写入时刻)。注意本表两条口径都**不是**实时活动信号:
    /// solo/plan/team 的调用行是整轮工具循环结束后才落(`solo.rs` 的统一出口),
    /// 所以看守必须叠加进程内心跳(`TaskService::activity`),本查询只服务
    /// 「没有心跳的任务」(重启前遗留、测试直插行)——判据合成见 `idle.rs`。
    ///
    /// 只看 running/planning:planned 是「等用户批准」的合法静止态,end/pending 不在看守范围。
    pub(super) fn running_tasks_activity(&self) -> Vec<(String, String)> {
        Self::blocking(|| {
            let Ok(conn) = self.db.read() else {
                tracing::warn!("查询运行中任务失败:数据库读连接不可用");
                return Vec::new();
            };
            let Ok(mut stmt) = conn.prepare(
                "SELECT t.id, COALESCE((SELECT MAX(c.created_at) FROM task_llm_calls c \
                 WHERE c.task_id = t.id), t.updated_at) \
                 FROM tasks t WHERE t.status IN ('running','planning')",
            ) else {
                tracing::warn!("查询运行中任务失败:语句准备失败");
                return Vec::new();
            };
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
            match rows {
                Ok(iter) => iter.filter_map(Result::ok).collect(),
                Err(e) => {
                    tracing::warn!(error = %e, "读取运行中任务活动时刻失败");
                    Vec::new()
                }
            }
        })
    }

    // ===== 任务 usage(WP4)=====

    /// 任务模式 usage 落库:每次 LLM 调用(规划/步骤/汇总)写一行 task_usage。
    /// model 取合并设置后的有效模型(与设置页口径一致;启动日志 env model 可能失真,见 WP6)。
    /// pub(crate):任务引擎(task_engine)六模式执行器统一经本出口落 usage(批次 4.3b;
    /// phase 对齐 legacy 口径:planner/agent/subagent/step/audit/summary,面板按 task_id 求和)。
    pub(crate) fn record_usage(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        out: &TaskGenOutput,
    ) {
        Self::blocking(|| {
            let model = self.task_settings().model;
            let conn = self.db.write();
            let result = conn.execute(
                "INSERT INTO task_usage (id, task_id, phase, step_index, model, prompt_tokens, completion_tokens, reasoning_tokens, created_at)              VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    Uuid::new_v4().to_string(),
                    task_id,
                    phase,
                    step_index.map(|i| i as i64),
                    model,
                    out.prompt_tokens,
                    out.completion_tokens,
                    out.reasoning_tokens,
                    now_iso(),
                ],
            );
            if let Err(e) = result {
                tracing::warn!(
                    op = "task_usage insert",
                    error = e.to_string(),
                    "任务 usage 落库失败"
                );
            } else {
                self.emit_event(
                    TaskEventKind::Usage,
                    task_id,
                    None,
                    None,
                    Some(format!(
                        "已记录 {phase} 阶段 token 用量(prompt {} / completion {})",
                        out.prompt_tokens, out.completion_tokens
                    )),
                );
            }
        })
    }

    // ===== 任务 LLM 调用追踪(批次 3)=====

    /// 任务侧每次 LLM 调用落一行 task_llm_calls(提示词/响应摘要 + 耗时 + 状态);
    /// 写库成功后发射 kind=llm_call 事件(WP4 纪律:仅 DB 写入成功后发射)。
    /// out 为 None 表示调用失败(超时/上游错误),token 记 0。
    /// finish_reason(可观测性问题①):取自 TaskGenOutput(流式 Finish 块聚合),
    /// None/空 = 未知,落库为 ''(旧行兼容);事件同步携带('' 省略字段)。
    /// 摘要截断:每条消息 role + 正文截 800 字符,整体 4000;响应截 2000。
    /// pub(crate):任务引擎(task_engine)solo 等执行器同样经本统一出口落调用追踪。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_llm_call(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        model: &str,
        messages: &[LlmMessage],
        response: &str,
        out: Option<&TaskGenOutput>,
        elapsed: Duration,
        status: &str,
    ) {
        Self::blocking(|| {
            let mut prompt_summary = String::new();
            for m in messages {
                if !prompt_summary.is_empty() {
                    prompt_summary.push('\n');
                }
                prompt_summary.push_str(&m.role);
                prompt_summary.push_str(": ");
                prompt_summary.push_str(&truncate_chars(&m.content, 800));
            }
            let (p, c, r) = out
                .map(|o| (o.prompt_tokens, o.completion_tokens, o.reasoning_tokens))
                .unwrap_or((0, 0, 0));
            // finish_reason:None(失败/工具循环旧路径/上游未下发)与 Some(空串)统一落 ''
            let finish_reason = out.and_then(|o| o.finish_reason.as_deref()).unwrap_or("");
            let conn = self.db.write();
            let result = conn.execute(
                "INSERT INTO task_llm_calls (id, task_id, phase, step_index, model, prompt_summary, response_summary, prompt_tokens, completion_tokens, reasoning_tokens, elapsed_ms, status, created_at, finish_reason) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    Uuid::new_v4().to_string(),
                    task_id,
                    phase,
                    step_index.map(|i| i as i64),
                    model,
                    truncate_chars(&prompt_summary, 4000),
                    truncate_chars(response, 2000),
                    p,
                    c,
                    r,
                    elapsed.as_millis() as i64,
                    status,
                    now_iso(),
                    finish_reason,
                ],
            );
            if let Err(e) = result {
                tracing::warn!(
                    op = "task_llm_calls insert",
                    error = e.to_string(),
                    "任务 LLM 调用追踪落库失败"
                );
            } else {
                let step = step_index
                    .map(|i| format!(" #{}", i + 1))
                    .unwrap_or_default();
                // phase/step_index 随事件透出(批次 R4):前端据此清对应流式缓冲
                self.emit_llm_call(
                    task_id,
                    phase,
                    step_index,
                    format!("{phase}{step} · {model} · {} tokens", p + c),
                    Some(finish_reason.to_string()),
                );
            }
        })
    }

    /// 截断自愈留痕批量落库(问题①的单一实现):solo.rs 与 custom.rs 各自复用过
    /// 一段逐行同构的循环,现收拢于此——被截断的那次调用补落一行 status=error
    /// (response_summary 标注触发原因与重发预算),并补落其 usage。
    ///
    /// 为何要补 usage:被截断那次同样消耗 token,只记最终行会让 usage_total
    /// 少于调用明细求和(2026-09-10 实测修复口径)。
    /// 调用情况面板据此看到完整「截断 → 提高预算重发」链路,实际调用次数可考。
    ///
    /// 批次 4.2:入参为 task_core 中性 DTO `TruncationHeal`(不再引用 agents 层
    /// SelfHealRecord),转换在 task_engine 边界完成(见 task_engine::executor::to_self_heals)。
    pub(crate) fn record_self_heals(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        model: &str,
        messages: &[LlmMessage],
        self_heals: &[crate::services::task_core::TruncationHeal],
    ) {
        for heal in self_heals {
            let heal_out = TaskGenOutput {
                text: String::new(),
                finish_reason: heal.finish_reason.clone(),
                prompt_tokens: heal.prompt_tokens,
                completion_tokens: heal.completion_tokens,
                reasoning_tokens: 0,
                reasoning_chars: 0,
                tool_calls: Vec::new(),
            };
            self.record_llm_call(
                task_id,
                phase,
                step_index,
                model,
                messages,
                &format!(
                    "(截断自愈){},输出上限翻倍至 {} 重发",
                    heal.note, heal.retried_max_tokens
                ),
                Some(&heal_out),
                Duration::ZERO,
                "error",
            );
            self.record_usage(task_id, phase, step_index, &heal_out);
        }
    }

    /// 调用追踪全量拉取(「调用情况」面板打开/事件重连时补拉),按发生顺序升序。
    pub(crate) fn list_llm_calls(&self, task_id: &str) -> Vec<TaskLlmCallRecord> {
        let Some(conn) = read_or_log(&self.db, "任务调用追踪 read") else {
            return Vec::new();
        };
        let mut stmt = match conn.prepare_cached(
            "SELECT id, task_id, phase, step_index, model, prompt_summary, response_summary, prompt_tokens, completion_tokens, reasoning_tokens, elapsed_ms, status, created_at, finish_reason \
                 FROM task_llm_calls WHERE task_id = ?1 ORDER BY created_at ASC, rowid ASC",
        ) {
            Ok(s) => s,
            Err(e) => return log_query_failure("任务调用追踪 prepare", e),
        };
        let query = stmt.query_map(params![task_id], |row| {
            Ok(TaskLlmCallRecord {
                id: row.get(0)?,
                task_id: row.get(1)?,
                phase: row.get(2)?,
                step_index: row.get(3)?,
                model: row.get(4)?,
                prompt_summary: row.get(5)?,
                response_summary: row.get(6)?,
                prompt_tokens: row.get(7)?,
                completion_tokens: row.get(8)?,
                reasoning_tokens: row.get(9)?,
                elapsed_ms: row.get(10)?,
                status: row.get(11)?,
                created_at: row.get(12)?,
                // 列由迁移保证存在(NOT NULL DEFAULT '');旧行读出 '' = 未知
                finish_reason: row.get(13)?,
            })
        });
        match query {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => log_query_failure("任务调用追踪 query_map", e),
        }
    }

    // ===== 任务消息(批次 R2:followup 追加指令 / plan_chat 规划对话) =====

    /// 任务消息落库:用户追加指令/规划对话的双方发言持久化(task_messages 表)。
    /// 落库成功即返回记录;不发射事件——消息增量经 status/plan 事件驱动前端
    /// 重拉详情(GET 响应携带 messages 全量)对齐,不引入新事件 kind。
    pub(crate) fn add_task_message(
        &self,
        task_id: &str,
        role: &str,
        kind: &str,
        content: &str,
    ) -> Result<TaskMessageRecord, String> {
        Self::blocking(|| {
            let record = TaskMessageRecord {
                id: Uuid::new_v4().to_string(),
                task_id: task_id.to_string(),
                role: role.to_string(),
                kind: kind.to_string(),
                content: content.to_string(),
                created_at: now_iso(),
            };
            let conn = self.db.write();
            conn.execute(
                "INSERT INTO task_messages (id, task_id, role, kind, content, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    record.id,
                    record.task_id,
                    record.role,
                    record.kind,
                    record.content,
                    record.created_at
                ],
            )
            .map_err(|e| format!("任务消息落库失败: {e}"))?;
            Ok(record)
        })
    }

    /// 任务消息全量列表(详情响应 messages 字段),按发生顺序升序
    /// (created_at 同毫秒时 rowid 兜底,与调用追踪同口径)。
    pub(crate) fn list_task_messages(&self, task_id: &str) -> Vec<TaskMessageRecord> {
        let Some(conn) = read_or_log(&self.db, "任务消息 read") else {
            return Vec::new();
        };
        let mut stmt = match conn.prepare_cached(
            "SELECT id, task_id, role, kind, content, created_at \
                 FROM task_messages WHERE task_id = ?1 ORDER BY created_at ASC, rowid ASC",
        ) {
            Ok(s) => s,
            Err(e) => return log_query_failure("任务消息 prepare", e),
        };
        // query_map 结果先绑定局部变量再 match(与 list_llm_calls 同口径):
        // 尾表达式直接 match 会触发 E0597(MappedRows 借用 stmt 的临时值 drop 顺序)
        let query = stmt.query_map(params![task_id], row_to_task_message);
        match query {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => log_query_failure("任务消息 query_map", e),
        }
    }

    /// 第 N 次追加的序号:kind=followup 的 user 消息计数。
    /// 约定在用户指令落库之后调用,返回值含本条在内(首轮追加 = 1)。
    pub(crate) fn followup_count(&self, task_id: &str) -> usize {
        let Some(conn) = read_or_log(&self.db, "追加计数 read") else {
            return 0;
        };
        conn.query_row(
            "SELECT COUNT(*) FROM task_messages WHERE task_id = ?1 AND kind = 'followup' AND role = 'user'",
            params![task_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n as usize)
        .unwrap_or(0)
    }

    // ===== 子任务 =====

    pub(crate) fn create_subtask(
        &self,
        task_id: &str,
        name: &str,
        instruction: &str,
    ) -> Result<String, String> {
        Self::blocking(|| {
            let id = Uuid::new_v4().to_string();
            let now = now_iso();
            let conn = self.db.write();
            conn.execute(
                "INSERT INTO task_subtasks (id, task_id, name, instruction, status, result, error, created_at, updated_at, finished_at) \
                 VALUES (?1, ?2, ?3, ?4, 'running', '', '', ?5, ?5, '')",
                params![id, task_id, name, instruction, now],
            )
            .map_err(|e| format!("创建子任务失败: {e}"))?;
            self.emit_event(
                TaskEventKind::Subtask,
                task_id,
                None,
                None,
                Some(format!("子任务「{name}」开始执行")),
            );
            Ok(id)
        })
    }

    pub(crate) fn set_subtask_status(
        &self,
        id: &str,
        status: TaskSubtaskStatus,
        result: Option<&str>,
        error: Option<&str>,
    ) -> bool {
        Self::blocking(|| {
            let conn = self.db.write();
            // 保持未提供字段不变:先读旧值(顺带取 task_id 供事件发射,避免额外查库)
            // finished_at 一并读出:终态只记首次(见下),不能无条件 now_iso() 覆盖
            let existing = conn
                .query_row(
                    "SELECT task_id, result, error, finished_at FROM task_subtasks WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .ok()
                .flatten();
            let (task_id, res, err, finished) = existing
                .map(|(t, r, e, f)| (Some(t), r, e, f))
                .unwrap_or((None, String::new(), String::new(), String::new()));
            let res = result.map(|s| s.to_string()).unwrap_or(res);
            let err = error.map(|s| s.to_string()).unwrap_or(err);
            // 首次进入终态才记 finished_at;pending/running 保持空串,重复置终态不改写首值
            let finished_at = if status.is_terminal() && finished.is_empty() {
                now_iso()
            } else {
                finished
            };
            let changed = conn
                .execute(
                    "UPDATE task_subtasks SET status = ?1, result = ?2, error = ?3, updated_at = ?4, finished_at = ?5 WHERE id = ?6",
                    params![status.as_str(), res, err, now_iso(), finished_at, id],
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            if changed {
                if let Some(tid) = task_id {
                    self.emit_event(
                        TaskEventKind::Subtask,
                        &tid,
                        None,
                        None,
                        Some(format!("子任务状态更新为 {}", status.as_str())),
                    );
                }
            }
            changed
        })
    }

    /// 子任务列表(读路径合并,可观测性问题⑤):DB(task_subtasks 表,legacy
    /// 逐步执行的子任务) + agent_subtask_service 内存覆盖层(multi/team 经 agentgo
    /// 排出的子 agent,task:{id} / task:{id}:main:{n} 虚拟 session 记录,批次 4.3b
    /// 起因 FK 守卫不落 agent_subtasks 表)。同一 id 覆盖层优先;合并后按
    /// created_at 升序(+id 字典序兜底,与同毫秒创建的团队子 agent 排序稳定)。
    /// 既定取舍:覆盖层随进程存活,重启后内存记录丢失,subtasks 仅剩 DB 行可见
    ///(multi/team 子 agent 为进程内执行单元,重启即中断,无持久化价值)。
    pub(super) fn list_subtasks(&self, task_id: &str) -> Vec<TaskSubtaskRecord> {
        // 覆盖层记录先收(task: 前缀虚拟 session 全部子 agent);
        // AgentSubtaskRecord → TaskSubtaskRecord 形状映射(status 走容错解析)
        let overlay: Vec<TaskSubtaskRecord> = self
            .agent_subtasks
            .list_by_session_prefix(&format!("task:{task_id}"))
            .into_iter()
            .map(|r| TaskSubtaskRecord {
                id: r.id,
                task_id: task_id.to_string(),
                name: r.name,
                instruction: r.instruction,
                status: TaskSubtaskStatus::from_str_lossy(&r.status),
                result: r.result,
                error: r.error,
                created_at: r.created_at,
                updated_at: r.updated_at,
                finished_at: r.finished_at,
            })
            .collect();
        let overlay_ids: std::collections::HashSet<String> =
            overlay.iter().map(|r| r.id.clone()).collect();

        let mut out = overlay;
        let Some(conn) = read_or_log(&self.db, "子任务列表 read") else {
            return out;
        };
        let mut stmt = match conn.prepare_cached(
            "SELECT id, task_id, name, instruction, status, result, error, created_at, updated_at, finished_at \
                 FROM task_subtasks WHERE task_id = ?1 ORDER BY created_at ASC",
        ) {
            Ok(s) => s,
            Err(e) => return log_query_failure("子任务列表 prepare", e),
        };
        let query = stmt.query_map(params![task_id], row_to_subtask);
        match query {
            Ok(rows) => {
                // 同一 id 覆盖层优先:DB 行与覆盖层冲突时丢弃 DB 行
                out.extend(
                    rows.filter_map(|r| r.ok())
                        .filter(|r| !overlay_ids.contains(r.id.as_str())),
                );
            }
            Err(e) => return log_query_failure("子任务列表 query_map", e),
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    /// 孤儿任务启动恢复(问题④):遗留 running/planning 置 ended + error 文本
    /// 「服务重启,任务中断」;planned(待批准可续跑)/pending/终态不动;
    /// 幂等(重复执行零命中);模拟「重开」(drop 后重新 open 同一库文件)后
    /// 再插一行 running 仍能恢复——恢复语义跨进程成立。
    /// 提交 2 追加:中断任务若已完成若干步骤,产出补写进 result(部分成果兜底);
    /// 无已完成产出不伪造,已有 result 不覆盖。
    #[test]
    fn recover_orphan_tasks_marks_interrupted_once() {
        let dir = TempDataDir::new("task-recover");
        let db_path = dir.join("kedai.db");
        {
            let db = Db::open(&db_path, &dir).expect("开库失败");
            {
                let conn = db.write();
                for (id, status) in [
                    ("t-running", "running"),
                    ("t-planning", "planning"),
                    ("t-planned", "planned"),
                    ("t-pending", "pending"),
                    ("t-done", "done"),
                ] {
                    conn.execute(
                        "INSERT INTO tasks (id, title, status, created_at, updated_at) \
                         VALUES (?1, '目标', ?2, 'c', 'u')",
                        params![id, status],
                    )
                    .unwrap();
                }
                // 部分成果兜底(提交 2)三态:有已完成步骤 → 补写拼装成果;只有失败/
                // 未开始步骤 → 保持空(不伪造);已有 result → 不覆盖
                let done_plan =
                    r#"[{"name":"步骤一","goal":"g","status":"done","result":"已完成产出"}]"#;
                let empty_plan =
                    r#"[{"name":"步骤一","goal":"g","status":"error","result":"失败原因"}]"#;
                for (id, plan) in [("t-salvage", done_plan), ("t-noplan", empty_plan)] {
                    conn.execute(
                        "INSERT INTO tasks (id, title, status, plan, created_at, updated_at) \
                         VALUES (?1, '目标', 'running', ?2, 'c', 'u')",
                        params![id, plan],
                    )
                    .unwrap();
                }
                conn.execute(
                    "INSERT INTO tasks (id, title, status, plan, result, created_at, updated_at) \
                     VALUES ('t-keep', '目标', 'running', ?1, '既有成果', 'c', 'u')",
                    params![done_plan],
                )
                .unwrap();
            }
            let mut ids = recover_orphan_tasks(&db);
            ids.sort();
            assert_eq!(
                ids,
                vec![
                    "t-keep".to_string(),
                    "t-noplan".to_string(),
                    "t-planning".to_string(),
                    "t-running".to_string(),
                    "t-salvage".to_string()
                ],
                "running/planning 应被恢复"
            );
            let result_of = |db: &Db, id: &str| -> String {
                db.read()
                    .unwrap()
                    .query_row("SELECT result FROM tasks WHERE id = ?1", params![id], |r| {
                        r.get(0)
                    })
                    .unwrap()
            };
            let salvaged = result_of(&db, "t-salvage");
            assert!(
                salvaged.contains("## 步骤一") && salvaged.contains("已完成产出"),
                "重启中断的任务应补写已完成步骤的产出: {salvaged}"
            );
            assert_eq!(result_of(&db, "t-noplan"), "", "无已完成产出时不得伪造成果");
            assert_eq!(
                result_of(&db, "t-keep"),
                "既有成果",
                "已有 result 不得被兜底覆盖"
            );
            let status_error_of = |db: &Db, id: &str| -> (String, String) {
                db.read()
                    .unwrap()
                    .query_row(
                        "SELECT status, error FROM tasks WHERE id = ?1",
                        params![id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap()
            };
            let (st, err) = status_error_of(&db, "t-running");
            assert_eq!(st, "ended", "running 应置 ended");
            assert!(err.contains("服务重启"), "error 文本应标注中断原因: {err}");
            assert_eq!(status_error_of(&db, "t-planning").0, "ended");
            // planned/pending/done 不动
            assert_eq!(
                status_error_of(&db, "t-planned").0,
                "planned",
                "planned 待批准可跨重启续跑,不动"
            );
            assert_eq!(status_error_of(&db, "t-pending").0, "pending");
            assert_eq!(status_error_of(&db, "t-done").0, "done");
            // 幂等:第二次执行零命中
            assert!(recover_orphan_tasks(&db).is_empty(), "重复执行应零命中");
        }
        // 重开(模拟服务重启):同一库文件再 open,既有行不重复恢复;
        // 新插一行 running → 恢复 → 已终结(「插一行 running → 重开 → 断言已终结」)
        let db2 = Db::open(&db_path, &dir).expect("重开库失败");
        assert!(
            recover_orphan_tasks(&db2).is_empty(),
            "重开后不应重复恢复既有行"
        );
        db2.write()
            .execute(
                "INSERT INTO tasks (id, title, status, created_at, updated_at) \
                 VALUES ('t-orphan', '孤儿', 'running', 'c', 'u')",
                [],
            )
            .unwrap();
        assert_eq!(recover_orphan_tasks(&db2), vec!["t-orphan".to_string()]);
        let (st, err): (String, String) = db2
            .read()
            .unwrap()
            .query_row(
                "SELECT status, error FROM tasks WHERE id = 't-orphan'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(st, "ended", "重开后新孤儿应被终结");
        assert!(err.contains("服务重启"), "error 文本应标注中断原因: {err}");
        drop(db2);
    }
}
