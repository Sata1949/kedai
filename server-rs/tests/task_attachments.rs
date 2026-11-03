// 任务附件集成测试(ATTACH-1,2026-11-03)。
//
// 覆盖三件事:三端点的线格式与状态码、落盘位置(工作区 `.kedai/attachments/` 与
// scratch 子目录两种)、以及**交付卫生不误报**(`.kedai` 是目录,hygiene_issues 只看
// 根级文件——这条是交接文档点名的「改完要跑一次确认」)。
//
// 复用 `tests/common` 的进程级 app(mock 连接器 + 临时数据目录 + 免鉴权)。
// 上传用真 multipart 字节:解析器是仓库唯一实现,绕过它测不出真实行为。
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::path::Path;
use tower::ServiceExt;
mod common;
use common::{send_json, test_app, test_lock};

/// 组一个多文件 multipart body(每个文件一个 `file` part)。
fn multipart_body(files: &[(&str, &[u8])]) -> (String, Vec<u8>) {
    let boundary = "KEDAIBOUND";
    let mut body = Vec::new();
    for (name, content) in files {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn upload(app: &axum::Router, task_id: &str, files: &[(&str, &[u8])]) -> (StatusCode, Value) {
    let (ct, body) = multipart_body(files);
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/tasks/{task_id}/attachments"))
        .header("content-type", ct)
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn list(app: &axum::Router, task_id: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/tasks/{task_id}/attachments"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn delete_one(app: &axum::Router, task_id: &str, aid: &str) -> StatusCode {
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/tasks/{task_id}/attachments/{aid}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn create_task(app: &axum::Router, title: &str, extra: Value) -> String {
    let mut body = json!({ "title": title });
    if let Value::Object(map) = extra {
        for (k, v) in map {
            body[&k] = v;
        }
    }
    let (status, json) = send_json(app, "POST", "/api/tasks", body).await;
    assert_eq!(status, StatusCode::CREATED, "创建任务应 201: {json}");
    json["task"]["id"].as_str().unwrap().to_string()
}

/// 未绑定任务:附件落 scratch 子目录,相对路径只有一级;详情顶层恒为数组。
#[tokio::test]
async fn unbound_task_attachments_roundtrip() {
    let _g = test_lock().await;
    let app = test_app();
    let id = create_task(app, "附件往返", json!({})).await;

    let (status, body) = upload(app, &id, &[("说明.txt", "第一行\n第二行".as_bytes())]).await;
    assert_eq!(status, StatusCode::CREATED, "上传应 201: {body}");
    let items = body["attachments"].as_array().expect("应返回本次上传清单");
    assert_eq!(items.len(), 1);
    let rel = items[0]["rel_path"].as_str().unwrap();
    assert!(
        rel.starts_with("attachments/"),
        "未绑定任务的相对路径应只有一级 attachments/,实际 {rel}"
    );
    assert_eq!(items[0]["name"], "说明.txt", "展示名保留原名");
    assert_eq!(items[0]["size"], "第一行\n第二行".len() as i64);

    // 清单端点与详情顶层都应有这一条
    let (st, body) = list(app, &id).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["attachments"].as_array().unwrap().len(), 1);
    let (st, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        detail["attachments"].as_array().unwrap().len(),
        1,
        "详情顶层必须带 attachments"
    );

    // 删除后清单归零
    let aid = items[0]["id"].as_str().unwrap();
    assert_eq!(delete_one(app, &id, aid).await, StatusCode::NO_CONTENT);
    let (_, body) = list(app, &id).await;
    assert!(body["attachments"].as_array().unwrap().is_empty());
    // 再删同一条 → 404(不伪装成功)
    assert_eq!(delete_one(app, &id, aid).await, StatusCode::NOT_FOUND);
}

/// 绑定工作区:落在 `<workspace>/.kedai/attachments/`,并自动补 `.gitignore`;
/// 同时验证交付卫生**不**把 `.kedai` 当成未声明草稿目录。
#[tokio::test]
async fn workspace_task_attachments_land_in_kedai_dir() {
    let _g = test_lock().await;
    let app = test_app();
    let ws = std::env::temp_dir().join(format!("kedai-attach-ws-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).expect("建工作区");

    let id = create_task(
        app,
        "工作区附件",
        json!({ "workspace": ws.to_string_lossy() }),
    )
    .await;

    let (status, body) = upload(
        app,
        &id,
        &[
            ("spec.txt", "规格正文".as_bytes()),
            ("shot.png", b"\x89PNG\r\n\x1a\n-fake"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "多文件上传应 201: {body}");
    let items = body["attachments"].as_array().unwrap();
    assert_eq!(items.len(), 2, "一个请求应能带多个文件");
    for item in items {
        let rel = item["rel_path"].as_str().unwrap();
        assert!(
            rel.starts_with(".kedai/attachments/"),
            "绑定工作区应落在 .kedai/attachments/,实际 {rel}"
        );
        assert!(
            ws.join(rel).is_file(),
            "落盘文件应真实存在:{}",
            ws.join(rel).display()
        );
    }

    // .gitignore 自动生成,避免污染用户 git 仓库
    let gi = ws.join(".kedai").join(".gitignore");
    assert_eq!(
        std::fs::read_to_string(&gi).expect("应生成 .gitignore"),
        "attachments/\n"
    );

    // 交付卫生只看根级文件:`.kedai` 是目录 → 不应出现任何 hygiene 提示
    let (st, detail) = send_json(app, "GET", &format!("/api/tasks/{id}"), Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    let hygiene = detail["delivery"]["hygiene_issues"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        hygiene.is_empty(),
        "附件目录不得被记成未声明草稿: {hygiene:?}"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

/// 上限与非法输入:空文件、超单文件上限都应在服务层被拒(400 语义)。
#[tokio::test]
async fn attachment_limits_are_enforced() {
    let _g = test_lock().await;
    let app = test_app();
    let id = create_task(app, "附件上限", json!({})).await;

    let (status, _) = upload(app, &id, &[("empty.txt", b"")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空文件应被拒");

    // 超过 25MB:构造 25MB+1 的字节(只在内存里,不落盘)
    let big = vec![b'x'; 25 * 1024 * 1024 + 1];
    let (status, _) = upload(app, &id, &[("big.bin", &big)]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "超单文件上限应被拒");
}

/// 任务不存在:三个端点都应 404(不制造「附件不存在」的歧义)。
#[tokio::test]
async fn missing_task_returns_404_on_all_attachment_endpoints() {
    let _g = test_lock().await;
    let app = test_app();
    let (status, _) = upload(app, "no-such-task", &[("a.txt", b"x")]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = list(app, "no-such-task").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        delete_one(app, "no-such-task", "no-such-att").await,
        StatusCode::NOT_FOUND
    );
}

/// 落盘名 sanitize:穿越尝试不会逃出附件目录。
#[tokio::test]
async fn traversal_in_filename_stays_inside_attachment_dir() {
    let _g = test_lock().await;
    let app = test_app();
    let ws = std::env::temp_dir().join(format!("kedai-attach-trav-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).expect("建工作区");
    let id = create_task(app, "穿越", json!({ "workspace": ws.to_string_lossy() })).await;

    let (status, body) = upload(app, &id, &[("../../escape.txt", b"x")]).await;
    assert_eq!(status, StatusCode::CREATED, "上传本身应成功: {body}");
    let rel = body["attachments"][0]["rel_path"].as_str().unwrap();
    assert!(
        rel.starts_with(".kedai/attachments/") && !rel.contains(".."),
        "落盘路径不得含上溯段,实际 {rel}"
    );
    assert!(ws.join(rel).is_file());
    assert!(
        !ws.join("escape.txt").exists(),
        "不得写到工作区根(穿越失败)"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// `attachments` 目录真的在磁盘上按 `{seq}-{safe}` 命名(而不是用原名)。
#[tokio::test]
async fn disk_names_are_sequenced_and_sanitized() {
    let _g = test_lock().await;
    let app = test_app();
    let id = create_task(app, "命名", json!({})).await;
    let (_, body) = upload(app, &id, &[("接口 说明.txt", b"y")]).await;
    let rel = body["attachments"][0]["rel_path"].as_str().unwrap();
    let disk = Path::new(rel).file_name().unwrap().to_str().unwrap();
    assert!(
        disk.starts_with("1-"),
        "磁盘名应有序号前缀防覆盖,实际 {disk}"
    );
    assert!(
        !disk.contains(' '),
        "磁盘名应 sanitize(不含空格),实际 {disk}"
    );
    assert_eq!(body["attachments"][0]["name"], "接口 说明.txt");
}
