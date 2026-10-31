// SDC 批(2026-10-31)新增:合并逻辑引用重映射与 seq 重排的回归网。
// 独立文件:测试不计 check-file-size 的 ratchet,生产文件保持在体积基线以内。
#![cfg(test)]

use super::super::DATABASE_FILE;
use super::*;
use crate::utils::test_support::TempDataDir;

fn temp_dir(tag: &str) -> TempDataDir {
    TempDataDir::new(&format!("migration-sdc-{tag}"))
}

/// 构造带回退快照与工具调用绑定的测试库(合并只要求两侧 schema 一致)。
fn create_snapshot_db(path: &Path, title: &str) {
    let conn = Connection::open(path.join(DATABASE_FILE)).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE characters(id TEXT PRIMARY KEY, name TEXT NOT NULL);
         CREATE TABLE sessions(id TEXT PRIMARY KEY, character_id TEXT NOT NULL REFERENCES characters(id));
         CREATE TABLE undo_snapshots(
           id TEXT PRIMARY KEY, session_id TEXT NOT NULL, tool_name TEXT NOT NULL,
           label TEXT NOT NULL DEFAULT '', payload TEXT NOT NULL, created_at TEXT NOT NULL,
           seq INTEGER);
         CREATE TABLE tool_calls(
           id TEXT PRIMARY KEY, agent_session_id TEXT NOT NULL, name TEXT NOT NULL,
           input TEXT NOT NULL, output TEXT NOT NULL, duration_ms INTEGER NOT NULL,
           created_at TEXT NOT NULL, snapshot_id TEXT);
         ALTER TABLE sessions ADD COLUMN title TEXT;",
    )
    .unwrap();
    conn.execute("INSERT INTO characters VALUES('same', '角色')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO sessions(id, character_id, title) VALUES('session', 'same', ?1)",
        [title],
    )
    .unwrap();
}

/// UNDOFIX2-L2:非外键逻辑引用必须随主键重映射。
///
/// 场景:两侧同名 session 内容不同(主键冲突 → 源侧会话换新 id),两侧同名
/// 快照内容不同(主键冲突 → 源侧快照换新 id)。源侧 `undo_snapshots.session_id`
/// 与 `tool_calls.snapshot_id` 都不是外键,旧实现原样搬入 → 挂到基线同名行 /
/// 指向不存在的快照。
#[test]
fn merge_remaps_logical_snapshot_references() {
    let baseline = temp_dir("l2-baseline");
    let source = temp_dir("l2-source");
    let first_root = temp_dir("l2-first");
    let first = first_root.join("work");
    create_snapshot_db(&baseline, "基线会话");
    create_snapshot_db(&source, "源会话");
    for (path, payload, tc) in [
        (&baseline, "{\"before\":\"baseline\"}", "tc-b"),
        (&source, "{\"before\":\"source\"}", "tc-s"),
    ] {
        let conn = Connection::open(path.join(DATABASE_FILE)).unwrap();
        conn.execute(
            "INSERT INTO undo_snapshots(id, session_id, tool_name, payload, created_at, seq) \
             VALUES('snap-b', 'session', 'update_variables', ?1, '2026-01-01T00:00:00.000Z', 1)",
            [payload],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tool_calls(id, agent_session_id, name, input, output, duration_ms, created_at, snapshot_id) \
             VALUES(?1, 'as', 'update_variables', '{}', '{}', 1, '2026-01-01T00:00:00.000Z', 'snap-b')",
            [tc],
        )
        .unwrap();
    }

    merge_data_dirs(&baseline, &source, &first).unwrap();
    let conn = Connection::open(first.join(DATABASE_FILE)).unwrap();

    // 会话主键冲突 → 两条会话;源侧快照应挂到新会话,而不是基线那条
    let (baseline_session, source_session): (String, String) = conn
        .query_row(
            "SELECT (SELECT id FROM sessions WHERE title='基线会话'), \
                    (SELECT id FROM sessions WHERE title='源会话')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_ne!(baseline_session, source_session, "冲突会话必须换新 id");
    let source_snap_session: String = conn
        .query_row(
            "SELECT session_id FROM undo_snapshots WHERE payload LIKE '%source%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        source_snap_session, source_session,
        "源侧快照 session_id 未跟随会话重映射(会挂到基线同名会话)"
    );

    // 快照主键冲突 → 源侧快照换新 id;源侧工具调用必须跟随
    let source_snap_id: String = conn
        .query_row(
            "SELECT id FROM undo_snapshots WHERE payload LIKE '%source%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(source_snap_id, "snap-b", "冲突快照必须换新 id");
    let source_tc_snap: String = conn
        .query_row(
            "SELECT snapshot_id FROM tool_calls WHERE id = 'tc-s'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        source_tc_snap, source_snap_id,
        "源侧 tool_calls.snapshot_id 未跟随快照主键重映射"
    );
    // 基线侧绑定保持原样
    let baseline_tc_snap: String = conn
        .query_row(
            "SELECT snapshot_id FROM tool_calls WHERE id = 'tc-b'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(baseline_tc_snap, "snap-b");

    // 全部快照引用可解析(不存在悬空 snapshot_id)
    let dangling: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tool_calls WHERE snapshot_id IS NOT NULL \
             AND snapshot_id NOT IN (SELECT id FROM undo_snapshots)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dangling, 0, "合并后存在指向不存在快照的 tool_calls");
}

/// UNDOFIX2-L2:`seq` 重排。两侧同一会话各有一批 seq=1..2 的快照,原样搬入会
/// 产生并列值;回退范围按 `seq >= target` 界定,并列值会让行为不确定。
#[test]
fn merge_renumbers_overlapping_snapshot_seq() {
    let baseline = temp_dir("seq-baseline");
    let source = temp_dir("seq-source");
    let first_root = temp_dir("seq-first");
    let first = first_root.join("work");
    create_snapshot_db(&baseline, "同一会话");
    create_snapshot_db(&source, "同一会话");
    for (path, prefix) in [(&baseline, "b"), (&source, "s")] {
        let conn = Connection::open(path.join(DATABASE_FILE)).unwrap();
        for turn in 1..=2 {
            conn.execute(
                "INSERT INTO undo_snapshots(id, session_id, tool_name, payload, created_at, seq) \
                 VALUES(?1, 'session', 'update_variables', ?2, ?3, ?4)",
                rusqlite::params![
                    format!("{prefix}{turn}"),
                    format!("{{\"before\":\"{prefix}{turn}\"}}"),
                    format!("2026-01-0{turn}T00:00:00.000Z"),
                    turn,
                ],
            )
            .unwrap();
        }
    }

    merge_data_dirs(&baseline, &source, &first).unwrap();
    let conn = Connection::open(first.join(DATABASE_FILE)).unwrap();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM undo_snapshots", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, 4, "两侧快照应全部保留");
    let distinct: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT seq) FROM undo_snapshots WHERE session_id = 'session'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(distinct, 4, "同一会话内 seq 必须两两不同");
    // 相对顺序保持:每个前缀内部仍按 created_at 递增
    let ordered: Vec<String> = conn
        .prepare("SELECT id FROM undo_snapshots WHERE session_id = 'session' ORDER BY seq ASC")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        ordered,
        vec!["b1", "s1", "b2", "s2"],
        "应按时间重排: {ordered:?}"
    );
}
