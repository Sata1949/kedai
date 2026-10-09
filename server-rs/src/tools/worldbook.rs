// worldbook_update 工具(RPFLOW-2 核心件:剧情推演 → 世界书词条同步)。
//
// 由 deep/agent 角色扮演流程的归档步**按名单释放**(`tool_sets::ARCHIVE_TOOLS`),
// 不进正文默认工具列表(见 `tool_sets::META_TOOLS`,同 `run_flow` 先例)——
// 避免正文生成轮被诱导改写用户的世界书原文设定。
//
// 生效范围(按设置两档开关对当前角色求值):
//   - 角色档 `worldbook_sync_character_enabled`(默认开):**绑定到当前角色**的世界书 +
//     角色卡内嵌 character_book;
//   - 全局档 `worldbook_sync_global_enabled`(默认关):未绑定角色(character_id 为空)的世界书。
//   两档全关 → skipped(不写任何东西)。注意分区必须**按绑定过滤**:`enabled_for_character`
//   的注入语义是「绑定书 ∪ 全局书」,整份采用会让「全局档关闭」拦不住对全局书的写入
//   (可写面按来源分档,不能直接复用注入面的检索结果)。
//
// 蓝绿灯语义(用户拍板):蓝灯 = constant 常驻条目 = 原著设定,**不改写**(改为同书新建
// 绿灯条目);绿灯 = 关键词触发条目,可直接更新(默认追加「【更新】」段,mode=replace 整段替换)。
use crate::models::types::{ToolContext, ToolDefinition, WorldBookEntryView};
use crate::tools::agent_tools::ToolDeps;
use crate::tools::registry::ToolRegistry;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

/// 工具名(注册 / 风险级 / 归档名单三处引用同一常量,避免字符串漂移)
pub const TOOL_NAME: &str = "worldbook_update";

/// 追加模式(默认)的段落前缀:命中绿灯条目时在原文后追加 `\n\n【更新】{content}` 一段
const APPEND_MARKER: &str = "【更新】";

/// 生效范围(返回体 scope 字段;与两档开关一一对应)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Character,
    Global,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Scope::Character => "character",
            Scope::Global => "global",
        }
    }
}

/// 可写来源(生效范围内的一本书):独立世界书 或 角色卡内嵌 character_book
struct Source {
    scope: Scope,
    /// 独立世界书 id(内嵌卡书为 None)
    book_id: Option<String>,
    /// 独立世界书名称(`book` 参数按「id 或名称」匹配用;内嵌卡书为 None)
    book_name: Option<String>,
    /// 返回体 book 字段的可读标签
    label: String,
    /// 该书当前条目视图(匹配用;收集来源时一次读出,避免逐条目重复查库)
    views: Vec<WorldBookEntryView>,
}

impl Source {
    /// `book` 参数是否指向本书(仅独立世界书可按 id/名称寻址)
    fn matches_book_arg(&self, arg: &str) -> bool {
        self.book_id.as_deref() == Some(arg) || self.book_name.as_deref() == Some(arg)
    }
}

/// 注册 worldbook_update(按名单释放的元工具,见文件头)
pub fn register_worldbook_tool(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: TOOL_NAME.into(),
            description: "把剧情推演导致的世界观/角色设定变化同步到世界书词条(归档专用)。\
                按 topic 在生效范围内查找条目:命中「绿灯」(非常驻)条目则更新其内容;\
                命中「蓝灯」(常驻)条目时不改写原文,改为在同书新建一条绿灯条目;\
                未命中时在首选世界书新建绿灯条目。"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "主题/条目名(与条目 comment 求包含、与 keys 求交集,大小写不敏感)"
                    },
                    "content": { "type": "string", "description": "要写入的最新内容" },
                    "keywords": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "新条目的触发关键词(仅新建条目时使用;缺省用 [topic])"
                    },
                    "book": {
                        "type": "string",
                        "description": "首选世界书 id 或名称(仅新建条目时生效;必须在生效范围内)"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["append", "replace"],
                        "description": "命中绿灯条目时的写法:append 追加「【更新】」段(默认),replace 整段替换"
                    }
                },
                "required": ["topic", "content"]
            }),
        },
        Arc::new(move |args: Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move { Ok(run_update(&deps, &ctx, &args)) })
        }),
    );
}

/// 求值主体:返回结构化 JSON 字符串(错误用 `{"error": "..."}` 形状,与既有工具一致)
fn run_update(deps: &ToolDeps, ctx: &ToolContext, args: &Value) -> String {
    let topic = arg_str(args, "topic");
    if topic.is_empty() {
        return error_json("缺少 topic 参数(主题/条目名)");
    }
    let content = arg_str(args, "content");
    if content.is_empty() {
        return error_json("缺少 content 参数(要写入的最新内容)");
    }
    let keywords = arg_string_list(args, "keywords");
    let book_arg = {
        let b = arg_str(args, "book");
        (!b.is_empty()).then_some(b)
    };
    let mode = {
        let m = arg_str(args, "mode");
        if m.is_empty() {
            "append".to_string()
        } else {
            m
        }
    };
    if mode != "append" && mode != "replace" {
        return error_json("mode 只支持 append(默认)/ replace");
    }

    // ---- 两档开关求值(快照取自 ToolDeps,与既有工具同款) ----
    let settings = deps.settings_snapshot();
    let char_gate = settings.worldbook_sync_character_enabled;
    let global_gate = settings.worldbook_sync_global_enabled;
    if !char_gate && !global_gate {
        return json!({
            "action": "skipped",
            "reason": "角色档与全局档均已关闭(设置页「世界书词条同步」),未写入任何内容",
        })
        .to_string();
    }

    let sources = collect_sources(deps, ctx, char_gate, global_gate);
    // `book` 指定时先行校验:不在生效范围内直接报错(绝不静默落到别的书)
    let book_pin: Option<usize> = match book_arg.as_deref() {
        Some(arg) => match sources.iter().position(|s| s.matches_book_arg(arg)) {
            Some(i) => Some(i),
            None => {
                return error_json(&format!("未找到世界书「{arg}」(或它不在当前生效范围内)"));
            }
        },
        None => None,
    };
    if sources.is_empty() {
        return json!({ "action": "skipped", "reason": no_target_reason(char_gate, global_gate) })
            .to_string();
    }

    // ---- 匹配:在生效范围的全部条目里 `comment` 含 topic,或 `keys` 与 topic/keywords 有交集 ----
    // 取重合度最高的一条,并列取先出现的(来源按优先级、条目按 collect_entries 的稳定排序)。
    let topic_lower = topic.to_lowercase();
    let needles: HashSet<String> = std::iter::once(topic_lower.clone())
        .chain(keywords.iter().map(|k| k.to_lowercase()))
        .collect();
    let mut best: Option<(usize, i64, bool, String, i64)> = None;
    for (si, src) in sources.iter().enumerate() {
        for v in &src.views {
            let Some(score) = match_score(v, &topic_lower, &needles) else {
                continue;
            };
            if best.as_ref().is_none_or(|(_, _, _, _, s)| score > *s) {
                best = Some((si, v.id, v.constant, v.comment.clone(), score));
            }
        }
    }
    // 新建条目的关键词:显式 keywords 优先,缺省用 [topic]
    let new_keys = if keywords.is_empty() {
        vec![topic.clone()]
    } else {
        keywords.clone()
    };

    match best {
        // 绿灯(非常驻):直接更新内容,其余字段原样保留
        Some((si, uid, false, comment, _)) => {
            let src = &sources[si];
            let ok = if mode == "replace" {
                let c = content.clone();
                update_content(deps, src, ctx, uid, move |_old| c.clone())
            } else {
                let c = content.clone();
                update_content(deps, src, ctx, uid, move |old| {
                    format!("{old}\n\n{APPEND_MARKER}{c}")
                })
            };
            if !ok {
                return error_json("更新世界书条目失败(条目可能刚被改动,请重试)");
            }
            json!({
                "action": "updated",
                "book": src.label,
                "comment": comment,
                "entry_id": uid,
                "scope": src.scope.as_str(),
            })
            .to_string()
        }
        // 蓝灯(常驻 = 原著设定):不改写该条目,在同一本书新建绿灯条目
        Some((si, _, true, _, _)) => {
            let src = &sources[si];
            match create_in(deps, src, ctx, &topic, &new_keys, &content) {
                Some(entry_id) => json!({
                    "action": "created",
                    "book": src.label,
                    "comment": topic,
                    "entry_id": entry_id,
                    "scope": src.scope.as_str(),
                })
                .to_string(),
                None => error_json("命中常驻(蓝灯)条目,按规则未改写它;但新建绿灯条目失败,请重试"),
            }
        }
        // 未命中:在首选书新建绿灯条目
        None => {
            match create_in_preferred(deps, &sources, ctx, book_pin, &topic, &new_keys, &content) {
                Ok((label, entry_id, scope)) => json!({
                    "action": "created",
                    "book": label,
                    "comment": topic,
                    "entry_id": entry_id,
                    "scope": scope.as_str(),
                })
                .to_string(),
                Err(e) => error_json(&e),
            }
        }
    }
}

/// 按两档开关收集生效范围内的可写来源,顺序 = 新建优先级(角色绑定书 → 卡内嵌书 → 全局书),
/// 也是匹配并列时的先出现顺序。
fn collect_sources(
    deps: &ToolDeps,
    ctx: &ToolContext,
    char_gate: bool,
    global_gate: bool,
) -> Vec<Source> {
    let svc = &deps.world_books;
    let cid = ctx.character_id.as_str();
    let mut bound: Vec<Source> = Vec::new();
    let mut global: Vec<Source> = Vec::new();
    for rec in svc.enabled_for_character(cid) {
        // 按绑定拆开再按闸门过滤:查询结果的语义是「绑定书 ∪ 全局书」,不能整份采用
        let scope = match rec.character_id.as_deref() {
            Some(c) if c == cid => {
                if !char_gate {
                    continue;
                }
                Scope::Character
            }
            None => {
                if !global_gate {
                    continue;
                }
                Scope::Global
            }
            // 防御:查询只应返回「绑定到本人 ∪ 全局」,其它角色的书不应出现
            Some(_) => continue,
        };
        let Some(views) = svc.entries(&rec.id) else {
            continue;
        };
        let src = Source {
            scope,
            book_id: Some(rec.id.clone()),
            book_name: Some(rec.name.clone()),
            label: rec.name.clone(),
            views,
        };
        if scope == Scope::Character {
            bound.push(src);
        } else {
            global.push(src);
        }
    }
    let mut sources = bound;
    if char_gate {
        if let Some(views) = svc.character_book_views(cid) {
            sources.push(Source {
                scope: Scope::Character,
                book_id: None,
                book_name: None,
                label: embedded_label(deps, cid),
                views,
            });
        }
    }
    sources.extend(global);
    sources
}

/// 内嵌卡书在返回体里的可读标签(取角色显示名;取不到退回通用名)
fn embedded_label(deps: &ToolDeps, character_id: &str) -> String {
    deps.characters
        .get(character_id)
        .map(|c| format!("{}·内嵌世界书", c.chara_name))
        .unwrap_or_else(|| "角色卡内嵌世界书".to_string())
}

/// 「没有可写目标」的 skipped 说明(按开关状态给可操作的原因)
fn no_target_reason(char_gate: bool, global_gate: bool) -> String {
    let mut reason = String::from("没有可写目标:");
    reason.push_str(if char_gate {
        "当前角色没有绑定世界书,也没有内嵌 character_book;"
    } else {
        "角色档已关闭;"
    });
    reason.push_str(if global_gate {
        "也没有全局世界书。"
    } else {
        "全局档已关闭。"
    });
    reason
}

/// 匹配得分(None = 不匹配):`comment` 含 topic 计 1 分;`keys` 与 (topic ∪ keywords)
/// 的交集每项计 1 分。调用方取得分最高者、并列取先出现的。
fn match_score(
    entry: &WorldBookEntryView,
    topic_lower: &str,
    needles: &HashSet<String>,
) -> Option<i64> {
    let mut score = 0i64;
    if !entry.comment.is_empty() && entry.comment.to_lowercase().contains(topic_lower) {
        score += 1;
    }
    for k in &entry.keys {
        if needles.contains(&k.to_lowercase()) {
            score += 1;
        }
    }
    (score > 0).then_some(score)
}

/// 更新命中条目内容:独立书走 `update_entry_content`;内嵌卡书走
/// `update_character_book_entry_content`(均只改 content 键,其余字段原样保留)
fn update_content<F>(deps: &ToolDeps, src: &Source, ctx: &ToolContext, uid: i64, compose: F) -> bool
where
    F: FnOnce(&str) -> String,
{
    let svc = &deps.world_books;
    match src.book_id.as_deref() {
        Some(id) => svc.update_entry_content(id, uid, compose).is_some(),
        None => svc
            .update_character_book_entry_content(&ctx.character_id, uid, compose)
            .is_some(),
    }
}

/// 在指定来源新建绿灯条目,返回新条目 uid
fn create_in(
    deps: &ToolDeps,
    src: &Source,
    ctx: &ToolContext,
    comment: &str,
    keys: &[String],
    content: &str,
) -> Option<i64> {
    let svc = &deps.world_books;
    match src.book_id.as_deref() {
        Some(id) => svc.create_trigger_entry(id, comment, keys, content),
        None => svc.create_character_book_entry(&ctx.character_id, comment, keys, content),
    }
}

/// 未命中时的落地:`book` 指定(`pin`)时只在指定书新建;未指定时按 `sources` 顺序
/// (角色绑定书 → 卡内嵌书 → 全局书)取第一本可写的。
fn create_in_preferred(
    deps: &ToolDeps,
    sources: &[Source],
    ctx: &ToolContext,
    pin: Option<usize>,
    comment: &str,
    keys: &[String],
    content: &str,
) -> Result<(String, i64, Scope), String> {
    let order: Vec<usize> = match pin {
        Some(idx) => vec![idx],
        None => (0..sources.len()).collect(),
    };
    let mut failed: Vec<String> = Vec::new();
    for idx in order {
        let src = &sources[idx];
        match create_in(deps, src, ctx, comment, keys, content) {
            Some(uid) => return Ok((src.label.clone(), uid, src.scope)),
            None => failed.push(src.label.clone()),
        }
    }
    Err(format!("写入失败:世界书 {} 均未写入成功", failed.join("/")))
}

/// 取字符串参数并 trim(缺失/非字符串 → 空串)
fn arg_str(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 取字符串数组参数;容忍单个字符串(按逗号/顿号/换行拆)——模型偶尔不按数组给
fn arg_string_list(args: &Value, key: &str) -> Vec<String> {
    let Some(v) = args.get(key) else {
        return Vec::new();
    };
    let parts: Vec<String> = match v {
        Value::Array(arr) => arr
            .iter()
            .filter_map(|x| x.as_str())
            .map(|s| s.to_string())
            .collect(),
        Value::String(s) => s
            .split([',', '，', '、', '\n'])
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    };
    parts
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 错误结果:`{"error": "..."}` 形状(与既有工具一致)
fn error_json(msg: &str) -> String {
    json!({ "error": msg }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::permissions::{PermissionDecision, ToolRisk};
    use crate::utils::test_support::TempDataDir;
    use serde_json::json;

    /// 测试角色 id
    const CHAR: &str = "charW";

    /// 测试夹具:临时数据目录 + 空依赖(内存库) + 已注册工具的注册表。
    /// 返回 (守卫, 注册表, 依赖):解构绑定按**逆序**析构,守卫在前才活到最后
    /// (见 `utils::test_support` 模块头)。
    fn setup() -> (TempDataDir, ToolRegistry, Arc<ToolDeps>) {
        let (dir, deps) = ToolDeps::dummy_for_test();
        let deps = Arc::new(deps);
        let registry = ToolRegistry::new();
        register_worldbook_tool(&registry, deps.clone());
        (dir, registry, deps)
    }

    /// 播种角色卡(内嵌 character_book 由 card 提供)
    fn seed_character(deps: &ToolDeps, card: Value) {
        let conn = deps.db.write();
        conn.execute(
            "INSERT OR REPLACE INTO characters (id, name, chara_name, description, file_path, data_raw, created_at) \
             VALUES (?1, 'w', '文文', '', '', ?2, '')",
            rusqlite::params![CHAR, card.to_string()],
        )
        .unwrap();
        drop(conn);
    }

    /// 上传一本世界书(entries 为原始条目数组),返回世界书 id
    fn upload(deps: &ToolDeps, name: &str, entries: Value, cid: Option<&str>) -> String {
        let raw = json!({ "name": name, "entries": entries });
        let bytes = serde_json::to_vec(&raw).unwrap();
        deps.world_books
            .upload(&bytes, "book.json", cid)
            .unwrap()
            .id
    }

    /// 开关两档(deps.settings 是共享快照,测试直接改)
    fn set_gates(deps: &ToolDeps, character: bool, global: bool) {
        let mut s = deps.settings.lock().unwrap();
        s.worldbook_sync_character_enabled = character;
        s.worldbook_sync_global_enabled = global;
    }

    fn ctx() -> ToolContext {
        ToolContext {
            session_id: "sessW".into(),
            character_id: CHAR.into(),
            agent_depth: 0,
            scope: None,
            budget: None,
        }
    }

    /// 以已裁决放行路径调用工具(风险级 Dangerous,测试直接给放行裁决)并解析结果
    async fn call(registry: &ToolRegistry, args: Value) -> Value {
        let decision = PermissionDecision {
            allowed: true,
            risk: ToolRisk::Dangerous,
            reason: "测试放行".into(),
        };
        let out = registry
            .execute_with_decision(TOOL_NAME, &args.to_string(), ctx(), &decision)
            .await
            .unwrap();
        serde_json::from_str(&out).unwrap()
    }

    /// 读独立世界书条目视图
    fn views(deps: &ToolDeps, book_id: &str) -> Vec<WorldBookEntryView> {
        deps.world_books.entries(book_id).unwrap()
    }

    /// ① 命中绿灯:append 追加「【更新】」段、replace 整段替换;其余字段与兄弟条目不变
    #[tokio::test]
    async fn green_hit_appends_then_replaces() {
        let (_dir, registry, deps) = setup();
        seed_character(&deps, json!({ "name": "卡" }));
        let book = upload(
            &deps,
            "角色书",
            json!([
                { "uid": 0, "comment": "码头", "keys": ["码头", "港口"], "content": "旧码头", "constant": false },
                { "uid": 1, "comment": "别的", "keys": ["别处"], "content": "别处内容", "constant": false }
            ]),
            Some(CHAR),
        );
        set_gates(&deps, true, false);

        // append(默认)
        let out = call(
            &registry,
            json!({ "topic": "码头", "content": "新码头已建成" }),
        )
        .await;
        assert_eq!(out["action"], json!("updated"));
        assert_eq!(out["book"], json!("角色书"));
        assert_eq!(out["comment"], json!("码头"));
        assert_eq!(out["entry_id"], json!(0));
        assert_eq!(out["scope"], json!("character"));
        let vs = views(&deps, &book);
        let hit = vs.iter().find(|v| v.id == 0).unwrap();
        assert_eq!(hit.content, "旧码头\n\n【更新】新码头已建成");
        assert_eq!(hit.keys, vec!["码头".to_string(), "港口".to_string()]);
        assert_eq!(
            vs.iter().find(|v| v.id == 1).unwrap().content,
            "别处内容",
            "兄弟条目不得受影响"
        );

        // replace:整段替换
        let out = call(
            &registry,
            json!({
                "topic": "港口", "content": "三次重建后的码头", "mode": "replace"
            }),
        )
        .await;
        assert_eq!(out["action"], json!("updated"));
        assert_eq!(out["entry_id"], json!(0), "keys 交集命中同一条目");
        assert_eq!(
            views(&deps, &book)
                .iter()
                .find(|v| v.id == 0)
                .unwrap()
                .content,
            "三次重建后的码头"
        );
        drop(registry);
    }

    /// ② 命中蓝灯:不改写原条目;在同一本书新建绿灯条目(keys = keywords 或 [topic])
    #[tokio::test]
    async fn blue_hit_keeps_origin_and_creates_green_entry() {
        let (_dir, registry, deps) = setup();
        seed_character(&deps, json!({ "name": "卡" }));
        let book = upload(
            &deps,
            "角色书",
            json!([
                { "uid": 0, "comment": "世界观", "keys": ["世界"], "content": "原著设定:大陆只有两块", "constant": true }
            ]),
            Some(CHAR),
        );
        set_gates(&deps, true, false);

        let out = call(
            &registry,
            json!({
                "topic": "世界观", "content": "新大陆被发现了", "keywords": ["大陆", "新大陆"]
            }),
        )
        .await;
        assert_eq!(out["action"], json!("created"));
        assert_eq!(out["book"], json!("角色书"));
        assert_eq!(out["comment"], json!("世界观"));
        assert_eq!(out["scope"], json!("character"));
        let vs = views(&deps, &book);
        assert_eq!(vs.len(), 2, "蓝灯条目不改写,应新增一条绿灯条目: {vs:?}");
        let origin = vs.iter().find(|v| v.id == 0).unwrap();
        assert_eq!(
            origin.content, "原著设定:大陆只有两块",
            "蓝灯(常驻)条目不得被改写"
        );
        assert!(origin.constant, "原条目仍为常驻");
        let created = vs.iter().find(|v| v.id == 1).unwrap();
        assert_eq!(created.comment, "世界观");
        assert_eq!(created.keys, vec!["大陆".to_string(), "新大陆".to_string()]);
        assert_eq!(created.content, "新大陆被发现了");
        assert!(!created.constant, "新建条目必须是绿灯(非常驻)");
        assert!(created.enabled, "新建条目必须是启用态");
        drop(registry);
    }

    /// ③ 未命中:在首选书(角色绑定世界书)新建绿灯条目,关键词缺省 = [topic]
    #[tokio::test]
    async fn miss_creates_green_entry_in_preferred_book() {
        let (_dir, registry, deps) = setup();
        seed_character(&deps, json!({ "name": "卡" }));
        let bound = upload(
            &deps,
            "角色书",
            json!([
                { "uid": 0, "comment": "码头", "keys": ["码头"], "content": "旧", "constant": false }
            ]),
            Some(CHAR),
        );
        let global = upload(
            &deps,
            "全局书",
            json!([
                { "uid": 0, "comment": "无关", "keys": ["无关"], "content": "x", "constant": false }
            ]),
            None,
        );
        set_gates(&deps, true, false);

        let out = call(
            &registry,
            json!({ "topic": "新主题", "content": "新主题内容" }),
        )
        .await;
        assert_eq!(out["action"], json!("created"));
        assert_eq!(out["book"], json!("角色书"), "首选 = 角色绑定世界书");
        let vs = views(&deps, &bound);
        let created = vs.iter().find(|v| v.comment == "新主题").unwrap();
        assert_eq!(
            created.keys,
            vec!["新主题".to_string()],
            "缺省关键词 = [topic]"
        );
        assert!(!created.constant, "新建条目是绿灯");
        assert!(created.enabled);
        assert_eq!(created.content, "新主题内容");
        assert_eq!(views(&deps, &global).len(), 1, "全局档关:全局书不得被写入");
        drop(registry);
    }

    /// ③b `book` 指定:新建落到指定书(按 id 或名称);指定书不在生效范围 → error
    #[tokio::test]
    async fn book_param_pins_creation_target() {
        let (_dir, registry, deps) = setup();
        seed_character(&deps, json!({ "name": "卡" }));
        let book_a = upload(
            &deps,
            "书甲",
            json!([{ "uid": 0, "comment": "甲", "keys": ["甲"], "content": "x", "constant": false }]),
            Some(CHAR),
        );
        let book_b = upload(
            &deps,
            "书乙",
            json!([{ "uid": 0, "comment": "乙", "keys": ["乙"], "content": "y", "constant": false }]),
            Some(CHAR),
        );
        let global = upload(
            &deps,
            "全局书",
            json!([{ "uid": 0, "comment": "全局", "keys": ["全局"], "content": "z", "constant": false }]),
            None,
        );
        set_gates(&deps, true, false);

        // 按名称指定第二本绑定书
        let out = call(
            &registry,
            json!({
                "topic": "指定主题", "content": "写入书乙", "book": "书乙"
            }),
        )
        .await;
        assert_eq!(out["action"], json!("created"));
        assert_eq!(out["book"], json!("书乙"));
        assert!(views(&deps, &book_b)
            .iter()
            .any(|v| v.comment == "指定主题"));
        assert!(views(&deps, &book_a)
            .iter()
            .all(|v| v.comment != "指定主题"));

        // 按 id 指定同一本
        let out = call(
            &registry,
            json!({
                "topic": "指定主题2", "content": "写入书乙", "book": book_b
            }),
        )
        .await;
        assert_eq!(out["action"], json!("created"));
        assert_eq!(out["book"], json!("书乙"));

        // 指定不在生效范围的书(全局档关)→ error 形状
        let out = call(
            &registry,
            json!({
                "topic": "指定主题3", "content": "x", "book": "全局书"
            }),
        )
        .await;
        assert!(out["error"].as_str().unwrap().contains("未找到世界书"));
        assert!(views(&deps, &global)
            .iter()
            .all(|v| v.comment != "指定主题3"));
        drop(registry);
    }

    /// ④ 两个开关的作用域差异:仅角色档开 / 仅全局档开(含内嵌卡书的可写性)
    #[tokio::test]
    async fn scope_follows_the_two_gates() {
        let (_dir, registry, deps) = setup();
        seed_character(
            &deps,
            json!({
                "name": "卡",
                "character_book": {
                    "name": "卡内书",
                    "entries": [
                        { "id": 0, "comment": "内嵌主题", "keys": ["内嵌主题"], "content": "内嵌旧", "constant": false }
                    ]
                }
            }),
        );
        let bound = upload(
            &deps,
            "角色书",
            json!([{ "uid": 0, "comment": "码头", "keys": ["码头"], "content": "角色书·旧", "constant": false }]),
            Some(CHAR),
        );
        let global = upload(
            &deps,
            "全局书",
            json!([{ "uid": 0, "comment": "码头", "keys": ["码头"], "content": "全局书·旧", "constant": false }]),
            None,
        );

        // ---- 仅角色档开(默认):只写角色绑定书 + 卡内嵌书 ----
        set_gates(&deps, true, false);
        let out = call(
            &registry,
            json!({ "topic": "码头", "content": "角色档改写" }),
        )
        .await;
        assert_eq!(out["action"], json!("updated"));
        assert_eq!(out["scope"], json!("character"));
        assert_eq!(out["book"], json!("角色书"));
        assert_eq!(
            views(&deps, &bound)[0].content,
            "角色书·旧\n\n【更新】角色档改写"
        );
        assert_eq!(
            views(&deps, &global)[0].content,
            "全局书·旧",
            "全局档关:全局书不得被写入"
        );

        // 卡内嵌书(角色档)命中并更新
        let out = call(
            &registry,
            json!({ "topic": "内嵌主题", "content": "内嵌新" }),
        )
        .await;
        assert_eq!(out["action"], json!("updated"));
        assert_eq!(out["scope"], json!("character"));
        assert!(out["book"].as_str().unwrap().contains("内嵌世界书"));
        let card = deps.world_books.character_book_views(CHAR).unwrap();
        assert_eq!(card[0].content, "内嵌旧\n\n【更新】内嵌新");

        // ---- 仅全局档开:只能写全局书;绑定书与内嵌书都不可写 ----
        set_gates(&deps, false, true);
        let out = call(
            &registry,
            json!({ "topic": "码头", "content": "全局档改写" }),
        )
        .await;
        assert_eq!(out["action"], json!("updated"));
        assert_eq!(out["scope"], json!("global"));
        assert_eq!(out["book"], json!("全局书"));
        assert_eq!(
            views(&deps, &global)[0].content,
            "全局书·旧\n\n【更新】全局档改写"
        );
        assert_eq!(
            views(&deps, &bound)[0].content,
            "角色书·旧\n\n【更新】角色档改写",
            "角色档关:绑定书不得再被写入"
        );
        // 内嵌书同样不可写:未命中 → 回落到全局书新建
        let out = call(
            &registry,
            json!({ "topic": "内嵌主题", "content": "不该写进卡内书" }),
        )
        .await;
        assert_eq!(out["action"], json!("created"));
        assert_eq!(out["book"], json!("全局书"), "角色档关时内嵌卡书不可写");
        let card = deps.world_books.character_book_views(CHAR).unwrap();
        assert_eq!(
            card[0].content, "内嵌旧\n\n【更新】内嵌新",
            "角色档关:内嵌书不得被写入"
        );
        drop(registry);
    }

    /// ⑤ 两档全关 → skipped,且不写任何东西
    #[tokio::test]
    async fn both_gates_off_skips_without_writing() {
        let (_dir, registry, deps) = setup();
        seed_character(&deps, json!({ "name": "卡" }));
        let bound = upload(
            &deps,
            "角色书",
            json!([{ "uid": 0, "comment": "码头", "keys": ["码头"], "content": "旧", "constant": false }]),
            Some(CHAR),
        );
        let global = upload(
            &deps,
            "全局书",
            json!([{ "uid": 0, "comment": "码头", "keys": ["码头"], "content": "旧", "constant": false }]),
            None,
        );
        set_gates(&deps, false, false);

        let out = call(&registry, json!({ "topic": "码头", "content": "不该写入" })).await;
        assert_eq!(out["action"], json!("skipped"));
        assert!(out["reason"].as_str().unwrap().contains("关闭"));
        assert_eq!(views(&deps, &bound)[0].content, "旧");
        assert_eq!(views(&deps, &global)[0].content, "旧");
        drop(registry);
    }

    /// ⑤b 开关开着但无任何可写目标(角色作用域无书 + 全局档关)→ skipped,不报硬错
    #[tokio::test]
    async fn no_writable_target_skips() {
        let (_dir, registry, deps) = setup();
        seed_character(&deps, json!({ "name": "卡" }));
        // 只有一本全局书,且全局档关(默认)
        let global = upload(
            &deps,
            "全局书",
            json!([{ "uid": 0, "comment": "码头", "keys": ["码头"], "content": "旧", "constant": false }]),
            None,
        );
        set_gates(&deps, true, false);

        let out = call(&registry, json!({ "topic": "码头", "content": "x" })).await;
        assert_eq!(out["action"], json!("skipped"));
        assert!(out["reason"].as_str().unwrap().contains("没有可写目标"));
        assert_eq!(views(&deps, &global)[0].content, "旧");
        drop(registry);
    }

    /// 参数与寻址错误一律返回 {"error": ...} 形状(与既有工具一致),且不写入
    #[tokio::test]
    async fn arg_errors_return_error_json() {
        let (_dir, registry, deps) = setup();
        seed_character(&deps, json!({ "name": "卡" }));
        set_gates(&deps, true, false);

        let out = call(&registry, json!({ "content": "x" })).await;
        assert!(
            out["error"].as_str().unwrap().contains("topic"),
            "缺 topic: {out}"
        );
        let out = call(&registry, json!({ "topic": "t" })).await;
        assert!(
            out["error"].as_str().unwrap().contains("content"),
            "缺 content: {out}"
        );
        let out = call(
            &registry,
            json!({ "topic": "t", "content": "x", "mode": "patch" }),
        )
        .await;
        assert!(
            out["error"].as_str().unwrap().contains("mode"),
            "非法 mode: {out}"
        );
        // book 指定但角色作用域没有任何书 → 未命中路径报错,信息含「未找到世界书」
        let out = call(
            &registry,
            json!({ "topic": "t", "content": "x", "book": "不存在的书" }),
        )
        .await;
        assert!(
            out["error"].as_str().unwrap().contains("未找到世界书"),
            "book 寻址失败: {out}"
        );
        drop(registry);
    }
}
