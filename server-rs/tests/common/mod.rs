//! 集成测试共用夹具(2026-10-25 C-2E)。
//!
//! 背景:此前 73 个测试文件各自复制 `test_app` / `send_json` / `test_lock` /
//! `wait_terminal`,合计约 1,700 行重复,改一处夹具约定要改几十处。C-2E 把它们
//! 收敛到本模块;每个测试文件仍以 `mod common;` **编进各自独立的测试二进制**,
//! 因此进程级 `OnceLock` app、每个二进制一份串行锁的语义与拆分前完全一致。
//!
//! 两处历史策略的变更:
//! - 旧注释担心 `tests/common` 会让 count-tests 的集成文件数虚高——`count-tests.mjs`
//!   现已把本目录排除在测试文件计数之外;
//! - 多会话并行改测试的争用点由「复制 N 份」换成「共享 1 份」,是有意的取舍。
#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use kedai_server::services::settings_service::RuntimeSettings;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

/// 测试进程级:把运行时主提示词目录指向空目录,避免真实项目根 `AGENTS_RUNTIME.md`
/// 混入 mock 断言(与 `build_test_app` 临时数据目录的隔离语义一致)。
fn runtime_prompt_isolation() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // uuid 唯一名(避免多测试二进制并发共用同名目录);守卫随本闭包析构即回收。
        // 目录是否存在不影响语义:`with_dir` 关闭内置默认回退,读不到文件即视为
        // 「无提示词」,与「指向一个空目录」等价(全仓无测试写该文件)。
        let dir = kedai_server::utils::test_support::TempDataDir::new("test-runtime-prompt-empty");
        std::env::set_var("KEDAI_RUNTIME_PROMPT_DIR", dir.path());
    });
}

/// 默认测试 app(mock 连接器 + 进程级临时数据目录 + 免鉴权,由 `build_test_app` 装配)。
pub fn test_app() -> &'static Router {
    static APP: OnceLock<Router> = OnceLock::new();
    APP.get_or_init(|| kedai_server::build_test_app().expect("构建测试应用失败"))
}

/// 需要在 app 初始化前强制 `CONNECTOR=mock` 的测试(脚本/导入等会先读环境变量的路径)。
pub fn test_app_mock() -> &'static Router {
    static APP: OnceLock<Router> = OnceLock::new();
    APP.get_or_init(|| {
        std::env::set_var("CONNECTOR", "mock");
        kedai_server::build_test_app().expect("构建测试应用失败")
    })
}

/// 需要隔离运行时主提示词的测试(避免仓库根 `AGENTS_RUNTIME.md` 进入 system 断言)。
pub fn test_app_with_prompt_isolation() -> &'static Router {
    static APP: OnceLock<Router> = OnceLock::new();
    runtime_prompt_isolation();
    APP.get_or_init(|| kedai_server::build_test_app().expect("构建测试应用失败"))
}

/// 需要**低于设置页下限**的预算来触发收尾路径时使用:构建独立 app,并在路由装配前
/// 直接改写内存设置(绕过 `PUT /api/settings` 的产品校验)。
///
/// 与 `test_app()` 的进程级单例不同,本函数每次调用新建 app 并清空临时数据目录
/// (同一测试二进制内只应有它一个 app);泄漏 Router 仅为让调用点保持 `&'static Router`
/// 与既有夹具同形,泄漏量是每进程一个。
pub fn test_app_with_settings(configure: impl FnOnce(&mut RuntimeSettings)) -> &'static Router {
    let app = kedai_server::build_test_app_configured(|state| {
        let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        configure(&mut settings);
    })
    .expect("构建测试应用失败");
    Box::leak(Box::new(app))
}

/// JSON 请求 + JSON 响应(响应非法 JSON 时按 `Value::Null` 处理,沿用原夹具语义)。
pub async fn send_json(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// 同上,但把原始响应体一并返回(少数要断言非 JSON / 纯文本回执的用例)。
pub async fn send_json_with_text(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

/// 本测试二进制内的串行锁(改全局设置 / 流程库 / 插件目录的用例必须持锁)。
fn lock_cell() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub async fn test_lock() -> MutexGuard<'static, ()> {
    lock_cell().lock().await
}

/// 持锁前顺带做运行时提示词隔离(少数 API 测试的历史语义)。
pub async fn test_lock_with_prompt_isolation() -> MutexGuard<'static, ()> {
    runtime_prompt_isolation();
    lock_cell().lock().await
}

/// 需要把锁作为引用交给他处 `lock()` 包装的用例。
pub fn test_lock_ref() -> &'static Mutex<()> {
    lock_cell()
}

/// 轮询任务直到终态;50 次 × 100ms(既有文件的主流超时)。
pub async fn wait_terminal(app: &Router, id: &str) -> (String, Value) {
    wait_terminal_with_attempts(app, id, 50).await
}

/// 指定轮询次数(每次 100ms);保留各任务用例历史上的超时差异。
pub async fn wait_terminal_with_attempts(
    app: &Router,
    id: &str,
    attempts: usize,
) -> (String, Value) {
    for _ in 0..attempts {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let st = json["task"]["status"].as_str().unwrap_or("").to_string();
        if matches!(st.as_str(), "done" | "partial" | "error" | "ended") {
            return (st, json);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("任务 {id} 未在超时内到达终态");
}

/// 以墙钟秒数轮询(少数长任务用例用 deadline 而不是固定次数)。
pub async fn wait_terminal_within_secs(app: &Router, id: &str, secs: u64) -> (String, Value) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        let (status, json) = send_json(app, "GET", &format!("/api/tasks/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let st = json["task"]["status"].as_str().unwrap_or("").to_string();
        if matches!(st.as_str(), "done" | "partial" | "error" | "ended") {
            return (st, json);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "任务 {id} 未在 {secs}s 内收敛: {json}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
