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
async fn stats_update_persists_to_disk() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path()));

    let chats_dir = dir.path().join("default-user").join("chats");
    std::fs::create_dir_all(&chats_dir).unwrap();

    let body = json!({ "example": "value" });
    let request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/stats/update")
            .header("content-type", "application/json"),
    )
    .body(Body::from(body.to_string()))
    .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let stats_path = dir.path().join("default-user").join("stats.json");
    assert!(stats_path.exists());
    let stats: Value = serde_json::from_str(&std::fs::read_to_string(&stats_path).unwrap()).unwrap();
    assert_eq!(stats.get("example").and_then(|v| v.as_str()), Some("value"));
    assert!(stats.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0) > 0);

    let get_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/stats/get")
            .header("content-type", "application/json"),
    )
    .body(Body::from("{}"))
    .unwrap();

    let get_response = app.oneshot(get_request).await.unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);
    let payload = read_json(get_response).await;
    assert_eq!(payload.get("example").and_then(|v| v.as_str()), Some("value"));
}

#[tokio::test]
async fn image_metadata_generates_entry() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path()));

    let backgrounds = dir.path().join("default-user").join("backgrounds");
    std::fs::create_dir_all(&backgrounds).unwrap();
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
    img.save(backgrounds.join("meta-test.png")).unwrap();

    let body = json!({ "path": "backgrounds/meta-test.png", "type": "bg" });
    let request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/image-metadata")
            .header("content-type", "application/json"),
    )
    .body(Body::from(body.to_string()))
    .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload = read_json(response).await;
    assert!(payload.get("hash").is_some());
    assert!(payload.get("aspectRatio").is_some());

    let index_path = dir.path().join("default-user").join("image-metadata.json");
    assert!(index_path.exists());
}

#[tokio::test]
async fn backups_chat_endpoints() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path()));

    let backups_dir = dir.path().join("default-user").join("backups");
    std::fs::create_dir_all(&backups_dir).unwrap();
    let backup_name = "chat_Test.jsonl";
    std::fs::write(
        backups_dir.join(backup_name),
        "{\"name\":\"User\",\"mes\":\"Hi\"}\n",
    )
    .unwrap();

    let get_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/backups/chat/get")
            .header("content-type", "application/json"),
    )
    .body(Body::from("{}"))
    .unwrap();
    let get_response = app.clone().oneshot(get_request).await.unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);
    let payload = read_json(get_response).await;
    let entries = payload.as_array().cloned().unwrap_or_default();
    assert!(entries.iter().any(|entry| {
        entry.get("file_name").and_then(|v| v.as_str()) == Some(backup_name)
    }));

    let download_body = json!({ "name": backup_name });
    let download_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/backups/chat/download")
            .header("content-type", "application/json"),
    )
    .body(Body::from(download_body.to_string()))
    .unwrap();
    let download_response = app.clone().oneshot(download_request).await.unwrap();
    assert_eq!(download_response.status(), StatusCode::OK);

    let delete_body = json!({ "name": backup_name });
    let delete_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/backups/chat/delete")
            .header("content-type", "application/json"),
    )
    .body(Body::from(delete_body.to_string()))
    .unwrap();
    let delete_response = app.oneshot(delete_request).await.unwrap();
    assert_eq!(delete_response.status(), StatusCode::OK);
    assert!(!backups_dir.join(backup_name).exists());
}

#[tokio::test]
async fn vector_insert_query_list_delete() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path()));

    let embeddings = json!({
        "hello": [1.0, 0.0],
        "world": [0.0, 1.0],
    });

    let insert_body = json!({
        "collectionId": "vec-a",
        "items": [{ "hash": 1, "text": "hello", "index": 0 }],
        "source": "webllm",
        "model": "parity",
        "embeddings": embeddings,
    });
    let insert_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/vector/insert")
            .header("content-type", "application/json"),
    )
    .body(Body::from(insert_body.to_string()))
    .unwrap();
    let insert_response = app.clone().oneshot(insert_request).await.unwrap();
    assert_eq!(insert_response.status(), StatusCode::OK);

    let list_body = json!({ "collectionId": "vec-a", "source": "webllm", "model": "parity" });
    let list_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/vector/list")
            .header("content-type", "application/json"),
    )
    .body(Body::from(list_body.to_string()))
    .unwrap();
    let list_response = app.clone().oneshot(list_request).await.unwrap();
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload = read_json(list_response).await;
    assert_eq!(list_payload.as_array().map(|a| a.len()), Some(1));

    let query_body = json!({
        "collectionId": "vec-a",
        "searchText": "hello",
        "topK": 1,
        "threshold": 0.5,
        "source": "webllm",
        "model": "parity",
        "embeddings": embeddings,
    });
    let query_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/vector/query")
            .header("content-type", "application/json"),
    )
    .body(Body::from(query_body.to_string()))
    .unwrap();
    let query_response = app.clone().oneshot(query_request).await.unwrap();
    assert_eq!(query_response.status(), StatusCode::OK);
    let query_payload = read_json(query_response).await;
    let hashes = query_payload.get("hashes").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    assert_eq!(hashes.len(), 1);

    let delete_body = json!({
        "collectionId": "vec-a",
        "hashes": [1],
        "source": "webllm",
        "model": "parity",
    });
    let delete_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/vector/delete")
            .header("content-type", "application/json"),
    )
    .body(Body::from(delete_body.to_string()))
    .unwrap();
    let delete_response = app.clone().oneshot(delete_request).await.unwrap();
    assert_eq!(delete_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn search_translate_and_extras_validation() {
    let dir = TempDir::new().unwrap();
    let app = build_router(test_config(dir.path()));

    let search_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/search/visit")
            .header("content-type", "application/json"),
    )
    .body(Body::from(json!({ "url": "ftp://example.com" }).to_string()))
    .unwrap();
    let search_response = app.clone().oneshot(search_request).await.unwrap();
    assert_eq!(search_response.status(), StatusCode::BAD_REQUEST);

    let translate_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/translate/google")
            .header("content-type", "application/json"),
    )
    .body(Body::from("{}"))
    .unwrap();
    let translate_response = app.clone().oneshot(translate_request).await.unwrap();
    assert_eq!(translate_response.status(), StatusCode::BAD_REQUEST);

    let classify_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/extra/classify")
            .header("content-type", "application/json"),
    )
    .body(Body::from(json!({ "text": "hello" }).to_string()))
    .unwrap();
    let classify_response = app.clone().oneshot(classify_request).await.unwrap();
    assert_eq!(classify_response.status(), StatusCode::NOT_IMPLEMENTED);

    let caption_request = add_user_headers(
        Request::builder()
            .method("POST")
            .uri("/api/extra/caption")
            .header("content-type", "application/json"),
    )
    .body(Body::from(json!({ "image": "" }).to_string()))
    .unwrap();
    let caption_response = app.oneshot(caption_request).await.unwrap();
    assert_eq!(caption_response.status(), StatusCode::BAD_REQUEST);
}
