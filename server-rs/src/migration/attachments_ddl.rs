// 任务附件表的幂等建表(ATTACH-1,2026-11-03)。
//
// 为什么单独一个文件而不是塞进 `ddl.rs`:那个文件已到 995 行,再涨会顶破
// `tools/file-size-baseline.json` 的「超千行只降不升」闸门(阈值 1000)。
// 本模块与 `ddl.rs` 同层同职责(幂等 DDL 升级),只是物理上分开放。
//
// **调用点与 `ensure_task_trace_table` 完全一致**:启动升级事务内一次、跨库合并的
// 基线侧与源快照侧各一次。漏任一处的后果不是「少建一张表」,而是合并时报
// 「基线缺少源表」直接中止(`migration/merge.rs` 的 schema 比对)。
use rusqlite::Connection;

/// 任务附件元数据表。文本必须与 `models/db/schema.rs` 的 `CREATE_TABLES` 逐字一致
/// (normalize 后比对,见 `tests/schema_migration_meta.rs`)。
const TASK_ATTACHMENTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS task_attachments (
  id         TEXT PRIMARY KEY,
  task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  name       TEXT NOT NULL,
  rel_path   TEXT NOT NULL,
  size       INTEGER NOT NULL,
  mime       TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_attachments_task ON task_attachments(task_id, created_at);
"#;

/// 幂等 schema 升级:旧库补建 task_attachments 表(PRAGMA 探测,已存在跳过)。
pub fn ensure_task_attachments_table(conn: &Connection) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("PRAGMA table_info(task_attachments)")
            .map_err(|e| format!("读取 task_attachments 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 task_attachments 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name);
        }
    }
    if !existing.is_empty() {
        return Ok(());
    }
    conn.execute_batch(TASK_ATTACHMENTS_DDL)
        .map_err(|e| format!("创建 task_attachments 表失败: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 幂等:第一次建表,第二次原样返回 Ok(不报「表已存在」)。
    #[test]
    fn ensure_is_idempotent() {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY);")
            .expect("建最小 tasks 表");
        ensure_task_attachments_table(&conn).expect("首建应成功");
        ensure_task_attachments_table(&conn).expect("重复调用应幂等");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_attachments'",
                [],
                |r| r.get(0),
            )
            .expect("查询表");
        assert_eq!(count, 1);
    }

    /// 索引也随建表一起创建(`attachments 清单`按 task_id 过滤)。
    #[test]
    fn ensure_creates_index() {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY);")
            .expect("建最小 tasks 表");
        ensure_task_attachments_table(&conn).expect("建表");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_task_attachments_task'",
                [],
                |r| r.get(0),
            )
            .expect("查询索引");
        assert_eq!(count, 1);
    }

    /// 外键级联:删除任务时附件行随之消失(CASCADE 在 PRAGMA foreign_keys ON 下生效)。
    #[test]
    fn deleting_task_cascades_attachments() {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch("PRAGMA foreign_keys = ON; CREATE TABLE tasks (id TEXT PRIMARY KEY);")
            .expect("建最小 tasks 表");
        ensure_task_attachments_table(&conn).expect("建表");
        conn.execute("INSERT INTO tasks (id) VALUES ('t1')", [])
            .expect("插入任务");
        conn.execute(
            "INSERT INTO task_attachments (id, task_id, name, rel_path, size, mime, created_at)
             VALUES ('a1', 't1', 'x.txt', 'attachments/1-x.txt', 3, 'text/plain', '2026-11-03T00:00:00Z')",
            [],
        )
        .expect("插入附件");
        conn.execute("DELETE FROM tasks WHERE id = 't1'", [])
            .expect("删任务");
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM task_attachments", [], |r| r.get(0))
            .expect("计数");
        assert_eq!(left, 0, "CASCADE 应带走附件行");
    }
}
