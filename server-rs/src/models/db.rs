// SQLite 访问层:rusqlite,建表与 Node 版(node:sqlite)完全一致,兼容现有 kedai.db
// 并发模型(2026-08 DB 并发改造):单写连接(Mutex 串行) + 只读连接池(r2d2),
// WAL 模式下读不阻塞写、写不阻塞读;写锁中毒恢复沿用 unwrap_or_else(into_inner)。
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// 只读连接池 / 池化只读连接类型别名(简化调用点签名)
pub type ReadPool = r2d2::Pool<SqliteConnectionManager>;
pub type PooledRead = r2d2::PooledConnection<SqliteConnectionManager>;

const CREATE_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS characters (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  chara_name  TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  file_path   TEXT NOT NULL,
  avatar_path TEXT,
  data_raw    TEXT NOT NULL,
  created_at  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
  id           TEXT PRIMARY KEY,
  character_id TEXT NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
  title        TEXT NOT NULL DEFAULT '新会话',
  created_at   TEXT NOT NULL,
  updated_at   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  role       TEXT NOT NULL CHECK (role IN ('user','assistant','system')),
  content    TEXT NOT NULL,
  extra      TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, id);
CREATE TABLE IF NOT EXISTS agent_sessions (
  id         TEXT PRIMARY KEY,
  session_id TEXT NOT NULL UNIQUE REFERENCES sessions(id) ON DELETE CASCADE,
  state      TEXT NOT NULL,
  plan       TEXT NOT NULL DEFAULT '[]',
  steps      TEXT NOT NULL DEFAULT '[]',
  step_index INTEGER NOT NULL DEFAULT 0,
  agent_mode TEXT NOT NULL DEFAULT 'fast',
  started_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tool_calls (
  id               TEXT PRIMARY KEY,
  agent_session_id TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
  name             TEXT NOT NULL,
  input            TEXT NOT NULL,
  output           TEXT NOT NULL,
  duration_ms      INTEGER NOT NULL,
  created_at       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tool_calls_agent ON tool_calls(agent_session_id);
CREATE TABLE IF NOT EXISTS world_books (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  character_id TEXT REFERENCES characters(id) ON DELETE CASCADE,
  enabled      INTEGER NOT NULL DEFAULT 1,
  source       TEXT NOT NULL DEFAULT 'upload',
  entry_count  INTEGER NOT NULL DEFAULT 0,
  data_raw     TEXT NOT NULL,
  created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_world_books_character ON world_books(character_id);
CREATE TABLE IF NOT EXISTS skills (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  content     TEXT NOT NULL DEFAULT '',
  enabled     INTEGER NOT NULL DEFAULT 1,
  created_at  TEXT NOT NULL,
  allowed_tools TEXT NOT NULL DEFAULT '[]',
  run_as_subagent INTEGER NOT NULL DEFAULT 0,
  model       TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS agent_subtasks (
  id           TEXT PRIMARY KEY,
  session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  character_id TEXT NOT NULL DEFAULT '',
  name         TEXT NOT NULL DEFAULT '',
  instruction  TEXT NOT NULL DEFAULT '',
  status       TEXT NOT NULL DEFAULT 'pending',
  result       TEXT NOT NULL DEFAULT '',
  error        TEXT NOT NULL DEFAULT '',
  created_at   TEXT NOT NULL,
  updated_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_subtasks_session ON agent_subtasks(session_id);
-- 任务模式(task 工作台):任务主表 + 子任务表。
-- 子任务独立于 agent_subtasks(后者的 session_id 外键指向 sessions 表,任务 id 不在其中)。
CREATE TABLE IF NOT EXISTS tasks (
  id           TEXT PRIMARY KEY,
  title        TEXT NOT NULL,
  status       TEXT NOT NULL DEFAULT 'pending',
  plan         TEXT NOT NULL DEFAULT '[]',
  result       TEXT NOT NULL DEFAULT '',
  error        TEXT NOT NULL DEFAULT '',
  character_id TEXT,
  created_at   TEXT NOT NULL,
  updated_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_created ON tasks(created_at);
CREATE TABLE IF NOT EXISTS task_subtasks (
  id           TEXT PRIMARY KEY,
  task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  name         TEXT NOT NULL DEFAULT '',
  instruction  TEXT NOT NULL DEFAULT '',
  status       TEXT NOT NULL DEFAULT 'pending',
  result       TEXT NOT NULL DEFAULT '',
  error        TEXT NOT NULL DEFAULT '',
  created_at   TEXT NOT NULL,
  updated_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_subtasks_task ON task_subtasks(task_id);
CREATE TABLE IF NOT EXISTS session_vars (
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  key        TEXT NOT NULL,
  value      TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL,
  PRIMARY KEY (session_id, key)
);
CREATE TABLE IF NOT EXISTS session_assistant_vars (
  session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
  data_raw   TEXT NOT NULL DEFAULT '{}',
  updated_at TEXT NOT NULL
);
-- 7 作用域变量通用表(计划二):scope=global|chat|character|preset|message|script|extension。
-- chat 作用域以 session_assistant_vars 为规范存储,本表为兼容镜像(启动幂等回填);
-- 其余作用域以本表为规范存储。message 作用域 scope_id = messages.id(字符串)。
CREATE TABLE IF NOT EXISTS scope_variables (
  scope      TEXT NOT NULL,
  scope_id   TEXT NOT NULL DEFAULT '',
  data_raw   TEXT NOT NULL DEFAULT '{}',
  updated_at TEXT NOT NULL,
  PRIMARY KEY (scope, scope_id)
);
CREATE TABLE IF NOT EXISTS quick_replies (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  name       TEXT NOT NULL,
  label      TEXT NOT NULL DEFAULT '',
  content    TEXT NOT NULL DEFAULT '',
  enabled    INTEGER NOT NULL DEFAULT 1,
  position   INTEGER NOT NULL DEFAULT 0,
  sort_order INTEGER NOT NULL DEFAULT 100,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_quick_replies_name ON quick_replies(name);
-- 用户脚本(ScriptTree,阶段三):scope=global 或 character。
-- 角色级脚本亦可存于角色卡 data_raw.extensions.tavern_helper(兼容 ST 卡导出),
-- 本表仅承载「非随卡导出」的全局脚本;角色级以角色卡为规范存储(见 user_script_service)。
CREATE TABLE IF NOT EXISTS user_scripts (
  id         TEXT PRIMARY KEY,
  scope      TEXT NOT NULL,
  owner_id   TEXT NOT NULL DEFAULT '',
  data_json  TEXT NOT NULL DEFAULT '[]',
  updated_at TEXT NOT NULL,
  UNIQUE (scope, owner_id)
);
CREATE INDEX IF NOT EXISTS idx_user_scripts_owner ON user_scripts(scope, owner_id);
CREATE TABLE IF NOT EXISTS session_usage (
  session_id        TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
  total_prompt      INTEGER NOT NULL DEFAULT 0,
  total_completion  INTEGER NOT NULL DEFAULT 0,
  total_tokens      INTEGER NOT NULL DEFAULT 0,
  updated_at        TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS global_usage (
  id                INTEGER PRIMARY KEY CHECK (id = 1),
  total_prompt      INTEGER NOT NULL DEFAULT 0,
  total_completion  INTEGER NOT NULL DEFAULT 0,
  total_tokens      INTEGER NOT NULL DEFAULT 0,
  updated_at        TEXT NOT NULL
);
INSERT OR IGNORE INTO global_usage (id, total_prompt, total_completion, total_tokens, updated_at)
VALUES (1, 0, 0, 0, '');
CREATE TABLE IF NOT EXISTS session_compactions (
  session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  upto_message_id INTEGER NOT NULL,
  summary         TEXT NOT NULL DEFAULT '',
  model           TEXT NOT NULL DEFAULT '',
  created_at      TEXT NOT NULL,
  PRIMARY KEY (session_id, upto_message_id)
);
CREATE INDEX IF NOT EXISTS idx_session_compactions ON session_compactions(session_id);
CREATE TABLE IF NOT EXISTS llm_requests (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  run_id      TEXT NOT NULL,
  seq         INTEGER NOT NULL,
  payload     TEXT NOT NULL,
  model       TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL,
  prompt_cache_hit_tokens   INTEGER NOT NULL DEFAULT 0,
  prompt_cache_miss_tokens  INTEGER NOT NULL DEFAULT 0,
  prompt_tokens             INTEGER NOT NULL DEFAULT 0,
  completion_tokens         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_llm_requests_session ON llm_requests(session_id, seq);
-- 契约变更历史(阶段 C):append-only 审计 + 回滚源。
-- op_kind: replace(整体替换)| remove(移除);before/after 为契约 JSON 文本或 NULL。
CREATE TABLE IF NOT EXISTS contract_changelog (
  seq          INTEGER PRIMARY KEY AUTOINCREMENT,
  character_id TEXT NOT NULL,
  source       TEXT NOT NULL,
  op_kind      TEXT NOT NULL,
  path         TEXT NOT NULL DEFAULT 'contract',
  before_json  TEXT,
  after_json   TEXT,
  rationale    TEXT,
  created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_contract_changelog_char ON contract_changelog(character_id, seq);
-- 契约运行态(P5):KaleidoState 落库,每会话一行(会话级唯一事实源)。
-- meta_json 为 KaleidoMeta(pending/confidence 等);revision_hash 预留(sha2 未引入,先空串)。
CREATE TABLE IF NOT EXISTS kaleido_state (
  session_id       TEXT PRIMARY KEY,
  contract_version INTEGER NOT NULL,
  stat_data        TEXT NOT NULL,
  meta_json        TEXT NOT NULL,
  revision_seq     INTEGER NOT NULL DEFAULT 0,
  revision_hash    TEXT NOT NULL DEFAULT '',
  updated_at       TEXT NOT NULL
);
-- 契约运行态变更日志(P5):append-only 领域 changelog(§7-ChangelogEntry 逐 op 落账)。
-- entry_json 为 ChangelogEntry(回填真实 seq 后序列化);revision_seq 取本会话最大 seq。
CREATE TABLE IF NOT EXISTS kaleido_changelog (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  turn_id    INTEGER NOT NULL,
  entry_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_kaleido_changelog_session ON kaleido_changelog(session_id, seq);
-- 跨会话记忆蒸馏(2026-08 落地项 2):按角色维度共享的长期记忆条目。
-- kind: distilled(LLM 蒸馏)/ tool(agent memory_write 工具写入)/ manual(手动添加);
-- selected: 是否参与注入候选(0/1);usage_count/last_usage 为衰减精选排序键。
-- character_id 不设外键:记忆须活过会话生命周期由用户显式管理(删角色不级联清记忆),
-- 与 agent_subtasks.character_id 同策略;source_session_id 记录来源会话(可空)。
CREATE TABLE IF NOT EXISTS memory_entries (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  character_id      TEXT NOT NULL,
  source_session_id TEXT,
  kind              TEXT NOT NULL CHECK (kind IN ('distilled','tool','manual')),
  content           TEXT NOT NULL,
  usage_count       INTEGER NOT NULL DEFAULT 0,
  last_usage        TEXT,
  selected          INTEGER NOT NULL DEFAULT 1,
  created_at        TEXT NOT NULL,
  updated_at        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_memory_entries_character ON memory_entries(character_id, selected);
-- 回填游标元表(2026-08 DB 并发改造):记录启动幂等回填的增量进度。
-- 当前唯一 key:message_scope_last_id(messages.id 已回填边界,含 extra='{}' 的跳过行)。
CREATE TABLE IF NOT EXISTS backfill_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
"#;

pub struct Db {
    /// 唯一写连接:所有写 SQL 经此串行执行(沿用既有单连接语义)
    writer: Mutex<Connection>,
    /// 只读连接池:WAL 模式下读与写互不阻塞;池大小 = CPU 核数
    readers: ReadPool,
}

/// 暴露建表 SQL 供迁移一致性测试比对(旧库 ALTER 补列后应与新建表 schema normalize 一致)
pub fn create_tables_sql() -> &'static str {
    CREATE_TABLES
}

impl Db {
    pub fn open(db_path: &Path, data_dir: &Path) -> Result<Self, String> {
        if let Err(e) = std::fs::create_dir_all(data_dir) {
            return Err(format!("创建数据目录失败: {e}"));
        }
        let conn = Connection::open(db_path).map_err(|e| format!("打开数据库失败: {e}"))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("设置 WAL 失败: {e}"))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| format!("设置外键失败: {e}"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
        conn.execute_batch(CREATE_TABLES)
            .map_err(|e| format!("建表失败: {e}"))?;
        // 幂等 schema 升级(2026-08 缓存感知管线):旧库 llm_requests 补 usage 缓存列
        crate::migration::ensure_llm_requests_usage_columns(&conn)
            .map_err(|e| format!("升级 llm_requests 缓存列失败: {e}"))?;
        // 幂等 schema 升级(落地项 3 技能渐进披露):旧库 skills 补 allowed_tools 等列
        crate::migration::ensure_skills_progressive_columns(&conn)
            .map_err(|e| format!("升级 skills 渐进披露列失败: {e}"))?;
        backfill_scope_variables(&conn)?;

        // 只读连接池:READ_ONLY 标志防止读路径误写;busy_timeout/foreign_keys 与写连接对齐。
        // 写连接全程持有(WAL/-shm 存在),只读连 WAL 库在 SQLite ≥3.22 下安全。
        // 池大小取 CPU 核数(读多为短查询,更多连接只会争 IO)。
        let pool_size =
            std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
        let manager = SqliteConnectionManager::file(db_path)
            .with_flags(
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .with_init(|c| {
                c.busy_timeout(std::time::Duration::from_secs(5))?;
                c.pragma_update(None, "foreign_keys", "ON")?;
                Ok(())
            });
        let readers = ReadPool::builder()
            .max_size(pool_size)
            .build(manager)
            .map_err(|e| format!("创建只读连接池失败: {e}"))?;
        Ok(Db {
            writer: Mutex::new(conn),
            readers,
        })
    }

    /// 取只读连接(池化):SELECT 专用;连接归还由 Drop 完成
    pub fn read(&self) -> Result<PooledRead, String> {
        self.readers
            .get()
            .map_err(|e| format!("获取只读连接失败: {e}"))
    }

    /// 取写连接(互斥):写 SQL 与「读+写同事务」混合场景使用;锁中毒按既有纪律恢复
    pub fn write(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// 启动幂等回填(计划二 · 7 作用域变量;2026-08 起 message 部分改增量游标):
/// 把既有存储镜像进 scope_variables 表。
/// 全部使用 INSERT OR IGNORE(主键 (scope, scope_id) 已存在即跳过),可重复执行不产生重复行。
///
/// message 回填游标语义:backfill_meta['message_scope_last_id'] = 已扫描到的 messages.id
/// 上界(含 extra='{}' 无需回填的行)。启动只扫 id > 游标 的增量,扫完把游标推进到
/// MAX(messages.id)。无游标的旧库升级:若 scope_variables 已有 'message' 行,说明旧版本
/// 已做过全量回填,游标直接初始化为 MAX(messages.id),避免重复全表扫描。
fn backfill_scope_variables(conn: &Connection) -> Result<(), String> {
    // 1) chat 镜像:session_assistant_vars 整树 → scope_variables('chat', session_id)
    //    (单条 INSERT..SELECT,代价低,保留全量)
    conn.execute(
        "INSERT OR IGNORE INTO scope_variables (scope, scope_id, data_raw, updated_at)
         SELECT 'chat', session_id, data_raw, updated_at FROM session_assistant_vars",
        [],
    )
    .map_err(|e| format!("回填 chat 作用域镜像失败: {e}"))?;

    // 2) message 回填(增量):messages.extra.mvu.stat_data 子树 → scope_variables('message', id)
    const CURSOR_KEY: &str = "message_scope_last_id";
    let cursor: Option<i64> = conn
        .query_row(
            "SELECT value FROM backfill_meta WHERE key = ?1",
            [CURSOR_KEY],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| s.parse::<i64>().ok());
    let max_message_id: i64 = conn
        .query_row("SELECT COALESCE(MAX(id), 0) FROM messages", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("读取消息最大 id 失败: {e}"))?;
    let last_id = match cursor {
        Some(v) => v,
        None => {
            // 旧库兜底:已有 message 作用域回填行 → 视为旧版全量已完成,游标跳到当前上界
            let already_backfilled: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM scope_variables WHERE scope = 'message')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|v| v == 1)
                .unwrap_or(false);
            if already_backfilled {
                conn.execute(
                    "INSERT OR REPLACE INTO backfill_meta (key, value) VALUES (?1, ?2)",
                    rusqlite::params![CURSOR_KEY, max_message_id.to_string()],
                )
                .map_err(|e| format!("初始化 message 回填游标失败: {e}"))?;
                max_message_id
            } else {
                0 // 全新库或从未回填:全量扫描一次
            }
        }
    };
    if last_id >= max_message_id {
        return Ok(()); // 无增量
    }
    let mut stmt = conn
        .prepare("SELECT id, extra FROM messages WHERE id > ?1 AND extra != '{}'")
        .map_err(|e| format!("准备消息回填语句失败: {e}"))?;
    let rows = stmt
        .query_map([last_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("遍历消息回填失败: {e}"))?;
    for row in rows.flatten() {
        let (id, extra) = row;
        let stat_data = serde_json::from_str::<serde_json::Value>(&extra)
            .ok()
            .and_then(|v| v.get("mvu").and_then(|m| m.get("stat_data")).cloned());
        let Some(stat_data) = stat_data else { continue };
        if stat_data.is_null() {
            continue;
        }
        conn.execute(
            "INSERT OR IGNORE INTO scope_variables (scope, scope_id, data_raw, updated_at)
             VALUES ('message', ?1, ?2, ?3)",
            rusqlite::params![id.to_string(), stat_data.to_string(), now_iso()],
        )
        .map_err(|e| format!("回填 message 作用域失败: {e}"))?;
    }
    // 游标推进到消息表上界(不是回填行最大 id):extra='{}' 的行也已确认无需回填
    conn.execute(
        "INSERT OR REPLACE INTO backfill_meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![CURSOR_KEY, max_message_id.to_string()],
    )
    .map_err(|e| format!("更新 message 回填游标失败: {e}"))?;
    Ok(())
}

/// ISO 时间戳(与 Node 版 new Date().toISOString() 对齐,UTC)
pub fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个带 character+session 的临时库,返回 (db, 目录, session_id)
    fn fixture(tag: &str) -> (Db, std::path::PathBuf, &'static str) {
        let dir = std::env::temp_dir().join(format!("kedai-db-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir.join("kedai.db"), &dir).unwrap();
        let conn = db.write();
        conn.execute(
            "INSERT INTO characters (id, name, chara_name, description, file_path, data_raw, created_at)
             VALUES ('c1', 'c', 'c', '', '', '{}', '')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, character_id, title, created_at, updated_at)
             VALUES ('s1', 'c1', 't', '', '')",
            [],
        )
        .unwrap();
        drop(conn);
        (db, dir, "s1")
    }

    fn add_msg(db: &Db, session_id: &str, extra: &str) -> i64 {
        let conn = db.write();
        conn.execute(
            "INSERT INTO messages (session_id, role, content, extra, created_at) VALUES (?1, 'user', 'x', ?2, '')",
            rusqlite::params![session_id, extra],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn cursor_of(db: &Db) -> Option<i64> {
        let conn = db.read().unwrap();
        conn.query_row(
            "SELECT value FROM backfill_meta WHERE key = 'message_scope_last_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| s.parse().ok())
    }

    fn scope_row(db: &Db, scope_id: &str) -> Option<String> {
        let conn = db.read().unwrap();
        conn.query_row(
            "SELECT data_raw FROM scope_variables WHERE scope = 'message' AND scope_id = ?1",
            [scope_id],
            |row| row.get(0),
        )
        .ok()
    }

    /// 增量回填:首批消息回填后,追加新消息再 open 只扫增量(游标推进,新行入库)
    #[test]
    fn backfill_message_scope_is_incremental() {
        let (db, dir, sid) = fixture("incr");
        let extra = r#"{"mvu":{"stat_data":{"hp":10}}}"#;
        let id1 = add_msg(&db, sid, extra);
        let id_empty = add_msg(&db, sid, "{}"); // 无需回填的行也要被游标覆盖
        drop(db);

        // 重新 open:首次回填(无游标、无存量 message 行 → 全量)
        let db = Db::open(&dir.join("kedai.db"), &dir).unwrap();
        assert_eq!(
            scope_row(&db, &id1.to_string()).as_deref(),
            Some(r#"{"hp":10}"#)
        );
        assert!(scope_row(&db, &id_empty.to_string()).is_none());
        assert_eq!(cursor_of(&db), Some(id_empty), "游标应推进到消息表上界");

        // 追加新消息后再次 open:只扫增量
        let id2 = add_msg(&db, sid, extra);
        drop(db);
        let db = Db::open(&dir.join("kedai.db"), &dir).unwrap();
        assert_eq!(
            scope_row(&db, &id2.to_string()).as_deref(),
            Some(r#"{"hp":10}"#),
            "新增消息应被增量回填"
        );
        assert_eq!(cursor_of(&db), Some(id2));
        drop(db);
        std::fs::remove_dir_all(dir).ok();
    }

    /// 旧库升级兜底:无游标但 scope_variables 已有 message 行(旧版全量已跑过)
    /// → 游标直接初始化为 MAX(messages.id),不做重复全扫
    #[test]
    fn backfill_cursor_initialized_from_existing_rows() {
        let (db, dir, sid) = fixture("legacy");
        let extra = r#"{"mvu":{"stat_data":{"hp":1}}}"#;
        let id1 = add_msg(&db, sid, extra);
        // 模拟旧版回填产物:scope_variables 已有 message 行,但无 backfill_meta 游标
        {
            let conn = db.write();
            conn.execute(
                "INSERT OR REPLACE INTO scope_variables (scope, scope_id, data_raw, updated_at)
                 VALUES ('message', ?1, '{\"hp\":1}', '')",
                [id1.to_string()],
            )
            .unwrap();
            conn.execute("DELETE FROM backfill_meta WHERE key = 'message_scope_last_id'", [])
                .unwrap();
        }
        let id2 = add_msg(&db, sid, extra); // 游标缺失后追加的消息
        drop(db);

        let db = Db::open(&dir.join("kedai.db"), &dir).unwrap();
        // 游标应直接初始化到 MAX(id),id2 不补扫(与旧版全量语义一致:不重复回填)
        assert_eq!(cursor_of(&db), Some(id2));
        // 既有行不被覆盖(INSERT OR IGNORE 语义保持)
        assert_eq!(scope_row(&db, &id1.to_string()).as_deref(), Some(r#"{"hp":1}"#));
        drop(db);
        std::fs::remove_dir_all(dir).ok();
    }

    /// 读写分离:写连接已提交的写入,只读池连接立即可见(WAL)
    #[test]
    fn read_pool_sees_committed_writes() {
        let (db, dir, sid) = fixture("rw");
        let id = add_msg(&db, sid, "{}");
        let conn = db.read().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id = ?1 AND session_id = ?2",
                rusqlite::params![id, sid],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        drop(conn);
        drop(db);
        std::fs::remove_dir_all(dir).ok();
    }
}
