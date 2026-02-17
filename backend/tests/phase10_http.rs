use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tower::ServiceExt;

use sillytavern_backend::api::router::build_router;
use sillytavern_backend::config::{AppConfig, ThumbnailDimensions};

fn test_config(data_root: &Path) -> AppConfig {
    AppConfig {
        data_root: data_root.to_path_buf(),
        listen_address: "127.0.0.1:0".to_string(),
        server_directory: PathBuf::from("."),
        log_level: "info".to_string(),
        backend_header: false,
        thumbnails_enabled: true,
        thumbnail_dimensions: ThumbnailDimensions {
            bg: (160, 90),
            avatar: (96, 144),
            persona: (96, 144),
        },
        cache_buster_enabled: false,
        cache_buster_user_agent_pattern: String::new(),
        lazy_load_characters: false,
        thumbnail_quality: 95,
        thumbnail_pngformat: false,
        chat_backup_enabled: true,
        chat_backup_max_total: -1,
        chat_backup_throttle_ms: 10_000,
        chat_backup_check_integrity: true,
        chat_backup_num_per_chat: 50,
        enable_extensions: true,
        enable_extensions_auto_update: true,
        enable_accounts: false,
        allow_keys_exposure: false,
        enable_discreet_login: false,
        whitelist_import_domains: Vec::new(),
        prefer_real_ip_header: false,
    }
}

fn add_user_headers(builder: axum::http::request::Builder) -> axum::http::request::Builder {
    builder
        .header("x-st-user-handle", "default-user")
        .header("x-st-user-name", "User")
        .header("x-st-user-admin", "true")
}

async fn read_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}))
}

#[tokio::test]
async fn data_maid_report_view_delete_finalize() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path()));

    let images_dir = dir
        .path()
        .join("default-user")
        .join("user")
        .join("images");
    std::fs::create_dir_all(&images_dir).unwrap();

    let img_path = images_dir.join("data-maid-orphan.png");
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
    img.save(&img_path).unwrap();

    let report_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/data-maid/report")
            .header("content-type", "application/json"),
    )
    .body(Body::from("{}"))
    .unwrap();
    let report_response = app.clone().oneshot(report_request).await.unwrap();
    assert_eq!(report_response.status(), StatusCode::OK);
    let payload = read_json(report_response).await;

    let token = payload
        .get("token")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    assert!(!token.is_empty());

    let images = payload
        .get("report")
        .and_then(|v| v.get("images"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(!images.is_empty());
    let hash = images[0]
        .get("hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    assert!(!hash.is_empty());

    let view_uri = format!("/api/data-maid/view?token={}&hash={}", token, hash);
    let view_request = add_user_headers(Request::builder().method("GET").uri(&view_uri))
        .body(Body::empty())
        .unwrap();
    let view_response = app.clone().oneshot(view_request).await.unwrap();
    assert_eq!(view_response.status(), StatusCode::OK);
    let bytes = to_bytes(view_response.into_body(), usize::MAX).await.unwrap();
    assert!(!bytes.is_empty());

    let delete_body = json!({ "token": token, "hashes": [hash] });
    let delete_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/data-maid/delete")
            .header("content-type", "application/json"),
    )
    .body(Body::from(delete_body.to_string()))
    .unwrap();
    let delete_response = app.clone().oneshot(delete_request).await.unwrap();
    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);
    assert!(!img_path.exists());

    let finalize_body = json!({ "token": token });
    let finalize_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/data-maid/finalize")
            .header("content-type", "application/json"),
    )
    .body(Body::from(finalize_body.to_string()))
    .unwrap();
    let finalize_response = app.clone().oneshot(finalize_request).await.unwrap();
    assert_eq!(finalize_response.status(), StatusCode::NO_CONTENT);

    let reuse_request = add_user_headers(Request::builder().method("GET").uri(&view_uri))
        .body(Body::empty())
        .unwrap();
    let reuse_response = app.oneshot(reuse_request).await.unwrap();
    assert_eq!(reuse_response.status(), StatusCode::FORBIDDEN);
}
