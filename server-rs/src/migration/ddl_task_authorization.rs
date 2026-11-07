// 任务模式三档授权的幂等补列（2026-11-06；从 ddl.rs 拆出）。
//
// ddl.rs 已接近 1000 行阈值，而本次新增的函数是独立、短小的 DDL 迁移，
// 拆开既守住 ratchet，也让「三档授权」的 schema 变更集中在一处。
use rusqlite::Connection;

/// 为 tasks 补 `tool_authorization` 列（可空无 DEFAULT；NULL = 旧任务/旧客户端，
/// 保持改动前的 `deny_dangerous` 行为，旧行零回填）。
///
/// 启动时（`Db::open`）与跨库合并前（`merge_databases` 两侧）各执行一次。
/// 列追加在表尾，与新版 `CREATE_TABLES` 的建表顺序一致（schema 归一化比对依赖）。
pub fn ensure_tasks_tool_authorization_column(conn: &Connection) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("PRAGMA table_info(tasks)")
            .map_err(|e| format!("读取 tasks 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 tasks 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name.to_lowercase());
        }
    }
    // tasks 表不存在(极旧快照/手工建库)时零列返回,直接跳过——
    // 建表由 CREATE_TABLES 或合并期 schema 比对负责,此处不能 ALTER 报错
    if existing.is_empty() {
        return Ok(());
    }
    if existing.iter().any(|c| c == "tool_authorization") {
        return Ok(());
    }
    conn.execute("ALTER TABLE tasks ADD COLUMN tool_authorization TEXT", [])
        .map_err(|e| format!("为 tasks 补 tool_authorization 列失败: {e}"))?;
    Ok(())
}
