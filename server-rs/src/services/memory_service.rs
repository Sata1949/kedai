// 跨会话记忆蒸馏服务(落地项 2,借鉴 Codex memories stage1_outputs 与 Reasonix 记忆修订):
// 记忆按 character_id 维度跨会话共享,解决长对话人设漂移。
//   - 蒸馏(distill):把会话历史交给 LLM 提取多条短记忆,每行一条落库(kind='distilled');
//   - 精选(select_for_injection):selected=1 的按 (usage_count DESC, last_usage DESC, id DESC)
//     排序取前 limit 条注入;排序键确定性(tie-break 用 id),保证记忆集合未变时槽内容逐字节稳定;
//   - 衰减(touch):注入完成后批量回写 usage_count+1 与 last_usage,只动计数不改已注入内容。
// 工具写入(tools/memory.rs)与手动添加(kind='manual')共用同一张 memory_entries 表。
use crate::models::db::{now_iso, Db};
use crate::models::types::{LlmMessage, MessageRecord};
use crate::services::session_service::SessionService;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::future::Future;
use std::sync::Arc;

/// 单次蒸馏最多落库的记忆条数(防 LLM 长输出灌爆记忆库)
pub const MAX_DISTILLED_LINES: usize = 64;

/// 记忆条目(memory_entries 表行)
#[derive(Debug, Clone, Serialize)]
pub struct MemoryEntry {
    pub id: i64,
    pub character_id: String,
    pub source_session_id: Option<String>,
    /// distilled | tool | manual
    pub kind: String,
    pub content: String,
    pub usage_count: i64,
    pub last_usage: Option<String>,
    pub selected: bool,
    pub created_at: String,
    pub updated_at: String,
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<MemoryEntry> {
    Ok(MemoryEntry {
        id: row.get(0)?,
        character_id: row.get(1)?,
        source_session_id: row.get(2)?,
        kind: row.get(3)?,
        content: row.get(4)?,
        usage_count: row.get(5)?,
        last_usage: row.get(6)?,
        selected: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

const ENTRY_COLUMNS: &str =
    "id, character_id, source_session_id, kind, content, usage_count, last_usage, selected, created_at, updated_at";

/// 蒸馏结果摘要
#[derive(Debug, Serialize)]
pub struct DistillOutcome {
    pub character_id: String,
    pub inserted: usize,
}

pub struct MemoryService {
    db: Arc<Db>,
}

/// 蒸馏系统指令:取向沿用 compaction(人物关系与立场、关键事件与因果、未回收伏笔),
/// 输出改为「每行一条短记忆」以支持服务端按行拆分。
pub fn distill_system_prompt() -> &'static str {
    "你是对话记忆蒸馏助手。从下面的角色扮演对话中提取值得跨会话长期记住的记忆条目,\
     每条一行、独立成句、简洁平实(中文)。必须覆盖:人物关系与立场、已发生的关键事件\
     与因果、未回收的伏笔/承诺/约定。不要输出一次性的场景描写、寒暄或与长期记忆无关的\
     细节;不要编号、不要项目符号、不要评论或「记忆如下」等元文本,只输出记忆条目本身。"
}

/// 蒸馏输入文本:按「角色: 内容」逐条拼接(与 compaction_user_text 同格式)。
pub fn distill_user_text(history: &[MessageRecord]) -> String {
    history
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| {
            let speaker = if m.role == "user" { "用户" } else { "角色" };
            format!("{speaker}: {}", m.content)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 蒸馏输出按行拆分:去空白、跳过空行、剥一次行首项目符号(- / *),上限 MAX_DISTILLED_LINES。
pub fn parse_distilled_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('-').trim_start_matches('*').trim();
        if line.is_empty() {
            continue;
        }
        if out.len() >= MAX_DISTILLED_LINES {
            break;
        }
        out.push(line.to_string());
    }
    out
}

/// 注入候选精选(纯函数):selected=1 的条目按
/// (usage_count DESC, last_usage DESC, id DESC) 排序取前 limit 条。
/// id 作最终 tie-break 保证确定性——记忆集合未变时输出顺序逐字节稳定(前缀缓存前提)。
pub fn select_for_injection<'a>(
    entries: &'a [MemoryEntry],
    limit: usize,
) -> Vec<&'a MemoryEntry> {
    let mut picked: Vec<&MemoryEntry> = entries.iter().filter(|e| e.selected).collect();
    picked.sort_by(|a, b| {
        b.usage_count
            .cmp(&a.usage_count)
            .then(b.last_usage.cmp(&a.last_usage))
            .then(b.id.cmp(&a.id))
    });
    picked.truncate(limit);
    picked
}

impl MemoryService {
    pub fn new(db: Arc<Db>) -> Self {
        MemoryService { db }
    }

    /// 角色全部记忆(最新在前),供列表 API
    pub fn list(&self, character_id: &str) -> Vec<MemoryEntry> {
        let conn = self.db.conn();
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT {ENTRY_COLUMNS} FROM memory_entries WHERE character_id = ?1 ORDER BY id DESC"
        )) else {
            return Vec::new();
        };
        stmt.query_map(params![character_id], row_to_entry)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    pub fn get(&self, id: i64) -> Option<MemoryEntry> {
        let conn = self.db.conn();
        conn.query_row(
            &format!("SELECT {ENTRY_COLUMNS} FROM memory_entries WHERE id = ?1"),
            params![id],
            row_to_entry,
        )
        .optional()
        .ok()
        .flatten()
    }

    /// 落库一条记忆(kind: distilled|tool|manual);content 空白视为非法。
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
        let now = now_iso();
        let conn = self.db.conn();
        conn.execute(
            "INSERT INTO memory_entries
               (character_id, source_session_id, kind, content, usage_count, last_usage, selected, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 0, NULL, 1, ?5, ?5)",
            params![character_id, source_session_id, kind, content, now],
        )
        .map_err(|e| format!("写入记忆失败: {e}"))?;
        let id = conn.last_insert_rowid();
        drop(conn);
        self.get(id).ok_or_else(|| "写入记忆后读取失败".into())
    }

    /// 手动添加(kind='manual')
    pub fn create_manual(
        &self,
        character_id: &str,
        content: &str,
    ) -> Result<MemoryEntry, String> {
        self.insert(character_id, None, "manual", content)
    }

    /// 编辑(content / selected 二选一或同时)
    pub fn update(
        &self,
        id: i64,
        content: Option<&str>,
        selected: Option<bool>,
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
            let conn = self.db.conn();
            let n = conn
                .execute(sql.as_str(), rusqlite::params_from_iter(values.iter().map(|v| v.as_ref())))
                .ok()?;
            if n == 0 {
                return None;
            }
        }
        self.get(id)
    }

    pub fn delete(&self, id: i64) -> bool {
        let conn = self.db.conn();
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
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(now)];
        for id in ids {
            params_vec.push(Box::new(*id));
        }
        let conn = self.db.conn();
        conn.execute(sql.as_str(), rusqlite::params_from_iter(params_vec.iter().map(|v| v.as_ref())))
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
        let mut inserted = 0usize;
        for line in lines {
            self.insert(&session.character_id, Some(session_id), "distilled", &line)?;
            inserted += 1;
        }
        Ok(DistillOutcome {
            character_id: session.character_id,
            inserted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(id: i64, usage: i64, last_usage: Option<&str>, selected: bool) -> MemoryEntry {
        MemoryEntry {
            id,
            character_id: "c1".into(),
            source_session_id: None,
            kind: "distilled".into(),
            content: format!("记忆{id}"),
            usage_count: usage,
            last_usage: last_usage.map(String::from),
            selected,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn service() -> (MemoryService, SessionService, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("kedai-memory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Db::open(&dir.join("kedai.db"), &dir).unwrap());
        let memory = MemoryService::new(db.clone());
        let sessions = SessionService::new(db);
        (memory, sessions, dir)
    }

    fn seed_session(_sessions: &SessionService, dir: &std::path::Path, character_id: &str, sid: &str) {
        // SessionService.db 为私有,测试以独立连接播种(与 compaction.rs 测试同模式)
        let conn = rusqlite::Connection::open(dir.join("kedai.db")).unwrap();
        conn.execute(
            "INSERT INTO characters (id, name, chara_name, description, file_path, data_raw, created_at)
             VALUES (?1, 'c', 'c', '', '', '{}', '')",
            params![character_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, character_id, title, created_at, updated_at)
             VALUES (?1, ?2, 't', '', '')",
            params![sid, character_id],
        )
        .unwrap();
    }

    fn msg(id: i64, role: &str, content: &str) -> MessageRecord {
        MessageRecord {
            id,
            session_id: "s1".into(),
            role: role.into(),
            content: content.into(),
            extra: json!({}),
            created_at: String::new(),
        }
    }

    /// memory_entries 表随 Db::open 自动建立(旧库启动幂等升级),列与索引齐全
    #[test]
    fn memory_table_created_on_open() {
        let (memory, _, dir) = service();
        let conn = memory.db.conn();
        let mut columns: Vec<String> = Vec::new();
        {
            let mut stmt = conn.prepare("PRAGMA table_info(memory_entries)").unwrap();
            let rows = stmt.query_map([], |row| row.get::<_, String>(1)).unwrap();
            for c in rows.flatten() {
                columns.push(c);
            }
        }
        for expected in [
            "id",
            "character_id",
            "source_session_id",
            "kind",
            "content",
            "usage_count",
            "last_usage",
            "selected",
            "created_at",
            "updated_at",
        ] {
            assert!(
                columns.iter().any(|c| c == expected),
                "memory_entries 应含列 {expected},实际: {columns:?}"
            );
        }
        // 索引:character_id + selected
        let mut idx_count = 0;
        {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_schema WHERE type='index' AND tbl_name='memory_entries'")
                .unwrap();
            let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
            for _ in rows.flatten() {
                idx_count += 1;
            }
        }
        assert!(idx_count >= 1, "memory_entries 应建 character_id+selected 索引");
        // kind CHECK 约束:非法 kind 拒绝
        assert!(memory.insert("c1", None, "bogus", "x").is_err());
        drop(conn);
        std::fs::remove_dir_all(dir).ok();
    }

    /// 精选排序:usage_count DESC → last_usage DESC(None 最后)→ id DESC(tie-break 确定性);
    /// selected=0 过滤;limit 截断
    #[test]
    fn select_orders_by_usage_recency_and_limit() {
        let entries = vec![
            entry(1, 0, None, true),                 // 从未使用,排最后
            entry(2, 5, Some("2026-08-01T00:00:00Z"), true),
            entry(3, 5, Some("2026-08-02T00:00:00Z"), true), // 同计数,更近使用在前
            entry(4, 9, Some("2026-07-01T00:00:00Z"), true), // 计数最高
            entry(5, 99, Some("2026-08-03T00:00:00Z"), false), // 未选中,不参与
            entry(6, 5, Some("2026-08-02T00:00:00Z"), true), // 与 3 完全同键,id 大在前
        ];
        let picked = select_for_injection(&entries, 10);
        let ids: Vec<i64> = picked.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![4, 6, 3, 2, 1], "排序应为 计数→最近使用→id(新在前): {ids:?}");
        // limit 截断
        let top2 = select_for_injection(&entries, 2);
        assert_eq!(
            top2.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![4, 6]
        );
        // 确定性:同集合两次调用输出一致(逐字节稳定前提)
        let again = select_for_injection(&entries, 10);
        assert_eq!(picked.len(), again.len());
        for (a, b) in picked.iter().zip(again.iter()) {
            assert_eq!(a.id, b.id);
        }
    }

    /// 蒸馏输出拆分:去空行、剥行首 -/*、上限截断
    #[test]
    fn parse_distilled_lines_splits_and_normalizes() {
        let text = "用户与角色在图书馆初识\n\n- 角色承诺周末带用户看画展\n* 用户透露自己害怕打雷\n  \n第4条\n";
        let lines = parse_distilled_lines(text);
        assert_eq!(
            lines,
            vec![
                "用户与角色在图书馆初识",
                "角色承诺周末带用户看画展",
                "用户透露自己害怕打雷",
                "第4条",
            ]
        );
        // 全空白输出 → 空(调用方应报错)
        assert!(parse_distilled_lines("  \n \n").is_empty());
        // 超上限截断
        let long = (0..100).map(|i| format!("记忆{i}")).collect::<Vec<_>>().join("\n");
        assert_eq!(parse_distilled_lines(&long).len(), MAX_DISTILLED_LINES);
    }

    /// 蒸馏落库:fake LLM 返回多行 → 每行一条 kind='distilled',
    /// character_id 按会话归属,source_session_id 记来源
    #[tokio::test]
    async fn distill_session_inserts_lines() {
        let (memory, sessions, dir) = service();
        seed_session(&sessions, &dir, "charA", "s1");
        sessions
            .add_message("s1", "user", "我们好像在哪见过?", json!({}))
            .unwrap();
        sessions
            .add_message("s1", "assistant", "图书馆,上周三的雨夜。", json!({}))
            .unwrap();

        let outcome = memory
            .distill_session(&sessions, "s1", |messages| async move {
                assert_eq!(messages[0].role, "system");
                assert!(messages[1].content.contains("图书馆"), "蒸馏输入应含历史: {}", messages[1].content);
                Ok("用户与角色在图书馆初识\n角色承诺周末看画展\n\n- 用户害怕打雷".into())
            })
            .await
            .unwrap();
        assert_eq!(outcome.inserted, 3);
        assert_eq!(outcome.character_id, "charA");

        let list = memory.list("charA");
        assert_eq!(list.len(), 3);
        assert!(list.iter().all(|e| e.kind == "distilled"));
        assert!(list.iter().all(|e| e.source_session_id.as_deref() == Some("s1")));
        assert!(list.iter().any(|e| e.content == "用户害怕打雷"), "- 前缀应被剥除");
        assert!(list.iter().all(|e| e.usage_count == 0 && e.selected));
        std::fs::remove_dir_all(dir).ok();
    }

    /// 空历史不蒸馏:不调 LLM(fake 计数为 0)、inserted=0
    #[tokio::test]
    async fn distill_session_empty_history_skips() {
        let (memory, sessions, dir) = service();
        seed_session(&sessions, &dir, "charB", "s2");
        let outcome = memory
            .distill_session(&sessions, "s2", |_| async {
                panic!("空历史不应调用 LLM");
            })
            .await
            .unwrap();
        assert_eq!(outcome.inserted, 0);
        assert!(memory.list("charB").is_empty());
        // 会话不存在 → 报错
        assert!(memory
            .distill_session(&sessions, "missing", |_| async { Ok("x".into()) })
            .await
            .is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    /// LLM 失败上抛;蒸馏输出全空白 → 报错不落库
    #[tokio::test]
    async fn distill_session_llm_failure_propagates() {
        let (memory, sessions, dir) = service();
        seed_session(&sessions, &dir, "charC", "s3");
        sessions
            .add_message("s3", "user", "你好", json!({}))
            .unwrap();
        assert!(memory
            .distill_session(&sessions, "s3", |_| async { Err("LLM 不可用".into()) })
            .await
            .is_err());
        assert!(memory
            .distill_session(&sessions, "s3", |_| async { Ok("  \n ".into()) })
            .await
            .is_err());
        assert!(memory.list("charC").is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    /// CRUD:手动添加/编辑/删除;touch 只动计数与时间戳
    #[test]
    fn crud_and_touch_semantics() {
        let (memory, _, dir) = service();
        let e = memory.create_manual("c1", "  用户喜欢薄荷茶  ").unwrap();
        assert_eq!(e.content, "用户喜欢薄荷茶", "content 应 trim");
        assert_eq!(e.kind, "manual");
        assert_eq!(e.source_session_id, None);

        // 编辑:content + selected
        let updated = memory.update(e.id, Some("用户喜欢洋甘菊茶"), Some(false)).unwrap();
        assert_eq!(updated.content, "用户喜欢洋甘菊茶");
        assert!(!updated.selected);
        // 空白 content → 404 语义(拒绝)
        assert!(memory.update(e.id, Some("   "), None).is_none());
        // 不存在的 id
        assert!(memory.update(9999, Some("x"), None).is_none());

        // touch:计数 +1、last_usage 落时间戳,content/selected 不动
        memory.touch(&[e.id]).unwrap();
        let touched = memory.get(e.id).unwrap();
        assert_eq!(touched.usage_count, 1);
        assert!(touched.last_usage.is_some());
        assert_eq!(touched.content, "用户喜欢洋甘菊茶");
        assert!(!touched.selected);
        // 空 ids 幂等
        memory.touch(&[]).unwrap();
        assert_eq!(memory.get(e.id).unwrap().usage_count, 1);

        // 删除
        assert!(memory.delete(e.id));
        assert!(!memory.delete(e.id));
        assert!(memory.get(e.id).is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    /// distill_user_text:只取 user/assistant,role 映射可读称呼
    #[test]
    fn distill_user_text_filters_roles() {
        let history = vec![
            msg(1, "user", "你好"),
            msg(2, "assistant", "你好呀"),
            msg(3, "system", "工具记忆条目"),
        ];
        let text = distill_user_text(&history);
        assert!(text.contains("用户: 你好"));
        assert!(text.contains("角色: 你好呀"));
        assert!(!text.contains("工具记忆条目"), "system 消息不进蒸馏输入");
    }
}
