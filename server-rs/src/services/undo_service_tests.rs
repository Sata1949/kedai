// undo_service 单元测试(UNDOFIX2 批收口:自 undo_service.rs 迁出)。
// `*_tests.rs` 不计 check-file-size 的 ratchet,生产文件得以回落基线以内。
// 测试内容与原内联模块逐字一致;两个新增用例见文内 UNDOFIX2-L3/L4 注。
#![cfg(test)]

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

/// UNDOFIX2-L4:预检的 P3 面——污染 payload 必须在预检阶段拒绝,而不是等到
/// apply 中途炸开。四个形态:
/// ① `before.role` 不在 `messages.role` 的 CHECK 白名单(`user/assistant/system`)时,
///    INSERT 会违反 CHECK → 恢复中途失败且重试仍失败;
/// ② 文件项 `existed` 缺失时,`{entry_type:"file", old:null}` 会被当「删除」静默删文件;
/// ③ 文件项 `old` 字符串没有长度上限,预检放行超大 payload。
#[test]
fn precheck_rejects_polluted_payload_shapes() {
    let dir = TempDataDir::new("undo-precheck-pollution");
    let (db, svc, sid) = build_service(&dir);
    let long_old = "x".repeat(MAX_SNAPSHOT_FILE_BYTES + 1);
    let cases: Vec<(&str, Value)> = vec![
        (
            "before.role 不在 messages.role 白名单",
            json!({"kind":"replace","files":[],"bubbles":[{"id":10,"before":{"id":10,"role":"tool","content":"x","extra":{},"created_at":"t"}}]}),
        ),
        (
            "文件项缺 existed",
            json!({"kind":"files","entries":[{"path":"a.md","entry_type":"file","old":null,"truncated":false}]}),
        ),
        (
            "文件项 old 超过 256KB",
            json!({"kind":"files","entries":[{"path":"a.md","entry_type":"file","existed":true,"old":long_old,"truncated":false}]}),
        ),
        (
            "替换段文件项缺 existed",
            json!({"kind":"replace","files":[{"path":"b.md","entry_type":"file","old":null,"truncated":false}],"bubbles":[]}),
        ),
    ];
    for (i, (tag, payload)) in cases.iter().enumerate() {
        let snap_id = format!("snap-bad-shape-{i}");
        {
            let conn = db.write();
            conn.execute(
                "INSERT INTO undo_snapshots (id, session_id, tool_name, label, payload, created_at, seq) VALUES (?1, ?2, 'replace', '污染负载', ?3, ?4, ?5)",
                params![snap_id, sid, payload.to_string(), now_iso(), 200 + i as i64],
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
        assert!(
            svc.session_of(&snap_id).is_some(),
            "{tag}: 预检拒绝不得删除快照"
        );
    }
}

/// UNDOFIX2-L3:文件快照的 256KB 上限必须在**读入正文之前**判定。
/// 旧实现先 `read_to_string` 再比长度,超大文件的峰值内存是文件本身大小。
#[test]
fn file_entry_checks_size_before_reading() {
    use std::io::Write;
    let dir = TempDataDir::new("undo-file-entry-size");
    let (_db, svc, _sid) = build_service(&dir);
    let root = svc.file_root("c1");
    std::fs::create_dir_all(&root).expect("建角色文件区失败");
    let big = root.join("big.txt");
    let mut f = std::fs::File::create(&big).expect("建大文件失败");
    f.write_all(&vec![b'x'; MAX_SNAPSHOT_FILE_BYTES + 1])
        .expect("写大文件失败");
    drop(f);

    let entry = svc.file_entry("c1", "big.txt", "file");
    assert_eq!(
        entry.get("truncated"),
        Some(&Value::Bool(true)),
        "超上限文件必须标 truncated,实际: {entry}"
    );
    assert_eq!(
        entry.get("old"),
        Some(&Value::Null),
        "超上限文件不得把正文放进快照,实际: {entry}"
    );

    // 边界:恰好等于上限仍应完整快照(上限语义是 ≤)
    let edge = root.join("edge.txt");
    std::fs::write(&edge, vec![b'y'; MAX_SNAPSHOT_FILE_BYTES]).expect("写边界文件失败");
    let edge_entry = svc.file_entry("c1", "edge.txt", "file");
    assert_eq!(
        edge_entry.get("truncated"),
        Some(&Value::Bool(false)),
        "恰好 256KB 应可快照,实际: {edge_entry}"
    );
    assert!(
        edge_entry.get("old").and_then(|v| v.as_str()).is_some(),
        "恰好 256KB 的正文应在快照内"
    );
}

fn memory_ctx(sid: &str) -> ToolContext {
    ToolContext {
        session_id: sid.to_string(),
        character_id: "c1".to_string(),
        agent_depth: 0,
        scope: None,
        budget: None,
    }
}

/// P1 回归(2026-10-17 复审):记忆快照必须绑定 memory_write 回传的真实行 id。
/// 旧实现按「本角色 max(id) 之后的第一行」猜——窗口期另一会话先插入时会绑错行,
/// 回退将删除无关记忆。
#[test]
fn memory_snapshot_binds_tool_output_id_not_guessed_row() {
    let dir = TempDataDir::new("undo-memory-bind");
    let (db, svc, sid) = build_service(&dir);
    let stager = svc
        .snapshot_before(
            "memory_write",
            &json!({ "fact": "目标记忆" }),
            &memory_ctx(&sid),
            None,
        )
        .expect("应产生 memory_write 快照");

    // 窗口期另一会话先写入(id 更小),旧实现会把它当成自己的插入行。
    let concurrent = svc
        .memory
        .insert("c1", None, "manual", "并发记忆")
        .expect("并发记忆应写入");
    let target = svc
        .memory
        .insert("c1", Some(&sid), "tool", "目标记忆")
        .expect("目标记忆应写入");
    assert!(concurrent.id < target.id, "前提:并发行 id 更小");

    let snap_id = stager
        .commit(
            &json!({
                "ok": true, "stored": "目标记忆",
                "id": target.id, "created": true,
            })
            .to_string(),
        )
        .expect("快照应落库");
    let payload: String = db
        .read()
        .unwrap()
        .query_row(
            "SELECT payload FROM undo_snapshots WHERE id = ?1",
            params![snap_id],
            |r| r.get(0),
        )
        .unwrap();
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(
        payload["inserted_id"],
        json!(target.id),
        "必须绑定工具回传的真实 id,不能按 max(id) 猜"
    );
}

/// P1 回归:去重命中时 memory_write 没有插入新行,不得产快照。
#[test]
fn memory_snapshot_skips_when_dedup_created_false() {
    let dir = TempDataDir::new("undo-memory-dedup");
    let (db, svc, sid) = build_service(&dir);
    let existing = svc
        .memory
        .insert("c1", None, "manual", "重复记忆")
        .expect("种子记忆应写入");
    let stager = svc
        .snapshot_before(
            "memory_write",
            &json!({ "fact": "重复记忆" }),
            &memory_ctx(&sid),
            None,
        )
        .expect("应产生 memory_write 快照");
    // 窗口期另一条记忆落地;旧实现会把它当成「自己插入的行」。
    let concurrent = svc
        .memory
        .insert("c1", None, "manual", "窗口期并发记忆")
        .expect("并发记忆应写入");
    assert!(concurrent.id > existing.id, "前提:并发行 id 更大");
    // 模拟工具命中去重:没有新行,created=false。
    let snap_id = stager.commit(
        &json!({
            "ok": true, "stored": "重复记忆",
            "id": existing.id, "created": false,
        })
        .to_string(),
    );
    assert!(snap_id.is_none(), "去重命中不得产快照");
    let count: i64 = db
        .read()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM undo_snapshots WHERE tool_name = 'memory_write'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "去重命中不得留下快照行");
}

/// P2 回归:气泡删除失败必须上抛并保留快照,不得「报成功 + 清恢复依据」。
#[test]
fn restore_propagates_bubble_delete_error_and_keeps_snapshot() {
    let dir = TempDataDir::new("undo-bubble-delete-error");
    let (db, svc, sid) = build_service(&dir);
    let msg = svc
        .sessions
        .add_message(&sid, "assistant", "待删除", json!({}))
        .expect("消息应写入");
    {
        let conn = db.write();
        conn.execute(
            "INSERT INTO undo_snapshots (id, session_id, tool_name, label, payload, created_at, seq) VALUES ('snap-bubble-del', ?1, 'write', '写入对话气泡', ?2, ?3, 10)",
            params![
                sid,
                json!({ "kind": "bubble_insert", "message_id": msg.id }).to_string(),
                now_iso()
            ],
        )
        .unwrap();
        conn.execute_batch(&format!(
            "CREATE TRIGGER block_msg_delete BEFORE DELETE ON messages \
             WHEN OLD.id = {} BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
            msg.id
        ))
        .unwrap();
    }
    let err = svc
        .restore("snap-bubble-del")
        .expect_err("删除失败必须上抛");
    assert!(err.contains("气泡"), "错误应说明气泡删除失败: {err}");
    assert!(
        svc.session_of("snap-bubble-del").is_some(),
        "失败必须保留快照"
    );
    assert!(
        svc.sessions.get_message(&sid, msg.id).is_some(),
        "消息应仍在"
    );
}

/// P2 回归:记忆删除失败同样必须上抛并保留快照。
#[test]
fn restore_propagates_memory_delete_error_and_keeps_snapshot() {
    let dir = TempDataDir::new("undo-memory-delete-error");
    let (db, svc, sid) = build_service(&dir);
    let mem = svc
        .memory
        .insert("c1", None, "manual", "待删除记忆")
        .expect("记忆应写入");
    {
        let conn = db.write();
        conn.execute(
            "INSERT INTO undo_snapshots (id, session_id, tool_name, label, payload, created_at, seq) VALUES ('snap-mem-del', ?1, 'memory_write', '写入记忆', ?2, ?3, 11)",
            params![
                sid,
                json!({ "kind": "memory_insert", "inserted_id": mem.id }).to_string(),
                now_iso()
            ],
        )
        .unwrap();
        conn.execute_batch(&format!(
            "CREATE TRIGGER block_mem_delete BEFORE DELETE ON memory_entries \
             WHEN OLD.id = {} BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
            mem.id
        ))
        .unwrap();
    }
    let err = svc.restore("snap-mem-del").expect_err("删除失败必须上抛");
    assert!(err.contains("记忆"), "错误应说明记忆删除失败: {err}");
    assert!(svc.session_of("snap-mem-del").is_some(), "失败必须保留快照");
    assert!(svc.memory.get(mem.id).is_some(), "记忆应仍在");
}
