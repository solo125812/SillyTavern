use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use base64::Engine;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

use sillytavern_backend::api::router::build_router;
use sillytavern_backend::config::{AppConfig, ThumbnailDimensions};

fn test_config(data_root: &Path, cache_buster_enabled: bool) -> AppConfig {
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
        cache_buster_enabled,
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

struct MultipartFile<'a> {
    field: &'a str,
    filename: &'a str,
    content_type: &'a str,
    data: Vec<u8>,
}

fn build_multipart_body(
    fields: &[(&str, &str)],
    files: &[MultipartFile<'_>],
) -> (String, Vec<u8>) {
    let boundary = format!("----st-parity-{}", Uuid::new_v4());
    let mut body = Vec::new();

    for (name, value) in fields {
        body.extend(format!("--{}\r\n", boundary).as_bytes());
        body.extend(
            format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", name).as_bytes(),
        );
        body.extend(value.as_bytes());
        body.extend(b"\r\n");
    }

    for file in files {
        body.extend(format!("--{}\r\n", boundary).as_bytes());
        body.extend(
            format!(
                "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n",
                file.field, file.filename
            )
            .as_bytes(),
        );
        body.extend(format!("Content-Type: {}\r\n\r\n", file.content_type).as_bytes());
        body.extend(&file.data);
        body.extend(b"\r\n");
    }

    body.extend(format!("--{}--\r\n", boundary).as_bytes());
    (format!("multipart/form-data; boundary={}", boundary), body)
}

fn fixture_png_bytes() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMB/6X9Qm4AAAAASUVORK5CYII=")
        .unwrap()
}

#[tokio::test]
async fn avatar_upload_sets_clear_site_data_on_overwrite() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path(), true));

    let png_bytes = fixture_png_bytes();
    let (content_type, body) = build_multipart_body(
        &[("overwrite_name", "avatar-overwrite.png")],
        &[MultipartFile {
            field: "avatar",
            filename: "avatar.png",
            content_type: "image/png",
            data: png_bytes,
        }],
    );

    let request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/avatars/upload")
            .header("content-type", content_type)
            .header("user-agent", "Mozilla/5.0"),
    )
    .body(Body::from(body))
    .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let header = response
        .headers()
        .get("clear-site-data")
        .and_then(|v| v.to_str().ok());
    assert_eq!(header, Some("\"cache\""));
}

#[tokio::test]
async fn backgrounds_upload_writes_file() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path(), false));

    let png_bytes = fixture_png_bytes();
    let (content_type, body) = build_multipart_body(
        &[],
        &[MultipartFile {
            field: "file",
            filename: "bg-upload.png",
            content_type: "image/png",
            data: png_bytes,
        }],
    );

    let request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/backgrounds/upload")
            .header("content-type", content_type),
    )
    .body(Body::from(body))
    .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let expected = dir
        .path()
        .join("default-user")
        .join("backgrounds")
        .join("bg-upload.png");
    assert!(expected.exists());
}

#[tokio::test]
async fn sprites_upload_writes_file() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path(), false));

    let png_bytes = fixture_png_bytes();
    let (content_type, body) = build_multipart_body(
        &[("label", "joy"), ("name", "SpriteChar"), ("spriteName", "joy")],
        &[MultipartFile {
            field: "file",
            filename: "joy.png",
            content_type: "image/png",
            data: png_bytes,
        }],
    );

    let request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/sprites/upload")
            .header("content-type", content_type),
    )
    .body(Body::from(body))
    .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let expected = dir
        .path()
        .join("default-user")
        .join("characters")
        .join("SpriteChar")
        .join("joy.png");
    assert!(expected.exists());
}

#[tokio::test]
async fn files_upload_allows_lenient_base64() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path(), false));

    let body = serde_json::json!({
        "name": "upload.txt",
        "data": "aGVsbG8=!!"
    });

    let request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/files/upload")
            .header("content-type", "application/json"),
    )
    .body(Body::from(body.to_string()))
    .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let expected = dir
        .path()
        .join("default-user")
        .join("user")
        .join("files")
        .join("upload.txt");
    assert!(expected.exists());
    let contents = std::fs::read_to_string(expected).unwrap();
    assert_eq!(contents, "hello");
}

#[tokio::test]
async fn images_upload_rejects_empty_payload() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path(), false));

    let body = serde_json::json!({
        "image": "",
        "format": "png"
    });

    let request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/images/upload")
            .header("content-type", "application/json"),
    )
    .body(Body::from(body.to_string()))
    .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "requires binding a local port for fixture server"]
async fn assets_download_writes_file() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path(), false));

    let asset_app = Router::new().route(
        "/asset.bin",
        get(|| async { (StatusCode::OK, "fixture-asset") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, asset_app).await;
    });

    let url = format!("http://{}/asset.bin", addr);
    let body = serde_json::json!({
        "url": url,
        "category": "bgm",
        "filename": "download.bin"
    });

    let request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/assets/download")
            .header("content-type", "application/json"),
    )
    .body(Body::from(body.to_string()))
    .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let expected = dir
        .path()
        .join("default-user")
        .join("assets")
        .join("bgm")
        .join("download.bin");
    assert!(expected.exists());
    let contents = std::fs::read_to_string(expected).unwrap();
    assert_eq!(contents, "fixture-asset");
}

#[tokio::test]
async fn assets_download_unwhitelisted_host_returns_404() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path(), false));

    let body = serde_json::json!({
        "url": "http://127.0.0.1:9/asset.bin",
        "category": "bgm",
        "filename": "download.bin"
    });

    let request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/assets/download")
            .header("content-type", "application/json"),
    )
    .body(Body::from(body.to_string()))
    .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
