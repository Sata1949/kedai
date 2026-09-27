//! 记忆服务的读写面:`MemoryService` 的全部方法(CRUD / 向量表 / 检索 / 蒸馏 / 淘汰)。

use crate::models::db::{now_iso, Db};
use crate::models::types::LlmMessage;
use crate::services::session_service::SessionService;
use crate::services::settings_service::RuntimeSettings;
use rusqlite::{params, OptionalExtension};
use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex};

// 子模块内统一用 super::* 拿到 mod.rs 的公共词汇(类型/常量)与 re-export 的自由函数。
use super::distill::row_to_entry;
use super::vectors::blob_to_vec;
use super::*;

impl MemoryService {
    pub fn new(db: Arc<Db>) -> Self {
        MemoryService {
            db,
            settings: std::sync::OnceLock::new(),
        }
    }

    /// 注入运行期设置句柄(由 AppState 在设置就绪后调用;幂等,已注入则忽略)。
    /// 未注入时淘汰/预算阈值取默认常量(单测与工具单测路径)。
    pub fn attach_settings(&self, settings: Arc<Mutex<RuntimeSettings>>) {
        let _ = self.settings.set(settings);
    }

    /// 运行期设置快照(未注入返回 None);供写入链路判断向量化是否启用
    pub fn settings_snapshot(&self) -> Option<RuntimeSettings> {
        self.settings
            .get()
            .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// 每角色记忆容量上限(0 = 不淘汰)
    fn max_entries(&self) -> usize {
        self.settings
            .get()
            .map(|s| {
                s.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .memory_max_entries
            })
            .unwrap_or(DEFAULT_MEMORY_MAX_ENTRIES) as usize
    }

    /// 角色全部记忆(最新在前),供列表 API
    pub fn list(&self, character_id: &str) -> Vec<MemoryEntry> {
        let Ok(conn) = self.db.read() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT {ENTRY_COLUMNS} FROM memory_entries WHERE character_id = ?1 ORDER BY id DESC"
        )) else {
            return Vec::new();
        };
        stmt.query_map(params![character_id], row_to_entry)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 批量按 id 取记忆(2026-09-16 性能批次 P-5):一条 `WHERE id IN (...)` 取回,
    /// **按入参顺序**返回,不存在的 id 静默跳过。
    ///
    /// 存在意义:蒸馏后为新增条目补向量时,调用方手上只有 `new_ids`,此前逐条 `get(id)`
    /// 是典型 N+1——N 条记忆就是 N 次往返 + N 次 prepare。批量版把往返降到 1 次。
    /// 顺序保证是硬要求:调用方要用「返回的向量」逐个配对「id 列表」,顺序错了会张冠李戴。
    pub fn get_many(&self, ids: &[i64]) -> Vec<MemoryEntry> {
        if ids.is_empty() {
            // SQL 的 `IN ()` 是语法错误,空输入必须短路
            return Vec::new();
        }
        let Ok(conn) = self.db.read() else {
            return Vec::new();
        };
        // 用与 id 个数等长的占位符;rusqlite 的 params_from_iter 按序绑定
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql =
            format!("SELECT {ENTRY_COLUMNS} FROM memory_entries WHERE id IN ({placeholders})");
        let Ok(mut stmt) = conn.prepare(&sql) else {
            return Vec::new();
        };
        let found: std::collections::HashMap<i64, MemoryEntry> =
            match stmt.query_map(rusqlite::params_from_iter(ids.iter()), row_to_entry) {
                Ok(rows) => rows.filter_map(|r| r.ok()).map(|e| (e.id, e)).collect(),
                // 与同文件 list() 的错误处理惯例一致:查询失败返回空,由调用方按「无内容」处理
                Err(_) => return Vec::new(),
            };
        // 按入参顺序重排(SQL 的 IN 不保证顺序),缺失项跳过
        ids.iter().filter_map(|id| found.get(id).cloned()).collect()
    }

    pub fn get(&self, id: i64) -> Option<MemoryEntry> {
        let conn = self.db.read().ok()?;
        conn.query_row(
            &format!("SELECT {ENTRY_COLUMNS} FROM memory_entries WHERE id = ?1"),
            params![id],
            row_to_entry,
        )
        .optional()
        .ok()
        .flatten()
    }

    // ===== 向量索引(Phase 3) =====

    /// 当前配置的 embedding 维度(0 = 未探测);settings 未注入时返回 0
    pub fn embedding_dim(&self) -> u32 {
        self.settings
            .get()
            .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).embedding_dim)
            .unwrap_or(0)
    }

    /// 确保 vec0 虚拟表存在且维度匹配。维度变化时**不自动删除**已有向量
    /// (数据安全优先),仅返回 false 由调用方提示「需重建索引」。
    /// 返回 Ok(true) = 表可用;Ok(false) = 维度不匹配需重建。
    pub fn ensure_vec_table(&self, dim: u32) -> Result<bool, String> {
        if dim == 0 {
            return Ok(false);
        }
        let conn = self.db.write();
        // 读现有元信息
        let current_dim: Option<u32> = conn
            .query_row(
                "SELECT value FROM memory_vec_meta WHERE key = 'dim'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok());
        let table_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_vectors'",
                [],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some();

        match (table_exists, current_dim) {
            (true, Some(d)) if d == dim => Ok(true),
            (true, Some(d)) if d != dim => {
                // 维度不匹配:记录待重建状态,不动旧表(用户点「重建向量索引」再 drop)
                conn.execute(
                    "INSERT INTO memory_vec_meta(key, value) VALUES('dim_mismatch', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![format!("{d}->{dim}")],
                )
                .map_err(|e| format!("记录维度不匹配失败: {e}"))?;
                Ok(false)
            }
            _ => {
                conn.execute_batch(&format!(
                    "CREATE VIRTUAL TABLE IF NOT EXISTS memory_vectors USING vec0(
                        memory_id INTEGER PRIMARY KEY,
                        embedding float[{dim}]
                     );"
                ))
                .map_err(|e| format!("创建向量表失败(维度 {dim}): {e}"))?;
                conn.execute(
                    "INSERT INTO memory_vec_meta(key, value) VALUES('dim', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![dim.to_string()],
                )
                .map_err(|e| format!("写入向量维度元信息失败: {e}"))?;
                conn.execute("DELETE FROM memory_vec_meta WHERE key = 'dim_mismatch'", [])
                    .ok();
                Ok(true)
            }
        }
    }

    /// 强制重建向量表(丢弃全部已存向量;调用方随后应触发回填)
    pub fn drop_vec_table(&self) -> Result<(), String> {
        let conn = self.db.write();
        conn.execute_batch("DROP TABLE IF EXISTS memory_vectors;")
            .map_err(|e| format!("删除向量表失败: {e}"))?;
        conn.execute(
            "DELETE FROM memory_vec_meta WHERE key IN ('dim','dim_mismatch')",
            [],
        )
        .ok();
        Ok(())
    }

    /// 写入/覆盖单条记忆的向量(先确保表存在)
    pub fn upsert_vector(&self, memory_id: i64, vec: &[f32]) -> Result<(), String> {
        if vec.is_empty() {
            return Ok(());
        }
        if !self.ensure_vec_table(vec.len() as u32)? {
            return Err("向量维度与索引表不匹配,请先重建向量索引".into());
        }
        let json = crate::services::embedding_service::vec_to_json(vec);
        let conn = self.db.write();
        // vec0 不支持 UPSERT 语法,先删后插
        conn.execute(
            "DELETE FROM memory_vectors WHERE memory_id = ?1",
            params![memory_id],
        )
        .map_err(|e| format!("删除旧向量失败: {e}"))?;
        conn.execute(
            "INSERT INTO memory_vectors(memory_id, embedding) VALUES (?1, ?2)",
            params![memory_id, json],
        )
        .map_err(|e| format!("写入向量失败: {e}"))?;
        Ok(())
    }

    /// 批量写入向量(同一事务,减少写锁往返)
    pub fn upsert_vectors(&self, items: &[(i64, Vec<f32>)]) -> Result<usize, String> {
        if items.is_empty() {
            return Ok(0);
        }
        let dim = items[0].1.len() as u32;
        if !self.ensure_vec_table(dim)? {
            return Err("向量维度与索引表不匹配,请先重建向量索引".into());
        }
        let mut conn = self.db.write();
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启事务失败: {e}"))?;
        let mut n = 0usize;
        for (id, vec) in items {
            if vec.is_empty() {
                continue;
            }
            let json = crate::services::embedding_service::vec_to_json(vec);
            tx.execute(
                "DELETE FROM memory_vectors WHERE memory_id = ?1",
                params![*id],
            )
            .map_err(|e| format!("删除旧向量失败: {e}"))?;
            tx.execute(
                "INSERT INTO memory_vectors(memory_id, embedding) VALUES (?1, ?2)",
                params![*id, json],
            )
            .map_err(|e| format!("写入向量失败: {e}"))?;
            n += 1;
        }
        tx.commit().map_err(|e| format!("提交向量事务失败: {e}"))?;
        Ok(n)
    }

    /// 读取指定记忆的向量(供混合召回;缺失返回 None)
    pub fn get_vectors(&self, ids: &[i64]) -> std::collections::HashMap<i64, Vec<f32>> {
        let mut out = std::collections::HashMap::new();
        if ids.is_empty() {
            return out;
        }
        let Ok(conn) = self.db.read() else {
            return out;
        };
        let table_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_vectors'",
                [],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some();
        if !table_exists {
            return out;
        }
        let placeholders = vec!["?"; ids.len()].join(", ");
        let sql = format!(
            "SELECT memory_id, embedding FROM memory_vectors WHERE memory_id IN ({placeholders})"
        );
        let Ok(mut stmt) = conn.prepare(&sql) else {
            return out;
        };
        let params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|i| Box::new(*i) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let rows = stmt.query_map(
            rusqlite::params_from_iter(params_vec.iter().map(|v| v.as_ref())),
            |r| {
                let id: i64 = r.get(0)?;
                // vec0 的向量列以 BLOB 返回(float32 小端紧密排列),非 TEXT
                let blob: Vec<u8> = r.get(1)?;
                Ok((id, blob))
            },
        );
        if let Ok(rows) = rows {
            for (id, blob) in rows.flatten() {
                if let Some(v) = blob_to_vec(&blob) {
                    out.insert(id, v);
                }
            }
        }
        out
    }

    /// 向量索引状态:总数 / 已嵌入数 / 维度 / 是否维度漂移
    pub fn vector_status(&self) -> VectorStatus {
        let conn = match self.db.read() {
            Ok(c) => c,
            Err(_) => return VectorStatus::default(),
        };
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_entries", [], |r| r.get(0))
            .unwrap_or(0);
        let table_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_vectors'",
                [],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some();
        let embedded: i64 = if table_exists {
            conn.query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
                .unwrap_or(0)
        } else {
            0
        };
        let dim: Option<u32> = conn
            .query_row(
                "SELECT value FROM memory_vec_meta WHERE key = 'dim'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok());
        let mismatch: Option<String> = conn
            .query_row(
                "SELECT value FROM memory_vec_meta WHERE key = 'dim_mismatch'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten();
        VectorStatus {
            total,
            embedded,
            dim,
            dim_mismatch: mismatch,
        }
    }

    /// 取出所有缺失向量的记忆(供手动回填);limit 控制单批大小
    pub fn entries_missing_vectors(&self, limit: usize) -> Vec<MemoryEntry> {
        let conn = match self.db.read() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let table_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_vectors'",
                [],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some();
        let sql = if table_exists {
            format!(
                "SELECT {ENTRY_COLUMNS} FROM memory_entries m \
                 WHERE NOT EXISTS (SELECT 1 FROM memory_vectors v WHERE v.memory_id = m.id) \
                 ORDER BY m.id LIMIT ?1"
            )
        } else {
            format!("SELECT {ENTRY_COLUMNS} FROM memory_entries m ORDER BY m.id LIMIT ?1")
        };
        let Ok(mut stmt) = conn.prepare(&sql) else {
            return Vec::new();
        };
        stmt.query_map(params![limit as i64], row_to_entry)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 全文检索(B1):FTS5 MATCH + bm25 升序;查询 trim 后 < 3 字符时回退 LIKE
    /// (trigram 分词器要求至少 3 字符,否则无命中)。
    pub fn search(&self, character_id: &str, q: &str, limit: usize) -> Vec<MemoryEntry> {
        let q = q.trim();
        if q.is_empty() || character_id.trim().is_empty() || limit == 0 {
            return Vec::new();
        }
        let Ok(conn) = self.db.read() else {
            return Vec::new();
        };
        if q.chars().count() < 3 {
            // 短查询回退 LIKE(转义 % 与 _ 防通配注入)
            let escaped = q
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let pattern = format!("%{escaped}%");
            let Ok(mut stmt) = conn.prepare(&format!(
                "SELECT {ENTRY_COLUMNS} FROM memory_entries \
                 WHERE character_id = ?1 AND content LIKE ?2 ESCAPE '\\' ORDER BY id DESC LIMIT ?3"
            )) else {
                return Vec::new();
            };
            return stmt
                .query_map(params![character_id, pattern, limit as i64], row_to_entry)
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default();
        }
        // FTS5 查询串:整体加双引号作短语匹配(trigram 下等价子串匹配),
        // 双引号内部再转义 " → "" 防语法错误
        let phrase = format!("\"{}\"", q.replace('"', "\"\""));
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT m.{cols} FROM memory_entries_fts f \
             JOIN memory_entries m ON m.id = f.rowid \
             WHERE f.content MATCH ?1 AND m.character_id = ?2 \
             ORDER BY bm25(memory_entries_fts), m.id DESC LIMIT ?3",
            cols = ENTRY_COLUMNS.replace(", ", ", m.")
        )) else {
            return Vec::new();
        };
        stmt.query_map(params![phrase, character_id, limit as i64], row_to_entry)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 落库一条记忆(kind: distilled|tool|manual);content 空白视为非法。
    /// B4 精确去重:同角色 + 相同 content(trim 后)已存在 → 不新建,
    /// usage_count+1 并返回已有条目。
    /// B3 淘汰:写入后按角色容量上限归档最低分条目(只置 selected=0)。
    pub fn insert(
        &self,
        character_id: &str,
        source_session_id: Option<&str>,
        kind: &str,
        content: &str,
    ) -> Result<MemoryEntry, String> {
        let content = content.trim();
        if content.is_empty() {
            return Err("记忆内容不能为空".into());
        }
        if character_id.trim().is_empty() {
            return Err("缺少 character_id".into());
        }
        if !matches!(kind, "distilled" | "tool" | "manual") {
            return Err(format!("非法记忆类型: {kind}"));
        }
        if let Some(existing) = self.find_exact(character_id, content) {
            self.touch(&[existing.id])?;
            return self
                .get(existing.id)
                .ok_or_else(|| "记忆去重后读取失败".into());
        }
        let now = now_iso();
        let id = {
            let conn = self.db.write();
            conn.execute(
                "INSERT INTO memory_entries
                   (character_id, source_session_id, kind, content, usage_count, last_usage, selected, created_at, updated_at, pinned)
                 VALUES (?1, ?2, ?3, ?4, 0, NULL, 1, ?5, ?5, 0)",
                params![character_id, source_session_id, kind, content, now],
            )
            .map_err(|e| format!("写入记忆失败: {e}"))?;
            conn.last_insert_rowid()
        };
        // 锁已释放再淘汰(evict 内部会取写锁;std::sync::Mutex 不可重入)
        self.evict_over_capacity(character_id)?;
        self.get(id).ok_or_else(|| "写入记忆后读取失败".into())
    }

    /// 精确去重查找:同角色 + content 完全相同(trim 后)的最早一条
    fn find_exact(&self, character_id: &str, content: &str) -> Option<MemoryEntry> {
        let conn = self.db.read().ok()?;
        conn.query_row(
            &format!(
                "SELECT {ENTRY_COLUMNS} FROM memory_entries \
                 WHERE character_id = ?1 AND content = ?2 ORDER BY id ASC LIMIT 1"
            ),
            params![character_id, content],
            row_to_entry,
        )
        .optional()
        .ok()
        .flatten()
    }

    /// B3 容量淘汰:每角色 selected 条目数超过 memory_max_entries 时,把最低分
    /// (usage_count ASC, last_usage ASC, id ASC)条目置 selected=0(只归档不删除)。
    /// 返回归档条数。pinned 条目最后才被归档(排序键把 pinned 排在最前保留)。
    pub fn evict_over_capacity(&self, character_id: &str) -> Result<usize, String> {
        let cap = self.max_entries();
        if cap == 0 {
            return Ok(0);
        }
        let conn = self.db.write();
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_entries WHERE character_id = ?1 AND selected = 1",
                params![character_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("统计记忆条数失败: {e}"))?;
        let excess = total - cap as i64;
        if excess <= 0 {
            return Ok(0);
        }
        // 最低分在前;pinned 条目排到最后才淘汰(保留优先级)
        let archived = conn
            .execute(
                "UPDATE memory_entries SET selected = 0, updated_at = ?2 WHERE id IN ( \
                   SELECT id FROM memory_entries WHERE character_id = ?1 AND selected = 1 \
                   ORDER BY pinned ASC, usage_count ASC, last_usage ASC, id ASC LIMIT ?3 \
                 )",
                params![character_id, now_iso(), excess],
            )
            .map_err(|e| format!("归档超限记忆失败: {e}"))?;
        Ok(archived)
    }

    /// B3 硬删除:清空该角色 selected=0 的归档条目,返回删除条数
    pub fn prune(&self, character_id: &str) -> Result<usize, String> {
        let conn = self.db.write();
        conn.execute(
            "DELETE FROM memory_entries WHERE character_id = ?1 AND selected = 0",
            params![character_id],
        )
        .map_err(|e| format!("清理归档记忆失败: {e}"))
    }

    /// 手动添加(kind='manual')
    pub fn create_manual(&self, character_id: &str, content: &str) -> Result<MemoryEntry, String> {
        self.insert(character_id, None, "manual", content)
    }

    /// 编辑(content / selected / pinned 任选,可同时)
    pub fn update(
        &self,
        id: i64,
        content: Option<&str>,
        selected: Option<bool>,
        pinned: Option<bool>,
    ) -> Option<MemoryEntry> {
        let mut sets: Vec<String> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(c) = content {
            let c = c.trim();
            if c.is_empty() {
                return None;
            }
            sets.push("content = ?".into());
            values.push(Box::new(c.to_string()));
        }
        if let Some(s) = selected {
            sets.push("selected = ?".into());
            values.push(Box::new(if s { 1i64 } else { 0i64 }));
        }
        if let Some(p) = pinned {
            sets.push("pinned = ?".into());
            values.push(Box::new(if p { 1i64 } else { 0i64 }));
        }
        if sets.is_empty() {
            return self.get(id);
        }
        sets.push("updated_at = ?".into());
        values.push(Box::new(now_iso()));
        let sql = format!(
            "UPDATE memory_entries SET {} WHERE id = {}",
            sets.join(", "),
            id
        );
        {
            let conn = self.db.write();
            let n = conn
                .execute(
                    sql.as_str(),
                    rusqlite::params_from_iter(values.iter().map(|v| v.as_ref())),
                )
                .ok()?;
            if n == 0 {
                return None;
            }
        }
        self.get(id)
    }

    pub fn delete(&self, id: i64) -> bool {
        let conn = self.db.write();
        conn.execute("DELETE FROM memory_entries WHERE id = ?1", params![id])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    /// 注入后衰减回写:批量 usage_count+1 与 last_usage,不改 content 与 selected,
    /// 也不触碰已注入消息数组(本轮内容在构建时已冻结)。
    pub fn touch(&self, ids: &[i64]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let now = now_iso();
        let placeholders = vec!["?"; ids.len()].join(", ");
        let sql = format!(
            "UPDATE memory_entries SET usage_count = usage_count + 1, last_usage = ?1, updated_at = ?1 \
             WHERE id IN ({placeholders})"
        );
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(now)];
        for id in ids {
            params_vec.push(Box::new(*id));
        }
        let conn = self.db.write();
        conn.execute(
            sql.as_str(),
            rusqlite::params_from_iter(params_vec.iter().map(|v| v.as_ref())),
        )
        .map_err(|e| format!("记忆使用计数回写失败: {e}"))?;
        Ok(())
    }

    /// 蒸馏一个会话:取历史(复用 SessionService 读法)→ 调 LLM → 按行落库。
    /// LLM 调用抽象为函数参数(mock/真连接器均可注入);空历史直接跳过(不调 LLM)。
    pub async fn distill_session<L, F>(
        &self,
        sessions: &SessionService,
        session_id: &str,
        llm: L,
    ) -> Result<DistillOutcome, String>
    where
        L: FnOnce(Vec<LlmMessage>) -> F,
        F: Future<Output = Result<String, String>>,
    {
        let Some(session) = sessions.get(session_id) else {
            return Err(format!("会话不存在: {session_id}"));
        };
        let history = sessions.get_messages(session_id);
        let user_text = distill_user_text(&history);
        if user_text.trim().is_empty() {
            // 空历史(无 user/assistant 消息)不蒸馏,不消耗 LLM 调用
            return Ok(DistillOutcome {
                character_id: session.character_id,
                inserted: 0,
                skipped: 0,
                new_ids: Vec::new(),
            });
        }
        let messages = vec![
            LlmMessage::plain("system", distill_system_prompt()),
            LlmMessage::plain("user", &user_text),
        ];
        let output = llm(messages).await?;
        let lines = parse_distilled_lines(&output);
        if lines.is_empty() {
            return Err("蒸馏结果为空,未写入记忆".into());
        }
        // B4 近似去重:与已有记忆 token Jaccard ≥ 阈值视为重复跳过;
        // 本批内也已接受的行同样参与比较(防同一输出内近义重复)
        let mut accepted: Vec<HashSet<String>> = self
            .list(&session.character_id)
            .iter()
            .map(|e| tokenize_for_similarity(&e.content))
            .collect();
        let mut inserted = 0usize;
        let mut skipped = 0usize;
        let mut new_ids: Vec<i64> = Vec::new();
        for line in lines {
            let tokens = tokenize_for_similarity(&line);
            let duplicate = accepted
                .iter()
                .any(|t| jaccard_similarity(t, &tokens) >= DISTILL_DEDUP_JACCARD_THRESHOLD);
            if duplicate {
                skipped += 1;
                continue;
            }
            let entry = self.insert(&session.character_id, Some(session_id), "distilled", &line)?;
            new_ids.push(entry.id);
            accepted.push(tokens);
            inserted += 1;
        }
        Ok(DistillOutcome {
            character_id: session.character_id,
            inserted,
            skipped,
            new_ids,
        })
    }
}
