// 世界书服务(独立世界书):上传/CRUD/按角色查询/注入条目收集/自动分配机制自检
use super::{log_query_failure, log_read_pool_failure};
use crate::models::db::{now_iso, Db};
use crate::models::types::{MessageRecord, WorldBookEntryView, WorldBookRecord};
use crate::parsing::world_book::{collect_entries, parse_world_book_file, WorldEntry};
use crate::parsing::world_book_convert::{convert_world_book, normalize_entry_value};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use uuid::Uuid;

pub struct WorldBookService {
    db: Arc<Db>,
    /// character_id → 解析后的注入条目(2026-09-14 性能缓存)。
    ///
    /// 为什么需要:`collect_entries_for_character` 在每轮生成的装配路径上被调用
    /// (context.rs 每请求、契约缓存未命中、每次 read 工具、每个子 agent),
    /// 而它每次都「查库 → 逐本 serde 解析 1MB JSON → 归一化条目」。
    /// 实测(真实角色卡)单次约 3.8ms,纯属重复劳动。
    ///
    /// 失效设计(fail-safe 的关键):**所有 world_books 写路径都在本结构体内**
    /// (upload/update/delete/add_entry/save_data_raw),写时整体清空即可——
    /// 无跨模块挂钩,也就没有「漏挂一个失效点」导致读到旧世界书的风险。
    /// 注意 `save_character_book` 写的是 characters.data_raw,而本函数只读
    /// `world_books` 表,故与它无关,不需要失效。
    entries_cache: Mutex<HashMap<String, Arc<Vec<WorldEntry>>>>,
    /// 条目缓存代际:每次失效自增(2026-10-15 并发修复批)。
    /// 填缓存在写锁之外构建,「构建期间发生失效」时若不比对代际就插入,
    /// 旧快照会留在刚被清空的缓存里直到下一次写——比对后拒绝插入并重试。
    entries_gen: AtomicU64,
    /// 世界书写入后的附加失效回调(可选;由 app_state 接线为「清空契约缓存」)。
    ///
    /// 为什么挂在这里:契约缓存(ContractRegistry)的来源之一是独立世界书的
    /// `[nlkaleido_contract]` 条目,而写路径**并非只有 API 端点**——worldbook_update
    /// 归档工具与脚本 `importRawWorldbook` 都绕过 API 直写本结构体。钩子挂在
    /// 全部写路径的唯一汇聚点(`invalidate_entries_cache`),这些路径自动覆盖,
    /// 不会「漏挂一个失效点」(与条目缓存自包含失效同一理由)。
    /// 只注入一次(OnceLock);不消费本服务的场景不注入即可。
    write_hook: OnceLock<Arc<dyn Fn() + Send + Sync>>,
}

/// 条目缓存容量上限:条目缓存按 character_id 计价,正常角色数量远小于此;
/// 超限整体清空(纯派生数据,重建代价仅一次解析)。
const ENTRIES_CACHE_CAP: usize = 256;

/// 列:0 id, 1 name, 2 character_id, 3 enabled, 4 source, 5 entry_count, 6 data_raw, 7 created_at, 8 character_name
fn row_to_record(row: &rusqlite::Row, with_data_raw: bool) -> rusqlite::Result<WorldBookRecord> {
    let data_raw: Option<String> = if with_data_raw { row.get(6)? } else { None };
    Ok(WorldBookRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        character_id: row.get(2)?,
        character_name: row.get(8)?,
        enabled: row.get::<_, i64>(3)? != 0,
        source: row.get(4)?,
        entry_count: row.get(5)?,
        data_raw: data_raw.and_then(|s| serde_json::from_str(&s).ok()),
        conversion: None,
        created_at: row.get(7)?,
    })
}

const LIST_SQL: &str = "SELECT w.id, w.name, w.character_id, w.enabled, w.source, w.entry_count, w.data_raw, w.created_at, c.chara_name \
                        FROM world_books w LEFT JOIN characters c ON c.id = w.character_id";

impl WorldBookService {
    pub fn new(db: Arc<Db>) -> Self {
        WorldBookService {
            db,
            entries_cache: Mutex::new(HashMap::new()),
            entries_gen: AtomicU64::new(0),
            write_hook: OnceLock::new(),
        }
    }

    /// 注入写路径的附加失效回调(由 app_state 接线:清空契约缓存)。
    /// 见字段 `write_hook` 的文档注释;只生效一次(OnceLock),重复注入忽略。
    pub fn set_write_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        let _ = self.write_hook.set(hook);
    }

    /// 清空条目缓存:**每个 world_books 写路径都必须调用**。
    /// 漏调会让生成读到旧世界书(fail-safe 由本结构体自包含性保证:写路径都在这里)。
    /// 钩子(契约缓存失效)在此统一触发——唯一汇聚点,API/工具/脚本写路径全覆盖。
    fn invalidate_entries_cache(&self) {
        // 先递增代际再清空(2026-10-15 并发修复批):填缓存侧以「代际是否变」判定可否插入。
        // 顺序反了会让「构建在写前、插入在失效后」的旧快照漏网——写路径清到的是当时还没插入的空位。
        self.entries_gen.fetch_add(1, Ordering::SeqCst);
        self.entries_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        // 回调在条目缓存锁释放后执行:契约注册表读条目会经本服务,避免跨模块锁序
        if let Some(hook) = self.write_hook.get() {
            hook();
        }
    }

    /// 列表(不含 data_raw),按 created_at DESC。
    /// 取连接/预编译/查询任一失败 → 记 warn 回退空列表(与其它列表函数同款 best-effort 语义),
    /// 不 panic 请求线程(阻塞线程 panic 会经 JoinError 放大为 500)。
    pub fn list(&self) -> Vec<WorldBookRecord> {
        let conn = match self.db.read() {
            Ok(c) => c,
            Err(e) => return log_read_pool_failure("世界书列表", e),
        };
        // SQL 为写死常量且表结构由 migration 保证,正常不会失败;仍按 best-effort 兜底
        let mut stmt = match conn.prepare_cached(&format!("{LIST_SQL} ORDER BY w.created_at DESC"))
        {
            Ok(s) => s,
            Err(e) => return log_query_failure("世界书列表 prepare_cached", e),
        };
        // 注意:query_map 结果须先绑定再返回,不能作为块尾表达式直接 match——
        // 块尾临时值的析构顺序会让 stmt/conn 提前 drop(borrow 检查 E0597)。
        let rows = match stmt.query_map([], |row| row_to_record(row, false)) {
            Ok(r) => r,
            Err(e) => return log_query_failure("世界书列表 query_map", e),
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn get(&self, id: &str) -> Option<WorldBookRecord> {
        let conn = self.db.read().ok()?;
        conn.query_row(&format!("{LIST_SQL} WHERE w.id = ?1"), params![id], |row| {
            row_to_record(row, true)
        })
        .optional()
        .ok()
        .flatten()
    }

    /// 上传独立世界书;character_id 为 Some 时绑定到角色(否则全局)。
    /// 上传时自动执行酒馆兼容转换:关键词拆分/常态激发自动判定/属性默认自动,
    /// 转换统计随响应返回(不落库,data_raw 存转换后结果)。
    pub fn upload(
        &self,
        buffer: &[u8],
        original_name: &str,
        character_id: Option<&str>,
    ) -> Result<WorldBookRecord, String> {
        let mut parsed = parse_world_book_file(buffer, original_name)?;
        let conversion = convert_world_book(&mut parsed.data_raw);
        let id = Uuid::new_v4().to_string();
        let entry_count = parsed.entries.len() as i64;
        let data_raw_str = serde_json::to_string(&parsed.data_raw).unwrap_or_else(|_| "{}".into());
        let created_at = now_iso();
        let character_name = character_id.and_then(|cid| self.character_name(cid));
        let record = {
            let conn = self.db.write();
            conn.execute(
                "INSERT INTO world_books (id, name, character_id, enabled, source, entry_count, data_raw, created_at) VALUES (?1, ?2, ?3, 1, 'upload', ?4, ?5, ?6)",
                params![id, parsed.name, character_id, entry_count, data_raw_str, created_at],
            )
            .map_err(|e| format!("写数据库失败: {e}"))?;
            WorldBookRecord {
                id: id.clone(),
                name: parsed.name,
                character_id: character_id.map(|s| s.to_string()),
                character_name,
                enabled: true,
                source: "upload".to_string(),
                entry_count,
                data_raw: Some(parsed.data_raw),
                conversion: Some(conversion),
                created_at,
            }
        };
        self.invalidate_entries_cache();
        Ok(record)
    }

    /// 查询角色显示名(绑定展示用)
    fn character_name(&self, character_id: &str) -> Option<String> {
        self.db
            .read()
            .ok()?
            .query_row(
                "SELECT chara_name FROM characters WHERE id = ?1",
                params![character_id],
                |row| row.get(0),
            )
            .ok()
    }

    /// 更新 enabled / character_id / name;字段为 None 表示不变。
    /// character_id 语义:Some(Some) 绑定,Some(None) 清除绑定(转全局),None 不变。
    ///
    /// 读改写在同一写锁临界区内完成(2026-10-15 并发修复批):原实现先经只读池 get、
    /// 再取写锁 UPDATE,同一本书的两次并发编辑(如 A 改 enabled、B 改 name)各自基于
    /// 旧快照整行回写,后者会把前者刚提交的字段打回旧值。
    pub fn update(
        &self,
        id: &str,
        enabled: Option<bool>,
        character_id: Option<Option<&str>>,
        name: Option<&str>,
    ) -> Option<WorldBookRecord> {
        let conn = self.db.write();
        let existing: WorldBookRecord = conn
            .query_row(&format!("{LIST_SQL} WHERE w.id = ?1"), params![id], |row| {
                row_to_record(row, true)
            })
            .optional()
            .ok()??;
        let new_enabled = enabled.unwrap_or(existing.enabled);
        let new_cid = character_id
            .map(|c| c.map(|s| s.to_string()))
            .unwrap_or(existing.character_id.clone());
        let new_name = name.unwrap_or(&existing.name).to_string();
        // 显示名查询走同一写连接,不取只读池:避免「持写锁等读连接」的跨池等待
        let character_name: Option<String> = new_cid.as_deref().and_then(|c| {
            conn.query_row(
                "SELECT chara_name FROM characters WHERE id = ?1",
                params![c],
                |row| row.get(0),
            )
            .ok()
        });
        let n = conn
            .execute(
                "UPDATE world_books SET enabled = ?1, character_id = ?2, name = ?3 WHERE id = ?4",
                params![new_enabled, new_cid.as_deref(), new_name, id],
            )
            .ok()?;
        if n == 0 {
            return None;
        }
        drop(conn);
        // 写成功才失效(enabled/绑定变更会影响 collect_entries_for_character 的结果)
        self.invalidate_entries_cache();
        Some(WorldBookRecord {
            id: id.to_string(),
            name: new_name,
            character_id: new_cid,
            character_name,
            enabled: new_enabled,
            source: existing.source,
            entry_count: existing.entry_count,
            data_raw: existing.data_raw,
            conversion: None,
            created_at: existing.created_at,
        })
    }

    pub fn delete(&self, id: &str) -> bool {
        let removed = self
            .db
            .write()
            .execute("DELETE FROM world_books WHERE id = ?1", params![id])
            .map(|n| n > 0)
            .unwrap_or(false);
        if removed {
            self.invalidate_entries_cache();
        }
        removed
    }

    /// 指定角色的有效独立世界书:绑定到该角色 或 全局,且 enabled。
    /// 排序附带 id 作全序键(前缀缓存稳定化):created_at 相同(同批导入等)的
    /// 多本书若无次级键,SQLite 返回顺序不确定 → 世界书注入顺序每轮可能漂移,
    /// 破坏 system/常驻注入的前缀逐字节一致性。正常情况下 id 次级键不改变结果,
    /// 只把原本不确定的并列顺序固定下来。
    pub fn enabled_for_character(&self, character_id: &str) -> Vec<WorldBookRecord> {
        let conn = match self.db.read() {
            Ok(c) => c,
            Err(e) => return log_read_pool_failure("角色有效世界书", e),
        };
        let mut stmt = match conn.prepare(&format!("{LIST_SQL} WHERE w.enabled = 1 AND (w.character_id = ?1 OR w.character_id IS NULL) ORDER BY w.created_at DESC, w.id")) {
            Ok(s) => s,
            Err(e) => return log_query_failure("角色有效世界书 prepare", e),
        };
        let rows = match stmt.query_map(params![character_id], |row| row_to_record(row, true)) {
            Ok(r) => r,
            Err(e) => return log_query_failure("角色有效世界书 query_map", e),
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// 注入条目收集:合并该角色的独立世界书(已过滤 enabled),按条目过滤规则在 engine 侧执行。
    /// 结果按 character_id 缓存(2026-09-14);返回值仍为 owned Vec 以保持既有签名,
    /// 但克隆远比重查库 + 重解析 JSON 便宜(前者是 memcpy,后者是 serde 解析 1MB)。
    ///
    /// 填充竞态防线(2026-10-15 并发修复批):未命中到插入之间不持写锁,若期间发生写入,
    /// 旧快照会被塞进刚被清空的缓存,之后直到下一次写都读到旧世界书。现在以代际计数兜底:
    /// 构建前后代际不一致就拒绝落缓存,并重试一轮拿新数据。
    pub fn collect_entries_for_character(&self, character_id: &str) -> Vec<WorldEntry> {
        if let Some(hit) = self
            .entries_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(character_id)
            .cloned()
        {
            return hit.as_ref().clone();
        }
        // 至多两轮:第二轮仅在「首轮构建期间发生过失效」时执行(重试一次)。
        // 两轮都撞上写入则返回最后构建值但不落缓存——宁可下次重查,绝不缓存可能陈旧的快照。
        let mut last: Arc<Vec<WorldEntry>> = Arc::new(Vec::new());
        for _ in 0..2 {
            let gen_before = self.entries_gen.load(Ordering::SeqCst);
            let built = Arc::new(self.build_entries(character_id));
            if self.try_cache_entries(character_id, gen_before, &built) {
                return built.as_ref().clone();
            }
            last = built;
        }
        last.as_ref().clone()
    }

    /// 查库构建条目(不走缓存):collect 的填充路径与竞态交错测试共用
    fn build_entries(&self, character_id: &str) -> Vec<WorldEntry> {
        let mut out: Vec<WorldEntry> = Vec::new();
        for book in self.enabled_for_character(character_id) {
            if let Some(raw) = &book.data_raw {
                out.extend(collect_entries(raw));
            }
        }
        out
    }

    /// 落缓存:仅当传入代际仍是当前代际时插入。`false` = 期间发生过失效,
    /// 调用方应视本次构建为「可能陈旧」并重试,不得再插入同一份数据。
    fn try_cache_entries(
        &self,
        character_id: &str,
        gen_before: u64,
        built: &Arc<Vec<WorldEntry>>,
    ) -> bool {
        let mut cache = self.entries_cache.lock().unwrap_or_else(|e| e.into_inner());
        if self.entries_gen.load(Ordering::SeqCst) != gen_before {
            return false;
        }
        if cache.len() >= ENTRIES_CACHE_CAP {
            cache.clear();
        }
        cache.insert(character_id.to_string(), built.clone());
        true
    }

    /// 自动分配属性机制自检:验证「角色为 None(自动)→ 常驻注入 system、激发注入 user」
    /// 的整条链路是否可用。检查项:
    ///   1. parse —— 世界书条目解析链路(collect_entries → WorldEntry)
    ///   2. auto_role —— 自动角色分配(collect_world_text_grouped 实测 role=None 分组)
    ///   3. convert —— 转换链路(normalize_entry_value 对变体样例的规范化)
    ///   4. ui_auto —— 前端「自动」选项存在性(静态声明)
    ///
    /// character_id 为 None 时仅做引擎/转换链路检查(无需数据库条目)。
    pub fn auto_assign_check(&self, character_id: Option<&str>) -> Value {
        let mut checks: Vec<Value> = Vec::new();

        // 1) 解析链路:取首本有效世界书的条目,验证 constant/keys 能正确读出
        let parse_detail = {
            let book = character_id
                .and_then(|cid| self.enabled_for_character(cid).into_iter().next())
                // list 不含 data_raw,需按 id 再取详情
                .or_else(|| self.list().into_iter().next().and_then(|b| self.get(&b.id)));
            match book {
                Some(b) => {
                    let entries = b.data_raw.as_ref().map(collect_entries).unwrap_or_default();
                    if entries.is_empty() {
                        format!("未在「{}」中找到有效条目", b.name)
                    } else {
                        let consts = entries.iter().filter(|e| e.constant).count();
                        let keyed = entries.iter().filter(|e| !e.keys.is_empty()).count();
                        format!(
                            "「{}」{} 条条目解析正常(常驻 {} 条、带关键词 {} 条)",
                            b.name,
                            entries.len(),
                            consts,
                            keyed
                        )
                    }
                }
                None => "无可用世界书,跳过条目级检查(引擎链路不受影响)".to_string(),
            }
        };
        checks.push(json!({
            "name": "parse",
            "label": "世界书条目解析链路",
            "status": "ok",
            "detail": parse_detail,
        }));

        // 2) 自动角色分配:构造 role=None 的常驻/激发条目,实测分组后的注入角色
        let mut vars = crate::parsing::assistant::AssistantVars::default();
        let history = vec![MessageRecord {
            id: 1,
            session_id: "auto-assign-check".into(),
            role: "user".into(),
            content: "测试消息含触发词".into(),
            extra: Value::Null,
            created_at: "now".into(),
        }];
        let sample = vec![
            WorldEntry {
                id: 0,
                comment: "常驻".into(),
                keys: vec![],
                keys_secondary: vec![],
                regex: None,
                use_regex: false,
                content: "常驻内容".into(),
                constant: true,
                enabled: true,
                position: 3,
                depth: 4,
                scan_depth: None,
                order: 100,
                case_sensitive: false,
                sticky: 0,
                cooldown: 0,
                probability: 100,
                use_probability: false,
                role: None,
                decorators: Vec::new(),
            },
            WorldEntry {
                id: 1,
                comment: "激发".into(),
                keys: vec!["触发词".into()],
                keys_secondary: vec![],
                regex: None,
                use_regex: false,
                content: "激发内容".into(),
                constant: false,
                enabled: true,
                position: 1,
                depth: 4,
                scan_depth: None,
                order: 100,
                case_sensitive: false,
                sticky: 0,
                cooldown: 0,
                probability: 100,
                use_probability: false,
                role: None,
                decorators: Vec::new(),
            },
        ];
        let grouped = crate::agents::engine::worldbook::collect_world_text_grouped(
            &sample, &history, &mut vars,
        );
        let constant_role = grouped.constant.first().map(|i| i.role.as_str());
        let triggered_role = grouped.triggered.first().map(|i| i.role.as_str());
        let (auto_ok, auto_detail) =
            if constant_role == Some("system") && triggered_role == Some("user") {
                (
                    true,
                    format!(
                        "常驻→system、激发→user(共 {} 条常态、{} 条激发)",
                        grouped.constant.len(),
                        grouped.triggered.len()
                    ),
                )
            } else {
                (
                    false,
                    format!("异常:常驻={constant_role:?},激发={triggered_role:?}"),
                )
            };
        checks.push(json!({
            "name": "auto_role",
            "label": "属性自动分配(role=自动)",
            "status": if auto_ok { "ok" } else { "error" },
            "detail": auto_detail,
        }));

        // 3) 转换链路:变体样例(逗号字符串 keys、字符串 constant、keywords 变体)规范化
        let sample = json!({
            "entries": [
                { "uid": 0, "keys": "车站, 铁路", "content": "A", "constant": "true" },
                { "uid": 1, "content": "世界观" },
                { "uid": 2, "keywords": ["码头"], "content": "B" }
            ]
        });
        let mut converted = sample.clone();
        let report = convert_world_book(&mut converted);
        let entry0 = normalize_entry_value(
            &json!({ "keys": "车站, 铁路", "constant": "true", "content": "A" }),
        );
        let entry1 = normalize_entry_value(&json!({ "content": "世界观" }));
        let convert_ok = report.total_count == 3
            && report.key_normalized_count == 2
            && entry0["keys"] == json!(["车站", "铁路"])
            && entry0["constant"] == json!(true)
            && entry1["constant"] == json!(true);
        checks.push(json!({
            "name": "convert",
            "label": "世界书转换机制",
            "status": if convert_ok { "ok" } else { "error" },
            "detail": if convert_ok {
                format!("转换正常(共 {} 条,关键词归一 {} 条,常态自动判定 {} 条)", report.total_count, report.key_normalized_count, report.constant_auto_count)
            } else {
                format!("转换异常:{report:?}")
            },
        }));

        // 4) 前端「自动」选项(静态声明:WorldBooksModal.vue 下拉含 role=null「自动」)
        checks.push(json!({
            "name": "ui_auto",
            "label": "前端「自动」选项",
            "status": "ok",
            "detail": "世界书编辑界面注入角色下拉已提供「自动」选项(role=null,按常态/激发自动分配)",
        }));

        let all_ok = checks.iter().all(|c| c["status"] == "ok");
        json!({
            "ok": all_ok,
            "checks": checks,
            "summary": if all_ok {
                "自动分配属性机制可用:条目解析、角色自动分配、转换机制均正常".to_string()
            } else {
                "自动分配属性机制存在异常,请查看各检查项 detail".to_string()
            },
        })
    }

    /// 条目预览(从 data_raw 提取,返回编辑视图,按 position,order,id 稳定排序)
    pub fn entries(&self, id: &str) -> Option<Vec<WorldBookEntryView>> {
        let rec = self.get(id)?;
        let raw = rec.data_raw?;
        let views = collect_entries(&raw)
            .into_iter()
            // 与角色卡侧 `character_book_views` 共用同一转换(此前此处是 entry_to_view 的
            // 内联重复实现,视图新增字段时两处不同步会导致独立书 GET 丢字段而角色卡 GET 有)
            .map(|e| crate::parsing::world_book::entry_to_view(&e))
            .collect();
        Some(views)
    }

    /// 新增条目:生成一个新 uid(当前最大 +1),写回 data_raw.entries,返回新条目视图。
    /// uid 计算与落库在 [`Self::update_raw`] 的写锁事务内完成(2026-10-15 并发修复批)——
    /// 原实现 get → 算 max → save 三步分离,并发新建会撞 uid 并静默覆盖。
    pub fn add_entry(&self, id: &str) -> Option<WorldBookEntryView> {
        let mut view_out: Option<WorldBookEntryView> = None;
        let done = self.update_raw(id, |raw| {
            if raw.get("entries").is_none() {
                raw.as_object_mut()
                    .ok_or_else(|| "世界书结构异常".to_string())?
                    .insert("entries".into(), Value::Array(Vec::new()));
            }
            let max_uid = collect_entries(raw)
                .into_iter()
                .map(|e| e.id)
                .max()
                .unwrap_or(0);
            let new_id = max_uid + 1;
            let view = WorldBookEntryView {
                id: new_id,
                comment: "新条目".into(),
                keys: Vec::new(),
                keys_secondary: Vec::new(),
                regex: None,
                use_regex: false,
                constant: false,
                enabled: true,
                content: String::new(),
                position: 0,
                depth: 4,
                // 新条目不预置扫描窗口:None = 未声明,窗口由上面的 depth 兜底(与改前行为一致)
                scan_depth: None,
                order: 100,
                case_sensitive: false,
                sticky: 0,
                cooldown: 0,
                probability: 100,
                use_probability: false,
                role: None,
            };
            if let Some(entries_value) = raw.get_mut("entries") {
                let new_value = crate::parsing::world_book::view_to_value(&view);
                if let Some(map) = entries_value.as_object_mut() {
                    map.insert(new_id.to_string(), new_value);
                } else if let Some(arr) = entries_value.as_array_mut() {
                    // 数组形态:只追加新条目(PUT 自 WBCONC 提交 2 起也是叠加+显式删除,
                    // 不再整组替换;POST 的语义仍然只是新增一条)
                    arr.push(new_value);
                }
            }
            view_out = Some(view);
            Ok(())
        });
        match done {
            Ok(true) => view_out,
            _ => None,
        }
    }

    /// 回写 data_raw(调用方已持有完整 raw 的底层写;读改写路径请走 [`Self::update_raw`]，
    /// 它把「读 → 改 → 写」收进同一把写锁)。
    /// 同时按新 raw 重算 `entry_count`(2026-09-13 批次 3 修复:该列此前只在插入时写一次,
    /// 编辑条目不回写 → 列表页条目数一直显示旧值)。
    pub fn save_data_raw(&self, id: &str, raw: &Value) -> Option<()> {
        let data_raw_str = serde_json::to_string(raw).ok()?;
        let entry_count = collect_entries(raw).len() as i64;
        let conn = self.db.write();
        // 影响 0 行 = 目标书已不存在(如并发删除),不得报成功(2026-10-15 并发修复批;
        // 对照 update() 的 n==0 → None 口径)。SQLite 对同值 UPDATE 仍计 1 行,判定安全。
        let n = conn
            .execute(
                "UPDATE world_books SET data_raw = ?1, entry_count = ?2 WHERE id = ?3",
                params![data_raw_str, entry_count, id],
            )
            .ok()?;
        drop(conn);
        if n == 0 {
            return None;
        }
        // 条目内容变更必须失效:这是 collect_entries_for_character 的直接输入源。
        // add_entry 亦经本方法落库,故在此一处失效即可覆盖两者。
        self.invalidate_entries_cache();
        Some(())
    }

    /// 独立世界书 data_raw 的读改写**单一入口**(写锁事务):锁内 SELECT → 闭包 → UPDATE +
    /// 重算 entry_count,全程持唯一写连接。此前各写路径是「get(只读池) → 内存改 →
    /// save_data_raw」三步分离:两个写者从同一旧快照出发时,后写者整份覆盖先写者(丢更新),
    /// 并发新建条目还会算出同一 uid 互相覆盖。角色卡内嵌书早已套用同形态的
    /// `character_data::update_data_raw`,本函数把独立书口径补齐(2026-10-15 并发修复批)。
    ///
    /// - `Ok(true)`:  找到世界书并已提交(条目缓存/契约缓存已按写路径口径失效);
    /// - `Ok(false)`: 世界书不存在(无写入);
    /// - `Err`:       读取/解析/闭包拒绝/序列化/SQL/提交任一失败——事务回滚,原值不变。
    fn update_raw<F>(&self, id: &str, mutate: F) -> Result<bool, String>
    where
        F: FnOnce(&mut Value) -> Result<(), String>,
    {
        self.update_raw_with(id, |value| {
            mutate(value)?;
            Ok(())
        })
        .map(|out| out.is_some())
    }

    /// [`Self::update_raw`] 的带返回值版本:闭包可把事务内计算结果(如合并后视图)
    /// 带出到调用方。值在提交成功后才返回,不存在「未落盘却回显」的窗口。
    fn update_raw_with<T, F>(&self, id: &str, mutate: F) -> Result<Option<T>, String>
    where
        F: FnOnce(&mut Value) -> Result<T, String>,
    {
        let mut conn = self.db.write();
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启世界书写事务失败: {e}"))?;
        let raw: Option<Option<String>> = tx
            .query_row(
                "SELECT data_raw FROM world_books WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("读取世界书失败: {e}"))?;
        let Some(raw) = raw else {
            // 行不存在:事务未写入任何内容,Drop 即回滚
            return Ok(None);
        };
        let raw = raw.ok_or_else(|| "世界书缺少原始数据".to_string())?;
        let mut value: Value =
            serde_json::from_str(&raw).map_err(|e| format!("解析世界书失败: {e}"))?;
        // 闭包拒绝(条目缺失/结构异常等)时事务回滚,不留下半写状态
        let out = mutate(&mut value)?;
        let serialized =
            serde_json::to_string(&value).map_err(|e| format!("序列化世界书失败: {e}"))?;
        let entry_count = collect_entries(&value).len() as i64;
        tx.execute(
            "UPDATE world_books SET data_raw = ?1, entry_count = ?2 WHERE id = ?3",
            params![serialized, entry_count, id],
        )
        .map_err(|e| format!("回写世界书失败: {e}"))?;
        tx.commit()
            .map_err(|e| format!("提交世界书事务失败: {e}"))?;
        drop(conn);
        // 提交成功才失效;先落写锁再失效(回调只动契约注册表,锁序与 save_data_raw 一致)
        self.invalidate_entries_cache();
        Ok(Some(out))
    }

    /// PUT /entries 的读改写一体化入口(写锁事务,2026-10-15 并发修复批)。
    /// 保存语义 = **叠加视图 + 显式 removed 删除**:未在视图、也未在 removed 的条目保留
    /// (窗口期内模型/卡脚本的写入不被旧快照删除);`added` 中的新行在锁内重分配 uid
    /// (不能靠「uid 已存在」推断——本地新增行与窗口期模型新建可能同号)。
    /// 返回值 = 提交后重新解析的合并视图(前端用它刷新草稿与基线)。
    /// `Ok(None)` 世界书不存在;`Err` 结构/IO 失败(调用方映射文案)。
    pub fn save_entries_merged(
        &self,
        id: &str,
        entries: &[WorldBookEntryView],
        removed: &[i64],
        added: &[i64],
    ) -> Result<Option<Vec<WorldBookEntryView>>, String> {
        let mut views = entries.to_vec();
        self.update_raw_with(id, |raw| {
            if raw.get("entries").is_none() {
                // 结构兜底与原 API 层实现一致:非对象 raw 静默跳过 merge(保存原样)
                raw.as_object_mut()
                    .map(|o| o.insert("entries".into(), Value::Array(Vec::new())));
            }
            let Some(entries_value) = raw.get_mut("entries") else {
                return Ok(Vec::new());
            };
            crate::parsing::world_book::assign_added_uids(entries_value, &mut views, added);
            crate::parsing::world_book::merge_entries_into(entries_value, &views, removed);
            Ok(collect_entries(entries_value)
                .iter()
                .map(crate::parsing::world_book::entry_to_view)
                .collect())
        })
    }

    /// 当前角色内嵌角色卡(character_book)的条目视图;无内嵌世界书返回 None
    pub fn character_book_views(&self, character_id: &str) -> Option<Vec<WorldBookEntryView>> {
        let raw = self
            .db
            .read()
            .ok()?
            .query_row(
                "SELECT data_raw FROM characters WHERE id = ?1",
                params![character_id],
                |row| row.get::<_, String>(0),
            )
            .ok()?;
        let raw: Value = serde_json::from_str(&raw).ok()?;
        let entries = crate::parsing::world_book::character_book_entries(&raw);
        if entries.is_empty() {
            return None;
        }
        Some(
            entries
                .iter()
                .map(crate::parsing::world_book::entry_to_view)
                .collect(),
        )
    }

    /// 回写角色卡 data_raw 中的 character_book(合并 entries 后 UPDATE characters)。
    /// 读改写经 `character_data::update_data_raw_with` 在**同一写锁事务**内完成:
    /// 原实现「read 取快照 → 内存改 → write 回写」两步间不持锁,并发编辑同一张卡时
    /// 后写者会用旧快照覆盖先写者的修改(丢更新)。保存语义与独立书一致:
    /// 叠加 + 显式 removed + added 重分配,返回合并后视图(失败/无内嵌书 → None)。
    pub fn save_character_book(
        &self,
        character_id: &str,
        views: &[WorldBookEntryView],
        removed: &[i64],
        added: &[i64],
    ) -> Option<Vec<WorldBookEntryView>> {
        let mut views = views.to_vec();
        crate::services::character_data::update_data_raw_with(&self.db, character_id, |raw| {
            // 定位 character_book:V2 顶层 / V3 data 子对象
            let cb = if raw.get("character_book").is_some() {
                raw.get_mut("character_book")
            } else {
                raw.get_mut("data")
                    .and_then(|d| d.get_mut("character_book"))
            };
            let cb = cb.ok_or_else(|| "角色卡缺少 character_book".to_string())?;
            let entries_value = cb
                .as_object_mut()
                .and_then(|o| o.get_mut("entries"))
                .ok_or_else(|| "character_book.entries 结构异常".to_string())?;
            crate::parsing::world_book::assign_added_uids(entries_value, &mut views, added);
            crate::parsing::world_book::merge_entries_into(entries_value, &views, removed);
            Ok(collect_entries(entries_value)
                .iter()
                .map(crate::parsing::world_book::entry_to_view)
                .collect())
        })
        .ok()
        .flatten()
    }
}

// ==================== 条目级写回(RPFLOW-2 `worldbook_update` 归档步) ====================
//
// 为什么不用 PUT /entries 那条回写通道:归档工具只需动**一条**条目,故这里按最小
// 改动面实现:定位一条 → 只改它的 content 键;其余字段(keys/position/enabled/order/
// extensions、content 里的 `@@` 装饰器行)与全部兄弟条目原样保留。
// PUT /entries 自 WBCONC 提交 2 起也已是「叠加 + 显式删除」,但本函数仍更小面:
// 不重写任何未命中条目,也不触发 uid 重分配。

/// 原始条目 uid(兼容 uid / id;直接读 JSON 值,不做条目归一化)
fn raw_entry_uid(v: &Value) -> Option<i64> {
    v.get("uid")
        .or_else(|| v.get("id"))
        .and_then(|x| x.as_i64())
}

/// 按 uid 定位条目容器内的条目(对象/数组两种形态;匹配口径与 `normalize_entry` 一致)
fn find_entry_mut(entries_value: &mut Value, uid: i64) -> Option<&mut Value> {
    match entries_value {
        Value::Object(map) => map.values_mut().find(|v| raw_entry_uid(v) == Some(uid)),
        Value::Array(arr) => arr.iter_mut().find(|v| raw_entry_uid(v) == Some(uid)),
        _ => None,
    }
}

/// 按 `compose` 改写条目对象的 content 键(原值为非字符串/缺失时按空串处理)
fn write_entry_content<F>(entry: &mut Value, compose: F) -> Option<()>
where
    F: FnOnce(&str) -> String,
{
    let map = entry.as_object_mut()?;
    let old = map.get("content").and_then(|c| c.as_str()).unwrap_or("");
    map.insert("content".into(), Value::String(compose(old)));
    Some(())
}

/// 新建绿灯(触发)条目的编辑视图:constant=false / enabled=true,其余取项目缺省
/// (position 0 / depth 4 / order 100,与 `add_entry` 的新建缺省同口径)。
fn trigger_entry_view(
    id: i64,
    comment: &str,
    keys: &[String],
    content: &str,
) -> WorldBookEntryView {
    WorldBookEntryView {
        id,
        comment: comment.to_string(),
        keys: keys.to_vec(),
        keys_secondary: Vec::new(),
        regex: None,
        use_regex: false,
        constant: false,
        enabled: true,
        content: content.to_string(),
        position: 0,
        depth: 4,
        scan_depth: None,
        order: 100,
        case_sensitive: false,
        sticky: 0,
        cooldown: 0,
        probability: 100,
        use_probability: false,
        role: None,
    }
}

/// 定位角色卡 data_raw 中的内嵌世界书(V2 顶层 character_book / V3 data.character_book);
/// 判据与 `save_character_book` 一致。
fn character_book_mut(raw: &mut Value) -> Option<&mut Value> {
    if raw.get("character_book").is_some() {
        raw.get_mut("character_book")
    } else {
        raw.get_mut("data")
            .and_then(|d| d.get_mut("character_book"))
    }
}

/// 内嵌世界书的条目容器(character_book.entries)。
/// `create_entries` = true 时 entries 键缺失会补一个空对象(新建条目用);false 时缺失返回 None。
fn character_book_entries_mut(raw: &mut Value, create_entries: bool) -> Option<&mut Value> {
    let cb = character_book_mut(raw)?.as_object_mut()?;
    if cb.get("entries").is_none() {
        if !create_entries {
            return None;
        }
        cb.insert("entries".into(), Value::Object(serde_json::Map::new()));
    }
    cb.get_mut("entries")
}

impl WorldBookService {
    /// 更新指定世界书内一条条目的内容(RPFLOW-2 归档步·绿灯路径):按 uid 定位 →
    /// 由 `compose` 以**原始 content** 生成新内容 → 只改 `content` 键写回 `data_raw`。
    ///
    /// 为什么基于原始 content 而不是归一化视图内容:`@@` 装饰器行在归一化时被剥离
    /// (解析层存进 decorators),基于视图追加会把装饰器行静默抹掉,改变条目的条件注入
    /// 与变量写入行为。条目不存在 / 世界书不存在或缺少 data_raw → None(无写入)。
    /// 读改写经 [`Self::update_raw`] 写锁事务(2026-10-15 并发修复批)。
    pub fn update_entry_content<F>(&self, id: &str, uid: i64, compose: F) -> Option<()>
    where
        F: FnOnce(&str) -> String,
    {
        let done = self.update_raw(id, |raw| {
            let target = find_entry_mut(
                raw.get_mut("entries")
                    .ok_or_else(|| "世界书缺少 entries".to_string())?,
                uid,
            )
            .ok_or_else(|| format!("世界书条目 uid={uid} 不存在"))?;
            write_entry_content(target, compose).ok_or_else(|| "条目结构异常".to_string())
        });
        // Ok(false)=世界书不存在;Err=闭包拒绝或事务失败。调用方(工具层)只看到 None,
        // 具体原因留 warn 便于排查(与内嵌书版 `update_character_book_entry_content` 同口径)。
        match done {
            Ok(true) => Some(()),
            Ok(false) => None,
            Err(e) => {
                tracing::warn!(id, uid, error = %e, "世界书条目更新未生效");
                None
            }
        }
    }

    /// 在指定世界书内**追加**一条绿灯(触发)条目,返回新条目 uid。
    /// 新 uid = 现有原始条目最大 uid + 1(空书为 1,同 `add_entry`);
    /// entries 为对象 → 插数字键;为数组 → 追加;缺失 → 建空数组后追加。
    /// uid 计算与落库在 [`Self::update_raw`] 写锁事务内完成(2026-10-15 并发修复批)——
    /// 原实现 get → 算 max → save 三步分离,并发新建会撞 uid 并静默覆盖。
    pub fn create_trigger_entry(
        &self,
        id: &str,
        comment: &str,
        keys: &[String],
        content: &str,
    ) -> Option<i64> {
        let mut new_id_out: Option<i64> = None;
        let done = self.update_raw(id, |raw| {
            if raw.get("entries").is_none() {
                raw.as_object_mut()
                    .ok_or_else(|| "世界书结构异常".to_string())?
                    .insert("entries".into(), Value::Array(Vec::new()));
            }
            let entries = raw
                .get_mut("entries")
                .ok_or_else(|| "世界书结构异常".to_string())?;
            let new_id = crate::parsing::world_book::max_entry_uid(entries).map_or(1, |m| m + 1);
            let value = crate::parsing::world_book::view_to_value(&trigger_entry_view(
                new_id, comment, keys, content,
            ));
            match entries {
                Value::Object(map) => {
                    map.insert(new_id.to_string(), value);
                }
                Value::Array(arr) => arr.push(value),
                _ => return Err("entries 结构异常".to_string()),
            }
            new_id_out = Some(new_id);
            Ok(())
        });
        match done {
            Ok(true) => new_id_out,
            _ => None,
        }
    }

    /// 更新角色卡内嵌世界书里一条条目的内容(语义同 [`Self::update_entry_content`]),
    /// 写回 `characters.data_raw`——经 `character_data::update_data_raw` 的**写锁事务**
    /// 读改写(与 `save_character_book` 同一底层通道;本条只改 content 键,不整组重写
    /// entries,未归一化兄弟条目与 `@@` 装饰器行均不受影响)。
    /// 角色不存在 / 卡无 character_book / 找不到 uid → None(无写入)。
    pub fn update_character_book_entry_content<F>(
        &self,
        character_id: &str,
        uid: i64,
        compose: F,
    ) -> Option<()>
    where
        F: FnOnce(&str) -> String,
    {
        let done =
            crate::services::character_data::update_data_raw(&self.db, character_id, |raw| {
                let target = character_book_entries_mut(raw, false)
                    .and_then(|entries| find_entry_mut(entries, uid))
                    .ok_or_else(|| format!("角色卡内嵌世界书缺少条目 uid={uid}"))?;
                write_entry_content(target, compose).ok_or_else(|| "内嵌条目结构异常".to_string())
            });
        // Ok(false)=角色不存在;Err=闭包拒绝或 SQL 失败。调用方只看到 None 并回
        // 「条目可能刚被改动,请重试」——具体原因(条目缺失 / 结构异常)留 warn 便于排查。
        match done {
            Ok(true) => Some(()),
            Ok(false) => None,
            Err(e) => {
                tracing::warn!(character_id, uid, error = %e, "角色卡内嵌世界书条目更新未生效");
                None
            }
        }
    }

    /// 在角色卡内嵌世界书内**追加**一条绿灯条目,返回新条目 uid。
    /// 新 uid = 卡内内嵌原始条目最大 uid + 1;写回经写锁事务。
    /// 卡无 character_book 时 None(不新建卡内结构)。
    pub fn create_character_book_entry(
        &self,
        character_id: &str,
        comment: &str,
        keys: &[String],
        content: &str,
    ) -> Option<i64> {
        let mut new_id = 0i64;
        let done =
            crate::services::character_data::update_data_raw(&self.db, character_id, |raw| {
                let entries = character_book_entries_mut(raw, true)
                    .ok_or_else(|| "角色卡缺少 character_book".to_string())?;
                let uid = crate::parsing::world_book::max_entry_uid(entries).map_or(1, |m| m + 1);
                let value = crate::parsing::world_book::view_to_value(&trigger_entry_view(
                    uid, comment, keys, content,
                ));
                match entries {
                    Value::Object(map) => {
                        map.insert(uid.to_string(), value);
                    }
                    Value::Array(arr) => arr.push(value),
                    _ => return Err("character_book.entries 结构异常".to_string()),
                }
                new_id = uid;
                Ok(())
            });
        match done {
            Ok(true) => Some(new_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::db::Db;
    use crate::utils::test_support::TempDataDir;

    /// 返回 (守卫, 服务):解构绑定按**逆序**析构,守卫在前才活到最后(见 test_support 模块头)
    fn service() -> (TempDataDir, WorldBookService) {
        let dir = TempDataDir::new("wb");
        let db = Arc::new(Db::open(&dir.join("kedai.db"), &dir).unwrap());
        (dir, WorldBookService::new(db))
    }

    fn upload_book(svc: &WorldBookService, name: &str, key: &str, cid: Option<&str>) -> String {
        let raw = json!({
            "name": name,
            "entries": [
                { "uid": 0, "comment": "地点", "keys": [key], "content": "内容", "constant": false }
            ]
        });
        let bytes = serde_json::to_vec(&raw).unwrap();
        svc.upload(&bytes, "book.json", cid).unwrap().id
    }

    /// P3-2(2026-09-14):条目缓存的**失效正确性**。
    /// 这是本优化唯一的风险点——漏失效会让生成读到旧世界书。
    /// 断言:① 重复读取结果一致(缓存生效);② 上传新书后立即可见;
    /// ③ 改 enabled 后立即可见;④ 改条目内容后立即可见;⑤ 删除后立即可见。
    #[test]
    fn entries_cache_invalidates_on_every_write_path() {
        let (_dir, svc) = service();
        // 全局世界书(character_id = None → 对所有角色生效,便于用同一角色观察)
        let first_id = upload_book(&svc, "书一", "关键词一", None);

        let before = svc.collect_entries_for_character("c1");
        assert_eq!(before.len(), 1, "初始应有一本有效世界书");
        assert_eq!(
            svc.collect_entries_for_character("c1").len(),
            1,
            "重复读取一致"
        );

        // ② 上传第二本 → 必须立即可见
        let _second_id = upload_book(&svc, "书二", "关键词二", None);
        let after_upload = svc.collect_entries_for_character("c1");
        assert_eq!(
            after_upload.len(),
            2,
            "上传新世界书后缓存必须失效,条目应变为 2"
        );
        assert!(
            after_upload
                .iter()
                .any(|e| e.keys.iter().any(|k| k == "关键词二")),
            "新书的条目必须出现在结果里"
        );

        // ③ 停用第一本 → 立即只剩一条
        svc.update(&first_id, Some(false), None, None).unwrap();
        assert_eq!(
            svc.collect_entries_for_character("c1").len(),
            1,
            "改 enabled 后缓存必须失效"
        );
        // 恢复启用
        svc.update(&first_id, Some(true), None, None).unwrap();
        assert_eq!(svc.collect_entries_for_character("c1").len(), 2);

        // ④ 改条目内容(经 save_data_raw,add_entry 亦走此路径)
        let raw = json!({
            "name": "书一",
            "entries": [
                { "uid": 0, "comment": "地点", "keys": ["改过的关键词"], "content": "新内容", "constant": false }
            ]
        });
        svc.save_data_raw(&first_id, &raw).unwrap();
        let after_edit = svc.collect_entries_for_character("c1");
        assert!(
            after_edit
                .iter()
                .any(|e| e.keys.iter().any(|k| k == "改过的关键词")),
            "改条目后缓存必须失效,应读到新关键词"
        );
        assert!(
            !after_edit
                .iter()
                .any(|e| e.keys.iter().any(|k| k == "关键词一")),
            "旧关键词不应残留"
        );

        // ⑤ 删除第一本 → 立即只剩一条
        assert!(svc.delete(&first_id), "删除应成功");
        assert_eq!(
            svc.collect_entries_for_character("c1").len(),
            1,
            "删除后缓存必须失效"
        );

        drop(svc);
    }

    /// 缓存按 character_id 隔离:不同角色互不污染
    #[test]
    fn entries_cache_is_per_character() {
        let (_dir, svc) = service();
        {
            let conn = svc.db.write();
            conn.execute(
                "INSERT INTO characters (id, name, chara_name, description, file_path, data_raw, created_at)
                 VALUES ('c1','n','n','','','{}',''), ('c2','n','n','','','{}','')",
                [],
            )
            .unwrap();
        }
        upload_book(&svc, "给 c1", "只属于c1", Some("c1"));

        let c1 = svc.collect_entries_for_character("c1");
        let c2 = svc.collect_entries_for_character("c2");
        assert_eq!(c1.len(), 1, "c1 应有自己的世界书");
        assert_eq!(c2.len(), 0, "c2 不应看到 c1 的世界书");

        drop(svc);
    }

    /// 写钩子(2026-10-14 修复批):契约缓存失效必须跟随**所有**世界书写路径。
    ///
    /// 背景:契约来源之一是独立世界书的 [nlkaleido_contract] 条目,但失效此前只在
    /// API 端点显式调用——worldbook_update 归档工具与脚本 importRawWorldbook 都
    /// 绕过 API 直写本服务,改到契约条目时契约缓存会继续供应旧契约直到重启。
    /// 断言:5 条写路径各触发一次;只读不触发;失败写不触发;内嵌卡书写入不触发
    /// (内嵌书不是契约来源:注册表只读独立书 collect_entries_for_character)。
    #[test]
    fn write_hook_fires_on_every_write_path_only() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (_dir, svc) = service();
        let count = Arc::new(AtomicUsize::new(0));
        {
            let c = count.clone();
            svc.set_write_hook(Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            }));
        }
        let hits = || count.load(Ordering::SeqCst);

        // 只读路径:一次都不触发
        let _ = svc.list();
        let _ = svc.collect_entries_for_character("c1");
        assert_eq!(hits(), 0, "只读路径不得触发写钩子");

        // ① upload
        let id = upload_book(&svc, "书一", "关键词一", None);
        assert_eq!(hits(), 1, "upload 应触发");

        // ② update;失败写(不存在的 id)不触发
        svc.update(&id, Some(false), None, None).unwrap();
        assert_eq!(hits(), 2, "update 应触发");
        assert!(svc.update("不存在的书", Some(true), None, None).is_none());
        assert_eq!(hits(), 2, "失败的 update 不得触发");

        // ③ save_data_raw(条目回写接口;update_entry_content/create_trigger_entry 均经此路)
        let raw = json!({ "name": "书一", "entries": [] });
        svc.save_data_raw(&id, &raw).unwrap();
        assert_eq!(hits(), 3, "save_data_raw 应触发");

        // ④ add_entry(经 save_data_raw 落库)
        svc.add_entry(&id).unwrap();
        assert_eq!(hits(), 4, "add_entry 应触发");

        // ⑤ delete;失败的删除不触发
        assert!(!svc.delete("不存在的书"));
        assert_eq!(hits(), 4, "失败的 delete 不得触发");
        assert!(svc.delete(&id));
        assert_eq!(hits(), 5, "delete 应触发");

        // 内嵌卡书(save_character_book)写的是 characters.data_raw,不是契约来源,
        // 也不被条目缓存覆盖——不触发
        {
            let conn = svc.db.write();
            conn.execute(
                "INSERT INTO characters (id, name, chara_name, description, file_path, data_raw, created_at) \
                 VALUES ('c1','n','n','','', ?1, '')",
                params![json!({ "character_book": { "entries": {} } }).to_string()],
            )
            .unwrap();
        }
        svc.save_character_book("c1", &[], &[], &[]).unwrap();
        assert_eq!(hits(), 5, "内嵌卡书写入不属于契约来源,不得触发写钩子");

        drop(svc);
    }

    /// ③ 失败测试(2026-10-15 并发修复批):并发新建条目不得撞 uid/丢条目。
    /// 旧实现 get(只读池) → 算 max_uid → save_data_raw(写锁)三步分离,两个写者
    /// 从同一 max 出发必然算出同一 uid,对象形态 map.insert 直接覆盖(静默丢更新)。
    #[test]
    fn concurrent_creates_do_not_collide_or_lose() {
        let (_dir, svc) = service();
        let svc = Arc::new(svc);
        let id = upload_book(&svc, "并发书", "关键词", None);
        let threads = 8usize;
        let per = 10usize;
        std::thread::scope(|s| {
            for t in 0..threads {
                let svc = svc.clone();
                let id = id.clone();
                s.spawn(move || {
                    for i in 0..per {
                        assert!(
                            svc.create_trigger_entry(&id, &format!("t{t}-{i}"), &["k".into()], "c")
                                .is_some(),
                            "create_trigger_entry 不得失败"
                        );
                    }
                });
            }
        });
        let entries = svc.entries(&id).expect("书应存在");
        // 基数 1(upload_book 的初始条目)+ threads*per,一条都不能少
        assert_eq!(
            entries.len(),
            1 + threads * per,
            "并发 create 不得丢条目(撞 uid 会静默覆盖)"
        );
        let mut uids: Vec<i64> = entries.iter().map(|e| e.id).collect();
        uids.sort_unstable();
        uids.dedup();
        assert_eq!(uids.len(), 1 + threads * per, "uid 必须唯一");
        drop(svc);
    }

    /// ⑤ 失败测试(2026-10-15 并发修复批):对不存在的书落盘必须报失败。
    /// 原实现忽略影响行数:get 与 update 之间书被删除时,0 行 UPDATE 仍返回 Some(())。
    #[test]
    fn save_data_raw_missing_book_returns_none() {
        let (_dir, svc) = service();
        let raw = json!({ "name": "不存在", "entries": [] });
        assert!(
            svc.save_data_raw("不存在的书", &raw).is_none(),
            "目标书不存在时不得把 0 行更新当成功"
        );
        drop(svc);
    }

    /// ④ 竞态交错测试(2026-10-15 并发修复批):缓存填充与写入交错时,
    /// 旧快照不得落缓存。手工构造时序——「构建(写前数据) → 写入(代际+1) → 落缓存」:
    /// 原实现无代际判定会把 1 条旧快照写进刚清空的缓存,此后读到旧世界书。
    #[test]
    fn stale_snapshot_is_not_cached_across_write_interleave() {
        use std::sync::atomic::Ordering;
        let (_dir, svc) = service();
        upload_book(&svc, "书一", "关键词一", None);

        // 1) T1 未命中缓存:读代际 + 构建(此刻内容 = 1 本)
        let gen_before = svc.entries_gen.load(Ordering::SeqCst);
        let stale = Arc::new(svc.build_entries("c1"));
        assert_eq!(stale.len(), 1);

        // 2) T2 完成一次写入:代际自增、缓存清空
        upload_book(&svc, "书二", "关键词二", None);

        // 3) T1 尝试落缓存:代际已变,必须拒绝
        assert!(
            !svc.try_cache_entries("c1", gen_before, &stale),
            "代际变化后不得落缓存"
        );

        // 4) 后续读取必须是新数据(缓存未被旧快照污染)
        assert_eq!(
            svc.collect_entries_for_character("c1").len(),
            2,
            "缓存不得残留旧快照"
        );

        // 正例:代际未变时正常落缓存
        let gen = svc.entries_gen.load(Ordering::SeqCst);
        let built = Arc::new(svc.build_entries("c1"));
        assert!(svc.try_cache_entries("c1", gen, &built), "代际未变应落缓存");
        drop(svc);
    }
}
