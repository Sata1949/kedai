// 数据位置端点(2026-11 盘符选择):GET 状态 + 参数校验。
//
// 集成测试环境由 `build_test_app` 显式设置 DATA_DIR,故 pending=false(不弹引导)、
// managed=false;不在这里选真实盘符(会写用户磁盘),迁移成功路径由
// `services::data_location_service` 的单测覆盖(临时根 + 注入指针)。
use axum::http::StatusCode;
use serde_json::{json, Value};
mod common;
use common::{send_json, test_app};

#[tokio::test]
async fn data_location_status_reports_explicit_data_dir() {
    let app = test_app();
    let (status, body) = send_json(app, "GET", "/api/data-location", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["managed"], json!(false), "显式 DATA_DIR 不由引导管理");
    assert_eq!(body["pending"], json!(false), "显式 DATA_DIR 不弹窗");
    assert!(body["data_dir"].as_str().is_some_and(|v| !v.is_empty()));
    assert!(body["images_dir"].as_str().is_some());
    assert!(body["task_records_dir"].as_str().is_some());
    assert!(body["imports_dir"].as_str().is_some());
    assert!(body["drives"].is_array());
}

#[tokio::test]
async fn data_location_rejects_invalid_drive() {
    let app = test_app();
    let (status, body) = send_json(app, "PUT", "/api/data-location", json!({ "drive": "1" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], json!("VALIDATION"));
    assert!(body["error"].as_str().is_some_and(|v| v.contains("盘符")));
}

#[tokio::test]
async fn imports_archive_rejects_missing_file() {
    let app = test_app();
    let (status, body) = send_json(app, "POST", "/api/imports/archive", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().is_some_and(|v| v.contains("文件")));
}

/// 归档成功路径:multipart 落盘到 `<DATA_DIR>/imports/`,文件名带时间戳前缀。
#[tokio::test]
async fn imports_archive_writes_file() {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let app = test_app();
    let body = "--BOUND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"bundle.json\"\r\nContent-Type: application/json\r\n\r\n{\"version\":1}\r\n--BOUND--\r\n";
    let req = Request::builder()
        .method("POST")
        .uri("/api/imports/archive")
        .header("content-type", "multipart/form-data; boundary=BOUND")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    let name = json["name"].as_str().expect("缺少归档文件名");
    assert!(name.ends_with("-bundle.json"), "文件名应保留扩展名: {name}");
}
