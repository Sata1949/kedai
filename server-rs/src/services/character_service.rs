// 角色卡服务(与 Node 版 character.service.ts 对齐):上传/CRUD/文件落盘
use super::{log_query_failure, log_read_pool_failure};
use crate::models::db::{now_iso, Db};
use crate::models::types::CharacterRecord;
use crate::parsing::character_card::{parse_character_card, safe_file_name};
use crate::parsing::character_derived;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

/// 内置「系统助手」角色 id(稳定,幂等;无提示词,作为默认助手使用)
pub const BUILTIN_SYSTEM_ID: &str = "builtin-system";

/// 角色行的 SELECT 列清单(两个调用点共用,**列序必须与 `row_to_character` 一致**:
/// 0 id / 1 name / 2 chara_name / 3 description / 4 file_path / 5 avatar_path /
/// 6 data_raw / 7 created_at / 8 derived_json)
const CHARACTER_COLUMNS: &str =
    "id, name, chara_name, description, file_path, avatar_path, data_raw, created_at, derived_json";

/// 从 data_raw 提取备用开场列表(alternate_greetings):取非空字符串,空数组返回 None
///
/// 实现已迁至 `parsing::character_derived`(写入侧与回退路径共用的单一计算源),
/// 此处保留薄包装以免既有调用点与文档引用失效。
fn extract_alternate_greetings(data_raw: &Value) -> Option<Vec<String>> {
    character_derived::extract_alternate_greetings(data_raw)
}

pub struct CharacterService {
    db: Arc<Db>,
    data_dir: PathBuf,
}

/// 从一行 character 构造记录。
///
/// 派生字段来源(2026-09-17 P-11):优先读 `derived_json` 列(**零 JSON 解析**),
/// 该列为空/非法时**回退解析 `data_raw`**——列是缓存而非事实源,任何不确定都退回权威数据。
///
/// 列序约定(两个调用点的 SELECT 必须与之一致):
/// 0 id / 1 name / 2 chara_name / 3 description / 4 file_path / 5 avatar_path /
/// 6 data_raw / 7 created_at / 8 derived_json
fn row_to_character(row: &rusqlite::Row, with_data_raw: bool) -> rusqlite::Result<CharacterRecord> {
    let raw_str: Option<String> = row.get(6)?;
    let derived_str: Option<String> = row.get(8).unwrap_or_default();
    // 详情路径需要 data_raw 本体;列表路径不解析(除非派生列不可用而必须回退)
    let mut data_raw_value: Option<Value> = None;
    let derived: Option<Value> = derived_str
        .as_deref()
        .and_then(crate::parsing::character_derived::parse_derived_json);

    let record_fields = match derived {
        Some(d) => {
            if with_data_raw {
                data_raw_value = raw_str
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok());
            }
            DerivedFields {
                first_mes: character_derived::derived_str(&d, "first_mes"),
                alternate_greetings: character_derived::derived_string_list(
                    &d,
                    "alternate_greetings",
                ),
                regex_scripts: d
                    .get("regex_scripts")
                    .and_then(|v| serde_json::from_value(v.clone()).ok()),
                creator: character_derived::derived_str(&d, "creator"),
                character_version: character_derived::derived_str(&d, "character_version"),
                creator_notes: character_derived::derived_str(&d, "creator_notes"),
            }
        }
        None => {
            // 回退:解析 data_raw 现算(旧库未回填 / 旁路写入未同步时走这里)
            data_raw_value = raw_str
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok());
            derive_from_data_raw(data_raw_value.as_ref())
        }
    };

    Ok(CharacterRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        chara_name: row.get(2)?,
        description: row.get(3)?,
        file_path: row.get(4)?,
        avatar_path: row.get(5)?,
        data_raw: if with_data_raw { data_raw_value } else { None },
        first_mes: record_fields.first_mes,
        alternate_greetings: record_fields.alternate_greetings,
        regex_scripts: record_fields.regex_scripts,
        card_plugins: None,
        creator: record_fields.creator,
        character_version: record_fields.character_version,
        creator_notes: record_fields.creator_notes,
        created_at: row.get(7)?,
    })
}

/// 派生字段集合(读取路径的内部中转,避免六个字段散着传)
struct DerivedFields {
    first_mes: Option<String>,
    alternate_greetings: Option<Vec<String>>,
    regex_scripts: Option<Vec<crate::parsing::regex_script::RegexScript>>,
    creator: Option<String>,
    character_version: Option<String>,
    creator_notes: Option<String>,
}

/// 回退路径:直接从 data_raw 现算派生字段(与 `compute_derived_json` 口径一致,
/// 后者是写入侧用的同一套规则;此处复用其输出再取字段,保证两条路径**不会漂移**)。
fn derive_from_data_raw(data_raw: Option<&Value>) -> DerivedFields {
    let Some(raw) = data_raw else {
        return DerivedFields {
            first_mes: None,
            alternate_greetings: None,
            regex_scripts: None,
            creator: None,
            character_version: None,
            creator_notes: None,
        };
    };
    let computed = character_derived::compute_derived_json(raw);
    let d: Value = serde_json::from_str(&computed).unwrap_or_else(|_| serde_json::json!({}));
    DerivedFields {
        first_mes: character_derived::derived_str(&d, "first_mes"),
        alternate_greetings: character_derived::derived_string_list(&d, "alternate_greetings"),
        regex_scripts: d
            .get("regex_scripts")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
        creator: character_derived::derived_str(&d, "creator"),
        character_version: character_derived::derived_str(&d, "character_version"),
        creator_notes: character_derived::derived_str(&d, "creator_notes"),
    }
}

impl CharacterService {
    pub fn new(db: Arc<Db>, data_dir: PathBuf) -> Self {
        std::fs::create_dir_all(data_dir.join("characters")).ok();
        std::fs::create_dir_all(data_dir.join("avatars")).ok();
        CharacterService { db, data_dir }
    }

    /// 列表不含 data_raw,按 created_at DESC。
    ///
    /// 2026-09-17 P-11:派生字段改读 `derived_json` 列(零 JSON 解析),故列表成本不再随
    /// 「卡数 × 卡体积」线性增长;列不可用时自动回退解析 data_raw(见 `row_to_character`)。
    /// `prepare_cached` 让语句在连接的语句缓存中复用(此前每次调用都重新 prepare)。
    pub fn list(&self) -> Vec<CharacterRecord> {
        let conn = match self.db.read() {
            Ok(c) => c,
            Err(e) => return log_read_pool_failure("角色列表", e),
        };
        let sql = format!("SELECT {CHARACTER_COLUMNS} FROM characters ORDER BY created_at DESC");
        let mut stmt = match conn.prepare_cached(&sql) {
            Ok(s) => s,
            Err(e) => return log_query_failure("角色列表 prepare", e),
        };
        let query = stmt.query_map([], |row| row_to_character(row, false));
        match query {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => log_query_failure("角色列表 query_map", e),
        }
    }

    pub fn get(&self, id: &str) -> Option<CharacterRecord> {
        let conn = self.db.read().ok()?;
        let sql = format!("SELECT {CHARACTER_COLUMNS} FROM characters WHERE id = ?1");
        conn.query_row(&sql, params![id], |row| row_to_character(row, true))
            .optional()
            .ok()
            .flatten()
    }

    /// 启动时注入默认「系统助手」角色:无提示词(description/first_mes 为空),作为通用助手。
    /// 幂等:内置 id 已存在则跳过;删除后下次启动会重新补回(属默认角色)。
    pub fn seed_default_character(&self) {
        if self.get(BUILTIN_SYSTEM_ID).is_some() {
            // 幂等重刷存量 V3 卡:把 data 子对象中的空顶层字段补到顶层
            self.reflatten_v3_cards();
            return;
        }
        let data_raw = serde_json::json!({
            "spec": "chara_card_v2",
            "spec_version": "1.0",
            "name": "系统助手",
            "description": "",
            "first_mes": "",
        });
        let created_at = now_iso();
        let file_path = self.data_dir.join("characters").join("builtin-system.json");
        let _ = std::fs::write(
            &file_path,
            serde_json::to_vec_pretty(&data_raw).unwrap_or_default(),
        );
        let data_raw_str = serde_json::to_string(&data_raw).unwrap_or_else(|_| "{}".into());
        let derived = character_derived::compute_derived_json(&data_raw);
        let _ = self.db.write().execute(
            "INSERT INTO characters (id, name, chara_name, description, file_path, avatar_path, data_raw, created_at, derived_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                BUILTIN_SYSTEM_ID,
                "system",
                "系统助手",
                "",
                file_path.to_string_lossy().to_string(),
                Option::<String>::None,
                data_raw_str,
                created_at,
                derived
            ],
        );
    }

    /// 幂等迁移存量 V3 角色卡:V3 卡正文位于 data 子对象,旧版本导入时顶层 V2 字段为空,
    /// 导致人设(description/personality/scenario 等)未注入提示词。此处把空顶层字段
    /// 从 data 子对象补全并同步 description/chara_name 列。二次运行:顶层已补全 → 无变更。
    ///
    /// 2026-09-17 P-11:改为**单次全表扫描**(原实现先 `SELECT id` 再逐个 id 查 `data_raw`,
    /// 是 1+N 次查询的 N+1);同时同步 `derived_json` 列,避免补全后列表仍显示补全前的值。
    pub fn reflatten_v3_cards(&self) {
        use crate::parsing::character_card::flatten_v3_data;
        let conn = self.db.write();
        // 先读完 (id, data_raw),再统一处理:避免在遍历游标上执行 UPDATE
        let mut rows_data: Vec<(String, String)> = Vec::new();
        {
            let Ok(mut stmt) = conn.prepare("SELECT id, data_raw FROM characters") else {
                return;
            };
            let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) else {
                return;
            };
            for r in rows.flatten() {
                rows_data.push(r);
            }
        }
        for (id, raw_str) in rows_data {
            let Some(raw) = serde_json::from_str::<Value>(&raw_str).ok() else {
                continue;
            };
            // 非 V3 或已补全 → 无需写库
            let is_v3 = matches!(
                raw.get("spec").and_then(|s| s.as_str()),
                Some("chara_card_v3")
            );
            if !is_v3 {
                continue;
            }
            let fixed = flatten_v3_data(raw.clone());
            if fixed == raw {
                continue;
            }
            let new_raw_str = serde_json::to_string(&fixed).unwrap_or_else(|_| "{}".into());
            // description 列:补全后顶层优先(与 upload 行为一致)
            let description = fixed
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let chara_name = fixed
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            // 派生列随 data_raw 同步重算(否则列表读到的是补全前的旧值)
            let derived = character_derived::compute_derived_json(&fixed);
            let _ = conn.execute(
                "UPDATE characters SET data_raw = ?1, description = ?2, chara_name = ?3, derived_json = ?4 WHERE id = ?5",
                params![new_raw_str, description, chara_name, derived, id],
            );
        }
    }

    pub fn upload(&self, buffer: &[u8], original_name: &str) -> Result<CharacterRecord, String> {
        let mut parsed = parse_character_card(buffer, original_name).map_err(|e| e.to_string())?;
        // 上传时自动规范化角色卡内嵌世界书(character_book.entries):
        // 修正关键词字段变体/逗号拆分、常态激发自动判定、属性默认自动,其余字段不动
        crate::parsing::world_book_convert::convert_world_book(&mut parsed.data);
        // 扩展名:优先原文件扩展名小写;无则按魔数推断
        let ext = detect_ext(original_name, buffer);
        let id = Uuid::new_v4().to_string();
        let safe = safe_file_name(original_name);
        let name = strip_ext(&safe);
        let file_name = format!("{id}{ext}");
        let chara_dir = self.data_dir.join("characters");
        let file_path = chara_dir.join(&file_name);
        let avatar_path = if let Some(avatar) = &parsed.avatar {
            let av_file = format!("{id}.png");
            let av_path = self.data_dir.join("avatars").join(&av_file);
            std::fs::write(&av_path, avatar).map_err(|e| format!("写头像失败: {e}"))?;
            Some(av_path.to_string_lossy().to_string())
        } else {
            None
        };
        std::fs::write(&file_path, buffer).map_err(|e| format!("写角色文件失败: {e}"))?;

        let chara_name = parsed
            .data
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| parsed.name.clone());
        let description = parsed
            .data
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let data_raw = serde_json::to_string(&parsed.data).unwrap_or_else(|_| "{}".into());
        let created_at = now_iso();

        let conn = self.db.write();
        let derived = character_derived::compute_derived_json(&parsed.data);
        conn.execute(
            "INSERT INTO characters (id, name, chara_name, description, file_path, avatar_path, data_raw, created_at, derived_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                name,
                chara_name,
                description,
                file_path.to_string_lossy(),
                avatar_path.as_deref(),
                data_raw,
                created_at,
                derived
            ],
        )
        .map_err(|e| format!("写数据库失败: {e}"))?;

        let regex_scripts = crate::parsing::regex_script::extract_regex_scripts(&parsed.data);
        let regex_scripts = if regex_scripts.is_empty() {
            None
        } else {
            Some(regex_scripts)
        };
        let first_mes = parsed
            .data
            .get("first_mes")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let alternate_greetings = extract_alternate_greetings(&parsed.data);
        // 卡元数据三件套(远程资源页口令推导用)
        let pick_str = |key: &str| {
            parsed
                .data
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
        };
        let creator = pick_str("creator");
        let character_version = pick_str("character_version");
        let creator_notes = pick_str("creator_notes");
        Ok(CharacterRecord {
            id,
            name,
            chara_name,
            description,
            file_path: file_path.to_string_lossy().to_string(),
            avatar_path,
            data_raw: Some(parsed.data),
            first_mes,
            alternate_greetings,
            regex_scripts,
            card_plugins: None,
            creator,
            character_version,
            creator_notes,
            created_at,
        })
    }

    /// 更新 chara_name / description / first_mes / alternate_greetings,同步写回 data_raw,保留其余。
    ///
    /// 读改写收口:`SELECT(列+data_raw) → 内存改 → UPDATE` 全部在**同一写锁事务**内完成。
    /// 原实现先经只读池 `self.get()` 取快照、再另取写锁回写,两步间不持锁——并发更新同一张卡时,
    /// 后写者会用旧 data_raw 快照覆盖先写者写入的其它字段(世界书/脚本/契约),即丢更新。
    pub fn update(
        &self,
        id: &str,
        chara_name: Option<&str>,
        description: Option<&str>,
        first_mes: Option<&str>,
        alternate_greetings: Option<&[String]>,
    ) -> Option<CharacterRecord> {
        // ===== 事务内:读旧值 → 合并 → 回写(含列更新) =====
        let (new_first_mes, new_alternate_greetings) = {
            let mut conn = self.db.write();
            let tx = conn.transaction().ok()?;
            let row: Option<(String, String, String)> = tx
                .query_row(
                    "SELECT chara_name, description, data_raw FROM characters WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .ok()?;
            let (old_name, old_desc, old_raw) = row?;
            let mut data_raw: Value = serde_json::from_str(&old_raw)
                .unwrap_or_else(|_| Value::Object(Default::default()));
            if let Some(obj) = data_raw.as_object_mut() {
                if let Some(n) = chara_name {
                    obj.insert("name".into(), Value::String(n.to_string()));
                }
                if let Some(d) = description {
                    obj.insert("description".into(), Value::String(d.to_string()));
                }
                if let Some(f) = first_mes {
                    // 空串表示清空开场白
                    obj.insert("first_mes".into(), Value::String(f.to_string()));
                }
                if let Some(list) = alternate_greetings {
                    // 空数组表示清空备用开场
                    let arr: Vec<Value> = list
                        .iter()
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| Value::String(s.to_string()))
                        .collect();
                    if arr.is_empty() {
                        obj.remove("alternate_greetings");
                    } else {
                        obj.insert("alternate_greetings".into(), Value::Array(arr));
                    }
                }
            }
            let new_name = chara_name.unwrap_or(&old_name).to_string();
            let new_desc = description.unwrap_or(&old_desc).to_string();
            let data_raw_str = serde_json::to_string(&data_raw).unwrap_or_else(|_| "{}".into());
            // 派生列随 data_raw 同步重算(2026-09-17 P-11)
            let derived = character_derived::compute_derived_json(&data_raw);
            let n = tx
                .execute(
                    "UPDATE characters SET chara_name = ?1, description = ?2, data_raw = ?3, derived_json = ?4 WHERE id = ?5",
                    params![new_name, new_desc, data_raw_str, derived, id],
                )
                .ok()?;
            if n == 0 {
                return None;
            }
            tx.commit().ok()?;
            // 返回记录用的开场白/备用开场:有更新用新值,否则留给下方 self.get 从新 data_raw 提取
            let fm = match first_mes {
                Some(f) if !f.is_empty() => Some(f.to_string()),
                Some(_) => None,
                None => None,
            };
            let ag = match alternate_greetings {
                Some(list) => {
                    let v: Vec<String> = list
                        .iter()
                        .filter(|s| !s.trim().is_empty())
                        .cloned()
                        .collect();
                    if v.is_empty() {
                        None
                    } else {
                        Some(v)
                    }
                }
                None => None,
            };
            (fm, ag)
        };
        self.get(id).map(|mut c| {
            // get 返回的 first_mes 从新 data_raw 提取,已一致;仅兜底
            if let Some(f) = new_first_mes {
                if c.first_mes.is_none() {
                    c.first_mes = Some(f);
                }
            }
            if let Some(g) = new_alternate_greetings {
                if c.alternate_greetings.is_none() {
                    c.alternate_greetings = Some(g);
                }
            }
            c
        })
    }

    /// 写入/移除角色卡内嵌契约(extensions.nlkaleido),其余 data_raw 字段保留。
    /// 契约按调用方给定的 JSON 原样存储(保留作者自定义字段);
    /// 结构校验由调用方(契约 API 的 parse_contract)前置完成,此处只管落库。
    /// 读改写经 `character_data::update_data_raw` 在同一写锁事务内完成(防丢更新)。
    pub fn set_embedded_contract(&self, id: &str, contract: Option<&Value>) -> Option<()> {
        let changed = crate::services::character_data::update_data_raw(&self.db, id, |data_raw| {
            let obj = data_raw
                .as_object_mut()
                .ok_or_else(|| "角色卡 data_raw 不是对象".to_string())?;
            match contract {
                Some(c) => {
                    let ext = obj
                        .entry("extensions")
                        .or_insert_with(|| Value::Object(Default::default()));
                    ext.as_object_mut()
                        .ok_or_else(|| "extensions 不是对象".to_string())?
                        .insert("nlkaleido".into(), c.clone());
                }
                None => {
                    if let Some(ext) = obj.get_mut("extensions") {
                        if let Some(ext_obj) = ext.as_object_mut() {
                            ext_obj.remove("nlkaleido");
                        }
                    }
                }
            }
            Ok(())
        })
        .ok()?;
        changed.then_some(())
    }

    /// 删除:删库记录 + 删原文件与头像文件,级联删会话/消息
    pub fn delete(&self, id: &str) -> bool {
        let Some(rec) = self.get(id) else {
            return false;
        };
        let conn = self.db.write();
        if conn
            .execute("DELETE FROM characters WHERE id = ?1", params![id])
            .map(|n| n > 0)
            .unwrap_or(false)
        {
            let _ = std::fs::remove_file(Path::new(&rec.file_path));
            if let Some(av) = &rec.avatar_path {
                let _ = std::fs::remove_file(Path::new(av));
            }
            true
        } else {
            false
        }
    }
}

/// 扩展名:原文件扩展名小写;空则按魔数(0x89 → .png,否则 .json)
fn detect_ext(original_name: &str, buffer: &[u8]) -> String {
    if let Some(pos) = original_name.rfind('.') {
        let ext = original_name[pos..].to_lowercase();
        if ext.len() > 1 && ext.len() <= 8 && !ext.contains(' ') {
            return ext;
        }
    }
    if buffer.first() == Some(&0x89) {
        ".png".to_string()
    } else {
        ".json".to_string()
    }
}

fn strip_ext(name: &str) -> String {
    if let Some(pos) = name.rfind('.') {
        name[..pos].to_string()
    } else {
        name.to_string()
    }
}
