// 回退快照服务(批次 6.1「undo」,L3):写工具执行前取逆操作负载落 undo_snapshots 表,
// 供「回退到此处」按快照逆序恢复。覆盖范围仅工具写入:
//   write/replace/create 的角色文件区文件与对话气泡、update_variables 变量树、
//   memory_write 记忆行;引擎自动落库的气泡不在范围。
// 两段式生命周期(快照点在 tools/registry.rs run_tool,所有工具执行唯一收口):
//   执行前 snapshot_before 取旧状态(内存暂存,不落库)→ 原执行 →
//   成功 commit(补记 inserted id 等执行后信息并落库)/ 失败 discard(直接丢弃)。
// 依赖注入:与 SessionService 共用同一 Arc<Db>(db 连接池不在 ToolDeps 上,
// 直接持有同源句柄最小侵入);经 ToolRegistry::set_undo(OnceLock)注入注册表,
// run_tool 收口读取;API 经 AppState.undo 访问。未注入(单元测试)时不产快照,
// 行为与旧版一致。
use crate::models::db::{now_iso, Db};
use crate::models::types::ToolContext;
use crate::services::memory_service::MemoryService;
use crate::services::session_service::SessionService;
use crate::services::settings_service::RuntimeSettings;
use crate::tools::agent_tools::safe_rel_path;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 纳入回退快照的写工具名单(其余工具不产生快照)
const WRITE_TOOLS: [&str; 5] = [
    "write",
    "replace",
    "create",
    "update_variables",
    "memory_write",
];

/// 单文件快照内容上限(256KB):超出记 truncated=true,restore 拒绝并中文提示
const MAX_SNAPSHOT_FILE_BYTES: usize = 256 * 1024;

/// 恢复被同会话生成占位拒绝时的固定文案(API 层据此映射 409;codex 复审 P1-1)。
pub const RESTORE_BUSY_MESSAGE: &str = "该会话正在生成中,请先停止生成再回退";

/// 列表项(不含 payload,供 GET /api/chat/sessions/{id}/undo 输出)
#[derive(Debug, Clone, Serialize)]
pub struct UndoSnapshotInfo {
    pub id: String,
    pub tool_name: String,
    pub label: String,
    pub anchor_message_id: Option<i64>,
    pub created_at: String,
    /// 产生快照的工具调用 call_id(批 D-1,可空:旧行与身份缺失为 NULL)。
    /// 供前端把快照与工具卡做可匹配核对,不伪造对应关系。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

/// 产生快照的工具调用身份(批 D-1):由引擎执行侧(run_id/call_id 的源头)传入,
/// 随快照落库,供「回退到此处」按调用精确绑定与审计关联。
#[derive(Debug, Clone)]
pub struct ToolCallIdentity {
    pub run_id: String,
    pub call_id: String,
}

/// 回退成功回执(批 D-2):目标快照的中文 label + 实际恢复的快照条数
/// (含目标本身;>1 表示同一批一并撤销了其后的操作)。
#[derive(Debug, Clone, Serialize)]
pub struct RestoreReceipt {
    pub label: String,
    pub restored_count: usize,
}

pub struct UndoService {
    db: Arc<Db>,
    sessions: Arc<SessionService>,
    memory: Arc<MemoryService>,
    /// 运行期设置(读 undo_enabled;全局开关,非双模式覆盖字段,故直接读基础值)
    settings: Arc<Mutex<RuntimeSettings>>,
    /// 数据目录(角色文件区 = data_dir/character_files/{character_id},
    /// 与 tools/agent_tools_shared.rs character_file_root 同规则)
    data_dir: PathBuf,
    /// 恢复期互斥(批 D-2):串行化并发/重复的 restore 请求——两个回退同时应用
    /// 逆操作会互相踩(各自读到同一批快照,交错写文件/删快照)。粗粒度单锁,
    /// 回退是低频用户动作,不按会话细分。
    restore_gate: Mutex<()>,
    /// 每会话「生成/恢复」协调锁(codex 复审 P1-1):同会话的生成任务与 restore
    /// 双侧都对本锁 try-lock,原子互斥——单靠 API 的 is_active 预检存在检查与
    /// 注册之间的 TOCTOU 窗口(生成可在两者之间开始)、且 commit_staged 不参与
    /// 任何协调。锁条目随会话惰性创建;条目超阈值时**机会性回收**空闲项
    /// (见 [`Self::session_lock`],codex 复审二轮 P2),被持有的锁不被回收。
    session_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// 执行前暂存的快照(未落库);commit/discard 两段式收口
pub struct SnapshotStager {
    service: Arc<UndoService>,
    /// 快照 id **前移**到执行前生成(批 D-1):执行成功后以此 id 落库并把同一 id
    /// 回传给引擎(SSE 工具结果元数据/tool_calls 绑定列),三方(快照/工具卡/绑定)
    /// 用同一 id,前端不再凭工具名与锚点猜测。
    id: String,
    session_id: String,
    tool_name: String,
    label: String,
    anchor_message_id: Option<i64>,
    /// 工具调用身份(批 D-1;无身份的调用路径为 None)
    call: Option<ToolCallIdentity>,
    payload: Value,
}

impl SnapshotStager {
    /// 执行成功:补记执行后信息(write 气泡的 message_id、memory_write 的插入 id)
    /// 并落库。返回落库成功的快照 id(引擎用于绑定工具调用);落库失败只记日志
    /// 不上抛——快照是增强能力,不影响主流程。
    pub fn commit(self, tool_output: &str) -> Option<String> {
        let service = self.service.clone();
        service.commit_staged(self, tool_output)
    }

    /// 执行失败:快照随本次执行一并丢弃(两段式:未执行的写入不需要逆操作)
    pub fn discard(self) {}
}

impl UndoService {
    pub fn new(
        db: Arc<Db>,
        sessions: Arc<SessionService>,
        memory: Arc<MemoryService>,
        settings: Arc<Mutex<RuntimeSettings>>,
        data_dir: PathBuf,
    ) -> Self {
        UndoService {
            db,
            sessions,
            memory,
            settings,
            data_dir,
            restore_gate: Mutex::new(()),
            session_locks: Mutex::new(HashMap::new()),
        }
    }

    /// 取(或惰性创建)某会话的「生成/恢复」协调锁(codex 复审 P1-1)。
    /// 生成侧在异步任务内 `try_lock_owned`(常态瞬时获得;restore 持锁期间失败并
    /// 以可见错误结束本次流);restore 侧在阻塞上下文 `try_lock`(生成进行中即
    /// 失败,返回 [`RESTORE_BUSY_MESSAGE`])。双侧都是 try 语义,不排队、不等待。
    pub fn session_lock(&self, session_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.session_locks.lock().unwrap_or_else(|e| e.into_inner());
        // 机会性回收(codex 复审二轮 P2):条目只随时间增长——超过阈值时清掉「仅剩
        // map 自己持有(strong_count==1)」的条目;被生成/恢复持有的锁 strong_count≥2,
        // 不会被误清。回收不影响功能:同 id 再取即新建。
        if map.len() > 64 {
            map.retain(|_, l| Arc::strong_count(l) > 1);
        }
        map.entry(session_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// 全局开关(锁中毒恢复取值,不 panic;只读取立即释放,不跨 .await)
    fn enabled(&self) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .undo_enabled
    }

    fn file_root(&self, character_id: &str) -> PathBuf {
        self.data_dir.join("character_files").join(character_id)
    }

    /// 快照时该会话 max(messages.id)(无消息为 NULL)
    fn max_message_id(&self, session_id: &str) -> Option<i64> {
        let conn = self.db.read().ok()?;
        conn.query_row(
            "SELECT MAX(id) FROM messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    }

    /// run_tool 收口调用:写工具且开关开启时取执行前快照;构建失败(参数不合法等,
    /// 工具执行本身也会报错)返回 None——快照 best-effort,绝不挡工具执行。
    /// `identity`:产生本次调用的 run_id/call_id(批 D-1;无身份路径传 None)。
    pub fn snapshot_before(
        self: &Arc<Self>,
        tool_name: &str,
        args: &Value,
        ctx: &ToolContext,
        identity: Option<&ToolCallIdentity>,
    ) -> Option<SnapshotStager> {
        if !WRITE_TOOLS.contains(&tool_name) || !self.enabled() {
            return None;
        }
        let (payload, label) = self.build_payload(tool_name, args, ctx)?;
        let anchor_message_id = self.max_message_id(&ctx.session_id);
        Some(SnapshotStager {
            service: self.clone(),
            id: uuid::Uuid::new_v4().to_string(),
            session_id: ctx.session_id.clone(),
            tool_name: tool_name.to_string(),
            label,
            anchor_message_id,
            call: identity.cloned(),
            payload,
        })
    }

    /// 按工具构建逆操作负载(只读旧状态,不做任何修改);返回 (payload, 中文 label)
    fn build_payload(
        &self,
        tool_name: &str,
        args: &Value,
        ctx: &ToolContext,
    ) -> Option<(Value, String)> {
        match tool_name {
            "write" => self.build_write(args, ctx),
            "replace" => self.build_replace(args, ctx),
            "create" => self.build_create(args, ctx),
            "update_variables" => {
                // 逆操作 = 整树写回(save_assistant_vars);原状态无记录时 before 为空树,
                // restore 写回空树(会留下一行空 data_raw,语义等价,可接受)
                let before = self.sessions.load_assistant_vars(&ctx.session_id).to_json();
                Some((
                    json!({ "kind": "vars", "before": before }),
                    "更新变量树".to_string(),
                ))
            }
            "memory_write" => {
                let fact = args.get("fact").and_then(|v| v.as_str())?.trim();
                if fact.is_empty() {
                    return None; // 工具必然失败,无需快照
                }
                // 执行后补记 inserted id:以本角色执行前 max(id) 为界,commit 查其后的新行
                let max_id_before = self.max_memory_id(&ctx.character_id);
                let label = format!("写入记忆:{}", truncate_chars(fact, 20));
                Some((
                    json!({
                        "kind": "memory_insert",
                        "max_id_before": max_id_before,
                        "inserted_id": Value::Null,
                    }),
                    label,
                ))
            }
            _ => None,
        }
    }

    fn max_memory_id(&self, character_id: &str) -> i64 {
        let Ok(conn) = self.db.read() else {
            return 0;
        };
        conn.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM memory_entries WHERE character_id = ?1",
            params![character_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
    }

    /// 文件快照项:old = 执行前内容;不存在为 null(逆操作 = 删除);
    /// 内容超限或不可读(非 UTF-8)标 truncated=true(逆操作拒绝)
    fn file_entry(&self, character_id: &str, rel: &str, entry_type: &str) -> Value {
        let full = self.file_root(character_id).join(rel);
        if !full.exists() {
            return json!({
                "path": rel, "entry_type": entry_type,
                "existed": false, "truncated": false, "old": Value::Null,
            });
        }
        if entry_type == "dir" {
            // 目录无内容快照;create 目录逆操作 = 删除空目录
            return json!({
                "path": rel, "entry_type": entry_type,
                "existed": true, "truncated": false, "old": Value::Null,
            });
        }
        match std::fs::read_to_string(&full) {
            Ok(content) if content.len() <= MAX_SNAPSHOT_FILE_BYTES => json!({
                "path": rel, "entry_type": entry_type,
                "existed": true, "truncated": false, "old": content,
            }),
            // 超限或读取失败:都无法精确恢复,统一按截断拒绝语义处理
            _ => json!({
                "path": rel, "entry_type": entry_type,
                "existed": true, "truncated": true, "old": Value::Null,
            }),
        }
    }

    fn build_write(&self, args: &Value, ctx: &ToolContext) -> Option<(Value, String)> {
        let target = args.get("target").and_then(|v| v.as_str())?;
        match target {
            "file" => {
                let path = args.get("path").and_then(|v| v.as_str())?;
                let rel = safe_rel_path(path).ok()?;
                let entry = self.file_entry(&ctx.character_id, &rel, "file");
                Some((
                    json!({ "kind": "files", "entries": [entry] }),
                    format!("写入文件 {rel}"),
                ))
            }
            // 气泡插入:执行前无 id 可记,commit 时从工具输出补 message_id(逆操作 = 删除该行)
            "bubble" => Some((
                json!({ "kind": "bubble_insert", "message_id": Value::Null }),
                "写入对话气泡".to_string(),
            )),
            _ => None,
        }
    }

    fn build_replace(&self, args: &Value, ctx: &ToolContext) -> Option<(Value, String)> {
        let ops = args.get("operations")?.as_array()?;
        let mut files: Vec<Value> = Vec::new();
        let mut bubbles: Vec<Value> = Vec::new();
        let mut seen_files: Vec<String> = Vec::new();
        let mut seen_bubbles: Vec<i64> = Vec::new();
        for op in ops {
            match op.get("target").and_then(|v| v.as_str()) {
                Some("file") => {
                    let Some(path) = op.get("path").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let Ok(rel) = safe_rel_path(path) else {
                        continue; // 单项会失败,无状态变化,无需快照
                    };
                    // 同一路径多次操作:以执行前整文件快照为准,只记一次
                    if seen_files.contains(&rel) {
                        continue;
                    }
                    seen_files.push(rel.clone());
                    files.push(self.file_entry(&ctx.character_id, &rel, "file"));
                }
                Some("bubble") => {
                    let Some(id) = op.get("id").and_then(|v| v.as_i64()) else {
                        continue;
                    };
                    if seen_bubbles.contains(&id) {
                        continue;
                    }
                    seen_bubbles.push(id);
                    // 执行前整行快照(行不存在 → before:null,该单项会失败,restore 跳过)
                    let before = self
                        .sessions
                        .get_message(&ctx.session_id, id)
                        .map(|m| {
                            json!({
                                "id": m.id, "role": m.role, "content": m.content,
                                "extra": m.extra, "created_at": m.created_at,
                            })
                        })
                        .unwrap_or(Value::Null);
                    bubbles.push(json!({ "id": id, "before": before }));
                }
                _ => {}
            }
        }
        if files.is_empty() && bubbles.is_empty() {
            return None;
        }
        let label = if !files.is_empty() && !bubbles.is_empty() {
            "修改文件与气泡".to_string()
        } else if !files.is_empty() {
            format!("修改文件 {}", truncate_chars(&seen_files.join("、"), 40))
        } else if seen_bubbles.len() == 1 {
            format!("修改气泡 #{}", seen_bubbles[0])
        } else {
            format!("修改 {} 个气泡", seen_bubbles.len())
        };
        Some((
            json!({ "kind": "replace", "files": files, "bubbles": bubbles }),
            label,
        ))
    }

    fn build_create(&self, args: &Value, ctx: &ToolContext) -> Option<(Value, String)> {
        let items = args.get("items")?.as_array()?;
        let mut entries: Vec<Value> = Vec::new();
        for it in items {
            let Some(ty) = it.get("type").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(path) = it.get("path").and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(rel) = safe_rel_path(path) else {
                continue;
            };
            if !matches!(ty, "dir" | "file") {
                continue;
            }
            // 已存在的项 create 会单项失败(无状态变化),不需要逆操作
            if self.file_root(&ctx.character_id).join(&rel).exists() {
                continue;
            }
            entries.push(json!({
                "path": rel, "entry_type": ty,
                "existed": false, "truncated": false, "old": Value::Null,
            }));
        }
        if entries.is_empty() {
            return None;
        }
        let label = if entries.len() == 1 {
            let ty = if entries[0]["entry_type"].as_str() == Some("dir") {
                "目录"
            } else {
                "文件"
            };
            format!("新建{} {}", ty, entries[0]["path"].as_str().unwrap_or(""))
        } else {
            format!("新建 {} 项", entries.len())
        };
        Some((json!({ "kind": "files", "entries": entries }), label))
    }

    /// 两段式第二段:补记执行后信息并以**执行前生成好的 id** 落库;失败只记日志。
    /// 返回落库成功的快照 id(供引擎绑定工具调用;失败/丢弃为 None)。
    fn commit_staged(&self, stager: SnapshotStager, tool_output: &str) -> Option<String> {
        let SnapshotStager {
            id,
            session_id,
            tool_name,
            label,
            anchor_message_id,
            call,
            mut payload,
            ..
        } = stager;
        match payload.get("kind").and_then(|v| v.as_str()) {
            Some("bubble_insert") => {
                // write 气泡成功的输出为 JSON 且必含 message_id;取不到说明输出契约漂移,丢弃快照
                let mid = serde_json::from_str::<Value>(tool_output)
                    .ok()
                    .and_then(|out| out.get("message_id").and_then(|v| v.as_i64()));
                let Some(mid) = mid else {
                    tracing::warn!(
                        session_id = session_id,
                        "回退快照丢弃:write 气泡输出缺 message_id"
                    );
                    return None;
                };
                payload["message_id"] = json!(mid);
            }
            Some("memory_insert") => {
                let before = payload
                    .get("max_id_before")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let inserted = self.new_memory_id_after(&session_id, before);
                let Some(inserted) = inserted else {
                    tracing::warn!(
                        session_id = session_id,
                        "回退快照丢弃:memory_write 后未找到新记忆行"
                    );
                    return None;
                };
                payload["inserted_id"] = json!(inserted);
            }
            _ => {}
        }
        let (call_id, run_id) = match &call {
            Some(c) => (Some(c.call_id.clone()), Some(c.run_id.clone())),
            None => (None, None),
        };
        let payload_str = payload.to_string();
        // 写锁 + 事务:seq 分配(全局 max+1)与插入原子完成——单写连接串行,无并发面,
        // 事务补一层「中途失败不留半行」的完整性。
        let mut conn = self.db.write();
        let result = (|| -> Result<(), String> {
            let tx = conn
                .transaction()
                .map_err(|e| format!("开启快照写事务失败: {e}"))?;
            let seq: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM undo_snapshots",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| format!("分配快照序号失败: {e}"))?;
            tx.execute(
                "INSERT INTO undo_snapshots (id, session_id, anchor_message_id, tool_name, label, payload, created_at, seq, call_id, run_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id,
                    session_id,
                    anchor_message_id,
                    tool_name,
                    label,
                    payload_str,
                    now_iso(),
                    seq,
                    call_id,
                    run_id
                ],
            )
            .map_err(|e| format!("写入回退快照失败: {e}"))?;
            tx.commit()
                .map_err(|e| format!("提交快照写事务失败: {e}"))?;
            Ok(())
        })();
        match result {
            Ok(()) => Some(id),
            Err(e) => {
                tracing::warn!(error = e, "回退快照落库失败(不影响工具执行结果)");
                None
            }
        }
    }

    /// memory_write 执行后:本角色 max_id_before 之后新插入的第一条记忆
    fn new_memory_id_after(&self, session_id: &str, before: i64) -> Option<i64> {
        let character_id = self.sessions.get(session_id).map(|s| s.character_id)?;
        let conn = self.db.read().ok()?;
        conn.query_row(
            "SELECT MIN(id) FROM memory_entries WHERE character_id = ?1 AND id > ?2",
            params![character_id, before],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// 会话快照列表(新→旧;不含 payload)。排序以 `seq` 为准(批 D-2:
    /// 毫秒时间戳同毫秒并列,不能再作排序/界定依据;旧行已按 rowid 回填 seq)。
    pub fn list(&self, session_id: &str) -> Vec<UndoSnapshotInfo> {
        let Ok(conn) = self.db.read() else {
            return Vec::new();
        };
        let mut stmt = match conn.prepare(
            "SELECT id, tool_name, label, anchor_message_id, created_at, call_id
             FROM undo_snapshots WHERE session_id = ?1
             ORDER BY COALESCE(seq, rowid) DESC",
        ) {
            Ok(s) => s,
            Err(e) => {
                return super::log_query_failure("回退快照列表 prepare", e);
            }
        };
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(UndoSnapshotInfo {
                id: row.get(0)?,
                tool_name: row.get(1)?,
                label: row.get(2)?,
                anchor_message_id: row.get(3)?,
                created_at: row.get(4)?,
                call_id: row.get(5)?,
            })
        });
        match rows {
            Ok(r) => r.filter_map(|x| x.ok()).collect(),
            Err(e) => super::log_query_failure("回退快照列表 query_map", e),
        }
    }

    /// 快照所属会话 id(供 API 层做「生成进行中不可回退」前置判定)。
    pub fn session_of(&self, id: &str) -> Option<String> {
        let conn = self.db.read().ok()?;
        conn.query_row(
            "SELECT session_id FROM undo_snapshots WHERE id = ?1",
            params![id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// 回退(批 D-2 语义 = 「回到所选快照**之前**的状态」):
    /// 取目标及其**同会话后续全部快照**(按 seq 界定的确定顺序),逆序(新→旧)
    /// 逐条应用逆操作——数据整体回到目标时点,而不是只撤销所选一条却删掉后续恢复依据。
    ///
    /// 安全性:
    /// - 全批**预检**在前:任一条 payload 损坏/路径非法/截断标志即拒绝整批,
    ///   不修改任何文件、不删除任何快照;
    /// - 中途失败:保留**全部**快照(逆操作各自幂等,修复原因后原样重试即可,
    ///   重放已执行过的逆操作是无害的);
    /// - 全部成功:按**精确 id 列表**清理已处理快照(不再用 `created_at >=` 扫删,
    ///   同毫秒的其它快照不再被误伤、批外新快照不受影响)。
    ///
    /// 返回 Ok(None) = 快照不存在(404 语义);Ok(Some(回执));Err = 失败(500 语义)。
    pub fn restore(&self, id: &str) -> Result<Option<RestoreReceipt>, String> {
        // 串行化并发/重复回退:两个请求同时读到同一批快照会交错写文件与删行
        let _gate = self.restore_gate.lock().unwrap_or_else(|e| e.into_inner());
        // 1) 目标 + 选批(seq >= 目标;新→旧)
        let Some((session_id, batch)) = self.load_restore_batch(id)? else {
            return Ok(None);
        };
        // 1b) 同会话生成互斥(codex 复审 P1-1):原子 try-lock——API 的 is_active
        // 预检只是快速失败路径,此处才是与「生成注册」真正互斥的判据。
        let session_lock = self.session_lock(&session_id);
        let _generation_guard = match session_lock.try_lock() {
            Ok(g) => g,
            Err(_) => return Err(RESTORE_BUSY_MESSAGE.to_string()),
        };
        // 2) 预检整批 + 解析 payload(不通过则零改动零删除)
        let mut steps: Vec<RestoreStep> = Vec::with_capacity(batch.len());
        for item in &batch {
            let payload = precheck_payload(&item.id, &item.payload_raw)
                .map_err(|e| format!("预检未通过,未做任何修改:{e}"))?;
            steps.push(RestoreStep {
                id: item.id.clone(),
                label: item.label.clone(),
                payload,
            });
        }
        // character_id 以会话记录为权威(文件区/记忆恢复需要)
        let character_id = self
            .sessions
            .get(&session_id)
            .map(|s| s.character_id)
            .ok_or_else(|| "会话不存在,无法回退".to_string())?;
        // 3) 逆序应用(新→旧):整批一起回到目标时点
        let total = steps.len();
        for (done, step) in steps.iter().enumerate() {
            if let Err(e) = self.apply_inverse(&session_id, &character_id, &step.payload) {
                return Err(format!(
                    "已恢复 {done}/{total} 条更新的快照(逆操作幂等,可修复原因后原样重试);\
                     处理「{}」时失败:{e}。快照记录已保留",
                    step.label
                ));
            }
        }
        // 4) 全部成功:按精确 id 列表清理已处理快照(批外新快照不受影响)。
        //    codex 复审 P2-1:清理放进**单事务**——中途任一 DELETE 失败即整体回滚,
        //    不出现「删了一半又回 500」的中间态(逆操作幂等,整批可原样重试)。
        {
            let mut conn = self.db.write();
            let tx = conn
                .transaction()
                .map_err(|e| format!("开启回退清理事务失败: {e}"))?;
            for step in &steps {
                tx.execute("DELETE FROM undo_snapshots WHERE id = ?1", params![step.id])
                    .map_err(|e| format!("清理回退快照失败: {e}"))?;
            }
            tx.commit()
                .map_err(|e| format!("提交回退清理事务失败: {e}"))?;
        }
        // steps 新→旧,末元素即本次点击的目标快照
        let target_label = steps.last().map(|s| s.label.clone()).unwrap_or_default();
        Ok(Some(RestoreReceipt {
            label: target_label,
            restored_count: total,
        }))
    }

    /// 读取目标及同会话 `seq >= 目标` 的全部快照(新→旧);
    /// 目标不存在返回 Ok(None)。payload 保持原文本,由 restore 统一预检/解析。
    fn load_restore_batch(&self, id: &str) -> Result<Option<(String, Vec<RestoreItem>)>, String> {
        let conn = self.db.read().map_err(|e| e.to_string())?;
        let target = conn
            .query_row(
                "SELECT session_id, COALESCE(seq, rowid) FROM undo_snapshots WHERE id = ?1",
                params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|e| format!("读取回退快照失败: {e}"))?;
        let Some((session_id, target_seq)) = target else {
            return Ok(None);
        };
        let mut stmt = conn
            .prepare(
                "SELECT id, label, payload FROM undo_snapshots
                 WHERE session_id = ?1 AND COALESCE(seq, rowid) >= ?2
                 ORDER BY COALESCE(seq, rowid) DESC",
            )
            .map_err(|e| format!("读取回退快照批失败: {e}"))?;
        let rows = stmt
            .query_map(params![session_id, target_seq], |row| {
                Ok(RestoreItem {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    payload_raw: row.get(2)?,
                })
            })
            .map_err(|e| format!("遍历回退快照批失败: {e}"))?;
        let batch: Vec<RestoreItem> = rows.filter_map(|r| r.ok()).collect();
        Ok(Some((session_id, batch)))
    }

    /// 按 payload kind 分派逆操作;数组类负载一律逆序(后执行的先撤销)
    fn apply_inverse(
        &self,
        session_id: &str,
        character_id: &str,
        payload: &Value,
    ) -> Result<(), String> {
        match payload.get("kind").and_then(|v| v.as_str()) {
            Some("files") => {
                let entries = payload
                    .get("entries")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                for entry in entries.iter().rev() {
                    self.restore_file_entry(character_id, entry)?;
                }
                Ok(())
            }
            Some("bubble_insert") => {
                let Some(mid) = payload.get("message_id").and_then(|v| v.as_i64()) else {
                    return Err("快照缺少气泡 id,无法回退".into());
                };
                // 幂等删除:行可能已被 truncate/手动删除,不存在不视为失败
                self.sessions.delete_message(session_id, mid);
                Ok(())
            }
            Some("replace") => {
                let files = payload
                    .get("files")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                for entry in files.iter().rev() {
                    self.restore_file_entry(character_id, entry)?;
                }
                let bubbles = payload
                    .get("bubbles")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                for row in bubbles.iter().rev() {
                    self.restore_bubble_row(session_id, row)?;
                }
                Ok(())
            }
            Some("vars") => {
                let before = payload
                    .get("before")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let vars = crate::parsing::assistant::AssistantVars::from_json(before);
                // 与 update_variables 工具同路径:只写 session_assistant_vars,不同步镜像
                self.sessions
                    .save_assistant_vars(session_id, &vars)
                    .map_err(|e| format!("恢复变量树失败: {e}"))?;
                Ok(())
            }
            Some("memory_insert") => {
                let Some(mid) = payload.get("inserted_id").and_then(|v| v.as_i64()) else {
                    return Err("快照缺少记忆 id,无法回退".into());
                };
                // 幂等删除:行可能已被手动删除
                self.memory.delete(mid);
                Ok(())
            }
            _ => Err("未知快照类型,无法回退".into()),
        }
    }

    /// 文件逆操作:old 有值 → 写回原内容;old=null + file → 删除;old=null + dir → 删空目录
    fn restore_file_entry(&self, character_id: &str, entry: &Value) -> Result<(), String> {
        let rel = entry
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "快照缺文件路径".to_string())?;
        let rel = safe_rel_path(rel)?; // 防御:payload 内路径理应合法
        if entry.get("truncated").and_then(|v| v.as_bool()) == Some(true) {
            return Err(format!("文件 {rel} 原始内容超过 256KB,快照不完整,无法回退"));
        }
        let full = self.file_root(character_id).join(&rel);
        match entry.get("old") {
            Some(Value::String(old)) => {
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("恢复文件 {rel} 时创建目录失败: {e}"))?;
                }
                std::fs::write(&full, old).map_err(|e| format!("恢复文件 {rel} 失败: {e}"))?;
                Ok(())
            }
            _ => {
                let is_dir = entry.get("entry_type").and_then(|v| v.as_str()) == Some("dir");
                if is_dir {
                    // 只删空目录:目录内若有后续内容(回退后用户/工具新写入),保守保留
                    match std::fs::remove_dir(&full) {
                        Ok(_) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => {}
                    }
                    Ok(())
                } else {
                    match std::fs::remove_file(&full) {
                        Ok(_) => Ok(()),
                        // 幂等:文件可能已不存在(单项执行失败/后续被删)
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(e) => Err(format!("删除文件 {rel} 失败: {e}")),
                    }
                }
            }
        }
    }

    /// 气泡逆操作:行还在 → UPDATE 回旧 content/extra;行已被删 → 整行插回(含原 id)。
    /// 直接走 SQL 而非 SessionService::update_message(编辑语义会附加 edited 标记;
    /// 整行恢复要求 content+extra 逐字节回到执行前)。
    fn restore_bubble_row(&self, session_id: &str, row: &Value) -> Result<(), String> {
        let Some(before) = row.get("before").filter(|b| !b.is_null()) else {
            return Ok(()); // 执行前行已不存在(该单项已失败),无状态变化
        };
        let id = before
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| "快照缺气泡 id".to_string())?;
        let content = before
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let extra_str = before
            .get("extra")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "{}".to_string());
        if self.sessions.get_message(session_id, id).is_some() {
            let conn = self.db.write();
            conn.execute(
                "UPDATE messages SET content = ?1, extra = ?2 WHERE session_id = ?3 AND id = ?4",
                params![content, extra_str, session_id, id],
            )
            .map_err(|e| format!("恢复气泡 #{id} 失败: {e}"))?;
        } else {
            let role = before
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("assistant");
            let created_at = before
                .get("created_at")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(now_iso);
            let conn = self.db.write();
            // 显式 id 插回(sqlite_sequence 不受影响,后续插入仍取 max+1)
            conn.execute(
                "INSERT INTO messages (id, session_id, role, content, extra, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, session_id, role, content, extra_str, created_at],
            )
            .map_err(|e| format!("插回气泡 #{id} 失败: {e}"))?;
        }
        Ok(())
    }
}

/// 恢复批中的一条(raw 形态:payload 尚未解析)
struct RestoreItem {
    id: String,
    label: String,
    payload_raw: String,
}

/// 恢复批中的一条(已过预检、payload 已解析)
struct RestoreStep {
    id: String,
    label: String,
    payload: Value,
}

/// 恢复前预检(批 D-2):逐条校验 payload 完整性与可恢复性,返回解析后的 payload。
/// 任一条失败 → 调用方拒绝整批(不修改任何文件,也不删除任何快照)。
/// 覆盖报告 §七 建议 3 的检查面:payload 可解析、kind 合法、文件路径、
/// 256KB 截断标志、气泡/记忆必要字段、变量树 JSON 基线。
fn precheck_payload(snapshot_id: &str, payload_raw: &str) -> Result<Value, String> {
    let payload: Value = serde_json::from_str(payload_raw)
        .map_err(|e| format!("快照 {snapshot_id} 数据损坏,无法回退: {e}"))?;
    match payload.get("kind").and_then(|v| v.as_str()) {
        Some("files") => {
            let entries = payload
                .get("entries")
                .and_then(|v| v.as_array())
                .ok_or_else(|| format!("快照 {snapshot_id} 缺 entries 数组"))?;
            for entry in entries {
                precheck_file_entry(snapshot_id, entry)?;
            }
        }
        Some("replace") => {
            let files = payload
                .get("files")
                .and_then(|v| v.as_array())
                .ok_or_else(|| format!("快照 {snapshot_id} 缺 files 数组"))?;
            for entry in files {
                precheck_file_entry(snapshot_id, entry)?;
            }
            let bubbles = payload
                .get("bubbles")
                .and_then(|v| v.as_array())
                .ok_or_else(|| format!("快照 {snapshot_id} 缺 bubbles 数组"))?;
            for row in bubbles {
                let rid = row
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| format!("快照 {snapshot_id} 气泡记录缺 id"))?;
                // codex 复审 P1-3(二轮加严):before 的读取面与 restore_bubble_row 一一
                // 对应——缺字段会走「默认值」静默改写(缺 content 写空串、缺 extra 写
                // {}),before.id 与外层 id 不一致会把内容覆写到别的消息行,都必须拒绝。
                match row.get("before") {
                    Some(Value::Null) => {}
                    Some(Value::Object(o)) => {
                        if o.get("content").and_then(|v| v.as_str()).is_none() {
                            return Err(format!(
                                "快照 {snapshot_id} 气泡 #{rid} 的 before.content 缺失或非字符串"
                            ));
                        }
                        if o.get("role").and_then(|v| v.as_str()).is_none() {
                            return Err(format!(
                                "快照 {snapshot_id} 气泡 #{rid} 的 before.role 缺失或非字符串"
                            ));
                        }
                        if o.get("created_at").and_then(|v| v.as_str()).is_none() {
                            return Err(format!(
                                "快照 {snapshot_id} 气泡 #{rid} 的 before.created_at 缺失或非字符串"
                            ));
                        }
                        // extra 类型不设限(历史导入可能存非对象 JSON,原样回写不失真),
                        // 但必须**在场**——缺席会被 restore 写成 "{}"。
                        if o.get("extra").is_none() {
                            return Err(format!(
                                "快照 {snapshot_id} 气泡 #{rid} 的 before.extra 缺失"
                            ));
                        }
                        if o.get("id").and_then(|v| v.as_i64()) != Some(rid) {
                            return Err(format!(
                                "快照 {snapshot_id} 气泡 #{rid} 的 before.id 与外层 id 不一致"
                            ));
                        }
                    }
                    _ => {
                        return Err(format!(
                            "快照 {snapshot_id} 气泡 #{rid} 的 before 缺失或类型非法(应为对象或 null)"
                        ));
                    }
                }
            }
        }
        Some("bubble_insert") => {
            payload
                .get("message_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| format!("快照 {snapshot_id} 缺气泡 id,无法回退"))?;
        }
        Some("vars") => {
            let before = payload
                .get("before")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("快照 {snapshot_id} 缺变量树基线"))?;
            serde_json::from_str::<Value>(before)
                .map_err(|e| format!("快照 {snapshot_id} 变量树基线损坏: {e}"))?;
        }
        Some("memory_insert") => {
            payload
                .get("inserted_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| format!("快照 {snapshot_id} 缺记忆 id,无法回退"))?;
        }
        Some(other) => {
            return Err(format!("快照 {snapshot_id} 类型未知({other}),无法回退"));
        }
        None => return Err(format!("快照 {snapshot_id} 缺类型字段,无法回退")),
    }
    Ok(payload)
}

/// 单个文件快照项的预检:路径必须通过 safe_rel_path(防御 payload 内路径),
/// 截断标志必须为假(256KB 上限外的内容无法精确恢复)。
fn precheck_file_entry(snapshot_id: &str, entry: &Value) -> Result<(), String> {
    let rel = entry
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("快照 {snapshot_id} 缺文件路径"))?;
    safe_rel_path(rel).map_err(|e| format!("快照 {snapshot_id} 文件路径非法({rel}): {e}"))?;
    // codex 复审 P1-3:payload 是持久化数据(可能被截断/手工污染),字段与类型必须
    // 齐备——apply 侧把「old 非字符串」一律当「逆操作 = 删除」,缺字段会静默删文件。
    match entry.get("entry_type").and_then(|v| v.as_str()) {
        Some("file") | Some("dir") => {}
        _ => {
            return Err(format!(
                "快照 {snapshot_id} 的文件项 {rel} 缺合法 entry_type(应为 file/dir)"
            ));
        }
    }
    match entry.get("old") {
        Some(Value::String(_)) | Some(Value::Null) => {}
        _ => {
            return Err(format!(
                "快照 {snapshot_id} 的文件项 {rel} 的 old 缺失或类型非法(应为字符串或 null)"
            ));
        }
    }
    match entry.get("truncated") {
        Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => {
            return Err(format!(
                "快照 {snapshot_id} 的文件 {rel} 原始内容超过 256KB,快照不完整,无法回退"
            ));
        }
        _ => {
            return Err(format!(
                "快照 {snapshot_id} 的文件项 {rel} 的 truncated 缺失或类型非法(应为布尔)"
            ));
        }
    }
    if entry.get("existed").is_some_and(|v| !v.is_boolean()) {
        return Err(format!(
            "快照 {snapshot_id} 的文件项 {rel} 的 existed 类型非法(应为布尔)"
        ));
    }
    Ok(())
}

/// 中文 label 辅助:按字符数截断(避免超长路径/事实撑爆列表展示)
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    /// 最小装配:临时库 + 角色/会话行 + 一条 vars 快照(不依赖 HTTP 与真实角色文件)
    fn build_service(dir: &TempDataDir) -> (Arc<Db>, Arc<UndoService>, String) {
        let db = Arc::new(Db::open(&dir.join("kedai.db"), dir.path()).expect("打开测试库失败"));
        {
            let conn = db.write();
            conn.execute(
                "INSERT INTO characters(id, name, chara_name, file_path, data_raw, created_at) VALUES ('c1', '测试', '测试', '', '{}', 't')",
                [],
            )
            .expect("插角色行失败");
        }
        let sessions = Arc::new(SessionService::new(db.clone()));
        let sid = sessions
            .create("c1", Some("锁测试"))
            .expect("建会话失败")
            .id;
        let memory = Arc::new(MemoryService::new(db.clone()));
        let settings = Arc::new(Mutex::new(RuntimeSettings::from_config(
            &crate::config::test_config(),
        )));
        let svc = Arc::new(UndoService::new(
            db.clone(),
            sessions,
            memory,
            settings,
            dir.path().to_path_buf(),
        ));
        {
            let conn = db.write();
            conn.execute(
                "INSERT INTO undo_snapshots (id, session_id, tool_name, label, payload, created_at, seq) VALUES ('snap-busy', ?1, 'update_variables', '更新变量树', '{\"kind\":\"vars\",\"before\":\"{}\"}', ?2, 1)",
                params![sid, now_iso()],
            )
            .expect("插快照行失败");
        }
        (db, svc, sid)
    }

    /// codex 复审 P1-1(确定性证据):生成侧持有每会话锁期间,restore 必须拒绝(busy);
    /// 释放后同一快照可正常恢复——证明「生成↔回退」互斥锁真实生效,而非仅 API 预检形态。
    /// 持锁形态与 /api/chat/send 的 `try_lock_owned` 同款(OwnedMutexGuard 自持 Arc)。
    #[test]
    fn restore_rejects_while_session_lock_held() {
        let dir = TempDataDir::new("undo-session-lock");
        let (_db, svc, sid) = build_service(&dir);
        let guard = svc
            .session_lock(&sid)
            .try_lock_owned()
            .expect("空闲锁应可获取");
        let err = svc.restore("snap-busy").expect_err("持锁期间应拒绝恢复");
        assert_eq!(err, RESTORE_BUSY_MESSAGE);
        drop(guard);
        let receipt = svc
            .restore("snap-busy")
            .expect("释放后应可恢复")
            .expect("快照应存在");
        assert_eq!(receipt.restored_count, 1);
    }

    /// codex 复审二轮 P2(三轮判假绿后重写):锁表机会性回收——空闲条目(仅 map 持有)
    /// 在超阈值时被清,被持有的锁不受影响。判据是**同一性**:回收后经 map 再取必须
    /// 还是「guard 自持的那一把」(Arc::ptr_eq)——若实现误清条目,再取会新建一把,
    /// ptr_eq 立即变假(旧版只断言 `held.try_lock()` 仍锁定,外部 Arc 在手时假绿,
    /// codex 三轮指出)。
    #[test]
    fn session_lock_prunes_idle_entries() {
        let dir = TempDataDir::new("undo-lock-prune");
        let (_db, svc, _sid) = build_service(&dir);
        let idle = svc.session_lock("gc-idle");
        let idle_weak = Arc::downgrade(&idle);
        drop(idle); // 只剩 map 持有 → 可回收
        let held = svc.session_lock("gc-held");
        let held_weak = Arc::downgrade(&held);
        // OwnedMutexGuard 自持一份 Arc(与 chat/send 的 try_lock_owned 同形态);
        // 丢掉外部引用后,若 map 误清条目,strong 只剩 guard 那一份。
        let held_guard = held.clone().try_lock_owned().expect("空闲锁应可获取");
        drop(held);
        // 灌到超阈值,再取一把触发回收
        for i in 0..70 {
            let _ = svc.session_lock(&format!("gc-fill-{i}"));
        }
        let _trigger = svc.session_lock("gc-trigger");
        assert!(
            idle_weak.upgrade().is_none(),
            "空闲条目应被回收(map 不再持有)"
        );
        let alive = held_weak.upgrade().expect("guard 自持的锁对象应仍在");
        let again = svc.session_lock("gc-held");
        assert!(
            Arc::ptr_eq(&again, &alive),
            "被持有的锁条目不得被回收:再取必须还是同一把(同一性判据)"
        );
        assert!(
            again.try_lock().is_err(),
            "同一把锁仍被 guard 持有,不可再取"
        );
        drop(held_guard);
        assert!(
            svc.session_lock("gc-held").try_lock().is_ok(),
            "释放后同 id 锁应可正常使用"
        );
    }

    /// codex 复审二轮 P1-3 残留:replace.bubbles 的 before 缺字段 / before.id 与
    /// 外层不一致 必须被预检拒绝(零改动)——apply 侧会拿默认值静默改写消息行。
    #[test]
    fn precheck_rejects_tampered_bubble_before() {
        let dir = TempDataDir::new("undo-bubble-precheck");
        let (db, svc, sid) = build_service(&dir);
        let cases: [(&str, Value); 3] = [
            (
                "缺 extra",
                json!({"kind":"replace","files":[],"bubbles":[{"id":10,"before":{"id":10,"role":"assistant","content":"x","created_at":"t"}}]}),
            ),
            (
                "before.id 与外层不一致",
                json!({"kind":"replace","files":[],"bubbles":[{"id":10,"before":{"id":11,"role":"assistant","content":"x","extra":{},"created_at":"t"}}]}),
            ),
            (
                "缺 role",
                json!({"kind":"replace","files":[],"bubbles":[{"id":10,"before":{"id":10,"content":"x","extra":{},"created_at":"t"}}]}),
            ),
        ];
        for (i, (tag, payload)) in cases.iter().enumerate() {
            let snap_id = format!("snap-bad-bubble-{i}");
            {
                let conn = db.write();
                conn.execute(
                    "INSERT INTO undo_snapshots (id, session_id, tool_name, label, payload, created_at, seq) VALUES (?1, ?2, 'replace', '修改气泡', ?3, ?4, ?5)",
                    params![snap_id, sid, payload.to_string(), now_iso(), 100 + i as i64],
                )
                .expect("插快照行失败");
            }
            let err = svc
                .restore(&snap_id)
                .expect_err(&format!("{tag} 应被预检拒绝"));
            assert!(
                err.contains("预检未通过"),
                "{tag} 的错误应说明预检拒绝: {err}"
            );
            // 零删除:该快照仍在
            assert!(
                svc.session_of(&snap_id).is_some(),
                "{tag}: 预检拒绝不得删除快照"
            );
        }
    }
}
