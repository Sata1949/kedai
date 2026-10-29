// config 单元测试(DB-7 双库检测)。`*_tests.rs` 不计 check-file-size 的 ratchet。
#![cfg(test)]

use super::*;
use crate::utils::test_support::TempDataDir;

/// 在一个临时目录里造「有用户数据」的库:建 kedai.db 并写入一条会话行。
/// 用最小 schema 而非整库建表——`dir_has_user_data` 只查 `sessions` 是否存在行。
fn make_data_dir(tag: &str) -> TempDataDir {
    let dir = TempDataDir::new(tag);
    let db = dir.join("kedai.db");
    let conn = Connection::open(&db).expect("建测试库失败");
    conn.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY);")
        .expect("建 sessions 表失败");
    conn.execute("INSERT INTO sessions (id) VALUES ('s1')", [])
        .expect("插会话行失败");
    dir
}

/// DB-7:选中目录之外还存在一套**有用户数据**的库时必须被报出来。
/// 这是「聊天记录消失」的可见性缺口——用户不会知道另一套库在哪。
#[test]
fn detects_other_data_dir_with_user_data() {
    let selected = TempDataDir::new("cfg-sel");
    let other = make_data_dir("cfg-other");
    let found = detect_split_data_dirs(selected.path(), &[other.path().to_path_buf()]);
    assert_eq!(found.len(), 1, "应检测到一套分叉库: {found:?}");
    assert_eq!(found[0].0, other.path());
    assert_eq!(found[0].1, "数据库（含角色或会话）");
}

/// 选中目录自己出现在候选列表里时不得自报分叉(否则每次启动都误报)。
#[test]
fn does_not_report_selected_dir_itself() {
    let selected = make_data_dir("cfg-self");
    let found = detect_split_data_dirs(selected.path(), &[selected.path().to_path_buf()]);
    assert!(found.is_empty(), "自身不得算分叉: {found:?}");
}

/// 空壳目录(曾启动过但没真正用过)不算分叉——与 `dir_has_user_data` 同判据。
#[test]
fn ignores_empty_shell_dir() {
    let selected = TempDataDir::new("cfg-sel2");
    let shell = TempDataDir::new("cfg-shell");
    // 只有空库文件、没有 settings.json、没有会话行
    Connection::open(shell.join("kedai.db")).expect("建空库失败");
    let found = detect_split_data_dirs(selected.path(), &[shell.path().to_path_buf()]);
    assert!(found.is_empty(), "空壳目录不得误报: {found:?}");
}

/// 有 settings.json 但无库也算有用户数据(存过设置即生成该文件)。
#[test]
fn settings_json_alone_counts_as_user_data() {
    let selected = TempDataDir::new("cfg-sel3");
    let other = TempDataDir::new("cfg-settings-only");
    std::fs::write(other.join("settings.json"), "{}").expect("写 settings.json 失败");
    let found = detect_split_data_dirs(selected.path(), &[other.path().to_path_buf()]);
    assert_eq!(found.len(), 1, "有 settings.json 应算用户数据: {found:?}");
    assert_eq!(found[0].1, "settings.json + 数据库");
}

/// 不存在的目录不报(候选列表可能包含未创建的默认位置)。
#[test]
fn ignores_missing_dir() {
    let selected = TempDataDir::new("cfg-sel4");
    let missing = selected.path().join("not-there");
    let found = detect_split_data_dirs(selected.path(), &[missing]);
    assert!(found.is_empty(), "不存在的目录不得报: {found:?}");
}
