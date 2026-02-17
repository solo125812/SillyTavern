//! Backup endpoints — Phase 9.
//!
//! Mirrors Node's [`src/endpoints/backups.js`](../../../src/endpoints/backups.js).
//!
//! ## Endpoints
//! - `POST /api/backups/chat/get`      — List chat backup files with info.
//! - `POST /api/backups/chat/delete`   — Delete a chat backup file.
//! - `POST /api/backups/chat/download` — Download a chat backup file.

use std::fs;
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::api::characters::reads::get_chat_info;
use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::jsonl::CHAT_BACKUPS_PREFIX;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ChatBackupRequest {
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/backups/chat/get` — List chat backup files with info.
///
/// Mirrors Node's `router.post('/chat/get')` in `backups.js:9-30`.
pub async fn get_chat_backups(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    let backup_files = match fs::read_dir(&dirs.backups) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                e.file_type().map(|ft| ft.is_file()).unwrap_or(false)
                    && name.ends_with(".jsonl")
                    && name.starts_with(CHAT_BACKUPS_PREFIX)
            })
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };

    let mut backup_models = Vec::new();
    for name in &backup_files {
        let file_path = dirs.backups.join(name);
        match get_chat_info(&file_path, false) {
            Ok(info) => {
                if !info.file_name.is_empty() {
                    if let Ok(val) = serde_json::to_value(&info) {
                        backup_models.push(val);
                    }
                }
            }
            Err(_) => continue,
        }
    }

    Json(json!(backup_models))
}

/// `POST /api/backups/chat/delete` — Delete a chat backup file.
///
/// Mirrors Node's `router.post('/chat/delete')` in `backups.js:32-53`.
pub async fn delete_chat_backup(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ChatBackupRequest>,
) -> impl IntoResponse {
    let name = match &body.name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let sanitized = sanitize_filename(&name, "");
    let file_path = dirs.backups.join(&sanitized);

    // Validate that the base name starts with the chat backups prefix
    let base_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    if !base_name.starts_with(CHAT_BACKUPS_PREFIX) {
        tracing::warn!("Attempt to delete non-chat backup file: {}", name);
        return StatusCode::BAD_REQUEST.into_response();
    }

    if !file_path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    match fs::remove_file(&file_path) {
        Ok(_) => (StatusCode::OK, "OK").into_response(),
        Err(e) => {
            tracing::error!("Failed to delete backup: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/backups/chat/download` — Download a chat backup file.
///
/// Mirrors Node's `router.post('/chat/download')` in `backups.js:55-75`.
pub async fn download_chat_backup(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ChatBackupRequest>,
) -> impl IntoResponse {
    let name = match &body.name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let sanitized = sanitize_filename(&name, "");
    let file_path = dirs.backups.join(&sanitized);

    // Validate that the base name starts with the chat backups prefix
    let base_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    if !base_name.starts_with(CHAT_BACKUPS_PREFIX) {
        tracing::warn!("Attempt to download non-chat backup file: {}", name);
        return StatusCode::BAD_REQUEST.into_response();
    }

    if !file_path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Read the file and send as download
    match fs::read(&file_path) {
        Ok(data) => {
            let content_disposition = format!(
                "attachment; filename=\"{}\"",
                base_name.replace('"', "\\\"")
            );

            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Disposition", content_disposition)
                .header("Content-Type", "application/octet-stream")
                .body(axum::body::Body::from(data))
                .unwrap_or_else(|_| {
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                })
        }
        Err(e) => {
            tracing::error!("Failed to read backup file: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
