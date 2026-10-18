// 世界书 API 集成测试:上传/列表/条目预览/开关/绑定/删除
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kedai_server::build_test_app;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

fn test_app() -> &'static axum::Router {
    static APP: OnceLock<axum::Router> = OnceLock::new();
    APP.get_or_init(|| build_test_app().expect("构建测试应用失败"))
}

/// 世界书测试共享同一 app/DB,须串行执行避免列表计数断言相互干扰
async fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

async fn send_json(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
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

async fn send_empty(app: &axum::Router, method: &str, path: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    resp.status()
}

/// 上传独立世界书(multipart:file + 可选 character_id)
async fn upload_world_book(
    app: &axum::Router,
    book: &Value,
    character_id: Option<&str>,
) -> (StatusCode, Value) {
    let mut body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"世界书.json\"\r\nContent-Type: application/json\r\n\r\n{}\r\n",
        book
    );
    if let Some(cid) = character_id {
        body.push_str(&format!(
            "--BOUND\r\nContent-Disposition: form-data; name=\"character_id\"\r\n\r\n{cid}\r\n"
        ));
    }
    body.push_str("--BOUND--\r\n");
    let req = Request::builder()
        .method("POST")
        .uri("/api/world-books/upload")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

fn sample_book() -> Value {
    json!({
        "name": "日向台街区",
        "entries": [
            { "uid": 0, "comment": "地点", "keys": ["图书馆", "时雨堂"], "content": "图书馆位于日向台,安静。", "constant": false, "disable": false, "position": 0 },
            { "uid": 1, "comment": "人物", "keys": [], "content": "芽衣是兔族少女。", "constant": true, "disable": false, "position": 0 },
            { "uid": 2, "comment": "禁用", "keys": ["公园"], "content": "不应出现。", "constant": false, "disable": true, "position": 0 },
            { "uid": 3, "comment": "正则", "keys": [], "regex": "\\d{4}-\\d{2}-\\d{2}", "use_regex": true, "content": "日期触发的内容。", "constant": false, "disable": false, "position": 0 }
        ]
    })
}

#[tokio::test]
async fn world_book_crud() {
    let _guard = test_lock().await;
    let app = test_app();

    // 上传(全局,未绑定角色)
    let (status, book) = upload_world_book(app, &sample_book(), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = book["id"].as_str().unwrap().to_string();
    assert_eq!(book["name"], json!("日向台街区"));
    assert_eq!(book["source"], json!("upload"));
    assert_eq!(book["entry_count"], json!(4));
    assert_eq!(book["enabled"], json!(true));
    assert!(book.get("character_id").is_none() || book["character_id"].is_null());

    // 列表包含本次上传(共享 DB,不依赖绝对计数)
    let (_, list) = send_json(app, "GET", "/api/world-books", json!({})).await;
    let ids: Vec<&str> = list["world_books"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["id"].as_str())
        .collect();
    assert!(ids.contains(&id.as_str()), "列表应包含本次上传的世界书");
    // 列表不含 data_raw(体积小)
    let mine = list["world_books"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["id"] == id)
        .unwrap();
    assert!(mine.get("data_raw").is_none());

    // 条目预览:4 条,禁用条目保留在 data_raw 但预览含全部(过滤在注入侧)
    let (status, entries) = send_json(
        app,
        "GET",
        &format!("/api/world-books/{id}/entries"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(entries["entries"].as_array().unwrap().len(), 4);
    let first = &entries["entries"][0];
    assert_eq!(first["comment"], json!("地点"));
    assert_eq!(first["keys"][0], json!("图书馆"));
    assert!(!first["constant"].as_bool().unwrap());
    let disabled = entries["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["comment"] == "禁用")
        .unwrap();
    assert!(!disabled["enabled"].as_bool().unwrap());
    // 正则条目:regex 与 use_regex 保留
    let regex_entry = entries["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["comment"] == "正则")
        .unwrap();
    assert_eq!(regex_entry["regex"], json!("\\d{4}-\\d{2}-\\d{2}"));

    // 更新:停用
    let (status, updated) = send_json(
        app,
        "PUT",
        &format!("/api/world-books/{id}"),
        json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["enabled"], json!(false));

    // 更新:改名 + 绑定角色(id 不存在则绑定仍落库,此处仅验证字段)
    let (_, updated) = send_json(
        app,
        "PUT",
        &format!("/api/world-books/{id}"),
        json!({ "name": "街区手册", "character_id": "" }),
    )
    .await;
    assert_eq!(updated["name"], json!("街区手册"));
    assert!(updated.get("character_id").is_none() || updated["character_id"].is_null());

    // 删除
    let status = send_empty(app, "DELETE", &format!("/api/world-books/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = send_json(app, "GET", "/api/world-books", json!({})).await;
    let ids_after: Vec<&str> = list["world_books"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["id"].as_str())
        .collect();
    assert!(
        !ids_after.contains(&id.as_str()),
        "删除后列表不应包含该世界书"
    );
    // 删除后条目预览 404
    let status = send_empty(app, "GET", &format!("/api/world-books/{id}/entries")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn world_book_invalid_upload_rejected() {
    let _guard = test_lock().await;
    let app = test_app();
    // 非 JSON
    let body = "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"bad.json\"\r\n\r\nnot json\r\n--BOUND--\r\n";
    let req = Request::builder()
        .method("POST")
        .uri("/api/world-books/upload")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 合法 JSON 但无有效条目
    let (status, _) = upload_world_book(app, &json!({ "name": "空书", "entries": [] }), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn world_book_bind_to_character() {
    let _guard = test_lock().await;
    let app = test_app();
    // 上传角色
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"角色.json\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        json!({
            "spec": "chara_card_v2", "spec_version": "1.0", "name": "测试角色",
            "description": "测试", "first_mes": "你好"
        })
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/characters/upload")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let char: Value = serde_json::from_slice(&bytes).unwrap();
    let cid = char["id"].as_str().unwrap().to_string();

    // 绑定上传
    let (status, book) = upload_world_book(app, &sample_book(), Some(&cid)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(book["character_id"], json!(cid));
    assert_eq!(book["character_name"], json!("测试角色"));

    // 清除绑定 → 全局
    let id = book["id"].as_str().unwrap().to_string();
    let (_, updated) = send_json(
        app,
        "PUT",
        &format!("/api/world-books/{id}"),
        json!({ "character_id": "" }),
    )
    .await;
    assert!(updated.get("character_id").is_none() || updated["character_id"].is_null());
}

/// WB-D 编辑面:`scan_depth`(关键词扫描窗口)经 API 可读可写,且**PUT 不带该字段时不得清空已声明值**
/// (清空会把窗口打回条目 depth 兜底,静默改变世界书触发行为)。
#[tokio::test]
async fn world_book_scan_depth_roundtrip() {
    let _guard = test_lock().await;
    let app = test_app();
    let (status, book) = upload_world_book(app, &sample_book(), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = book["id"].as_str().unwrap().to_string();

    let entry = |with_window: bool| {
        let mut e = json!({
            "id": 0, "comment": "窗口", "keys": ["车站"], "regex": null, "use_regex": false,
            "constant": false, "enabled": true, "content": "车站很繁忙。", "position": 2,
            "depth": 1
        });
        if with_window {
            e.as_object_mut()
                .unwrap()
                .insert("scan_depth".into(), json!(2));
        }
        json!({ "entries": [e] })
    };

    // 写入显式窗口 → GET 回显
    let (status, saved) = send_json(
        app,
        "PUT",
        &format!("/api/world-books/{id}/entries"),
        entry(true),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let saved_entry = saved["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == json!(0))
        .expect("响应合并视图中必须包含 uid=0");
    assert_eq!(saved_entry["scan_depth"], json!(2), "PUT 响应回显窗口");

    let url = format!("/api/world-books/{id}/entries");
    let (_, got) = send_json(app, "GET", &url, json!({})).await;
    let got_entry = got["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == json!(0))
        .expect("GET 必须包含 uid=0");
    assert_eq!(
        got_entry["scan_depth"],
        json!(2),
        "视图必须承载 scan_depth,前端才有编辑入口"
    );

    // 旧客户端形态:同一条目但不带 scan_depth → 窗口保持,不被清空
    let (status, _) = send_json(app, "PUT", &url, entry(false)).await;
    assert_eq!(status, StatusCode::OK);
    let (_, got) = send_json(app, "GET", &url, json!({})).await;
    let got_entry = got["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == json!(0))
        .expect("GET 必须包含 uid=0");
    assert_eq!(
        got_entry["scan_depth"],
        json!(2),
        "PUT 不带 scan_depth 不得清空已声明窗口"
    );
}

/// 独立世界书条目编辑:PUT 叠加回写(关键词/常驻/激发/位置)+ 显式 removed 删除 → GET 验证
#[tokio::test]
async fn world_book_entries_editable() {
    let _guard = test_lock().await;
    let app = test_app();
    let (status, book) = upload_world_book(app, &sample_book(), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = book["id"].as_str().unwrap().to_string();
    let url = format!("/api/world-books/{id}/entries");

    // 回写:改关键词、开常驻、停用、改位置、改内容(省略 uid=3 → 新语义下必须保留)
    let (status, saved) = send_json(
        app,
        "PUT",
        &url,
        json!({ "entries": [
            { "id": 0, "comment": "地点", "keys": ["车站", "铁路"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "车站很繁忙。", "position": 2 },
            { "id": 1, "comment": "人物", "keys": [], "regex": null, "use_regex": false, "constant": true, "enabled": true, "content": "芽衣是兔族少女。", "position": 0 },
            { "id": 2, "comment": "禁用", "keys": ["公园"], "regex": null, "use_regex": false, "constant": false, "enabled": false, "content": "不应出现。", "position": 0 }
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["ok"], json!(true));
    assert_eq!(
        saved["entries"].as_array().unwrap().len(),
        4,
        "PUT 响应必须是合并后视图:未列出的 uid=3 仍保留"
    );
    assert!(
        saved["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == json!(3)),
        "响应必须包含窗口期/未列出条目"
    );

    let (_, entries) = send_json(app, "GET", &url, json!({})).await;
    let arr = entries["entries"].as_array().unwrap();
    assert_eq!(arr.len(), 4, "未列出条目保留:原 4 条全在");
    let loc = arr.iter().find(|e| e["comment"] == "地点").unwrap();
    assert_eq!(loc["keys"], json!(["车站", "铁路"]));
    assert_eq!(loc["position"], json!(2));
    let person = arr.iter().find(|e| e["comment"] == "人物").unwrap();
    assert!(person["constant"].as_bool().unwrap());
    let disabled = arr.iter().find(|e| e["comment"] == "禁用").unwrap();
    assert!(!disabled["enabled"].as_bool().unwrap());

    // 显式删除 uid=3:只有出现在 removed 里的条目才被删除
    let (status, saved) = send_json(
        app,
        "PUT",
        &url,
        json!({ "entries": [
            { "id": 0, "comment": "地点", "keys": ["车站", "铁路"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "车站很繁忙。", "position": 2 }
        ], "removed": [3] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        saved["entries"].as_array().unwrap().len(),
        3,
        "removed 显式删除后应剩 3 条"
    );
    let (_, entries) = send_json(app, "GET", &url, json!({})).await;
    assert_eq!(entries["entries"].as_array().unwrap().len(), 3);
    assert!(
        !entries["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == json!(3)),
        "removed 条目不得复活"
    );

    // POST 新增条目:分配新 id;空条目(无内容无关键词)按约定不参与注入,列表暂不显示
    let (status, added) = send_json(app, "POST", &url, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let new_id = added["entry"]["id"]
        .as_i64()
        .expect("新增条目应返回数字 id");
    assert!(new_id >= 3, "新 id 应递增分配,实际 {new_id}");
    let (_, entries) = send_json(app, "GET", &url, json!({})).await;
    assert_eq!(
        entries["entries"].as_array().unwrap().len(),
        3,
        "空条目被过滤,列表仍 3 条"
    );

    // 编辑该新条目(填入内容与关键词)→ 回写后生效且出现
    let (status, _) = send_json(
        app,
        "PUT",
        &url,
        json!({ "entries": [
            { "id": 0, "comment": "地点", "keys": ["车站", "铁路"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "车站很繁忙。", "position": 2 },
            { "id": 1, "comment": "人物", "keys": [], "regex": null, "use_regex": false, "constant": true, "enabled": true, "content": "芽衣是兔族少女。", "position": 0 },
            { "id": 2, "comment": "禁用", "keys": ["公园"], "regex": null, "use_regex": false, "constant": false, "enabled": false, "content": "不应出现。", "position": 0 },
            { "id": new_id, "comment": "新增", "keys": ["雨天"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "下雨天设定。", "position": 0 }
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, entries) = send_json(app, "GET", &url, json!({})).await;
    let arr = entries["entries"].as_array().unwrap();
    assert_eq!(arr.len(), 4, "编辑新条目后应为 4 条");
    assert!(arr
        .iter()
        .any(|e| e["comment"] == "新增" && e["keys"] == json!(["雨天"])));
}

/// WBCONC 核心回归:用户打开编辑器后,窗口期模型/卡脚本写入的条目不得被旧快照 PUT 删除
#[tokio::test]
async fn world_book_put_preserves_window_writes() {
    let _guard = test_lock().await;
    let app = test_app();
    let (status, book) = upload_world_book(app, &sample_book(), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = book["id"].as_str().unwrap().to_string();
    let url = format!("/api/world-books/{id}/entries");

    // 窗口期写入(模拟归档步/卡脚本):新增 uid=99
    let (status, windowed) = send_json(
        app,
        "PUT",
        &url,
        json!({ "entries": [
            { "id": 99, "comment": "窗口期新增", "keys": ["窗口"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "模型在用户编辑期间写入。", "position": 0 }
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        windowed["entries"].as_array().unwrap().len(),
        5,
        "叠加语义下窗口期新增不影响原有 4 条"
    );

    // 用户用打开时的旧快照保存(只带 uid=0,不带 removed)
    let (status, saved) = send_json(
        app,
        "PUT",
        &url,
        json!({ "entries": [
            { "id": 0, "comment": "地点", "keys": ["车站"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "用户改过的内容。", "position": 2 }
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let saved_arr = saved["entries"].as_array().unwrap();
    assert!(
        saved_arr.iter().any(|e| e["id"] == json!(99)),
        "响应合并视图必须包含窗口期新增条目"
    );
    assert!(
        saved_arr.iter().any(|e| e["comment"] == "窗口期新增"),
        "窗口期条目内容不得被旧快照覆盖"
    );

    let (_, entries) = send_json(app, "GET", &url, json!({})).await;
    let arr = entries["entries"].as_array().unwrap();
    assert_eq!(arr.len(), 5, "旧快照保存后窗口期新增仍存活");
    let loc = arr.iter().find(|e| e["id"] == json!(0)).unwrap();
    assert_eq!(loc["content"], json!("用户改过的内容。"));
    assert!(
        arr.iter().any(|e| e["id"] == json!(99)),
        "窗口期新增不得被删除"
    );
}

/// removed + added 撞号:本地新增行与窗口期模型新建同号时,服务端重分配且两者都存活
#[tokio::test]
async fn world_book_put_removed_and_added_collision() {
    let _guard = test_lock().await;
    let app = test_app();
    let (status, book) = upload_world_book(app, &sample_book(), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = book["id"].as_str().unwrap().to_string();
    let url = format!("/api/world-books/{id}/entries");

    // 窗口期:模型从 max=3 出发创建 uid=4
    let (status, _) = send_json(
        app,
        "PUT",
        &url,
        json!({ "entries": [
            { "id": 4, "comment": "模型新建", "keys": ["模型"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "模型窗口期内容。", "position": 0 }
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 用户本地新增行也从 max=3 出发得到 id=4,保存时显式 added=[4],并删除 uid=3
    let (status, saved) = send_json(
        app,
        "PUT",
        &url,
        json!({
            "entries": [
                { "id": 4, "comment": "本地新增", "keys": ["本地"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "本地新增内容。", "position": 0 }
            ],
            "removed": [3],
            "added": [4]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let arr = saved["entries"].as_array().unwrap();
    let model = arr
        .iter()
        .find(|e| e["comment"] == "模型新建")
        .expect("窗口期模型条目必须存活");
    assert_eq!(model["content"], json!("模型窗口期内容。"));
    let local = arr
        .iter()
        .find(|e| e["comment"] == "本地新增")
        .expect("本地新增条目必须落库");
    assert_eq!(local["id"], json!(5), "撞号的 added 行必须重分配到 max+1");
    assert_eq!(local["content"], json!("本地新增内容。"));
    assert!(
        !arr.iter().any(|e| e["id"] == json!(3)),
        "removed 条目必须删除"
    );
    assert_eq!(arr.len(), 5, "原 4 条 - 删除 1 + 模型 1 + 本地 1 = 5");
}

/// 角色卡内嵌数组形态:未列出条目保留、removed 删除、added 撞号重分配
#[tokio::test]
async fn character_book_put_preserves_unlisted_and_reassigns_added() {
    let _guard = test_lock().await;
    let app = test_app();
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"数组书.json\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        json!({
            "spec": "chara_card_v2", "spec_version": "1.0", "name": "数组书娘",
            "description": "测试", "first_mes": "你好",
            "character_book": {
                "name": "数组世界",
                "entries": [
                    { "id": 0, "comment": "世界观", "keys": [], "content": "兽人社会。", "constant": true, "disable": false, "position": 0 },
                    { "id": 1, "comment": "地点", "keys": ["图书馆"], "content": "图书馆安静。", "constant": false, "disable": false, "position": 0 }
                ]
            }
        })
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/characters/upload")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let char: Value = serde_json::from_slice(&bytes).unwrap();
    let cid = char["id"].as_str().unwrap().to_string();
    let url = format!("/api/characters/{cid}/world-entries");

    // 窗口期:模型从 max=1 出发新建 id=2
    let (status, _) = send_json(
        app,
        "PUT",
        &url,
        json!({ "entries": [
            { "id": 2, "comment": "模型新建", "keys": ["模型"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "窗口期内容。", "position": 0 }
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 用户旧快照只带 id=0,本地新增行 id=2(撞号)+ added,并删除 id=1
    let (status, saved) = send_json(
        app,
        "PUT",
        &url,
        json!({
            "entries": [
                { "id": 0, "comment": "世界观", "keys": [], "regex": null, "use_regex": false, "constant": true, "enabled": true, "content": "用户改过的世界观。", "position": 1 },
                { "id": 2, "comment": "本地新增", "keys": ["本地"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "本地内容。", "position": 0 }
            ],
            "removed": [1],
            "added": [2]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let arr = saved["entries"].as_array().unwrap();
    assert_eq!(arr.len(), 3, "原 2 条 - 删 1 + 模型 1 + 本地 1 = 3");
    assert!(
        !arr.iter().any(|e| e["id"] == json!(1)),
        "removed 条目必须删除"
    );
    let model = arr
        .iter()
        .find(|e| e["comment"] == "模型新建")
        .expect("窗口期模型条目必须存活");
    assert_eq!(model["content"], json!("窗口期内容。"));
    let local = arr
        .iter()
        .find(|e| e["comment"] == "本地新增")
        .expect("本地新增条目必须落库");
    assert_eq!(local["id"], json!(3), "数组形态撞号同样重分配");
}

/// 角色卡内嵌世界书:GET/PUT /api/characters/{id}/world-entries
#[tokio::test]
async fn character_card_world_entries_editable() {
    let _guard = test_lock().await;
    let app = test_app();
    let body = format!(
        "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"带世界书.json\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--BOUND--\r\n",
        json!({
            "spec": "chara_card_v2", "spec_version": "1.0", "name": "书娘",
            "description": "测试", "first_mes": "你好",
            "character_book": {
                "name": "书娘的世界",
                "entries": [
                    { "id": 0, "comment": "世界观", "keys": [], "content": "兽人社会。", "constant": true, "disable": false, "position": 0 },
                    { "id": 1, "comment": "地点", "keys": ["图书馆"], "content": "图书馆安静。", "constant": false, "disable": false, "position": 0 }
                ]
            }
        })
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/characters/upload")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let char: Value = serde_json::from_slice(&bytes).unwrap();
    let cid = char["id"].as_str().unwrap().to_string();

    // GET 内嵌条目
    let (status, entries) = send_json(
        app,
        "GET",
        &format!("/api/characters/{cid}/world-entries"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(entries["entries"].as_array().unwrap().len(), 2);
    let world = entries["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["comment"] == "世界观")
        .unwrap();
    assert!(world["constant"].as_bool().unwrap());
    assert!(world["enabled"].as_bool().unwrap());

    // PUT 回写:改关键词/位置/停用
    let (status, saved) = send_json(
        app,
        "PUT",
        &format!("/api/characters/{cid}/world-entries"),
        json!({ "entries": [
            { "id": 0, "comment": "世界观", "keys": [], "regex": null, "use_regex": false, "constant": true, "enabled": false, "content": "兽人社会。", "position": 1 },
            { "id": 1, "comment": "地点", "keys": ["书库", "书架"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "图书馆安静。", "position": 0 },
            { "id": 2, "comment": "新条目", "keys": ["雨天"], "regex": null, "use_regex": false, "constant": false, "enabled": true, "content": "下雨天不出门。", "position": 0 }
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["ok"], json!(true));

    // 回读验证(条目经 character_book_entries 重新提取,3 条)
    let (_, entries) = send_json(
        app,
        "GET",
        &format!("/api/characters/{cid}/world-entries"),
        json!({}),
    )
    .await;
    let arr = entries["entries"].as_array().unwrap();
    assert_eq!(arr.len(), 3);
    let world = arr.iter().find(|e| e["comment"] == "世界观").unwrap();
    assert_eq!(world["position"], json!(1));
    assert!(!world["enabled"].as_bool().unwrap());
    let loc = arr.iter().find(|e| e["comment"] == "地点").unwrap();
    assert_eq!(loc["keys"], json!(["书库", "书架"]));
}

/// 上传自动转换:变体(逗号分隔字符串 keys、字符串 constant、keywords 字段)在
/// 上传时被规范化,响应携带 conversion 统计,条目接口返回规范字段。
#[tokio::test]
async fn upload_converts_variant_entries() {
    let _guard = test_lock().await;
    let app = test_app();
    let book = json!({
        "name": "变体书",
        "entries": [
            { "uid": 0, "comment": "逗号拆分", "keys": "车站, 铁路、码头", "content": "交通设定。", "constant": "true", "disable": false },
            { "uid": 1, "comment": "keywords变体", "keywords": ["火车站"], "content": "车站设定。", "constant": 0 },
            { "uid": 2, "comment": "缺失constant", "content": "世界观设定。" }
        ]
    });
    let (status, rec) = upload_world_book(app, &book, None).await;
    assert_eq!(status, StatusCode::CREATED);
    // 转换统计
    let conv = &rec["conversion"];
    assert_eq!(conv["total_count"], json!(3));
    assert_eq!(conv["converted_count"], json!(3), "三条均有变体需转换");
    assert_eq!(
        conv["key_normalized_count"],
        json!(2),
        "条目0 逗号拆分 + 条目1 keywords 变体"
    );
    assert_eq!(
        conv["constant_auto_count"],
        json!(1),
        "条目2 缺失 constant 自动判定"
    );

    // 条目接口返回规范化字段
    let id = rec["id"].as_str().unwrap();
    let (_, entries) = send_json(
        app,
        "GET",
        &format!("/api/world-books/{id}/entries"),
        json!({}),
    )
    .await;
    let arr = entries["entries"].as_array().unwrap();
    assert_eq!(arr.len(), 3);
    let e0 = arr.iter().find(|e| e["comment"] == "逗号拆分").unwrap();
    assert_eq!(e0["keys"], json!(["车站", "铁路", "码头"]));
    assert_eq!(e0["constant"], json!(true), "字符串 \"true\" 转布尔");
    let e1 = arr.iter().find(|e| e["comment"] == "keywords变体").unwrap();
    assert_eq!(e1["keys"], json!(["火车站"]));
    assert_eq!(e1["constant"], json!(false), "数字 0 转布尔");
    let e2 = arr.iter().find(|e| e["comment"] == "缺失constant").unwrap();
    assert_eq!(e2["constant"], json!(true), "无关键词无正则 → 常驻");
    assert!(
        e2["role"].is_null() || e2.get("role").is_none(),
        "role 缺失 → 自动"
    );
}

/// 自动分配机制自检:GET /api/world-books/auto-assign-check 返回 ok:true
/// 且含 parse / auto_role / convert / ui_auto 全部检查项。
#[tokio::test]
async fn auto_assign_check_reports_ok() {
    let _guard = test_lock().await;
    let app = test_app();
    let (status, result) =
        send_json(app, "GET", "/api/world-books/auto-assign-check", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result["ok"],
        json!(true),
        "自动分配机制应可用,summary={}",
        result["summary"]
    );
    let names: Vec<String> = result["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap_or("").to_string())
        .collect();
    for expect in ["parse", "auto_role", "convert", "ui_auto"] {
        assert!(
            names.contains(&expect.to_string()),
            "缺少检查项 {expect}:{names:?}"
        );
    }
    assert!(
        result["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["status"] == "ok"),
        "所有检查项应为 ok:{result}"
    );
}
