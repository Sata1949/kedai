// 建表 SQL:与 Node 版(node:sqlite)建表完全一致,兼容现有 kedai.db(自 db.rs 迁入)

/// 全量建表语句(IF NOT EXISTS 幂等;含索引、种子行与表级注释)
pub(super) const CREATE_TABLES: &str = r#"
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

/// 暴露建表 SQL 供迁移一致性测试比对(旧库 ALTER 补列后应与新建表 schema normalize 一致)
pub fn create_tables_sql() -> &'static str {
    CREATE_TABLES
}
