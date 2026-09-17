// 角色卡派生列(derived_json)行为测试(2026-09-17 P-11)。
//
// 背景:`characters.data_raw` 是完整 V2/V3 JSON。列表接口此前**无论如何都对每张卡做整份
// `serde_json::from_str`**,只为取 6 个派生字段(first_mes / alternate_greetings /
// regex_scripts / creator / character_version / creator_notes),于是列表成本是
// O(卡数 × 卡体积);1MB 级卡片的会话列表会明显卡顿。
//
// 做法:加 `derived_json` 列缓存派生字段,列表路径直读该列。**关键设计决策**:该列是
// **缓存而非事实源** —— 列值为空或非法时回退解析 `data_raw`,故:
//   - 回填从「正确性必需」降为「性能必需」,可增量推进;
//   - 旧库未回填、旁路写入未同步等情形都不会显示错数据(最差只是慢);
//   - 新旧库混用安全。
//
// 三条判别性断言(各针对一类会漏的实现错误):
//   1. **直读证明**:data_raw 是非法 JSON 而 derived_json 合法 → 列表仍返回正确派生字段。
//      若实现仍解析 data_raw,非法 JSON 会让字段变 None → 必红。
//   2. **回退证明**:derived_json 为空/非法而 data_raw 合法 → 列表仍返回派生字段。
//      漏了回退,未回填的旧库会显示空开场白。
//   3. **陈旧防护**:经各条写入路径改 data_raw 后,列表派生字段必须随之更新。
//      漏同步任一路径,界面就会显示旧开场白/旧正则。
use kedai_server::models::db::Db;
use kedai_server::services::character_service::CharacterService;
use kedai_server::services::user_script_service::UserScriptService;
use kedai_server::services::world_book_service::WorldBookService;
use kedai_server::utils::test_support::TempDataDir;
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Arc;

/// 造一张带全部派生字段的合法角色卡
fn card_with(name: &str, first_mes: &str, creator: &str) -> Value {
    json!({
        "spec": "chara_card_v2",
        "spec_version": "1.0",
        "name": name,
        "description": "人设描述",
        "first_mes": first_mes,
        "alternate_greetings": ["备用开场一", "备用开场二"],
        "creator": creator,
        "character_version": "2.1",
        "creator_notes": "作者备注",
        "extensions": {
            "regex_scripts": [{
                "id": "r1",
                "script_name": "测试脚本",
                "find_regex": "<A/>",
                "replace_string": "<b>x</b>",
                "markdown_only": false,
                "enabled": true
            }]
        }
    })
}

/// 建隔离环境;返回(守卫, 同一 Db 句柄, 角色服务)
fn service(tag: &str) -> (TempDataDir, Arc<Db>, Arc<CharacterService>) {
    let dir = TempDataDir::new(&format!("p11-{tag}"));
    let db = Arc::new(Db::open(&dir.join("kedai.db"), dir.path()).expect("建库失败"));
    let svc = Arc::new(CharacterService::new(db.clone(), dir.path().to_path_buf()));
    (dir, db, svc)
}

/// 绕过服务层直插一行,用于精确控制 data_raw 与 derived_json 的内容
fn insert_raw(db: &Db, id: &str, data_raw: &str, derived_json: &str) {
    db.write()
        .execute(
            "INSERT INTO characters \
             (id, name, chara_name, description, file_path, avatar_path, data_raw, created_at, derived_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                "测试卡",
                "测试角色",
                "",
                "test.json",
                Option::<String>::None,
                data_raw,
                "2026-09-17T00:00:00Z",
                derived_json
            ],
        )
        .expect("插入角色行失败");
}

/// 读某行的 derived_json 列原文
fn derived_of(db: &Db, id: &str) -> String {
    db.read()
        .unwrap()
        .query_row(
            "SELECT derived_json FROM characters WHERE id = ?1",
            params![id],
            |r| r.get::<_, String>(0),
        )
        .expect("读取 derived_json 失败")
}

/// 取列表中指定 id 的记录(不存在即 panic,避免静默通过)
fn pick(svc: &CharacterService, id: &str) -> kedai_server::models::types::CharacterRecord {
    svc.list()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("列表应含 {id}"))
}

// ===== 结构:列已由迁移链补上 =====

#[test]
fn derived_json_column_exists_after_open() {
    let (_dir, db, _svc) = service("column");
    let cols: Vec<String> = db
        .read()
        .unwrap()
        .prepare("PRAGMA table_info(characters)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(
        cols.iter().any(|c| c == "derived_json"),
        "characters 表应有 derived_json 列,实际列:{cols:?}"
    );
}

// ===== 判别性 1:直读证明 =====

#[test]
fn list_reads_derived_column_not_data_raw() {
    let (_dir, db, svc) = service("direct-read");
    let derived = serde_json::to_string(&json!({
        "first_mes": "来自列的开场白",
        "alternate_greetings": ["列备用一"],
        "creator": "列作者",
        "character_version": "9.9",
        "creator_notes": "列备注"
    }))
    .unwrap();
    // data_raw 故意是非法 JSON:任何解析它的实现都只能拿到 None
    insert_raw(&db, "c-broken", "{ 这不是合法 JSON", &derived);

    let rec = pick(&svc, "c-broken");
    assert_eq!(
        rec.first_mes.as_deref(),
        Some("来自列的开场白"),
        "列表必须直读 derived_json;若解析了非法 data_raw,first_mes 会是 None"
    );
    assert_eq!(rec.creator.as_deref(), Some("列作者"));
    assert_eq!(rec.character_version.as_deref(), Some("9.9"));
    assert_eq!(rec.creator_notes.as_deref(), Some("列备注"));
    assert_eq!(
        rec.alternate_greetings.as_deref(),
        Some(["列备用一".to_string()].as_slice())
    );
}

// ===== 判别性 2:回退证明 =====

#[test]
fn list_falls_back_to_data_raw_when_derived_empty() {
    let (_dir, db, svc) = service("fallback");
    let raw = serde_json::to_string(&card_with("回退卡", "回退开场白", "回退作者")).unwrap();
    insert_raw(&db, "c-legacy", &raw, "{}"); // 等价于旧库尚未回填

    let rec = pick(&svc, "c-legacy");
    assert_eq!(
        rec.first_mes.as_deref(),
        Some("回退开场白"),
        "列为空时必须回退解析 data_raw,否则未回填的旧库会显示空开场白"
    );
    assert_eq!(rec.creator.as_deref(), Some("回退作者"));
    assert_eq!(
        rec.regex_scripts.as_ref().map(|v| v.len()),
        Some(1),
        "回退路径也要提取正则脚本"
    );
    assert_eq!(
        rec.alternate_greetings.as_deref(),
        Some(["备用开场一".to_string(), "备用开场二".to_string()].as_slice())
    );
}

#[test]
fn list_falls_back_when_derived_invalid() {
    let (_dir, db, svc) = service("fallback-invalid");
    let raw = serde_json::to_string(&card_with("坏列卡", "坏列开场白", "作者")).unwrap();
    insert_raw(&db, "c-badd", &raw, "{ 列也坏了");

    assert_eq!(
        pick(&svc, "c-badd").first_mes.as_deref(),
        Some("坏列开场白"),
        "列非法 JSON 时也要回退,不能只在空字符串时回退"
    );
}

// ===== 判别性 3:陈旧防护(逐条写入路径) =====

#[test]
fn upload_populates_derived_column() {
    let (_dir, db, svc) = service("upload");
    let raw = serde_json::to_vec(&card_with("上传卡", "上传开场白", "上传作者")).unwrap();
    let rec = svc.upload(&raw, "uploaded.json").expect("上传应成功");

    let derived: Value = serde_json::from_str(&derived_of(&db, &rec.id)).expect("列应是合法 JSON");
    assert_eq!(derived["first_mes"], json!("上传开场白"));
    assert_eq!(derived["creator"], json!("上传作者"));
    assert_eq!(derived["character_version"], json!("2.1"));
    assert_eq!(derived["creator_notes"], json!("作者备注"));
    assert_eq!(
        derived["alternate_greetings"],
        json!(["备用开场一", "备用开场二"])
    );
    assert_eq!(
        derived["regex_scripts"].as_array().map(|a| a.len()),
        Some(1)
    );
}

#[test]
fn update_refreshes_derived_column() {
    let (_dir, db, svc) = service("via-update");
    let raw = serde_json::to_vec(&card_with("编辑卡", "编辑前开场白", "编辑前作者")).unwrap();
    let rec = svc.upload(&raw, "edit.json").expect("上传应成功");

    assert!(
        svc.update(
            &rec.id,
            Some("编辑后角色名"),
            Some("编辑后人设"),
            Some("编辑后开场白"),
            None
        )
        .is_some(),
        "编辑应成功"
    );

    let got = pick(&svc, &rec.id);
    assert_eq!(
        got.first_mes.as_deref(),
        Some("编辑后开场白"),
        "update 后列表派生字段必须更新"
    );
    assert_eq!(got.chara_name, "编辑后角色名");
    // 列内容也要新(只断言列表会被回退掩盖,见 update_data_raw_writes_fresh_derived_column 说明)
    let col: Value = serde_json::from_str(&derived_of(&db, &rec.id)).expect("列应合法");
    assert_eq!(col["first_mes"], json!("编辑后开场白"));
}

#[test]
fn set_embedded_contract_keeps_derived_column_consistent() {
    let (_dir, db, svc) = service("via-contract");
    let raw = serde_json::to_vec(&card_with("契约卡", "契约开场白", "契约作者")).unwrap();
    let rec = svc.upload(&raw, "contract.json").expect("上传应成功");

    // set_embedded_contract 经 character_data::update_data_raw(收口)
    svc.set_embedded_contract(&rec.id, Some(&json!({ "name": "测试契约" })))
        .expect("写内嵌契约应成功");

    let parsed: Value = serde_json::from_str(&derived_of(&db, &rec.id)).expect("列应是合法 JSON");
    assert_eq!(
        parsed["first_mes"],
        json!("契约开场白"),
        "经收口写入后派生列必须仍与 data_raw 一致"
    );
    assert_eq!(pick(&svc, &rec.id).first_mes.as_deref(), Some("契约开场白"));
}

/// **列内容**断言:经收口改 data_raw 后,`derived_json` 列本身必须是新值。
///
/// 为什么必须直查列而不能只断言列表输出:该列设计为**缓存而非事实源**,读取侧在列不可用时
/// 回退解析 data_raw —— 于是「列没同步」这一缺陷在列表输出上会被回退**掩盖**,
/// 行为断言永远为绿。变异测试实测确认:把收口处的派生计算替换为常量 `{}` 后,
/// 仅断言列表输出的用例全部照常通过。故此类断言必须落到列上。
///
/// 触发路径用 `WorldBookService::save_character_book`(公开 API),它内部走的就是
/// `character_data::update_data_raw` 收口(该函数本身是 `pub(crate)`,集成测试取不到)。
#[test]
fn save_character_book_writes_fresh_derived_column() {
    let (_dir, db, svc) = service("collector-column");
    // 卡里需带 character_book(该 API 要求条目结构存在)
    let mut card = card_with("列值卡", "列值开场白", "列值作者");
    card["character_book"] = json!({ "name": "测试世界书", "entries": [] });
    let raw = serde_json::to_vec(&card).unwrap();
    let rec = svc.upload(&raw, "collector-col.json").expect("上传应成功");

    WorldBookService::new(db.clone())
        .save_character_book(&rec.id, &[])
        .expect("保存内嵌世界书应成功");

    let col: Value = serde_json::from_str(&derived_of(&db, &rec.id)).expect("列应是合法 JSON");
    assert_eq!(
        col["first_mes"],
        json!("列值开场白"),
        "收口必须把派生值写进列;写成 {{}}(常量)会让列表退化为逐卡解析(性能回退且无告警)"
    );
    assert_eq!(col["creator"], json!("列值作者"));
}

/// **列内容**断言:旁路 `write_character_raw`(旧字段迁移)也必须刷新列
#[test]
fn legacy_script_migration_writes_fresh_derived_column() {
    let (_dir, db, _svc) = service("bypass-column");
    let mut legacy = card_with("旁路列卡", "旁路列新开场白", "旁路列新作者");
    legacy["extensions"]["TavernHelper_scripts"] = json!([{ "name": "旧布局", "content": "x" }]);
    insert_raw(
        &db,
        "c-bypass-col",
        &serde_json::to_string(&legacy).unwrap(),
        "{}",
    );

    UserScriptService::new(db.clone())
        .get_character("c-bypass-col")
        .expect("读取脚本树应成功");

    let col: Value = serde_json::from_str(&derived_of(&db, "c-bypass-col")).expect("列应合法");
    assert_eq!(
        col["first_mes"],
        json!("旁路列新开场白"),
        "旁路直写 data_raw 后必须把新派生值写进列(漏同步则列停留在旧值/空值)"
    );
}

#[test]
fn save_script_tree_refreshes_derived_column() {
    let (_dir, db, svc) = service("via-save-tree");
    let raw = serde_json::to_vec(&card_with("脚本树卡", "脚本树开场白", "脚本树作者")).unwrap();
    let rec = svc.upload(&raw, "tree.json").expect("上传应成功");

    let scripts = UserScriptService::new(db);
    // character 作用域保存经 character_data::update_data_raw(收口)
    scripts
        .save_tree(
            "character",
            &rec.id,
            &json!([{ "type": "script", "name": "测试脚本" }]),
        )
        .expect("保存脚本树应成功");

    let got = pick(&svc, &rec.id);
    assert_eq!(
        got.first_mes.as_deref(),
        Some("脚本树开场白"),
        "保存脚本树只动 extensions,派生字段应保持正确"
    );
}

/// 生产旁路:`UserScriptService::get_character` 的旧字段迁移会**整份回写 data_raw**
/// (user_script_service.rs 的 `write_character_raw`),不走收口。若该路径不同步派生列,
/// 列表就会读到陈旧值 —— 本用例用「迁移时 data_raw 里的开场白已被改掉」来判别。
#[test]
fn legacy_script_migration_direct_write_refreshes_derived_column() {
    let (_dir, db, svc) = service("via-direct-write");
    // 卡里同时含有:① 旧布局触发迁移;② 更新的派生字段(与列里的旧值不同)
    let mut legacy = card_with("旁路卡", "旁路新开场白", "旁路新作者");
    legacy["extensions"]["TavernHelper_scripts"] = json!([
        { "name": "旧布局脚本", "content": "x" }
    ]);
    let raw = serde_json::to_string(&legacy).unwrap();
    // 列里预置**旧值**:若旁路不同步,列表会读列拿到旧开场白
    let stale = serde_json::to_string(&json!({
        "first_mes": "旁路旧开场白",
        "creator": "旁路旧作者"
    }))
    .unwrap();
    insert_raw(&db, "c-bypass", &raw, &stale);

    // 触发旧字段迁移 → 走 write_character_raw 整份回写 data_raw
    let scripts = UserScriptService::new(db.clone());
    scripts.get_character("c-bypass").expect("读取脚本树应成功");

    let got = pick(&svc, "c-bypass");
    assert_eq!(
        got.first_mes.as_deref(),
        Some("旁路新开场白"),
        "直写路径必须同步派生列,否则列表显示陈旧开场白(列里是旧值)"
    );
    assert_eq!(got.creator.as_deref(), Some("旁路新作者"));
}

#[test]
fn reflatten_updates_derived_column() {
    let (_dir, db, svc) = service("reflatten");
    // V3 布局:正文在 data 子对象,顶层 first_mes 为空
    let v3 = json!({
        "spec": "chara_card_v3",
        "spec_version": "1.0",
        "name": "V3卡",
        "first_mes": "",
        "data": {
            "name": "V3卡",
            "description": "V3人设",
            "first_mes": "V3开场白",
            "creator": "V3作者"
        }
    });
    insert_raw(&db, "c-v3", &serde_json::to_string(&v3).unwrap(), "{}");

    svc.reflatten_v3_cards();

    assert_eq!(
        pick(&svc, "c-v3").first_mes.as_deref(),
        Some("V3开场白"),
        "reflatten 补全 V3 顶层后,列表派生字段应为补全后的值"
    );
}

// ===== 边界 =====

#[test]
fn empty_card_yields_none_fields() {
    let (_dir, db, svc) = service("empty");
    let raw = serde_json::to_string(&json!({ "spec": "chara_card_v2", "name": "空卡" })).unwrap();
    insert_raw(&db, "c-empty", &raw, "{}");

    let got = pick(&svc, "c-empty");
    assert!(got.first_mes.is_none());
    assert!(got.alternate_greetings.is_none());
    assert!(got.regex_scripts.is_none());
    assert!(got.creator.is_none());
    assert!(got.creator_notes.is_none());
}

/// 列表仍不返回 data_raw(体积大),详情才返回 —— 派生列不得改变该契约
#[test]
fn list_omits_data_raw_but_get_includes_it() {
    let (_dir, _db, svc) = service("data-raw-scope");
    let raw = serde_json::to_vec(&card_with("范围卡", "范围开场白", "范围作者")).unwrap();
    let rec = svc.upload(&raw, "scope.json").expect("上传应成功");

    assert!(
        pick(&svc, &rec.id).data_raw.is_none(),
        "列表不得携带 data_raw"
    );
    assert!(
        svc.get(&rec.id).expect("详情应存在").data_raw.is_some(),
        "详情必须携带 data_raw"
    );
}

/// 多张卡:列表派生字段互不串台(证明按行读列,而非复用首行结果)
#[test]
fn multiple_cards_have_independent_derived_fields() {
    let (_dir, _db, svc) = service("multi");
    let mut ids = Vec::new();
    for i in 0..8 {
        let raw = serde_json::to_vec(&card_with(
            &format!("卡{i}"),
            &format!("开场白{i}"),
            &format!("作者{i}"),
        ))
        .unwrap();
        ids.push(
            svc.upload(&raw, &format!("c{i}.json"))
                .expect("上传应成功")
                .id,
        );
    }

    for (i, id) in ids.iter().enumerate() {
        let got = pick(&svc, id);
        assert_eq!(
            got.first_mes.as_deref(),
            Some(format!("开场白{i}").as_str()),
            "第 {i} 张卡的派生字段不得被其它卡覆盖"
        );
        assert_eq!(got.creator.as_deref(), Some(format!("作者{i}").as_str()));
    }
}

// ===== 回填(migration 侧的幂等回填函数) =====

/// 清掉回填标记,模拟「尚未回填过的库」。
///
/// 时序说明(实测踩到):`Db::open` 的升级链**会跑一次回填并写标记**——那时表通常是空的,
/// 于是标记存在但没有任何行被回填。之后再插入的行(尤其经 `insert_raw` 绕过服务层的行)
/// 列会是 '{}',而二次回填因标记已存在直接返回。故要测回填逻辑本身,必须先清标记。
fn clear_backfill_mark(db: &Db) {
    db.write()
        .execute(
            "DELETE FROM backfill_meta WHERE key = 'characters_derived_backfilled'",
            [],
        )
        .expect("清回填标记失败");
}

/// 回填证明:`derived_json` 为空的行必须被真正写满,且落 `backfill_meta` 标记。
///
/// 为什么单独测这一条:回退路径的存在使「没回填」在行为上不可见(列表仍正确),
/// 于是回填是否真的生效只能**直查列**来验证。若回填失效,正确性不受影响但性能收益归零
/// ——那正是本批次的目的,故必须有断言守住。
#[test]
fn backfill_fills_empty_derived_column_once() {
    let (_dir, db, _svc) = service("backfill");
    let raw = serde_json::to_string(&card_with("待回填卡", "待回填开场白", "待回填作者")).unwrap();
    // 模拟旧库:列是默认值 '{}',且回填尚未发生过
    insert_raw(&db, "c-backfill", &raw, "{}");
    clear_backfill_mark(&db);
    assert_eq!(derived_of(&db, "c-backfill"), "{}");

    kedai_server::migration::ensure_characters_derived_backfill(&db.write()).expect("回填应成功");

    let col: Value = serde_json::from_str(&derived_of(&db, "c-backfill")).expect("列应合法");
    assert_eq!(
        col["first_mes"],
        json!("待回填开场白"),
        "回填必须把派生值写进列,否则列表永远走逐卡解析(收益归零)"
    );
    assert_eq!(col["creator"], json!("待回填作者"));
    assert_eq!(
        db.read()
            .unwrap()
            .query_row(
                "SELECT value FROM backfill_meta WHERE key = 'characters_derived_backfilled'",
                [],
                |r| r.get::<_, String>(0)
            )
            .ok()
            .as_deref(),
        Some("1"),
        "回填必须落 backfill_meta 标记(否则每次启动都全表重算)"
    );
}

/// 回填幂等:标记存在时二次执行不得重算覆盖(否则每次启动都全表重写)
#[test]
fn backfill_is_idempotent_and_preserves_existing() {
    let (_dir, db, _svc) = service("backfill-idem");
    let raw = serde_json::to_string(&card_with("幂等卡", "原开场白", "原作者")).unwrap();
    insert_raw(&db, "c-idem", &raw, "{}");
    clear_backfill_mark(&db);
    kedai_server::migration::ensure_characters_derived_backfill(&db.write())
        .expect("首次回填应成功");

    // 人为把列改成特定值,再跑一次回填:标记已存在 → 不得覆盖
    db.write()
        .execute(
            "UPDATE characters SET derived_json = ?1 WHERE id = ?2",
            params![r#"{"first_mes":"手工写入"}"#, "c-idem"],
        )
        .unwrap();
    kedai_server::migration::ensure_characters_derived_backfill(&db.write())
        .expect("二次回填应成功");

    let col: Value = serde_json::from_str(&derived_of(&db, "c-idem")).unwrap();
    assert_eq!(
        col["first_mes"],
        json!("手工写入"),
        "已回填过(标记存在)时不得重算覆盖"
    );
}

/// 回填遇 data_raw 非法 JSON:跳过该行且不中断(其余行照常回填)
#[test]
fn backfill_skips_invalid_rows_without_aborting() {
    let (_dir, db, _svc) = service("backfill-bad");
    insert_raw(&db, "c-bad", "{ 非法 JSON", "{}");
    clear_backfill_mark(&db);
    let good = serde_json::to_string(&card_with("好卡", "好卡开场白", "好卡作者")).unwrap();
    insert_raw(&db, "c-good", &good, "{}");

    kedai_server::migration::ensure_characters_derived_backfill(&db.write())
        .expect("即使有坏行,回填也不得整体失败");

    let col: Value = serde_json::from_str(&derived_of(&db, "c-good")).unwrap();
    assert_eq!(col["first_mes"], json!("好卡开场白"), "好行应已回填");
    // 坏行:列保持 '{}',由读取侧回退解析处理(它解析不了 → 各字段为 None,但不报错)
    assert_eq!(derived_of(&db, "c-bad"), "{}");
}
