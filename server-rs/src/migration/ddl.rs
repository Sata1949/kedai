// 幂等 DDL 升级:新增表 DDL 常量与 ensure_* 补列/补表函数(缺失才执行,已存在跳过)。
use rusqlite::Connection;

/// 计划二新增的 7 作用域变量表。合并前对两侧各补一次 DDL,避免旧版本数据库
/// (无此表)与新版本数据库合并时因「基线缺少源表」停止(§4.3)。
pub(super) const SCOPE_VARIABLES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS scope_variables (
  scope      TEXT NOT NULL,
  scope_id   TEXT NOT NULL DEFAULT '',
  data_raw   TEXT NOT NULL DEFAULT '{}',
  updated_at TEXT NOT NULL,
  PRIMARY KEY (scope, scope_id)
)"#;
/// 阶段三新增的用户脚本表(ScriptTree 全局脚本)。合并前对两侧各补一次 DDL,
/// 与 SCOPE_VARIABLES_DDL 同理,避免旧库与新库合并时因「基线缺少源表」停止(§4.3)。
pub(super) const USER_SCRIPTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS user_scripts (
  id         TEXT PRIMARY KEY,
  scope      TEXT NOT NULL,
  owner_id   TEXT NOT NULL DEFAULT '',
  data_json  TEXT NOT NULL DEFAULT '[]',
  updated_at TEXT NOT NULL,
  UNIQUE (scope, owner_id)
)"#;
/// 上下文压缩摘要表。合并前对两侧各补一次 DDL,与 SCOPE_VARIABLES_DDL 同理,
/// 避免旧库与新库合并时因「基线缺少源表」停止(§4.3)。
pub(super) const SESSION_COMPACTIONS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS session_compactions (
  session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  upto_message_id INTEGER NOT NULL,
  summary         TEXT NOT NULL DEFAULT '',
  model           TEXT NOT NULL DEFAULT '',
  created_at      TEXT NOT NULL,
  PRIMARY KEY (session_id, upto_message_id)
)"#;
/// LLM 请求快照表(第四点·主题 A):记录每次真正下发给模型的完整消息数组。
/// 合并前对两侧各补一次 DDL,避免旧库与新库合并时因「基线缺少源表」停止(§4.3)。
pub(super) const LLM_REQUESTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS llm_requests (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  run_id      TEXT NOT NULL,
  seq         INTEGER NOT NULL,
  payload     TEXT NOT NULL,
  model       TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL
)"#;
/// llm_requests 的 usage 缓存列(2026-08 缓存感知管线):(列名, DDL 片段)。
/// 旧库经 ALTER ADD COLUMN 补齐;列追加在表尾,与新版 CREATE_TABLES 建出的
/// schema normalize 后一致,保证跨库合并的 schema 一致性比对不冲突。
const LLM_REQUESTS_USAGE_COLUMNS: [(&str, &str); 4] = [
    (
        "prompt_cache_hit_tokens",
        "prompt_cache_hit_tokens INTEGER NOT NULL DEFAULT 0",
    ),
    (
        "prompt_cache_miss_tokens",
        "prompt_cache_miss_tokens INTEGER NOT NULL DEFAULT 0",
    ),
    ("prompt_tokens", "prompt_tokens INTEGER NOT NULL DEFAULT 0"),
    (
        "completion_tokens",
        "completion_tokens INTEGER NOT NULL DEFAULT 0",
    ),
];

/// 幂等 schema 升级:为 llm_requests 补 usage 缓存列(缺失才 ALTER,已存在跳过)。
/// 启动时(Db::open)与跨库合并前(merge_databases 两侧)各执行一次。
pub fn ensure_llm_requests_usage_columns(conn: &Connection) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("PRAGMA table_info(llm_requests)")
            .map_err(|e| format!("读取 llm_requests 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 llm_requests 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name.to_lowercase());
        }
    }
    for (name, ddl) in LLM_REQUESTS_USAGE_COLUMNS {
        if existing.iter().any(|c| c == name) {
            continue;
        }
        conn.execute(&format!("ALTER TABLE llm_requests ADD COLUMN {ddl}"), [])
            .map_err(|e| format!("为 llm_requests 补列 {name} 失败: {e}"))?;
    }
    Ok(())
}

/// skills 渐进披露列(落地项 3):(列名, DDL 片段)。旧库经 ALTER ADD COLUMN 补齐;
/// 列追加在表尾,与新版 CREATE_TABLES 建出的 schema normalize 后一致。
const SKILLS_PROGRESSIVE_COLUMNS: [(&str, &str); 3] = [
    ("allowed_tools", "allowed_tools TEXT NOT NULL DEFAULT '[]'"),
    (
        "run_as_subagent",
        "run_as_subagent INTEGER NOT NULL DEFAULT 0",
    ),
    ("model", "model TEXT NOT NULL DEFAULT ''"),
];

/// 幂等 schema 升级:为 skills 补渐进披露列(缺失才 ALTER,已存在跳过)。
/// 启动时(Db::open)与跨库合并前(merge_databases 两侧)各执行一次。
/// skills 表不存在(极旧库/测试手工建库)时零列返回,直接跳过——
/// 建表由 create_tables_sql 或合并期 schema 比对负责,此处不能 ALTER 报错。
pub fn ensure_skills_progressive_columns(conn: &Connection) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("PRAGMA table_info(skills)")
            .map_err(|e| format!("读取 skills 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 skills 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name.to_lowercase());
        }
    }
    if existing.is_empty() {
        return Ok(());
    }
    for (name, ddl) in SKILLS_PROGRESSIVE_COLUMNS {
        if existing.iter().any(|c| c == name) {
            continue;
        }
        conn.execute(&format!("ALTER TABLE skills ADD COLUMN {ddl}"), [])
            .map_err(|e| format!("为 skills 补列 {name} 失败: {e}"))?;
    }
    Ok(())
}
/// 幂等 schema 升级(批次 4 六模式):为 tasks 补 task_mode 列(缺失才 ALTER,
/// 已存在跳过)。启动时(Db::open)执行;旧行经 DEFAULT 'legacy' 零迁移成本。
/// 列追加在表尾,与新版 CREATE_TABLES 建出的 schema normalize 后一致。
pub fn ensure_tasks_task_mode_column(conn: &Connection) -> Result<(), String> {
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
    if existing.iter().any(|c| c == "task_mode") {
        return Ok(());
    }
    conn.execute(
        "ALTER TABLE tasks ADD COLUMN task_mode TEXT NOT NULL DEFAULT 'legacy'",
        [],
    )
    .map_err(|e| format!("为 tasks 补 task_mode 列失败: {e}"))?;
    Ok(())
}

/// 幂等 schema 升级(执行者库):为 tasks 补 executor_id 列(缺失才 ALTER,已存在跳过)。
/// 启动时(Db::open)与跨库合并前(merge_databases 两侧)各执行一次。
/// **可空且无 DEFAULT**:NULL 表示「未指定执行者」(通用执行者),
/// 与旧行语义一致,故零迁移成本——旧任务不需要任何回填。
pub fn ensure_tasks_executor_id_column(conn: &Connection) -> Result<(), String> {
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
    if existing.iter().any(|c| c == "executor_id") {
        return Ok(());
    }
    conn.execute("ALTER TABLE tasks ADD COLUMN executor_id TEXT", [])
        .map_err(|e| format!("为 tasks 补 executor_id 列失败: {e}"))?;
    Ok(())
}

/// 幂等 schema 升级(二维批次 5a 任务绑定流程 + 7b 对比模式):为 tasks 补 flow_id、
/// flow_snapshot 与 flow_ids 三列(各自缺失才 ALTER,已存在跳过)。启动时(Db::open)
/// 与跨库合并前(merge_databases 两侧)各执行一次。
///
/// **三列都可空、无 DEFAULT**:NULL = 「未绑定流程 / 尚无快照 / 强制模式」,与旧行语义一致,
/// 故零迁移成本——旧任务不需要任何回填(未绑定的任务到执行开始时才按当时的当前流程
/// 捕获快照)。列序固定为 flow_id → flow_snapshot → flow_ids,与新建库的建表顺序一致
/// (schema normalize 比对依赖追加顺序)。
pub fn ensure_tasks_flow_columns(conn: &Connection) -> Result<(), String> {
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
    if !existing.iter().any(|c| c == "flow_id") {
        conn.execute("ALTER TABLE tasks ADD COLUMN flow_id TEXT", [])
            .map_err(|e| format!("为 tasks 补 flow_id 列失败: {e}"))?;
    }
    if !existing.iter().any(|c| c == "flow_snapshot") {
        conn.execute("ALTER TABLE tasks ADD COLUMN flow_snapshot TEXT", [])
            .map_err(|e| format!("为 tasks 补 flow_snapshot 列失败: {e}"))?;
    }
    if !existing.iter().any(|c| c == "flow_ids") {
        conn.execute("ALTER TABLE tasks ADD COLUMN flow_ids TEXT", [])
            .map_err(|e| format!("为 tasks 补 flow_ids 列失败: {e}"))?;
    }
    // 任务级连接(A 批 B1):同样追加在表尾,与新版建表顺序一致(schema 归一化比对依赖)
    if !existing.iter().any(|c| c == "connection_id") {
        conn.execute("ALTER TABLE tasks ADD COLUMN connection_id TEXT", [])
            .map_err(|e| format!("为 tasks 补 connection_id 列失败: {e}"))?;
    }
    Ok(())
}

/// 幂等 schema 升级(编码通道批次):为 tasks 补 workspace 列(缺失才 ALTER,已存在跳过)。
/// 启动时(Db::open)与跨库合并前(merge_databases 两侧)各执行一次。
/// **可空且无 DEFAULT**:NULL 表示「未绑定工作区」,与旧行语义一致,
/// 故零迁移成本——旧任务不需要任何回填。列在表尾,与新版建表顺序一致
/// (schema 归一化比对依赖)。
pub fn ensure_tasks_workspace_column(conn: &Connection) -> Result<(), String> {
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
    if existing.iter().any(|c| c == "workspace") {
        return Ok(());
    }
    conn.execute("ALTER TABLE tasks ADD COLUMN workspace TEXT", [])
        .map_err(|e| format!("为 tasks 补 workspace 列失败: {e}"))?;
    Ok(())
}

/// 幂等 schema 升级(可观测性问题①):为 task_llm_calls 补 finish_reason 列
///(缺失才 ALTER,已存在跳过)。启动时(Db::open)与跨库合并前(merge_databases
/// 两侧)各执行一次;旧行经 DEFAULT '' 零迁移成本('' = 未知/未下发,
/// 不等于 stop,避免旧数据被误读为正常收尾)。
/// 列追加在表尾,与新版 CREATE_TABLES 建出的 schema normalize 后一致。
/// task_llm_calls 表不存在(极旧快照/手工建库)时零列返回,直接跳过——
/// 建表由 create_tables_sql 或合并期 schema 比对负责,此处不能 ALTER 报错。
pub fn ensure_task_llm_calls_finish_reason_column(conn: &Connection) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("PRAGMA table_info(task_llm_calls)")
            .map_err(|e| format!("读取 task_llm_calls 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 task_llm_calls 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name.to_lowercase());
        }
    }
    if existing.is_empty() {
        return Ok(());
    }
    if existing.iter().any(|c| c == "finish_reason") {
        return Ok(());
    }
    conn.execute(
        "ALTER TABLE task_llm_calls ADD COLUMN finish_reason TEXT NOT NULL DEFAULT ''",
        [],
    )
    .map_err(|e| format!("为 task_llm_calls 补 finish_reason 列失败: {e}"))?;
    Ok(())
}

/// 列补全的共用实现(2026-09-16 批次 4):两张子任务表要补同一个 `finished_at`,
/// 逐个手抄一遍 PRAGMA 探测会把「表不存在直接跳过」这条易错的边界抄漏。
///
/// 语义与既有 ensure_* 列迁移一致:
/// - 表不存在(极旧快照/手工建库/测试手工建库)时**零列返回**,交建表批负责,不得 ALTER 报错;
/// - 列已存在即跳过(幂等,重复执行不产生重复列);
/// - 列追加在表尾,与新版 CREATE_TABLES 建出的 schema normalize 后一致
///   (跨库合并的 schema 一致性比对依赖此点)。
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    column_type: &str,
) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|e| format!("读取 {table} 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 {table} 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name.to_lowercase());
        }
    }
    if existing.is_empty() {
        return Ok(());
    }
    if existing.iter().any(|c| c == column) {
        return Ok(());
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {column_type}"),
        [],
    )
    .map_err(|e| format!("为 {table} 补 {column} 列失败: {e}"))?;
    Ok(())
}

/// 幂等 schema 升级(2026-09-17 P-11):为 characters 补 `derived_json` 派生列。
///
/// 用途:缓存从 `data_raw` 提取的 6 个派生字段(first_mes / alternate_greetings /
/// regex_scripts / creator / character_version / creator_notes),让列表接口**零 JSON 解析**
/// ——此前无论列表还是详情都对每张卡做整份 `serde_json::from_str`,列表成本是
/// O(卡数 × 卡体积)。
///
/// **该列是缓存而非事实源**:读取侧在列为空/非法时回退解析 `data_raw`,故:
/// - 旧库未回填、旁路写入未同步等情形只会「慢」,不会显示错数据;
/// - 回填是性能必需而非正确性必需,可增量推进;
/// - 新旧版本代码混用安全(旧代码 SELECT 全为显式列名,不读该列)。
///
/// 带 DEFAULT(不是可空列):`merge.rs` 的 `merge_table` 用「源表全列 + 动态占位符」插入,
/// 旧快照行不含该列,靠默认值补齐;旧库 ALTER 时既有行也自动取默认值(零迁移成本)。
/// 类型串与 `CREATE_TABLES` 内**逐字一致**,否则 `PRAGMA table_info` 的 dflt 指纹不等即红。
pub fn ensure_characters_derived_json_column(conn: &Connection) -> Result<(), String> {
    add_column_if_missing(
        conn,
        "characters",
        "derived_json",
        "TEXT NOT NULL DEFAULT '{}'",
    )
}

/// 幂等 schema 升级(批次 4 子任务终态语义):为 agent_subtasks 补 `finished_at` 列。
///
/// 用途:让「已完成再被 agentend 召回」与「中途被召回」可分——前者保持 `done` 且
/// `finished_at` 非空,只有后者才落 `ended`。旧行经 DEFAULT '' 零迁移成本
/// (`''` = 未知/该列引入前完成,不等于「未完成」,不反推语义)。
/// 启动时(Db::open)与跨库合并前(merge_databases 两侧)各执行一次。
pub fn ensure_agent_subtasks_finished_at_column(conn: &Connection) -> Result<(), String> {
    add_column_if_missing(
        conn,
        "agent_subtasks",
        "finished_at",
        "TEXT NOT NULL DEFAULT ''",
    )
}

/// 幂等 schema 升级(批次 4):为 task_subtasks 补 `finished_at` 列。
///
/// task_subtasks 是 legacy 路径下 agent_subtasks 的等价表,契约映射
/// (tools/check-contract.mjs 的「任务子任务」组)与迁移期 schema 指纹比对都要求
/// 两表同形状——只补一张会让跨库合并报「基线缺少列」。
pub fn ensure_task_subtasks_finished_at_column(conn: &Connection) -> Result<(), String> {
    add_column_if_missing(
        conn,
        "task_subtasks",
        "finished_at",
        "TEXT NOT NULL DEFAULT ''",
    )
}

/// 契约变更历史表(阶段 C):append-only 审计 + 回滚源。合并前对两侧各补一次 DDL,
/// 与 SCOPE_VARIABLES_DDL 同理,避免旧库与新库合并时因「基线缺少源表」停止(§4.3)。
/// 索引不参与 schema 一致性比对(schema_map 仅读 type='table'),无需在此重复。
/// 注意:seq 仅库内局部有效——跨库合并(merge_databases)时主键冲突会被重映射,
/// rationale 中「回滚自 seq N」仅作提示,合并后时间线以 created_at 为准。
pub(super) const CONTRACT_CHANGELOG_DDL: &str = r#"
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
)"#;
/// 契约运行态表(P5):kaleido_state(每会话一行)+ kaleido_changelog(逐 op append)。
/// 合并前对两侧各补一次 DDL,与 SCOPE_VARIABLES_DDL 同理(§4.3);索引不参与
/// schema 一致性比对(schema_map 仅读 type='table')。kaleido_state 按业务键
/// session_id 单列主键,合并冲突走 INSERT 重映射路径(与其他 TEXT 主键表一致)。
pub(super) const KALEIDO_STATE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS kaleido_state (
  session_id       TEXT PRIMARY KEY,
  contract_version INTEGER NOT NULL,
  stat_data        TEXT NOT NULL,
  meta_json        TEXT NOT NULL,
  revision_seq     INTEGER NOT NULL DEFAULT 0,
  revision_hash    TEXT NOT NULL DEFAULT '',
  updated_at       TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS kaleido_changelog (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  turn_id    INTEGER NOT NULL,
  entry_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_kaleido_changelog_session ON kaleido_changelog(session_id, seq);
"#;
/// 跨会话记忆蒸馏表(落地项 2):memory_entries。合并前对两侧各补一次 DDL,
/// 与 SCOPE_VARIABLES_DDL 同理(§4.3);索引不参与 schema 一致性比对。
/// 正文只有 content 一列(蒸馏产物即短记忆行,无 raw/summary 拆分——若未来需要
/// 保留原文,拆两条或 ALTER 补列,normalize_sql 已兼容表尾追加列)。
pub(super) const MEMORY_ENTRIES_DDL: &str = r#"
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
  updated_at        TEXT NOT NULL,
  pinned            INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_memory_entries_character ON memory_entries(character_id, selected);
"#;

/// 记忆全文检索(升级工作流 B1):FTS5 外部内容表 + 3 个同步 trigger。
/// 与 models/db/schema.rs CREATE_TABLES 内的文本保持一致(normalize 比对依赖);
/// FTS 虚拟表与其影子表不参与合并比对(见 merge.rs is_fts_table)。
pub(super) const MEMORY_ENTRIES_FTS_DDL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS memory_entries_fts USING fts5(
  content,
  content='memory_entries',
  content_rowid='id',
  tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS memory_entries_ai AFTER INSERT ON memory_entries BEGIN
  INSERT INTO memory_entries_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER IF NOT EXISTS memory_entries_ad AFTER DELETE ON memory_entries BEGIN
  INSERT INTO memory_entries_fts(memory_entries_fts, rowid, content)
    VALUES('delete', old.id, old.content);
END;
CREATE TRIGGER IF NOT EXISTS memory_entries_au AFTER UPDATE ON memory_entries BEGIN
  INSERT INTO memory_entries_fts(memory_entries_fts, rowid, content)
    VALUES('delete', old.id, old.content);
  INSERT INTO memory_entries_fts(rowid, content) VALUES (new.id, new.content);
END;
"#;

/// 幂等 schema 升级(B2 分层注入):为 memory_entries 补 pinned 列(缺失才 ALTER,
/// 已存在跳过)。列追加在表尾,与新版 CREATE_TABLES 建出的 schema normalize 后一致。
/// memory_entries 表不存在时零列返回直接跳过(建表由 CREATE_TABLES 负责)。
pub fn ensure_memory_entries_pinned_column(conn: &Connection) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("PRAGMA table_info(memory_entries)")
            .map_err(|e| format!("读取 memory_entries 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 memory_entries 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name.to_lowercase());
        }
    }
    if existing.is_empty() || existing.iter().any(|c| c == "pinned") {
        return Ok(());
    }
    conn.execute(
        "ALTER TABLE memory_entries ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
        [],
    )
    .map_err(|e| format!("为 memory_entries 补 pinned 列失败: {e}"))?;
    Ok(())
}

/// 幂等回填(B1 检索底座):FTS5 外部内容表建好后执行一次 rebuild,把旧库已有记忆
/// 灌进索引;backfill_meta 记标记避免每次启动重跑(rebuild 是 O(全表),不能进热启动路径)。
/// 新建库(空表)rebuild 代价可忽略,同样只做一次。
pub fn ensure_memory_entries_fts_backfill(conn: &Connection) -> Result<(), String> {
    const KEY: &str = "memory_fts_rebuilt";
    let done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM backfill_meta WHERE key = ?1)",
            [KEY],
            |row| row.get::<_, i64>(0),
        )
        .map(|v| v == 1)
        .unwrap_or(false);
    if done {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO memory_entries_fts(memory_entries_fts) VALUES('rebuild')",
        [],
    )
    .map_err(|e| format!("记忆全文索引回填失败: {e}"))?;
    conn.execute(
        "INSERT OR REPLACE INTO backfill_meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![KEY, "1"],
    )
    .map_err(|e| format!("写入记忆索引回填标记失败: {e}"))?;
    Ok(())
}
/// 幂等回填(2026-09-17 P-11):为已存在的角色行计算 `derived_json`。
///
/// 与 `ensure_characters_derived_json_column` 配套:加列让旧库有该列(既有行取默认 `'{}'`),
/// 本函数把 `'{}'` 的行逐行补齐。`backfill_meta` 记标记,只做一次。
///
/// **必须事务外执行**(`models/db/mod.rs` 的调用点已按此约定):逐行读 data_raw + 解析 +
/// 回写,大库下是一次长写,放进升级事务会把 WAL 撑大(与 FTS rebuild 同理)。
///
/// 定位是**性能回填而非正确性回填**:中途失败、漏掉某些行都不影响正确性——列表读取侧在
/// 列为空/非法时回退解析 data_raw。故此处失败只留痕、不阻断启动。
pub fn ensure_characters_derived_backfill(conn: &Connection) -> Result<(), String> {
    const KEY: &str = "characters_derived_backfilled";
    let done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM backfill_meta WHERE key = ?1)",
            [KEY],
            |row| row.get::<_, i64>(0),
        )
        .map(|v| v == 1)
        .unwrap_or(false);
    if done {
        return Ok(());
    }
    // 收集待回填行的 (id, data_raw):先读完再写,避免在遍历游标上同时执行 UPDATE
    let mut pending: Vec<(String, String)> = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, data_raw FROM characters WHERE derived_json = '{}'")
            .map_err(|e| format!("读取待回填角色失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("遍历待回填角色失败: {e}"))?;
        for r in rows.flatten() {
            pending.push(r);
        }
    }
    for (id, raw) in &pending {
        // data_raw 非法 JSON:保持 '{}'(读取侧会回退解析,且此处无法算出派生值)
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let derived = crate::parsing::character_derived::compute_derived_json(&value);
        conn.execute(
            "UPDATE characters SET derived_json = ?1 WHERE id = ?2",
            rusqlite::params![derived, id],
        )
        .map_err(|e| format!("回填角色 {id} 派生列失败: {e}"))?;
    }
    conn.execute(
        "INSERT OR REPLACE INTO backfill_meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![KEY, "1"],
    )
    .map_err(|e| format!("写入角色派生列回填标记失败: {e}"))?;
    Ok(())
}

/// 任务消息表(批次 R2 多轮用户输入):任务全程的用户输入(followup 追加指令 /
/// plan_chat 批准环节对话)与助手产出按行落库,任务删除随外键级联清除。
/// 合并前对两侧各补一次 DDL,与 SCOPE_VARIABLES_DDL 同理,
/// 避免旧库与新库合并时因「基线缺少源表」停止(§4.3);索引不参与 schema 一致性比对。
/// 文本与 models/db/schema.rs CREATE_TABLES 内的建表语句保持一致(normalize 比对依赖)。
const TASK_MESSAGES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS task_messages (
  id         TEXT PRIMARY KEY,
  task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  role       TEXT NOT NULL CHECK (role IN ('user','assistant')),
  kind       TEXT NOT NULL DEFAULT 'normal',
  content    TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_messages_task ON task_messages(task_id, created_at);
"#;

/// 幂等 schema 升级(批次 R2):task_messages 表缺失才建(PRAGMA table_info 探测,
/// 已存在跳过),与 ensure_*_column 同款探测模式,只是对象从列升级为整表。
/// 启动时(Db::open)与跨库合并前(merge_databases 两侧)各执行一次。
pub fn ensure_task_messages_table(conn: &Connection) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("PRAGMA table_info(task_messages)")
            .map_err(|e| format!("读取 task_messages 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 task_messages 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name);
        }
    }
    // 表不存在时 PRAGMA 返回零行(与 ensure_skills_progressive_columns 同口径)
    if !existing.is_empty() {
        return Ok(());
    }
    conn.execute_batch(TASK_MESSAGES_DDL)
        .map_err(|e| format!("创建 task_messages 表失败: {e}"))?;
    Ok(())
}

/// 任务文件变更台账(2026-09-30 批次 4,PRODCAP-4):工作区文件每次被改动记一行,
/// 供任务详情展示「改了什么」/ 查看 diff / 单文件回滚。
/// 文本与 models/db/schema.rs CREATE_TABLES 内的建表语句保持一致(normalize 比对依赖)。
const TASK_FILE_CHANGES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS task_file_changes (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  step_index   INTEGER,
  path         TEXT NOT NULL,
  op           TEXT NOT NULL CHECK (op IN ('create','modify','delete','rollback')),
  source       TEXT NOT NULL CHECK (source IN ('tool','bash','rollback')),
  before_hash  TEXT,
  after_hash   TEXT NOT NULL,
  before_bytes INTEGER NOT NULL DEFAULT 0,
  after_bytes  INTEGER NOT NULL DEFAULT 0,
  before_blob  BLOB,
  truncated    INTEGER NOT NULL DEFAULT 0,
  created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_file_changes_task ON task_file_changes(task_id, id);
"#;

/// 幂等 schema 升级:旧库补建 task_file_changes 表(PRAGMA 探测,已存在跳过)。
/// 启动时(Db::open)与跨库合并前(merge_databases 两侧)各执行一次。
pub fn ensure_task_file_changes_table(conn: &Connection) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("PRAGMA table_info(task_file_changes)")
            .map_err(|e| format!("读取 task_file_changes 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 task_file_changes 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name);
        }
    }
    if !existing.is_empty() {
        return Ok(());
    }
    conn.execute_batch(TASK_FILE_CHANGES_DDL)
        .map_err(|e| format!("创建 task_file_changes 表失败: {e}"))?;
    Ok(())
}

/// 任务扫描标记(2026-09-30 批次 4b):bash 侧树扫描单次未能完整检出时记一行。
/// 文本与 models/db/schema.rs CREATE_TABLES 内的建表语句保持一致(normalize 比对依赖)。
const TASK_SCAN_MARKS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS task_scan_marks (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  reason     TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_scan_marks_task ON task_scan_marks(task_id, id);
"#;

/// 幂等 schema 升级:旧库补建 task_scan_marks 表(PRAGMA 探测,已存在跳过)。
/// 启动时(Db::open)与跨库合并前(merge_databases 两侧)各执行一次——
/// **两侧各调一次是硬要求**:N-1 库与当前库互相合并时,漏补的一侧会被
/// schema 比对判为「基线缺少源表」并中止合并。
pub fn ensure_task_scan_marks_table(conn: &Connection) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("PRAGMA table_info(task_scan_marks)")
            .map_err(|e| format!("读取 task_scan_marks 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 task_scan_marks 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name);
        }
    }
    if !existing.is_empty() {
        return Ok(());
    }
    conn.execute_batch(TASK_SCAN_MARKS_DDL)
        .map_err(|e| format!("创建 task_scan_marks 表失败: {e}"))?;
    Ok(())
}

/// 任务事件表(PRODCAP-1,2026-10-05):任务生命周期事件按任务内单调 seq 落库。
/// 文本与 models/db/schema.rs CREATE_TABLES 内的建表语句保持一致(normalize 比对依赖)。
const TASK_EVENTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS task_events (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id       TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  seq           INTEGER NOT NULL,
  kind          TEXT NOT NULL,
  title         TEXT,
  status        TEXT,
  detail        TEXT,
  finish_reason TEXT,
  phase         TEXT,
  step_index    INTEGER,
  created_at    TEXT NOT NULL,
  UNIQUE(task_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_task_events_task_seq ON task_events(task_id, seq);
"#;

/// 幂等 schema 升级:旧库补建 task_events 表(PRAGMA 探测,已存在跳过)。
/// 启动时(Db::open)与跨库合并前(merge_databases 两侧)各执行一次——
/// **两侧各调一次是硬要求**:N-1 库与当前库互相合并时,漏补的一侧会被
/// schema 比对判为「基线缺少源表」并中止合并。
pub fn ensure_task_events_table(conn: &Connection) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare("PRAGMA table_info(task_events)")
            .map_err(|e| format!("读取 task_events 列失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("遍历 task_events 列失败: {e}"))?;
        for name in rows.flatten() {
            existing.push(name);
        }
    }
    if !existing.is_empty() {
        return Ok(());
    }
    conn.execute_batch(TASK_EVENTS_DDL)
        .map_err(|e| format!("创建 task_events 表失败: {e}"))?;
    Ok(())
}

/// 命令执行审计表(bash 工具与 Android 执行层):每次尝试执行(含被拒绝的)
/// 落一行,供设置面板审计查看与事后追溯。
/// 为什么必须落库:root/ADB 级命令不可逆,「谁在何时以什么等级跑了什么」
/// 是唯一的回溯依据(见 docs/契约-协议与配置.md 与 docs/计划.md 合规要求)。
pub(super) const EXEC_AUDIT_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS exec_audit (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  ts             TEXT NOT NULL,
  source         TEXT NOT NULL DEFAULT 'chat',
  task_id        TEXT,
  session_id     TEXT,
  command        TEXT NOT NULL,
  shell          TEXT NOT NULL DEFAULT '',
  tier           TEXT NOT NULL DEFAULT 'sandbox',
  risk           TEXT NOT NULL DEFAULT 'sensitive',
  decision       TEXT NOT NULL DEFAULT 'allowed',
  exit_code      INTEGER,
  stdout_summary TEXT NOT NULL DEFAULT '',
  stderr_summary TEXT NOT NULL DEFAULT '',
  risk_flag      TEXT NOT NULL DEFAULT ''
)"#;
/// 审计按时间倒序查询的辅助索引。
pub(super) const EXEC_AUDIT_INDEX_DDL: &str =
    "CREATE INDEX IF NOT EXISTS idx_exec_audit_ts ON exec_audit(ts DESC)";

/// 幂等补建命令执行审计表(旧库无此表时创建)。
/// DDL 全用 IF NOT EXISTS,启动与跨库合并前各执行一次均安全(同 task_messages 模式)。
pub fn ensure_exec_audit_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(EXEC_AUDIT_DDL)
        .map_err(|e| format!("创建 exec_audit 表失败: {e}"))?;
    conn.execute_batch(EXEC_AUDIT_INDEX_DDL)
        .map_err(|e| format!("创建 exec_audit 索引失败: {e}"))?;
    Ok(())
}

/// 电脑操作审计表(CU-1,2026-10-06):每次 cu 类工具调用尝试落一行(成功/被拒都落)。
/// 与 exec_audit 分表的理由见 schema.rs 同名表注释;**屏幕像素一律不入库**。
/// 文本与 models/db/schema.rs CREATE_TABLES 内的建表语句保持一致(normalize 比对依赖)。
pub(super) const CU_AUDIT_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS cu_audit (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  ts         TEXT NOT NULL,
  source     TEXT NOT NULL DEFAULT 'chat',
  task_id    TEXT,
  session_id TEXT,
  platform   TEXT NOT NULL DEFAULT '',
  action     TEXT NOT NULL DEFAULT '',
  target     TEXT NOT NULL DEFAULT '',
  decision   TEXT NOT NULL DEFAULT 'allowed',
  result     TEXT NOT NULL DEFAULT '',
  error_code TEXT NOT NULL DEFAULT '',
  image_ref  TEXT NOT NULL DEFAULT '',
  png_bytes  INTEGER,
  sha256     TEXT NOT NULL DEFAULT ''
)"#;
/// 审计按时间倒序查询的辅助索引。
pub(super) const CU_AUDIT_INDEX_DDL: &str =
    "CREATE INDEX IF NOT EXISTS idx_cu_audit_ts ON cu_audit(ts DESC)";

/// 幂等补建电脑操作审计表(旧库无此表时创建;同 exec_audit 模式,启动与跨库合并前各跑一次均安全)。
pub fn ensure_cu_audit_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(CU_AUDIT_DDL)
        .map_err(|e| format!("创建 cu_audit 表失败: {e}"))?;
    conn.execute_batch(CU_AUDIT_INDEX_DDL)
        .map_err(|e| format!("创建 cu_audit 索引失败: {e}"))?;
    Ok(())
}

/// 幂等补列(D1 审计增强,2026-09-26):exec_audit 加 `risk_flag`。
///
/// 取值 `''` | `data_dir_touch`(命令文本命中数据目录绝对路径)| `parent_climb`
/// (含 `..` 路径段)。**只标记不拦截**:模型有 shell 就能读进程可读的任意路径,cwd jail
/// 只约束相对路径与缺省目录;标记的价值是让「模型把用户数据目录当草稿纸」这类行为在
/// 审计表里可筛可查(2026-09 实测证据见 docs/经验.md),不是假装封堵。
/// 列追加在表尾,与新版 EXEC_AUDIT_DDL 建出的 schema normalize 后一致。
pub fn ensure_exec_audit_risk_flag_column(conn: &Connection) -> Result<(), String> {
    add_column_if_missing(conn, "exec_audit", "risk_flag", "TEXT NOT NULL DEFAULT ''")
}

/// 性能索引补建(2026-09-13 批次 3):旧库补 `sessions.character_id` 与 `tasks` 过滤列索引。
/// 此前这些列表查询走全表扫描 + 排序(外键列 SQLite 不自动建索引)。
/// 与 `schema.rs` 的同名 `CREATE INDEX IF NOT EXISTS` 保持一致——新增/改动索引时
/// **两处都要改**(迁移元测试 `tests/schema_migration_meta.rs` 会逐表比对索引集合把关)。
const PERF_INDEX_DDL: &str = "
CREATE INDEX IF NOT EXISTS idx_sessions_character ON sessions(character_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_character ON tasks(character_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
";

/// 幂等 schema 升级(批次 3):旧库补建会话/任务列表查询索引。
pub fn ensure_perf_indexes(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(PERF_INDEX_DDL)
        .map_err(|e| format!("创建性能索引失败: {e}"))?;
    Ok(())
}
