// 迁移元测试(2026-09-13 批次 2,P0-2):防「schema.rs 加了列/表/索引但忘了写
// ensure_* 迁移」——症状是旧库升级后运行时 `no such column`,只在老用户机器上炸,
// 新装机与其它全量测试都发现不了。
//
// 做法:
//   ① 用冻结基线(fixtures/schema_baseline_v0_3_0_beta.sql = 0.3.0-beta 发布时的
//      CREATE_TABLES 原文)在临时目录建一个「老库」;
//   ② 对该库执行 Db::open(启动路径会跑全部 ensure_* 幂等迁移);
//   ③ 另建全新库(Db::open 直接建全量 schema);
//   ④ 逐表比对结构指纹:PRAGMA table_info(名称/类型/NOT NULL/默认值/主键)+ 索引清单。
// 两侧必须完全一致;新增结构而漏写迁移 → 不一致 → 本测试失败。
use kedai_server::models::db::Db;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// 冻结基线建表 SQL(**历史快照,不得随代码演进更新**;见文件头注释)
const BASELINE_SQL: &str = include_str!("fixtures/schema_baseline_v0_3_0_beta.sql");

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kedai-schema-meta-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 结构指纹:表名 → 有序的「列定义 + 索引定义」清单
fn fingerprint(db: &Db) -> BTreeMap<String, Vec<String>> {
    let conn = db.write();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let tables: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    };
    for t in tables {
        let mut items: Vec<String> = Vec::new();
        {
            // 表名来自 sqlite_master(非用户输入),format! 拼接无注入面
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({t})")).unwrap();
            let rows = stmt
                .query_map([], |r| {
                    let name: String = r.get(1)?;
                    let ctype: String = r.get(2)?;
                    let notnull: i64 = r.get(3)?;
                    let dflt: Option<String> = r.get(4)?;
                    let pk: i64 = r.get(5)?;
                    Ok(format!(
                        "col:{name}|{ctype}|nn={notnull}|dflt={dflt:?}|pk={pk}"
                    ))
                })
                .unwrap();
            items.extend(rows.map(|r| r.unwrap()));
        }
        // FTS5 虚拟表不支持 PRAGMA index_list(会报错):跳过索引项,仅比列
        if let Ok(mut stmt) = conn.prepare(&format!("PRAGMA index_list({t})")) {
            let rows = stmt
                .query_map([], |r| {
                    let name: String = r.get(1)?;
                    let origin: String = r.get(3)?;
                    Ok(format!("idx:{name}|{origin}"))
                })
                .unwrap();
            let mut idx: Vec<String> = rows.map(|r| r.unwrap()).collect();
            idx.sort();
            items.extend(idx);
        }
        out.insert(t, items);
    }
    out
}

/// 老库(0.3.0-beta 基线)经 Db::open 升级后,结构必须与全新库逐表一致
#[test]
fn old_database_upgrades_to_current_schema() {
    // ① 老库:执行冻结基线建表 SQL
    let old_dir = temp_dir("old");
    let old_path = old_dir.join("kedai.db");
    {
        let conn = rusqlite::Connection::open(&old_path).unwrap();
        conn.execute_batch(BASELINE_SQL)
            .expect("基线 SQL 应可执行(文件被截断?)");
    }
    // ② 升级:Db::open 跑 ensure_* 迁移(与真实启动路径一致)
    let upgraded = Db::open(&old_path, &old_dir).expect("老库应能被 Db::open 升级打开");

    // ③ 全新库
    let fresh_dir = temp_dir("fresh");
    let fresh = Db::open(&fresh_dir.join("kedai.db"), &fresh_dir).expect("全新库应能建立");

    // ④ 比对表集合与逐表结构
    let a = fingerprint(&upgraded);
    let b = fingerprint(&fresh);
    assert!(
        b.len() >= 20,
        "全新库表数异常({} 张):基线文件可能被截断或 schema 解析异常",
        b.len()
    );

    let only_upgraded: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let only_fresh: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    assert!(
        only_upgraded.is_empty(),
        "升级库存在全新库没有的表(迁移多建了表?):{only_upgraded:?}"
    );
    assert!(
        only_fresh.is_empty(),
        "全新库存在升级库没有的表(新增表漏写 ensure_* 迁移?):{only_fresh:?}"
    );

    for (table, fresh_defs) in &b {
        let upgraded_defs = &a[table];
        assert_eq!(
            upgraded_defs, fresh_defs,
            "表 {table} 升级后结构与全新库不一致(新增列/索引漏写 ensure_* 迁移?)"
        );
    }

    let _ = std::fs::remove_dir_all(&old_dir);
    let _ = std::fs::remove_dir_all(&fresh_dir);
}
