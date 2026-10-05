// 任务事件广播(WP4 任务模式实时化):TaskService 内嵌 broadcast 通道,
// 各 DB 写入方法落库成功后发射 SseEvent::Task(kind = created/status/plan/
// subtask/usage/deleted/llm_call/agent_status/approval_required/delta/
// flow_bound/file_changed),GET /api/tasks/events 订阅本通道并转发为 SSE,
// 取代前端 1s REST 轮询。无订阅者时 send 返回 Err,属正常,一律忽略。
// 批次 R4:新增 kind=delta 流式增量(emit_delta + DeltaBatcher 攒批),
// 与 llm_call 事件同步携带 phase/step_index 供前端对齐流式缓冲。
// PRODCAP-1(2026-10-05):事件持久化 —— 除 delta(暂态)外全部落 `task_events` 表并携带
// seq/at,支持断点补拉与 SSE 回放。落库与广播**分成两个函数**:持写连接的调用点先
// `persist_event(&conn, …)`(与事实表写在**同一事务**内)再 `broadcast_event(…)`;
// 不持连接的调用点(引擎侧)用 `emit_event`(内部短取连接 → persist → 广播)。
use super::*;
// 批次 B.3 依赖倒置:DeltaBatcher 本体已机械搬迁至 task_core::delta
// (使 task_engine 侧可直接引用而不经 task_service),此处仅引入返回类型。
use crate::services::task_core::DeltaBatcher;
// PRODCAP-1:persist_event 显式接收调用方持有的写连接(见函数文档:防自死锁)
use rusqlite::Connection;

/// broadcast 通道容量:事件为瞬时通知,消费端(SSE 转发)实时读取;
/// 一次完整执行约产生十余条事件,64 足以吸收短时突发。
/// 溢出由接收端按 Lagged 跳过 —— PRODCAP-1 起**不再等于丢数据**:
/// 事件已落 `task_events` 表,前端按 `seq > lastSeq + 1` 判定缺口并补拉/回放。
pub(super) const EVENTS_CAPACITY: usize = 64;

/// 每任务事件保留条数(PRODCAP-1):超出按 seq 删除最旧,窗口外补拉返回
/// `truncated` 标记而非假装 0 条。清理放在写事务末尾(不新增定时任务)。
const EVENTS_KEEP_PER_TASK: i64 = 2000;

impl TaskService {
    /// 订阅任务事件流(broadcast;晚加入的订阅者只能收到订阅之后的事件——
    /// 要「不漏」请配合 `GET /api/tasks/events?task_id=&after=` 的回放,或补拉端点)。
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<SseEvent> {
        self.events.subscribe()
    }

    /// 事件落库(单一出处,PRODCAP-1):分配任务内单调 seq → INSERT → 窗口清理。
    ///
    /// **显式传入写连接、本函数不取锁**:本服务全部写路径共用一把**非重入**互斥锁
    /// (`Db::write()`),持锁调用点若让本函数再取锁会**自死锁**。故连接由调用方持有,
    /// 且调用方应把「事实表写 + 本事件写」放进**同一事务**,提交后再广播
    /// (见 `set_status` / `record_llm_call` 等;先例 `character_data.rs` 的事务写法)。
    ///
    /// 返回 `(seq, created_at)`;失败返回 Err —— 调用方**不得静默吞掉**:
    /// 事务回滚 + `tracing::error!` 留痕 + 不广播(宁可无事件,不要「有广播无落库」
    /// 造成前端补拉时对不上账)。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn persist_event(
        conn: &Connection,
        task_id: &str,
        kind: TaskEventKind,
        title: Option<&str>,
        status: Option<TaskStatus>,
        detail: Option<&str>,
        finish_reason: Option<&str>,
        phase: Option<&str>,
        step_index: Option<usize>,
    ) -> Result<(u64, String), String> {
        let seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM task_events WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("分配任务事件序号失败: {e}"))?;
        let at = now_iso();
        conn.execute(
            "INSERT INTO task_events (task_id, seq, kind, title, status, detail, finish_reason, phase, step_index, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task_id,
                seq,
                kind.as_str(),
                title,
                status.map(|s| s.as_str()),
                detail,
                finish_reason,
                phase,
                step_index.map(|i| i as i64),
                at,
            ],
        )
        .map_err(|e| format!("写入任务事件失败: {e}"))?;
        // 窗口清理:同事务末尾删最旧(seq <= 新 seq - 保留数)
        conn.execute(
            "DELETE FROM task_events WHERE task_id = ?1 AND seq <= ?2",
            params![task_id, seq - EVENTS_KEEP_PER_TASK],
        )
        .map_err(|e| format!("清理任务事件窗口失败: {e}"))?;
        Ok((seq as u64, at))
    }

    /// 补拉/回放共用的历史扫描(静态、显式传连接,可内存库单测)。
    ///
    /// 读 `seq > after` 的行,按 seq 升序取至多 `limit` 条,映射为与实时广播**同形**的
    /// `SseEvent::Task` 帧(含 `seq`/`at`)——补拉结果可原样走前端的同一分发,不搞第二套形状。
    ///
    /// 返回 `(帧列表, truncated)`:`truncated` = 请求的起点落在保留窗口之前
    /// (`after + 1 < 现存最小 seq`)——那意味着「窗口外的更早事件确实存在过但已被清理」,
    /// 与「本来就没有事件」必须区分,故显式标记而不是假装空。`after` 已达/超过最新 seq
    /// 则返回空列表 + `truncated=false`(调用方据此回空数组,**不是 404**)。
    pub(crate) fn scan_events(
        conn: &Connection,
        task_id: &str,
        after: u64,
        limit: usize,
    ) -> (Vec<SseEvent>, bool) {
        // 窗口边界先读:同时决定 truncated 与「无新行」短路
        let bounds: Option<(i64, i64)> = conn
            .query_row(
                "SELECT MIN(seq), MAX(seq) FROM task_events WHERE task_id = ?1",
                params![task_id],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .ok()
            .and_then(|(min, max)| min.zip(max));
        let Some((min_seq, max_seq)) = bounds else {
            return (Vec::new(), false);
        };
        // u64 → i64 用饱和转换:`after > i64::MAX` 的入参(如 u64::MAX)直接回绕成负数会把
        // 「超过最新 seq」误判成「窗口前起点」——饱和到 i64::MAX 后按超界处理(回空 + 不标截断)
        let after = i64::try_from(after).unwrap_or(i64::MAX);
        if after >= max_seq {
            return (Vec::new(), false);
        }
        let truncated = after + 1 < min_seq;
        let mut stmt = match conn.prepare_cached(
            "SELECT seq, kind, title, status, detail, finish_reason, phase, step_index, created_at \
             FROM task_events WHERE task_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
        ) {
            Ok(s) => s,
            Err(e) => return (log_query_failure("任务事件补拉 prepare", e), truncated),
        };
        let query = stmt.query_map(params![task_id, after, limit as i64], |row| {
            let kind_wire: String = row.get(1)?;
            let kind = TaskEventKind::from_wire(&kind_wire);
            if kind.is_none() {
                // 只有本进程写入过的 kind 才会进表;读不出枚举 = 数据被外部改动或版本回退,
                // 属真实异常而非噪声(照仓内「绝不静默放过」纪律留痕),该帧仍原样下发。
                tracing::warn!(
                    task_id = task_id,
                    kind = kind_wire,
                    "任务事件分类无法识别,按未知分类下发"
                );
            }
            let status_wire: Option<String> = row.get(3)?;
            Ok(SseEvent::Task {
                task_id: task_id.to_string(),
                kind,
                title: row.get(2)?,
                status: status_wire.as_deref().map(TaskStatus::from_str_lossy),
                detail: row.get(4)?,
                finish_reason: row.get(5)?,
                phase: row.get(6)?,
                step_index: row.get::<_, Option<i64>>(7)?.map(|i| i as usize),
                seq: Some(row.get::<_, i64>(0)? as u64),
                at: Some(row.get(8)?),
            })
        });
        // query_map 结果先绑定局部变量再 match(与 list_llm_calls 同口径,防 E0597)
        match query {
            Ok(rows) => (rows.filter_map(|r| r.ok()).collect(), truncated),
            Err(e) => (log_query_failure("任务事件补拉 query_map", e), truncated),
        }
    }

    /// 任务事件补拉(端点用;只读池):`GET /api/tasks/{id}/events` 的数据源,
    /// 语义见 `scan_events`。
    pub(crate) fn list_events(
        &self,
        task_id: &str,
        after: u64,
        limit: usize,
    ) -> (Vec<SseEvent>, bool) {
        let Some(conn) = read_or_log(&self.db, "任务事件 read") else {
            return (Vec::new(), false);
        };
        Self::scan_events(&conn, task_id, after, limit)
    }

    /// SSE 回放用的一次性历史读取:窗口内历史**全部**补发(`limit` = 保留上限),
    /// 与补拉端点的分页语义分开——回放要的是「一条流从断点接上」,不是翻页。
    pub(crate) fn list_events_replay(&self, task_id: &str, after: u64) -> (Vec<SseEvent>, bool) {
        self.list_events(task_id, after, EVENTS_KEEP_PER_TASK as usize)
    }

    /// 任务行是否已不存在(persist 失败后判「瞬态广播」用;只读一次主键查询)。
    fn task_row_missing(conn: &Connection, task_id: &str) -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?1)",
            params![task_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n == 0)
        .unwrap_or(false)
    }

    /// 广播一条任务事件(**不写库**):由持连接的调用点在事务提交后调用,
    /// 或由 `emit_event` 在 persist 之后调用。send 仅在无订阅者时返回 Err,属正常,忽略。
    /// 顺带刷新活动心跳(提交 3 · D7):事件是「任务还活着」的最细粒度信号。
    pub(crate) fn broadcast_event(&self, event: SseEvent) {
        if let SseEvent::Task { task_id, .. } = &event {
            self.touch_activity(task_id);
        }
        let _ = self.events.send(event);
    }

    /// 「事实写 + 事件落库**同事务** + 提交后广播」的统一出口(PRODCAP-1)。
    ///
    /// 覆盖最常见形态:单条 UPDATE/INSERT + 「发生变更才发事件」。**连接由调用方持有**
    /// (本函数不取锁——防与调用点的 `Db::write()` 自死锁)。`task_id = None` 时只执行
    /// 事实写、不发事件(子任务无归属任务的形态)。
    ///
    /// 返回「事实是否发生变更」(`execute` 行数 > 0)。返回 true 时事件**必已落库并广播**;
    /// 落库/提交失败 → 回滚 + `tracing::error!` 留痕 + 返回 false(**不静默丢事件**:
    /// 宁可无广播,也不要「有广播无落库」让前端补拉对不上账)。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_with_event(
        &self,
        conn: &Connection,
        task_id: Option<&str>,
        sql: &str,
        sql_params: &[&dyn rusqlite::ToSql],
        kind: TaskEventKind,
        title: Option<String>,
        status: Option<TaskStatus>,
        detail: Option<String>,
    ) -> bool {
        let tx = match conn.unchecked_transaction() {
            Ok(tx) => tx,
            Err(e) => {
                tracing::error!(error = e.to_string(), "任务写事务开启失败,事实写已放弃");
                return false;
            }
        };
        let changed = tx.execute(sql, sql_params).map(|n| n > 0).unwrap_or(false);
        let tid = match task_id.filter(|_| changed) {
            Some(tid) => tid,
            None => {
                if let Err(e) = tx.commit() {
                    tracing::error!(error = e.to_string(), "任务写事务提交失败");
                    return false;
                }
                return changed;
            }
        };
        let seq_at = match Self::persist_event(
            &tx,
            tid,
            kind,
            title.as_deref(),
            status,
            detail.as_deref(),
            None,
            None,
            None,
        ) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(
                    task_id = tid,
                    error = e,
                    "任务事件落库失败,事实与事件一并回滚"
                );
                return false;
            }
        };
        if let Err(e) = tx.commit() {
            tracing::error!(
                task_id = tid,
                error = e.to_string(),
                "任务写事务提交失败,事实与事件一并回滚"
            );
            return false;
        }
        self.broadcast_event(Self::task_event_payload(
            tid,
            kind,
            title,
            status,
            detail,
            None,
            None,
            None,
            Some(seq_at.0),
            Some(seq_at.1),
        ));
        true
    }

    /// 构造一条落库型任务事件的广播载荷(seq/at 来自 `persist_event` 的返回值)。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn task_event_payload(
        task_id: &str,
        kind: TaskEventKind,
        title: Option<String>,
        status: Option<TaskStatus>,
        detail: Option<String>,
        finish_reason: Option<String>,
        phase: Option<String>,
        step_index: Option<usize>,
        seq: Option<u64>,
        at: Option<String>,
    ) -> SseEvent {
        SseEvent::Task {
            task_id: task_id.to_string(),
            kind: Some(kind),
            title,
            status,
            detail,
            finish_reason,
            phase,
            step_index,
            seq,
            at,
        }
    }

    /// 发射任务事件(不持有写连接的调用点用:引擎桥 / 执行器 / 计划批准等)。
    ///
    /// 内部短取写连接:`persist_event` → 提交(**事务提交后**)→ `broadcast_event`。
    /// 持锁调用点**不要**用本函数(会自死锁),改用 `persist_event` + `broadcast_event`。
    /// 落库失败:`tracing::error!` 留痕、**不广播**(不静默丢事件),调用方语义不变。
    ///
    /// **两类事件按瞬态广播(不落库,`seq=None`)**:
    /// - `delta`(流式增量,唯一发射点是 DeltaBatcher::flush);
    /// - `deleted`——任务行删除即**级联清空**该任务的事件史(`ON DELETE CASCADE`),
    ///   先把 `deleted` 落库再删任务等于刚写就被级联删掉;反过来先删任务则外键失败。
    ///   故「删除」本身只能是瞬态通知,前端靠列表刷新兜底(既有行为不变)。
    pub(crate) fn emit_event(
        &self,
        kind: TaskEventKind,
        task_id: &str,
        title: Option<String>,
        status: Option<TaskStatus>,
        detail: Option<String>,
    ) {
        if kind == TaskEventKind::Delta || kind == TaskEventKind::Deleted {
            self.broadcast_event(Self::task_event_payload(
                task_id, kind, title, status, detail, None, None, None, None, None,
            ));
            return;
        }
        let (seq, at) = {
            let mut conn = self.db.write();
            let tx = match conn.transaction() {
                Ok(tx) => tx,
                Err(e) => {
                    tracing::error!(
                        task_id = task_id,
                        error = e.to_string(),
                        "任务事件事务开启失败,事件未落库"
                    );
                    return;
                }
            };
            let persisted = Self::persist_event(
                &tx,
                task_id,
                kind,
                title.as_deref(),
                status,
                detail.as_deref(),
                None,
                None,
                None,
            );
            let seq_at = match persisted {
                Ok(v) => v,
                Err(e) => {
                    if Self::task_row_missing(&tx, task_id) {
                        // 任务行已不存在(用户删除运行中任务后引擎补发的少量事件):
                        // 事件行有 tasks(id) 外键,写不进去;按瞬态广播,不当故障。
                        tracing::warn!(task_id = task_id, "任务行已删除,事件按瞬态广播(不落库)");
                        drop(tx);
                        self.broadcast_event(Self::task_event_payload(
                            task_id, kind, title, status, detail, None, None, None, None, None,
                        ));
                        return;
                    }
                    tracing::error!(task_id = task_id, error = e, "任务事件落库失败,已放弃广播");
                    return;
                }
            };
            if let Err(e) = tx.commit() {
                tracing::error!(
                    task_id = task_id,
                    error = e.to_string(),
                    "任务事件事务提交失败,事件未落库"
                );
                return;
            }
            seq_at
        };
        self.broadcast_event(Self::task_event_payload(
            task_id,
            kind,
            title,
            status,
            detail,
            None,
            None,
            None,
            Some(seq),
            Some(at),
        ));
    }

    /// 构造一个挂在本服务广播通道上的 delta 攒批器(引擎桥 sink 与
    /// generate_text 旁路共用同一攒批出口,保证全任务模式口径一致)。
    /// kind=delta 事件的唯一发射点是 DeltaBatcher::flush。
    pub(crate) fn delta_batcher(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
    ) -> DeltaBatcher {
        DeltaBatcher::new(self.events.clone(), task_id, phase, step_index)
    }
}

// delta 攒批器(批次 R4;批次 B.3 本体已搬迁至 task_core::delta,
// 语义与使用方说明见 task_core::delta)。
#[cfg(test)]
mod tests {
    use super::*;
    // 常量随攒批器本体迁至 task_core::delta,单测直接自该处引用
    use crate::services::task_core::delta::{DELTA_FLUSH_CHARS, DELTA_FLUSH_WINDOW};

    /// 构造一个挂在裸 broadcast 通道上的攒批器(单测无需 TaskService 实例)
    fn batcher(
        phase: &str,
        step_index: Option<usize>,
    ) -> (DeltaBatcher, tokio::sync::broadcast::Receiver<SseEvent>) {
        let (tx, rx) = tokio::sync::broadcast::channel(8);
        (DeltaBatcher::new(tx, "t1", phase, step_index), rx)
    }

    /// 字符阈值:单批攒满 80 字立即发射;不足部分由 flush 收尾;两批拼接 == 原文。
    /// 事件须为 kind=delta 且携带 phase/step_index(前端缓冲 key 与对齐依据);
    /// 且**不携带 seq/at**(暂态事件不落库,PRODCAP-1)。
    #[test]
    fn delta_batcher_flushes_on_char_threshold() {
        let (mut b, mut rx) = batcher("step", Some(0));
        let long = "字".repeat(DELTA_FLUSH_CHARS);
        b.push(&long);
        let Ok(SseEvent::Task {
            kind,
            detail,
            phase,
            step_index,
            seq,
            at,
            ..
        }) = rx.try_recv()
        else {
            panic!("达到字符阈值应立即发射一条 delta");
        };
        assert_eq!(kind, Some(TaskEventKind::Delta));
        assert_eq!(detail.as_deref(), Some(long.as_str()));
        assert_eq!(phase.as_deref(), Some("step"));
        assert_eq!(step_index, Some(0));
        assert_eq!(seq, None, "delta 不落库 → 不携带 seq");
        assert_eq!(at, None, "delta 不落库 → 不携带 at");

        b.push("尾巴");
        assert!(rx.try_recv().is_err(), "未到阈值不应发射");
        b.flush();
        let Ok(SseEvent::Task { detail, .. }) = rx.try_recv() else {
            panic!("flush 应发出残留尾段");
        };
        assert_eq!(detail.as_deref(), Some("尾巴"));
        assert!(rx.try_recv().is_err(), "flush 后通道应空");
    }

    /// 时间窗:不足字符阈值时按 200ms 窗发射(保护 broadcast 容量的另一闸门);
    /// 窗口未到不发射。start_paused 下 Instant::now 受控,advance 推进虚拟时钟。
    #[tokio::test(start_paused = true)]
    async fn delta_batcher_flushes_on_time_window() {
        let (mut b, mut rx) = batcher("planner", None);
        b.push("少量");
        b.flush_if_due();
        assert!(rx.try_recv().is_err(), "时间窗未到不应发射");
        tokio::time::advance(DELTA_FLUSH_WINDOW + std::time::Duration::from_millis(1)).await;
        b.flush_if_due();
        let Ok(SseEvent::Task {
            kind,
            detail,
            phase,
            step_index,
            ..
        }) = rx.try_recv()
        else {
            panic!("时间窗到应发射");
        };
        assert_eq!(kind, Some(TaskEventKind::Delta));
        assert_eq!(detail.as_deref(), Some("少量"));
        assert_eq!(phase.as_deref(), Some("planner"));
        assert_eq!(step_index, None, "非步骤类阶段不携带 step_index");
    }

    /// `TaskEventKind::as_str()` 与 serde 线格式逐变体一致(PRODCAP-1):
    /// 前者进 `task_events.kind`,后者进 SSE 载荷——漂移会让「补拉的行」与
    /// 「广播的帧」对不上账,故用测试钉死。新增变体须同时补两处(编译器兜住 as_str)。
    #[test]
    fn task_event_kind_as_str_matches_serde_wire() {
        let all = [
            TaskEventKind::Created,
            TaskEventKind::Status,
            TaskEventKind::Plan,
            TaskEventKind::Subtask,
            TaskEventKind::Usage,
            TaskEventKind::Deleted,
            TaskEventKind::LlmCall,
            TaskEventKind::AgentStatus,
            TaskEventKind::ApprovalRequired,
            TaskEventKind::Delta,
            TaskEventKind::FlowBound,
            TaskEventKind::FileChanged,
        ];
        for kind in all {
            let wire = serde_json::to_value(kind).expect("TaskEventKind 应可序列化");
            assert_eq!(
                wire,
                serde_json::Value::String(kind.as_str().to_string()),
                "as_str 与 serde 线格式不一致: {kind:?}"
            );
            // PRODCAP-1:from_wire 与 as_str 互为逆(补拉读回「广播的帧」同一分类名)
            assert_eq!(
                TaskEventKind::from_wire(kind.as_str()),
                Some(kind),
                "from_wire 未能读回 as_str 的输出: {kind:?}"
            );
        }
        assert_eq!(
            TaskEventKind::from_wire("no_such_kind"),
            None,
            "未知分类应返回 None(降级为「不驱动刷新」),不得猜值"
        );
    }

    /// PRODCAP-1:事件落库分配任务内单调 seq、字段逐字入库、窗口按 2000 条清理。
    /// 直接在裸连接上验证 `persist_event`(与广播解耦的纯 DB 部分)。
    #[test]
    fn persist_event_allocates_seq_and_prunes_window() {
        let conn = rusqlite::Connection::open_in_memory().expect("内存库");
        conn.execute_batch(crate::models::db::create_tables_sql())
            .expect("建表");
        // 外键(tasks)需先有行:task_events.task_id REFERENCES tasks(id)
        for tid in ["t1", "t2"] {
            conn.execute(
                "INSERT INTO tasks (id, title, created_at, updated_at) VALUES (?1, 't', '2026-10-05T00:00:00Z', '2026-10-05T00:00:00Z')",
                rusqlite::params![tid],
            )
            .expect("建任务行");
        }

        // ① 连续三条 → seq 1/2/3,字段逐字一致(含 status/detail 与 at 非空)
        let (s1, at1) = TaskService::persist_event(
            &conn,
            "t1",
            TaskEventKind::Created,
            Some("标题"),
            None,
            Some("任务已创建"),
            None,
            None,
            None,
        )
        .expect("落库");
        assert_eq!(s1, 1);
        assert!(!at1.is_empty(), "created_at 应非空");
        let (s2, _) = TaskService::persist_event(
            &conn,
            "t1",
            TaskEventKind::Status,
            None,
            Some(TaskStatus::Running),
            Some("任务状态更新为 running"),
            None,
            None,
            None,
        )
        .expect("落库");
        let (s3, _) = TaskService::persist_event(
            &conn,
            "t1",
            TaskEventKind::Plan,
            None,
            None,
            Some("执行计划已更新(共 2 步)"),
            None,
            None,
            None,
        )
        .expect("落库");
        assert_eq!((s2, s3), (2, 3), "seq 应为任务内单调 1/2/3");
        let (kind, status, detail): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT kind, status, detail FROM task_events WHERE task_id='t1' AND seq=2",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("读回");
        assert_eq!(kind, "status");
        assert_eq!(status.as_deref(), Some("running"));
        assert_eq!(detail.as_deref(), Some("任务状态更新为 running"));

        // ② 序号按任务隔离:另一任务的 seq 从 1 起
        let (other, _) = TaskService::persist_event(
            &conn,
            "t2",
            TaskEventKind::Created,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("落库");
        assert_eq!(other, 1, "seq 是**任务内**单调,不同任务互不影响");

        // ③ 窗口清理:发满 2001 条 → 恰剩 2000 条、最旧 seq = 2
        for _ in 0..(EVENTS_KEEP_PER_TASK as usize + 1 - 3) {
            TaskService::persist_event(
                &conn,
                "t1",
                TaskEventKind::Usage,
                None,
                None,
                Some("用量"),
                None,
                None,
                None,
            )
            .expect("落库");
        }
        let (count, min_seq): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), MIN(seq) FROM task_events WHERE task_id='t1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("统计");
        assert_eq!(count, EVENTS_KEEP_PER_TASK, "窗口应恰保留 2000 条");
        assert_eq!(min_seq, 2, "最旧一条应为 seq=2(seq=1 已被窗口删除)");
    }

    /// PRODCAP-1 补拉/回放共用的 `scan_events`:seq 升序、`after` 过滤、
    /// 窗口截断显式标记、`after` 超过最新 seq 回空(不假装有)。
    /// 静态函数直接在裸连接上验证(与端点/回放共用同一实现,见 api::tasks 两个 handler)。
    #[test]
    fn scan_events_filters_orders_and_marks_truncation() {
        let conn = rusqlite::Connection::open_in_memory().expect("内存库");
        conn.execute_batch(crate::models::db::create_tables_sql())
            .expect("建表");
        conn.execute(
            "INSERT INTO tasks (id, title, created_at, updated_at) VALUES ('t1', 't', '2026-10-05T00:00:00Z', '2026-10-05T00:00:00Z')",
            [],
        )
        .expect("建任务行");

        // 三条:seq 1/2/3
        for detail in [
            "任务已创建",
            "任务状态更新为 running",
            "执行计划已更新(共 1 步)",
        ] {
            TaskService::persist_event(
                &conn,
                "t1",
                TaskEventKind::Status,
                None,
                Some(TaskStatus::Running),
                Some(detail),
                None,
                None,
                None,
            )
            .expect("落库");
        }
        let seqs_of = |events: &[SseEvent]| -> Vec<u64> {
            events
                .iter()
                .filter_map(|e| match e {
                    SseEvent::Task { seq, .. } => *seq,
                    _ => None,
                })
                .collect()
        };

        // ① after=1 → 只要 2/3,升序,且帧与广播同形(含 seq/at 与逐字字段)
        let (events, truncated) = TaskService::scan_events(&conn, "t1", 1, 100);
        assert!(!truncated, "起点在窗口内不应标截断");
        assert_eq!(
            seqs_of(&events),
            vec![2, 3],
            "应按 seq 升序且只回 after 之后的行"
        );
        let Some(SseEvent::Task {
            kind,
            detail,
            at,
            task_id,
            ..
        }) = events.first()
        else {
            panic!("应为 Task 帧");
        };
        assert_eq!(*kind, Some(TaskEventKind::Status));
        assert_eq!(task_id, "t1");
        assert_eq!(detail.as_deref(), Some("任务状态更新为 running"));
        assert!(at.as_deref().is_some_and(|s| !s.is_empty()), "at 应非空");

        // ② limit 生效(夹取在端点层,扫描层按调用方给定值截断)
        let (events, _) = TaskService::scan_events(&conn, "t1", 0, 1);
        assert_eq!(events.len(), 1);

        // ③ after 已达/超过最新 seq → 空 + 不标截断(端点据此回空数组,不是 404)
        let (events, truncated) = TaskService::scan_events(&conn, "t1", 3, 100);
        assert!(events.is_empty() && !truncated);
        let (events, truncated) = TaskService::scan_events(&conn, "t1", 99, 100);
        assert!(events.is_empty() && !truncated);
        // u64 超界(如 ?after=18446744073709551615)按「超过最新 seq」处理:
        // 回绕成负数会把「超界」误判成「窗口前起点」(返回全量 + 误标 truncated),故饱和转换
        let (events, truncated) = TaskService::scan_events(&conn, "t1", u64::MAX, 100);
        assert!(
            events.is_empty() && !truncated,
            "u64 超界入参应回空且不标截断(不得回绕为负数)"
        );

        // ④ 窗口截断:补到 2001 条 → after=0 标 truncated、首条 seq=2、恰回 2000 条
        for _ in 0..(EVENTS_KEEP_PER_TASK as usize + 1 - 3) {
            TaskService::persist_event(
                &conn,
                "t1",
                TaskEventKind::Usage,
                None,
                None,
                Some("用量"),
                None,
                None,
                None,
            )
            .expect("落库");
        }
        let (events, truncated) =
            TaskService::scan_events(&conn, "t1", 0, EVENTS_KEEP_PER_TASK as usize);
        assert!(
            truncated,
            "起点早于保留窗口应显式标记(不得假装「没有更多」)"
        );
        assert_eq!(events.len(), EVENTS_KEEP_PER_TASK as usize);
        assert_eq!(
            seqs_of(&events).first(),
            Some(&2),
            "窗口删除后首条应为 seq=2"
        );

        // ⑤ 恰在窗口边缘(after+1 == 最小 seq)= 无缺口,不标截断
        let (events, truncated) = TaskService::scan_events(&conn, "t1", 1, 10);
        assert!(!truncated, "after+1 == 最小 seq 时没有丢行");
        assert_eq!(seqs_of(&events).first(), Some(&2));
    }
}
