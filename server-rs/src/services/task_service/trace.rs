// 任务执行轨迹(TMSHOW-1,2026-11-01):任务模式**中间过程**的结构化落库与读取。
//
// 为什么需要本模块:`task_llm_calls` 是「每次调用一行」的记账(OBS-1 知情接受,工具循环
// 整轮一行),`task_events.agent_status` 只有 200 字人类可读进展——两者都恢复不出
// 「哪一轮思考了什么、调了哪个工具、bash 回了什么」。前端「全消息显示」与
// 「压缩 + 计数」两档无数据可渲染,本模块是它们的唯一数据源。
//
// 三条口径(与 events.rs 的持久化纪律同款):
// 1. **写入侧硬截断**:正文超 `TRACE_CONTENT_MAX_CHARS` 即截断并置 `truncated=1`。
//    不设闸门时单任务可膨胀到几十 MB(fs_read 可返回整文件、思考可达数千字)。
// 2. **保留窗口**:每任务超 `TRACE_KEEP_PER_TASK` 条时按 seq 删最旧,与 task_events 的
//    2000 条窗口同口径(清理放在写事务末尾,不新增定时任务)。
// 3. **落库成功才广播**:轨迹行先写库(含 seq),提交后广播 kind=trace 事件;写失败
//    `tracing::error!` 留痕且不广播——宁可无推送,不要「有推送无落库」导致补拉对不上账。
use super::*;
use crate::models::types::{TaskTraceKind, TaskTraceRecord};
use rusqlite::Connection;

/// 单条轨迹正文上限(字符)。取值依据:
/// - 思考正文常见 1k~5k 字,8k 能覆盖绝大多数完整一轮;
/// - 工具出参可能极大(fs_read 整文件),8k 是「够看懂发生了什么」与「不吃爆库」的折中;
/// - 前端渲染单条 8k 字符的卡片仍在可接受范围内(虚拟滚动只挂载视口内的行)。
pub(crate) const TRACE_CONTENT_MAX_CHARS: usize = 8000;

/// 每任务轨迹保留条数。与 task_events 的 2000 同口径:
/// 长任务(team 全题实测主 agent 单步可上百轮、总计数百条工具调用)留足余量,
/// 同时给库体量一个硬上界(2000 × 8k 字符 ≈ 16MB 理论上限,实际远低于此)。
const TRACE_KEEP_PER_TASK: i64 = 2000;

/// 轨迹行列表(读取侧返回)。与 `scan_events` 同形:`truncated` 表示请求起点早于保留窗口,
/// 「窗口外确实有更早的条目但已被清理」与「本来就没有」必须区分。
pub(crate) struct TracePage {
    pub(crate) rows: Vec<TaskTraceRecord>,
    pub(crate) truncated: bool,
}

impl TaskService {
    /// 落一条轨迹(单一写入出处)。
    ///
    /// `content` 超上限在此截断并置 `truncated`;空正文直接丢弃(空思考/空结果没有
    /// 展示价值,落库只会让前端多渲染一张空卡)。
    ///
    /// 返回 `Some(seq)` = 已落库(调用方据此广播),`None` = 未落库(空正文 / 任务行已不存在)。
    /// 任务行不存在时静默跳过:引擎在用户删除任务后仍会补发少量事件,轨迹同理,
    /// 不是故障(外键会在写入时报错,这里先按任务存在性短路,避免刷 error 日志)。
    pub(crate) fn record_trace(
        &self,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        kind: TaskTraceKind,
        name: Option<&str>,
        content: &str,
    ) -> Option<i64> {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return None;
        }
        // 截断按**字符**而非字节:中文思考按字节截会把多字节字符劈成半个,
        // 前端渲染出乱码。truncated 标记让界面能如实标注「已截断」。
        let (body, truncated) = truncate_chars(trimmed, TRACE_CONTENT_MAX_CHARS);
        let mut conn = self.db.write();
        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(e) => {
                tracing::error!(task_id = task_id, error = e.to_string(), "轨迹事务开启失败");
                return None;
            }
        };
        match Self::insert_trace(
            &tx, task_id, phase, step_index, kind, name, &body, truncated,
        ) {
            Ok(seq) => {
                if let Err(e) = tx.commit() {
                    tracing::error!(
                        task_id = task_id,
                        error = e.to_string(),
                        "轨迹事务提交失败,轨迹未落库"
                    );
                    return None;
                }
                Some(seq)
            }
            Err(e) => {
                if Self::task_row_missing(&tx, task_id) {
                    // 任务已被删除(外键会让 INSERT 失败):按正常结束处理,不打 error
                    tracing::debug!(task_id = task_id, "任务行已删除,轨迹跳过落库");
                } else {
                    tracing::error!(task_id = task_id, error = e, "任务轨迹落库失败");
                }
                None
            }
        }
    }

    /// 轨迹插入(显式传连接、本函数不取锁):与 events::persist_event 同一纪律——
    /// 调用方持有写锁时再让本函数取锁会自死锁。
    #[allow(clippy::too_many_arguments)]
    fn insert_trace(
        conn: &Connection,
        task_id: &str,
        phase: &str,
        step_index: Option<usize>,
        kind: TaskTraceKind,
        name: Option<&str>,
        content: &str,
        truncated: bool,
    ) -> Result<i64, String> {
        let seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM task_trace WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("分配轨迹序号失败: {e}"))?;
        conn.execute(
            "INSERT INTO task_trace \
               (task_id, seq, phase, step_index, kind, name, content, truncated, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                task_id,
                seq,
                phase,
                step_index.map(|i| i as i64),
                kind.as_str(),
                name,
                content,
                if truncated { 1i64 } else { 0i64 },
                now_iso(),
            ],
        )
        .map_err(|e| format!("写入轨迹失败: {e}"))?;
        // 窗口清理(同事务末尾):超出保留条数的最旧行按 seq 删除
        conn.execute(
            "DELETE FROM task_trace WHERE task_id = ?1 AND seq <= ?2",
            params![task_id, seq - TRACE_KEEP_PER_TASK],
        )
        .map_err(|e| format!("清理轨迹窗口失败: {e}"))?;
        Ok(seq)
    }

    /// 按 seq 回读单条轨迹(广播载荷用)。
    ///
    /// 为什么不在 `record_trace` 直接返回构造好的记录:截断发生在写入侧,广播必须携带
    /// **库里的权威形态**(截断后的正文 + truncated 标记),否则前端本地缓存与补拉结果
    /// 不一致——同一条轨迹在「实时」与「刷新后」显示成两种样子。
    pub(crate) fn trace_by_seq(&self, task_id: &str, seq: i64) -> Option<TaskTraceRecord> {
        let conn = self.db.read().ok()?;
        Self::select_trace_by_seq(&conn, task_id, seq)
            .ok()
            .flatten()
    }

    /// 单条查询的静态实现(显式传连接,可内存库单测)。
    pub(crate) fn select_trace_by_seq(
        conn: &Connection,
        task_id: &str,
        seq: i64,
    ) -> Result<Option<TaskTraceRecord>, String> {
        conn.query_row(
            "SELECT id, task_id, seq, phase, step_index, kind, name, content, truncated, created_at \
             FROM task_trace WHERE task_id = ?1 AND seq = ?2",
            params![task_id, seq],
            |row| {
                Ok(TaskTraceRecord {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    seq: row.get(2)?,
                    phase: row.get(3)?,
                    step_index: row.get(4)?,
                    kind: row.get(5)?,
                    name: row.get(6)?,
                    content: row.get(7)?,
                    truncated: row.get::<_, i64>(8)? != 0,
                    created_at: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("回读轨迹失败: {e}"))
    }

    /// 读取轨迹(补拉端点与断线恢复共用)。
    ///
    /// 语义与 `events::scan_events` 逐条对齐:读 `seq > after`,按 seq 升序取至多 `limit`
    /// 条;`truncated` = 请求起点落在保留窗口之前。`after` 已达最新则返回空列表 +
    /// `truncated=false`(调用方据此回空数组,**不是 404**)。
    ///
    /// 读失败向上抛:把缺失当权威会让前端显示一份残缺的"完整过程"(2026-10-25 审计 P1
    /// 同款判断)。
    pub(crate) fn list_trace(
        &self,
        task_id: &str,
        after: i64,
        limit: usize,
    ) -> Result<TracePage, String> {
        let conn = self
            .db
            .read()
            .map_err(|e| format!("读取轨迹失败(连接不可用): {e}"))?;
        Self::scan_trace(&conn, task_id, after, limit)
    }

    /// 轨迹扫描的静态实现(显式传连接,可内存库单测)。
    pub(crate) fn scan_trace(
        conn: &Connection,
        task_id: &str,
        after: i64,
        limit: usize,
    ) -> Result<TracePage, String> {
        // 窗口边界先读:同时决定 truncated 与「无新行」短路。
        // 聚合查询恒返回一行(NULL 表示无轨迹),真正的 SQL 错误必须向上抛。
        let (min_seq, max_seq): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT MIN(seq), MAX(seq) FROM task_trace WHERE task_id = ?1",
                params![task_id],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .map_err(|e| format!("读取轨迹窗口失败: {e}"))?;
        let Some((min_seq, max_seq)) = min_seq.zip(max_seq) else {
            return Ok(TracePage {
                rows: Vec::new(),
                truncated: false,
            });
        };
        if after >= max_seq {
            return Ok(TracePage {
                rows: Vec::new(),
                truncated: false,
            });
        }
        // 起点早于窗口下界:更早的条目存在过但已被清理,如实标记
        let truncated = after + 1 < min_seq;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, seq, phase, step_index, kind, name, content, truncated, created_at \
                 FROM task_trace WHERE task_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
            )
            .map_err(|e| format!("准备轨迹查询失败: {e}"))?;
        let rows = stmt
            .query_map(params![task_id, after, limit as i64], |row| {
                Ok(TaskTraceRecord {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    seq: row.get(2)?,
                    phase: row.get(3)?,
                    step_index: row.get(4)?,
                    kind: row.get(5)?,
                    name: row.get(6)?,
                    content: row.get(7)?,
                    truncated: row.get::<_, i64>(8)? != 0,
                    created_at: row.get(9)?,
                })
            })
            .map_err(|e| format!("执行轨迹查询失败: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("读取轨迹行失败: {e}"))?);
        }
        Ok(TracePage {
            rows: out,
            truncated,
        })
    }
}

/// 按字符截断(中文安全):返回 (截断后正文, 是否真的截断)。
/// 未超限时原样返回且标记 false,不产生额外拷贝语义。
pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> (String, bool) {
    let mut iter = text.char_indices();
    match iter.nth(max_chars) {
        // 第 max_chars 个字符之后还有内容 → 需要截断
        Some((idx, _)) => (text[..idx].to_string(), true),
        None => (text.to_string(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_chars_keeps_short_text_intact() {
        let (body, truncated) = truncate_chars("短思考", 10);
        assert_eq!(body, "短思考");
        assert!(!truncated);
    }

    #[test]
    fn truncate_chars_is_char_safe_for_chinese() {
        // 中文按字节截会劈成半个字符;按字符截必须整字边界。
        let text = "思考内容".repeat(4); // 16 字
        let (body, truncated) = truncate_chars(&text, 5);
        assert!(truncated);
        assert_eq!(body.chars().count(), 5);
        assert_eq!(body, "思考内容思");
    }

    #[test]
    fn truncate_chars_exact_boundary_not_truncated() {
        let text = "12345";
        let (body, truncated) = truncate_chars(text, 5);
        assert_eq!(body, text);
        assert!(!truncated, "恰好等于上限不应标记截断");
    }

    #[test]
    fn trace_kind_roundtrip() {
        // as_str / from_wire 互逆:读库把 kind 文本读回枚举,漂移会让轨迹静默分类错误。
        for kind in [
            TaskTraceKind::Reasoning,
            TaskTraceKind::ToolCall,
            TaskTraceKind::ToolResult,
            TaskTraceKind::StepResult,
        ] {
            assert_eq!(TaskTraceKind::from_wire(kind.as_str()), Some(kind));
        }
        assert_eq!(TaskTraceKind::from_wire("unknown"), None);
    }

    /// 空表读取返回空页而非错误(与 events::scan_events 同口径:没有轨迹不是 404)。
    #[test]
    fn scan_trace_on_empty_table_returns_empty_page() {
        use crate::utils::test_support::TempDataDir;
        let dir = TempDataDir::new("task-trace-empty");
        let db_path = dir.join("kedai.db");
        let db = crate::models::db::Db::open(&db_path, &dir).expect("开库失败");
        let conn = db.write();
        let page = TaskService::scan_trace(&conn, "missing", 0, 10).expect("空表读取应成功");
        assert!(page.rows.is_empty());
        assert!(!page.truncated);
    }

    /// 轨迹按 seq 升序返回、`after` 过滤生效、越界 `after` 回空页。
    #[test]
    fn scan_trace_orders_and_filters_by_seq() {
        use crate::utils::test_support::TempDataDir;
        let dir = TempDataDir::new("task-trace-scan");
        let db_path = dir.join("kedai.db");
        let db = crate::models::db::Db::open(&db_path, &dir).expect("开库失败");
        let conn = db.write();
        conn.execute(
            "INSERT INTO tasks (id, title, status, created_at, updated_at) \
             VALUES ('t1', '目标', 'running', 'c', 'u')",
            [],
        )
        .expect("建任务行");

        for (kind, body) in [
            (TaskTraceKind::Reasoning, "先想一下"),
            (TaskTraceKind::ToolCall, r#"{"cmd":"ls"}"#),
            (TaskTraceKind::ToolResult, "a.txt"),
        ] {
            TaskService::insert_trace(&conn, "t1", "agent", None, kind, None, body, false)
                .expect("写轨迹");
        }

        let all = TaskService::scan_trace(&conn, "t1", 0, 100).expect("读轨迹");
        assert_eq!(all.rows.len(), 3, "三条轨迹应全部返回");
        assert_eq!(all.rows[0].seq, 1);
        assert_eq!(all.rows[2].seq, 3);
        assert_eq!(all.rows[0].kind, "reasoning");
        assert_eq!(all.rows[1].kind, "tool_call");
        assert!(!all.truncated, "起点落在窗口内不应标记 truncated");

        let tail = TaskService::scan_trace(&conn, "t1", 1, 100).expect("增量读");
        assert_eq!(tail.rows.len(), 2, "after=1 应只返回后两条");
        assert_eq!(tail.rows[0].seq, 2);

        let beyond = TaskService::scan_trace(&conn, "t1", 3, 100).expect("越界读");
        assert!(beyond.rows.is_empty(), "after 已达最新应回空列表");
        assert!(!beyond.truncated);
    }

    /// 超限正文截断并置标记:写入侧硬截断是库体量的唯一上界。
    #[test]
    fn insert_trace_marks_truncated_body() {
        use crate::utils::test_support::TempDataDir;
        let dir = TempDataDir::new("task-trace-truncate");
        let db_path = dir.join("kedai.db");
        let db = crate::models::db::Db::open(&db_path, &dir).expect("开库失败");
        let conn = db.write();
        conn.execute(
            "INSERT INTO tasks (id, title, status, created_at, updated_at) \
             VALUES ('t1', '目标', 'running', 'c', 'u')",
            [],
        )
        .expect("建任务行");

        let long = "长".repeat(TRACE_CONTENT_MAX_CHARS + 100);
        let (body, truncated) = truncate_chars(&long, TRACE_CONTENT_MAX_CHARS);
        TaskService::insert_trace(
            &conn,
            "t1",
            "agent",
            None,
            TaskTraceKind::Reasoning,
            None,
            &body,
            truncated,
        )
        .expect("写截断轨迹");

        let page = TaskService::scan_trace(&conn, "t1", 0, 10).expect("读轨迹");
        assert_eq!(page.rows.len(), 1);
        assert!(page.rows[0].truncated, "超限正文必须标记 truncated");
        assert_eq!(
            page.rows[0].content.chars().count(),
            TRACE_CONTENT_MAX_CHARS,
            "截断后长度应等于上限"
        );
    }

    /// 保留窗口按 seq 删最旧,且 `truncated` 如实标记「起点早于窗口下界」。
    ///
    /// TMSHOW-4(2026-11-04)补:此前 `truncated` 的**置真**分支没有任何机检——
    /// 三条既有测试只覆盖了「窗口内不截断」与「正文超限截断」,而「请求起点落在
    /// 保留窗口之前」这一条读端语义(前端据此提示「更早过程已清理」)无人守住。
    /// 这里直接写窗口边界两侧的行,避免真造 2001 条:
    /// 写入 `seq = TRACE_KEEP_PER_TASK + 1`,触发 `DELETE ... seq <= 1`,于是
    /// 窗口下界变成 2。
    #[test]
    fn scan_trace_marks_truncated_before_retention_window() {
        use crate::utils::test_support::TempDataDir;
        let dir = TempDataDir::new("task-trace-window");
        let db_path = dir.join("kedai.db");
        let db = crate::models::db::Db::open(&db_path, &dir).expect("开库失败");
        let conn = db.write();
        conn.execute(
            "INSERT INTO tasks (id, title, status, created_at, updated_at) \
             VALUES ('t1', '目标', 'running', 'c', 'u')",
            [],
        )
        .expect("建任务行");

        // 写满窗口 + 1 行:最后一行写入时会把 seq=1 清理掉。
        let total = TRACE_KEEP_PER_TASK + 1;
        for i in 1..=total {
            TaskService::insert_trace(
                &conn,
                "t1",
                "agent",
                None,
                TaskTraceKind::Reasoning,
                None,
                &format!("第 {i} 条"),
                false,
            )
            .expect("写轨迹");
        }

        // 起点在窗口之前(after=0 要求 seq>0,而窗口下界已是 2)→ 必须标 truncated
        let before = TaskService::scan_trace(&conn, "t1", 0, 10).expect("窗口前读取");
        assert!(
            before.truncated,
            "请求起点早于保留窗口时必须标 truncated(after=0,窗口下界=2)"
        );
        assert_eq!(before.rows[0].seq, 2, "最旧一条已被窗口清掉");

        // 起点落在窗口内(after=1 → 要求 seq>1,正是窗口下界)→ 不算截断
        let inside = TaskService::scan_trace(&conn, "t1", 1, 10).expect("窗口内读取");
        assert!(
            !inside.truncated,
            "起点正好等于窗口下界时不应标 truncated:更早的条目确实不存在了,不是缺失"
        );
        assert_eq!(inside.rows[0].seq, 2);
    }
}
