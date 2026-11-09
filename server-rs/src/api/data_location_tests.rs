#![cfg(test)]

use super::*;
use crate::utils::test_support::TempDataDir;
use axum::{body::Body, middleware::Next, Router};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn fixture(dir: &TempDataDir) -> (Arc<AppState>, Router) {
    let mut config = crate::config::test_config();
    config.data_dir = dir.join("source");
    config.images_dir = config.data_dir.join("images");
    config.imports_dir = config.data_dir.join("imports");
    config.task_scratch_dir = dir.join("scratch");
    config.data_root = None;
    config.auth_required = false;
    let mut state = AppState::new(config.clone()).unwrap();
    let activity = state.db.data_activity.clone();
    Arc::get_mut(&mut state).unwrap().core.data_location = Arc::new(
        crate::services::data_location_service::DataLocationService::with_pointer_path(
            config,
            dir.join("data-location.json"),
            activity,
        ),
    );
    let router = crate::api::build_router(state.clone());
    (state, router)
}

fn request(path: &str, body: serde_json::Value) -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .method("PUT")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn migration_rejects_inflight_settings_write() {
    let dir = TempDataDir::new("p1-move-inflight");
    let (state, router) = fixture(&dir);
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = started.clone();
    let router = router.layer(axum::middleware::from_fn(
        move |req: axum::extract::Request, next: Next| {
            let notify = notify.clone();
            async move {
                if req.uri().path() == "/api/settings" {
                    notify.notify_one();
                }
                next.run(req).await
            }
        },
    ));
    let lock = state.guards.settings_update.clone().lock_owned().await;
    let pending = tokio::spawn(router.clone().oneshot(request(
        "/api/settings",
        serde_json::json!({"default_temperature":0.6}),
    )));
    started.notified().await;
    let response = router
        .oneshot(request(
            "/api/data-location",
            serde_json::json!({"root":dir.join("Kedai-Data")}),
        ))
        .await
        .unwrap();
    drop(lock);
    pending.await.unwrap().unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "在途设置写入时必须拒绝迁移: {}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        !dir.join("data-location.json").exists(),
        "拒绝迁移不得提交数据根指针"
    );
}

#[tokio::test]
async fn migration_freezes_old_instance_until_restart() {
    let dir = TempDataDir::new("p1-move-frozen");
    let (_state, router) = fixture(&dir);
    let moved = router
        .clone()
        .oneshot(request(
            "/api/data-location",
            serde_json::json!({"root":dir.join("Kedai-Data")}),
        ))
        .await
        .unwrap();
    assert_eq!(moved.status(), StatusCode::OK);
    let write = router
        .clone()
        .oneshot(request(
            "/api/settings",
            serde_json::json!({"default_temperature":0.7}),
        ))
        .await
        .unwrap();
    let status = write.status();
    let body = write.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "迁移提交后旧实例必须阻止继续写入: {}",
        String::from_utf8_lossy(&body)
    );
    let health = router
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        health.status(),
        StatusCode::OK,
        "维护态仍可查询健康以便重启"
    );
}

#[tokio::test]
async fn migration_pointer_failure_reopens_source_without_migrated_marker() {
    let dir = TempDataDir::new("p1-move-pointer-failure");
    let (state, router) = fixture(&dir);
    std::fs::create_dir_all(dir.join("data-location.json")).unwrap();
    let failed = router
        .clone()
        .oneshot(request(
            "/api/data-location",
            serde_json::json!({"root":dir.join("Kedai-Data")}),
        ))
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        !crate::data_location::has_migrated_marker(&state.config.data_dir),
        "指针未提交时旧根仍为权威"
    );
    let write = router
        .oneshot(request(
            "/api/settings",
            serde_json::json!({"default_temperature":0.8}),
        ))
        .await
        .unwrap();
    assert_eq!(write.status(), StatusCode::OK, "迁移失败后恢复原实例");
}
