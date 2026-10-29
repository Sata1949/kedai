// API 集成测试 · IMPHUB 批新增端点(2026-10-09):
//   全局正则库 GET/PUT /api/regex-scripts(⑥)
//   插件批量导入/导出/移动 POST /api/plugins/tools/{import,move} + GET /api/plugins/tools/export(⑦)
//   流程移动 POST /api/agent-flows/move
//   综合导出 GET /api/export/bundle?mode=(bundle v1 信封与模式取段)
//
// 与既有集成测试同款:mock 连接器 + 进程级临时数据目录 + 免鉴权;
// 测试辅助函数已收敛到 `tests/common`(2026-10-25 C-2E);每个测试文件仍是独立测试二进制。
// 本文件内的用例共享同一个 app 与数据目录,故**改目录/改库的用例必须 `test_lock()` 串行**
// (与 api_misc_assets.rs 同款文件内串行锁)。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::sync::MutexGuard;
use tower::ServiceExt;
mod common;
use common::{send_json, test_app, test_lock_ref as test_lock};

async fn lock() -> MutexGuard<'static, ()> {
    test_lock().lock().await
}

/// 原始 body 文本(综合导出要断言**键序**,解析成 Value 会丢序:serde_json 的 Map 是 BTreeMap)
async fn send_raw(app: &axum::Router, method: &str, path: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

// ==================== ⑥ 全局正则库 ====================

/// 整体读写 + 宽进口径:canonical(snake_case)/酒馆(camelCase)两种键名都收,
/// 取不到查找模式的条目跳过并计数;`enabled:false` 必须往返保住(不得被归一复活)
#[tokio::test]
async fn regex_scripts_roundtrip_and_lenient_import() {
    let app = test_app();
    let _guard = lock().await;

    let (status, body) = send_json(app, "GET", "/api/regex-scripts", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["scripts"], json!([]), "初始库应为空: {body}");

    let (status, body) = send_json(
        app,
        "PUT",
        "/api/regex-scripts",
        json!({ "scripts": [
            { "id": "g1", "script_name": "停用条目", "find_regex": "/<P\\/>/g",
              "replace_string": "<b>x</b>", "markdown_only": true, "enabled": false },
            { "id": "g2", "scriptName": "酒馆形态", "findRegex": "/A/g",
              "replaceString": "B", "markdownOnly": true, "disabled": false },
            { "id": "bad", "replace_string": "只有替换体" }
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 2);
    assert_eq!(body["skipped"], 1, "缺查找模式的条目应跳过并计数: {body}");

    let (status, body) = send_json(app, "GET", "/api/regex-scripts", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let scripts = body["scripts"].as_array().unwrap();
    assert_eq!(scripts.len(), 2);
    assert_eq!(scripts[0]["script_name"], "停用条目");
    assert_eq!(
        scripts[0]["enabled"], false,
        "停用条目往返后不得复活: {body}"
    );
    assert_eq!(
        scripts[1]["find_regex"], "/A/g",
        "camelCase 应被归一: {body}"
    );
    assert_eq!(scripts[1]["script_name"], "酒馆形态");

    // 裸数组形态同样收(手改文件/旧导入件的宽容度)
    let (status, body) = send_json(
        app,
        "PUT",
        "/api/regex-scripts",
        json!([{ "id": "solo", "find_regex": "A", "replace_string": "B" }]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 1);
}

// ==================== ⑦ 插件批量导入 / 移动 / 导出 ====================

fn plugin_entry(file: &str, folder: &str, name: &str) -> Value {
    json!({
        "file": file,
        "folder": folder,
        "name": name,
        "description": format!("{name} 描述"),
        "parameters": { "type": "object", "properties": { "x": { "type": "string" } } },
        "script": "return args.x",
    })
}

/// 批量导入 → 注册可见 → 导出回读 → 移动分组(含移回未分组)→ 404/400 边界
#[tokio::test]
async fn plugin_import_move_export_roundtrip() {
    let app = test_app();
    let _guard = lock().await;

    let (status, body) = send_json(
        app,
        "POST",
        "/api/plugins/tools/import",
        json!({ "plugins": [
            plugin_entry("imphub_a.json", "工具/天气", "imphub_tool_a"),
            plugin_entry("imphub_b.json", "", "imphub_tool_b")
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["imported"], 2);
    assert_eq!(body["loaded"], 2, "两个插件都应注册进注册表: {body}");

    // 列表:folder 随条目返回(管理面分组的数据源)
    let (status, body) = send_json(app, "GET", "/api/plugins/tools", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let tools = body["tools"].as_array().unwrap();
    let a = tools.iter().find(|t| t["file"] == "imphub_a.json").unwrap();
    assert_eq!(a["name"], "imphub_tool_a");
    assert_eq!(a["folder"], "工具/天气");
    let b = tools.iter().find(|t| t["file"] == "imphub_b.json").unwrap();
    assert_eq!(b["folder"], "", "空 folder = 未分组");

    // 导出:文件名 + folder + 配置本体(综合导出⑦ 的数据源)
    let (status, body) = send_json(app, "GET", "/api/plugins/tools/export", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let exported = body["plugins"].as_array().unwrap();
    let a = exported
        .iter()
        .find(|p| p["file"] == "imphub_a.json")
        .expect("导出应含 imphub_a.json");
    assert_eq!(a["folder"], "工具/天气");
    assert_eq!(a["script"], "return args.x");
    assert!(
        a["parameters"]["type"].is_string(),
        "参数 schema 应随包: {body}"
    );

    // 移动:路径规整(书写差异不在文件里裂成两份)
    let (status, body) = send_json(
        app,
        "POST",
        "/api/plugins/tools/move",
        json!({ "file": "imphub_a.json", "folder": "/工具//其他/" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["folder"], "工具/其他");
    let (_, body) = send_json(app, "GET", "/api/plugins/tools/export", json!({})).await;
    let a = body["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["file"] == "imphub_a.json")
        .unwrap();
    assert_eq!(a["folder"], "工具/其他", "移动应落盘: {body}");

    // 移回未分组:空串删键
    let (status, _) = send_json(
        app,
        "POST",
        "/api/plugins/tools/move",
        json!({ "file": "imphub_a.json", "folder": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send_json(app, "GET", "/api/plugins/tools/export", json!({})).await;
    let a = body["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["file"] == "imphub_a.json")
        .unwrap();
    assert_eq!(a["folder"], "", "{body}");

    // 移动不存在的文件 → 404
    let (status, _) = send_json(
        app,
        "POST",
        "/api/plugins/tools/move",
        json!({ "file": "nope.json", "folder": "A" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 非法文件名 → 400(净化口径与上传/删除同一把)
    let (status, _) = send_json(
        app,
        "POST",
        "/api/plugins/tools/move",
        json!({ "file": "../evil.json", "folder": "A" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 同名文件覆盖导入(第二轮):文件不追加、内容按新值落盘
    let (status, body) = send_json(
        app,
        "POST",
        "/api/plugins/tools/import",
        json!({ "plugins": [
            plugin_entry("imphub_b.json", "覆盖组", "imphub_tool_b")
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["imported"], 1);
    let (_, body) = send_json(app, "GET", "/api/plugins/tools/export", json!({})).await;
    let matches: Vec<&Value> = body["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["file"] == "imphub_b.json")
        .collect();
    assert_eq!(matches.len(), 1, "同名覆盖不得留下两份: {body}");
    assert_eq!(matches[0]["folder"], "覆盖组", "覆盖应生效: {body}");
}

/// 批量导入的两条拒绝路径(**全部通过才落盘**):
/// ① 任一项无效 → 400 且同批的合法项也不落盘;② 与内置工具重名 → 400(不劫持,PLGM 语义不回归)
#[tokio::test]
async fn plugin_import_rejects_batch_atomically() {
    let app = test_app();
    let _guard = lock().await;

    // ① 本批第二项 script 为空 → 整批拒绝,第一项不得落盘
    let (status, body) = send_json(
        app,
        "POST",
        "/api/plugins/tools/import",
        json!({ "plugins": [
            plugin_entry("imphub_c.json", "", "imphub_tool_c"),
            { "file": "imphub_d.json", "name": "imphub_tool_d", "description": "", "script": "" }
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("2"), "{body}");
    let (_, body) = send_json(app, "GET", "/api/plugins/tools/export", json!({})).await;
    assert!(
        !body["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["file"] == "imphub_c.json"),
        "整批拒绝时同批合法项也不得落盘: {body}"
    );

    // ② 与内置工具重名:注册失败如实回 400(插件不得劫持内置)
    let (status, body) = send_json(
        app,
        "POST",
        "/api/plugins/tools/import",
        json!({ "plugins": [
            { "file": "imphub_read.json", "name": "read", "description": "劫持",
              "parameters": {}, "script": "return args.x" }
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("内置"), "{body}");
    // IMPHUB-L4:失败路径**不得留下未注册的落盘文件**。旧实现阶段 1 只校验 JSON 形状,
    // 重名判定在阶段 3(sync_plugins)才做,文件已先落盘 —— 目录里留下无人注册的
    // imphub_read.json,重启/重载后仍会被加载并再次拒绝。
    let (_, body) = send_json(app, "GET", "/api/plugins/tools/export", json!({})).await;
    assert!(
        !body["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["file"] == "imphub_read.json"),
        "重名被拒后不得留下落盘文件: {body}"
    );
}

// ==================== 流程 move ====================

/// 移动流程到文件夹:**不动 current_flow_id**(整理分组不是选择动作),路径规整入库
#[tokio::test]
async fn flow_move_keeps_current_selection() {
    let app = test_app();
    let _guard = lock().await;

    let (status, body) = send_json(app, "GET", "/api/agent-flows", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let current = body["library"]["current_flow_id"]
        .as_str()
        .expect("内置流程应已选中一个")
        .to_string();
    let target = body["library"]["flows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] != current.as_str())
        .or_else(|| body["library"]["flows"].as_array().unwrap().first())
        .map(|f| f["id"].as_str().unwrap().to_string())
        .unwrap();

    let (status, body) = send_json(
        app,
        "POST",
        "/api/agent-flows/move",
        json!({ "id": target, "folder": " /整理// 分组/ " }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["library"]["current_flow_id"].as_str(),
        Some(current.as_str()),
        "移动不得改变当前选中流程: {body}"
    );
    let moved = body["library"]["flows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == target.as_str())
        .unwrap();
    assert_eq!(moved["folder"], "整理/分组", "路径应规整入库: {body}");

    // 空 folder 移回未分组:字段整体省略(空值不落盘,旧库/旧搬运包形状不变)
    let (status, body) = send_json(
        app,
        "POST",
        "/api/agent-flows/move",
        json!({ "id": target, "folder": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let moved = body["library"]["flows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == target.as_str())
        .unwrap();
    assert!(moved.get("folder").is_none(), "空 folder 应省略: {body}");

    // 不存在的流程 → 400(错误文案点名 id)
    let (status, body) = send_json(
        app,
        "POST",
        "/api/agent-flows/move",
        json!({ "id": "no-such-flow", "folder": "A" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("不存在"), "{body}");
}

// ==================== 综合导出 bundle ====================

/// roleplay 导出:6 段齐全 + 段序固定 + 版本键 + 生成器;
/// task 导出:①⑥ 段不出现(roleplay 专属)
#[tokio::test]
async fn export_bundle_shape_by_mode() {
    let app = test_app();
    let _guard = lock().await;

    let (status, text) = send_raw(app, "GET", "/api/export/bundle?mode=roleplay").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        text.starts_with(r#"{"ok":true,"bundle":{"kedai_import_bundle":1,"#),
        "version 键应在信封开头: {text}"
    );
    assert!(text.contains(r#""app_mode":"roleplay""#), "{text}");
    assert!(
        text.contains(r#""format":"kedai_inject""#),
        "① 应为 kedai 注入形态: {text}"
    );
    let pos = |k: &str| text.find(&format!("\"{k}\"")).unwrap_or(usize::MAX);
    assert!(
        pos("preset_entries") < pos("flows"),
        "段序:①应在②前: {text}"
    );
    assert!(pos("flows") < pos("prompts"), "段序:②应在③④前: {text}");
    assert!(pos("prompts") < pos("skills"), "段序:③④应在⑤前: {text}");
    assert!(
        pos("skills") < pos("regex_scripts"),
        "段序:⑤应在⑥前: {text}"
    );
    assert!(
        pos("regex_scripts") < pos("plugins"),
        "段序:⑥应在⑦前: {text}"
    );
    assert!(
        text.contains(r#""generator":{"app":"kedai","version":""#),
        "{text}"
    );

    let (status, text) = send_raw(app, "GET", "/api/export/bundle?mode=task").await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains(r#""app_mode":"task""#), "{text}");
    assert!(
        !text.contains("preset_entries"),
        "task 导出不得含①预设条目: {text}"
    );
    assert!(
        !text.contains("regex_scripts"),
        "task 导出不得含⑥预设正则: {text}"
    );
    for key in ["flows", "prompts", "skills", "plugins"] {
        assert!(
            text.contains(&format!("\"{key}\"")),
            "task 导出应含 {key}: {text}"
        );
    }
}

/// §2.1.3 段内归一:导出件里 floors 按 order 排序并**重编为 0..n**
/// (归一收在导出侧,导入侧才能对任意来源的 floors 直接用 order 当展示序)
#[tokio::test]
async fn export_bundle_normalizes_floor_order() {
    let app = test_app();
    let _guard = lock().await;

    let (status, body) = send_json(
        app,
        "PUT",
        "/api/prompt-inject",
        json!({ "config": {
            "mode": "complex",
            "simple": {},
            "floors": [
                { "id": "b", "name": "后", "content": "B", "role": "system",
                  "position": "system", "depth": 0, "enabled": true, "order": 5 },
                { "id": "a", "name": "前", "content": "A", "role": "system",
                  "position": "system", "depth": 0, "enabled": true, "order": 1 }
            ]
        }}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, text) = send_raw(app, "GET", "/api/export/bundle?mode=roleplay").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&text).unwrap();
    let section = &v["bundle"]["sections"]["preset_entries"];
    assert_eq!(section["format"], "kedai_inject", "判别位恒 kedai 注入形态");
    let floors = section["floors"].as_array().expect("导出件应含 floors");
    assert_eq!(floors.len(), 2);
    assert_eq!(floors[0]["name"], "前", "应按 order 排序: {text}");
    assert_eq!(floors[0]["order"], 0, "order 应重编为 0..n: {text}");
    assert_eq!(floors[1]["name"], "后");
    assert_eq!(floors[1]["order"], 1);
}
