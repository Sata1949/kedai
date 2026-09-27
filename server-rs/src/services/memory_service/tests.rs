//! `memory_service` 的单元测试(由原文件内的 `#[cfg(test)] mod tests { … }` 整段搬来,原样保留)。
//!
//! `#![cfg(test)]` 必须留在**列首**且字面为 `#[cfg(test)]`:`tools/check-arch.mjs` 的
//! `productionLineCount`/`productionSource` 靠 `^#!?\[cfg\(test\)\]` 切出生产区,否则本文件会被
//! 当成生产代码,规则 E(`services` 生产代码禁用 `.expect(`,基线为空集)会把测试里的断言全部判红。
#![cfg(test)]
use super::*;
use crate::models::db::Db;
use crate::models::types::MessageRecord;
use crate::services::session_service::SessionService;
use crate::services::settings_service::RuntimeSettings;
use crate::utils::test_support::TempDataDir;
use rusqlite::params;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

fn entry(id: i64, usage: i64, last_usage: Option<&str>, selected: bool) -> MemoryEntry {
    entry_pinned(id, usage, last_usage, selected, false)
}

fn entry_pinned(
    id: i64,
    usage: i64,
    last_usage: Option<&str>,
    selected: bool,
    pinned: bool,
) -> MemoryEntry {
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
        pinned,
    }
}

/// 返回 (守卫, 服务...):解构绑定按**逆序**析构,守卫在前才活到最后(见 test_support 模块头)
fn service() -> (TempDataDir, MemoryService, SessionService) {
    let dir = TempDataDir::new("memory");
    let db = Arc::new(Db::open(&dir.join("kedai.db"), &dir).unwrap());
    let memory = MemoryService::new(db.clone());
    let sessions = SessionService::new(db);
    (dir, memory, sessions)
}

/// 批量按 id 取记忆(2026-09-16 性能批次 P-5):蒸馏后为新增条目补向量时,
/// 此前逐条 `get(id)` 是 N+1 —— 同一份内容要发 N 条 SELECT。`get_many` 用
/// 一条 `WHERE id IN (...)` 取回,并**保持入参顺序**(调用方依赖 id 与内容对齐)。
#[test]
fn get_many_preserves_input_order_and_skips_missing() {
    let (_dir, memory, _sessions) = service();
    // 插 3 条,记录 id
    let a = memory.create_manual("c1", "第一条").unwrap();
    let b = memory.create_manual("c1", "第二条").unwrap();
    let c = memory.create_manual("c1", "第三条").unwrap();
    // 刻意打乱顺序 + 混入不存在的 id
    let got = memory.get_many(&[c.id, 999_999, a.id, b.id]);
    assert_eq!(
        got.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![c.id, a.id, b.id],
        "应按入参顺序返回,并静默跳过不存在的 id"
    );
    assert_eq!(got[0].content, "第三条");
    assert_eq!(got[2].content, "第二条");
}

/// 空入参不查库、直接返回空(SQL `IN ()` 是语法错误,顺手锁定边界)
#[test]
fn get_many_empty_input_returns_empty() {
    let (_dir, memory, _sessions) = service();
    let _ = memory.create_manual("c1", "有内容").unwrap();
    assert!(memory.get_many(&[]).is_empty());
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
    let (_dir, memory, _) = service();
    let conn = memory.db.read().unwrap();
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
        "pinned",
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
            .prepare(
                "SELECT name FROM sqlite_schema WHERE type='index' AND tbl_name='memory_entries'",
            )
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
        for _ in rows.flatten() {
            idx_count += 1;
        }
    }
    assert!(
        idx_count >= 1,
        "memory_entries 应建 character_id+selected 索引"
    );
    // kind CHECK 约束:非法 kind 拒绝
    assert!(memory.insert("c1", None, "bogus", "x").is_err());
    drop(conn);
}

/// 精选排序:pinned 优先 → usage_count DESC → last_usage DESC(None 最后)
/// → id DESC(tie-break 确定性);selected=0 过滤;limit 截断
#[test]
fn select_orders_by_usage_recency_and_limit() {
    let entries = vec![
        entry(1, 0, None, true), // 从未使用,排最后
        entry(2, 5, Some("2026-08-01T00:00:00Z"), true),
        entry(3, 5, Some("2026-08-02T00:00:00Z"), true), // 同计数,更近使用在前
        entry(4, 9, Some("2026-07-01T00:00:00Z"), true), // 计数最高
        entry(5, 99, Some("2026-08-03T00:00:00Z"), false), // 未选中,不参与
        entry(6, 5, Some("2026-08-02T00:00:00Z"), true), // 与 3 完全同键,id 大在前
        entry_pinned(7, 0, None, true, true),            // pinned:即使零使用也置顶
    ];
    let picked = select_for_injection(&entries, 10);
    let ids: Vec<i64> = picked.iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        vec![7, 4, 6, 3, 2, 1],
        "排序应为 pinned→计数→最近使用→id(新在前): {ids:?}"
    );
    // limit 截断
    let top2 = select_for_injection(&entries, 2);
    assert_eq!(top2.iter().map(|e| e.id).collect::<Vec<_>>(), vec![7, 4]);
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
    let text =
        "用户与角色在图书馆初识\n\n- 角色承诺周末带用户看画展\n* 用户透露自己害怕打雷\n  \n第4条\n";
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
    let long = (0..100)
        .map(|i| format!("记忆{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(parse_distilled_lines(&long).len(), MAX_DISTILLED_LINES);
}

/// 蒸馏落库:fake LLM 返回多行 → 每行一条 kind='distilled',
/// character_id 按会话归属,source_session_id 记来源
#[tokio::test]
async fn distill_session_inserts_lines() {
    let (dir, memory, sessions) = service();
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
            assert!(
                messages[1].content.contains("图书馆"),
                "蒸馏输入应含历史: {}",
                messages[1].content
            );
            Ok("用户与角色在图书馆初识\n角色承诺周末看画展\n\n- 用户害怕打雷".into())
        })
        .await
        .unwrap();
    assert_eq!(outcome.inserted, 3);
    assert_eq!(outcome.skipped, 0);
    assert_eq!(outcome.character_id, "charA");

    let list = memory.list("charA");
    assert_eq!(list.len(), 3);
    assert!(list.iter().all(|e| e.kind == "distilled"));
    assert!(list
        .iter()
        .all(|e| e.source_session_id.as_deref() == Some("s1")));
    assert!(
        list.iter().any(|e| e.content == "用户害怕打雷"),
        "- 前缀应被剥除"
    );
    assert!(list.iter().all(|e| e.usage_count == 0 && e.selected));
}

/// 空历史不蒸馏:不调 LLM(fake 计数为 0)、inserted=0
#[tokio::test]
async fn distill_session_empty_history_skips() {
    let (dir, memory, sessions) = service();
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
}

/// LLM 失败上抛;蒸馏输出全空白 → 报错不落库
#[tokio::test]
async fn distill_session_llm_failure_propagates() {
    let (dir, memory, sessions) = service();
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
}

/// CRUD:手动添加/编辑/删除;touch 只动计数与时间戳
#[test]
fn crud_and_touch_semantics() {
    let (_dir, memory, _) = service();
    let e = memory.create_manual("c1", "  用户喜欢薄荷茶  ").unwrap();
    assert_eq!(e.content, "用户喜欢薄荷茶", "content 应 trim");
    assert_eq!(e.kind, "manual");
    assert_eq!(e.source_session_id, None);

    // 编辑:content + selected + pinned
    let updated = memory
        .update(e.id, Some("用户喜欢洋甘菊茶"), Some(false), Some(true))
        .unwrap();
    assert_eq!(updated.content, "用户喜欢洋甘菊茶");
    assert!(!updated.selected);
    assert!(updated.pinned);
    // 空白 content → 404 语义(拒绝)
    assert!(memory.update(e.id, Some("   "), None, None).is_none());
    // 不存在的 id
    assert!(memory.update(9999, Some("x"), None, None).is_none());

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
}

/// Phase 3 向量索引端到端(不调外部 API):
/// vec0 表懒建 → 写入向量 → 读取回环 → 混合召回按向量相似度排序。
#[test]
fn vector_index_roundtrip_and_hybrid_recall() {
    let (_dir, memory, _) = service();
    let a = memory.create_manual("c1", "用户喜欢薄荷茶").unwrap();
    let b = memory
        .create_manual("c1", "角色承诺周末带用户看画展")
        .unwrap();

    // 维度非法(0)时拒绝
    assert!(memory.ensure_vec_table(0).is_ok_and(|ok| !ok));

    // 建表 + 写入(4 维便于手工构造语义关系)
    let va = vec![1.0, 0.0, 0.0, 0.0];
    let vb = vec![0.0, 1.0, 0.0, 0.0];
    memory.upsert_vector(a.id, &va).unwrap();
    memory.upsert_vector(b.id, &vb).unwrap();

    // 读取回环:向量数值与维度一致
    let got = memory.get_vectors(&[a.id, b.id]);
    assert_eq!(got.len(), 2, "两条向量都应读到");
    assert_eq!(got[&a.id].len(), 4);
    assert!((got[&a.id][0] - 1.0).abs() < 1e-6, "向量值应往返一致");

    // 覆盖写:同一 id 再写不应产生重复行
    memory.upsert_vector(a.id, &va).unwrap();
    assert_eq!(memory.get_vectors(&[a.id]).len(), 1, "覆盖写后仍只有一行");
    assert_eq!(memory.vector_status().embedded, 2, "向量总数应为 2");

    // 混合召回:查询向量贴近 a → a 排前(即使词面无交集)
    let entries = memory.list("c1");
    let qv = vec![1.0, 0.0, 0.0, 0.0];
    let recalled = select_recall_hybrid(&entries, "完全无关的查询", 2, &[], Some(&qv), &got);
    assert_eq!(
        recalled.first().map(|e| e.id),
        Some(a.id),
        "向量贴近 a 时应召回 a 在前: {:?}",
        recalled.iter().map(|e| e.id).collect::<Vec<_>>()
    );

    // 无向量时退化为纯 Jaccard:词面命中 b 的内容
    let recalled2 = select_recall(&entries, "画展", 2, &[]);
    assert_eq!(
        recalled2.first().map(|e| e.id),
        Some(b.id),
        "无向量时按词面召回"
    );

    // 维度不匹配:换维度后 ensure 返回 false(保留旧数据,不自动删)
    assert!(memory.ensure_vec_table(8).is_ok_and(|ok| !ok));
    let st = memory.vector_status();
    assert!(st.dim_mismatch.is_some(), "应记录维度漂移");
    assert_eq!(st.embedded, 2, "漂移时旧向量保留");

    // 显式重建:清表后可重新写入新维度
    memory.drop_vec_table().unwrap();
    assert_eq!(memory.vector_status().embedded, 0, "重建后向量清空");
    let v8 = vec![0.5f32; 8];
    memory.upsert_vector(a.id, &v8).unwrap();
    assert_eq!(memory.vector_status().dim, Some(8), "新维度生效");
}

/// B1 检索:中文 FTS5 命中(trigram);<3 字符回退 LIKE;
/// 更新/删除经 trigger 同步索引;查询短语中的双引号不炸语法
#[test]
fn search_hits_chinese_via_fts_and_falls_back_to_like() {
    let (_dir, memory, _) = service();
    memory
        .insert("c1", None, "manual", "用户与角色在图书馆初识")
        .unwrap();
    memory.insert("c1", None, "manual", "角色害怕打雷").unwrap();
    memory
        .insert("c2", None, "manual", "另一个角色的图书馆")
        .unwrap();

    // FTS5 中文命中(≥3 字符)
    let hits = memory.search("c1", "图书馆", 10);
    assert_eq!(hits.len(), 1, "中文应经 FTS5 命中: {hits:?}");
    assert_eq!(hits[0].content, "用户与角色在图书馆初识");
    // 角色隔离:c2 的记忆不出现在 c1 检索结果
    assert!(memory
        .search("c2", "图书馆", 10)
        .iter()
        .all(|e| e.character_id == "c2"));

    // <3 字符回退 LIKE(trigram 无法匹配 2 字)
    let short = memory.search("c1", "打雷", 10);
    assert_eq!(short.len(), 1, "2 字符查询应回退 LIKE 命中: {short:?}");
    assert_eq!(short[0].content, "角色害怕打雷");
    // LIKE 通配符转义:查询 "%" 不应匹配全部
    assert!(
        memory.search("c1", "%%", 10).is_empty(),
        "% 应被转义而非通配"
    );

    // 更新后索引同步(trigger AFTER UPDATE)
    memory
        .update(hits[0].id, Some("用户与角色在美术馆初识"), None, None)
        .unwrap();
    assert!(
        memory.search("c1", "图书馆", 10).is_empty(),
        "旧内容应已出索引"
    );
    assert_eq!(memory.search("c1", "美术馆", 10).len(), 1, "新内容应入索引");

    // 删除后索引同步(trigger AFTER DELETE)
    memory.delete(short[0].id);
    assert!(
        memory.search("c1", "打雷", 10).is_empty(),
        "删除后不应再命中"
    );

    // limit 生效
    for i in 0..5 {
        memory
            .insert("c3", None, "manual", &format!("共同关键词记忆{i}"))
            .unwrap();
    }
    assert_eq!(memory.search("c3", "共同关键词", 2).len(), 2);
}

/// B3 淘汰:超过 memory_max_entries 时把最低分条目置 selected=0(只归档不删除),
/// prune 硬删除归档条目;容量 0 = 不淘汰
#[test]
fn evict_archives_lowest_score_and_prune_removes() {
    let (_dir, memory, _) = service();
    // 先落 5 条(未接设置 → 默认容量 200,不淘汰),再压低容量触发淘汰
    let ids: Vec<i64> = (0..5)
        .map(|i| {
            memory
                .insert("cap", None, "manual", &format!("容量测试记忆{i}"))
                .unwrap()
                .id
        })
        .collect();
    // 前两条 usage 更高(第 3..5 条最低分被归档)
    memory.touch(&ids[0..2]).unwrap();
    let settings = Arc::new(Mutex::new(RuntimeSettings::from_config(
        &crate::config::AppConfig::from_env(),
    )));
    settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .memory_max_entries = 3;
    memory.attach_settings(settings);
    let archived = memory.evict_over_capacity("cap").unwrap();
    assert_eq!(archived, 2, "超容量 2 条应被归档");

    let all = memory.list("cap");
    assert_eq!(all.len(), 5, "淘汰只归档不删除: {all:?}");
    let selected: Vec<i64> = all.iter().filter(|e| e.selected).map(|e| e.id).collect();
    assert_eq!(selected.len(), 3, "选中条目应压到容量上限");
    assert!(
        selected.contains(&ids[0]) && selected.contains(&ids[1]),
        "高使用条目应保留: selected={selected:?} ids={ids:?}"
    );

    // 幂等:再次淘汰无变化
    assert_eq!(memory.evict_over_capacity("cap").unwrap(), 0);

    // prune:硬删除归档条目,返回条数;再次 prune 幂等为 0
    assert_eq!(memory.prune("cap").unwrap(), 2);
    assert_eq!(memory.list("cap").len(), 3);
    assert_eq!(memory.prune("cap").unwrap(), 0);

    // 容量 0 = 不淘汰
    let settings0 = Arc::new(Mutex::new(RuntimeSettings::from_config(
        &crate::config::AppConfig::from_env(),
    )));
    settings0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .memory_max_entries = 0;
    let (_dir0, memory0, _) = service();
    memory0.attach_settings(settings0);
    for i in 0..6 {
        memory0
            .insert("nocap", None, "manual", &format!("不淘汰记忆{i}"))
            .unwrap();
    }
    assert_eq!(
        memory0.list("nocap").iter().filter(|e| e.selected).count(),
        6,
        "容量 0 应不淘汰"
    );
}

/// B4 精确去重:同角色相同 content(trim 后)不新建,usage_count+1 并返回已有条目;
/// 不同角色 / 不同内容照常新建
#[test]
fn insert_dedups_exact_content() {
    let (_dir, memory, _) = service();
    let first = memory
        .insert("d1", None, "manual", "  用户喜欢薄荷茶  ")
        .unwrap();
    assert_eq!(first.usage_count, 0);
    let again = memory
        .insert("d1", Some("s9"), "tool", "用户喜欢薄荷茶")
        .unwrap();
    assert_eq!(again.id, first.id, "相同 content 应复用已有条目");
    assert_eq!(again.usage_count, 1, "去重应 usage_count+1");
    assert_eq!(memory.list("d1").len(), 1, "不应新建第二条");

    // 不同角色同内容:各建一条
    memory
        .insert("d2", None, "manual", "用户喜欢薄荷茶")
        .unwrap();
    assert_eq!(memory.list("d2").len(), 1);

    // 不同内容:新建
    memory.insert("d1", None, "manual", "用户喜欢咖啡").unwrap();
    assert_eq!(memory.list("d1").len(), 2);
}

/// B4 近似去重:蒸馏输出与已有记忆 Jaccard ≥ 0.8 跳过;
/// 同一批内近义行也只落一条;不相似行照常落库
#[tokio::test]
async fn distill_skips_near_duplicates_by_jaccard() {
    let (dir, memory, sessions) = service();
    seed_session(&sessions, &dir, "charJ", "sj");
    sessions
        .add_message("sj", "user", "聊聊记忆", json!({}))
        .unwrap();
    // 已有记忆:与第一条蒸馏行高度相似
    memory
        .insert("charJ", None, "manual", "用户与角色在图书馆初识")
        .unwrap();

    let outcome = memory
        .distill_session(&sessions, "sj", |_| async {
            // 第 1 行与已有记忆仅差尾字(Jaccard ≈ 0.91 ≥ 0.8)→ 跳过
            Ok("用户与角色在图书馆初识了\n角色害怕打雷".into())
        })
        .await
        .unwrap();
    assert_eq!(outcome.inserted, 1, "近似重复应跳过,仅落 1 条");
    assert_eq!(outcome.skipped, 1);
    let contents: Vec<String> = memory
        .list("charJ")
        .iter()
        .map(|e| e.content.clone())
        .collect();
    assert!(contents.iter().any(|c| c == "角色害怕打雷"));
    assert!(
        !contents.iter().any(|c| c.contains("初识了")),
        "近似重复不应落库: {contents:?}"
    );
}

/// 相似度工具:中文 bigram + ASCII 整词;Jaccard 对称、空集为 0
#[test]
fn similarity_tokens_and_jaccard() {
    let a = tokenize_for_similarity("用户喜欢薄荷茶");
    let b = tokenize_for_similarity("用户喜欢薄荷茶");
    assert_eq!(jaccard_similarity(&a, &b), 1.0, "同串相似度应为 1");
    let c = tokenize_for_similarity("角色害怕打雷");
    assert_eq!(jaccard_similarity(&a, &c), 0.0, "无关串相似度应为 0");
    // ASCII 整词 + 大小写归一
    let d = tokenize_for_similarity("Hello World");
    assert!(d.contains("hello") && d.contains("world"));
    assert_eq!(
        jaccard_similarity(&d, &tokenize_for_similarity("hello world")),
        1.0
    );
    // 空集
    assert_eq!(jaccard_similarity(&HashSet::new(), &d), 0.0);
    assert!(tokenize_for_similarity("   ").is_empty());
}

/// B2 召回(纯函数):按相关性降序、selected 过滤、exclude 排除通道 1、
/// 上限截断、零相似不召回、排序确定性
#[test]
fn select_recall_ranks_by_relevance_and_excludes() {
    let mut e1 = entry(1, 0, None, true);
    e1.content = "用户与角色在图书馆初识".into();
    let mut e2 = entry(2, 0, None, true);
    e2.content = "用户喜欢薄荷茶".into();
    let mut e3 = entry(3, 0, None, true);
    e3.content = "角色害怕打雷".into();
    let mut e4 = entry(4, 0, None, false);
    e4.content = "用户与角色在图书馆初识".into(); // 未选中,不参与
    let entries = vec![e1, e2, e3, e4];

    let recalled = select_recall(&entries, "还记得图书馆的事吗", 3, &[]);
    assert_eq!(recalled.len(), 1, "只有 e1 相关: {recalled:?}");
    assert_eq!(recalled[0].id, 1);
    // selected=0 不召回
    assert!(
        select_recall(&entries, "图书馆", 3, &[1]).is_empty(),
        "exclude 后应无命中"
    );
    // limit=0 与空查询
    assert!(select_recall(&entries, "图书馆", 0, &[]).is_empty());
    assert!(select_recall(&entries, "   ", 3, &[]).is_empty());
    // 确定性:两次调用顺序一致
    let a = select_recall(&entries, "图书馆 薄荷茶 打雷", 3, &[]);
    let b = select_recall(&entries, "图书馆 薄荷茶 打雷", 3, &[]);
    assert_eq!(
        a.iter().map(|e| e.id).collect::<Vec<_>>(),
        b.iter().map(|e| e.id).collect::<Vec<_>>()
    );
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
