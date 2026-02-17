//! File management endpoints — Phase 3.
//!
//! Mirrors Node's [`src/endpoints/files.js`](../../../src/endpoints/files.js).
//!
//! ## Endpoints
//! - `POST /api/files/sanitize-filename` — sanitize a filename.
//! - `POST /api/files/upload` — upload a file (base64).
//! - `POST /api/files/delete` — delete a file.
//! - `POST /api/files/verify` — verify file existence.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    Json,
};
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::{sanitize_filename_strip, validate_asset_filename};

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Request body for `POST /api/files/sanitize-filename`.
#[derive(Debug, Deserialize)]
pub struct SanitizeFilenameRequest {
    /// Filename to sanitize.
    #[serde(rename = "fileName")]
    pub file_name: String,
}

/// Response for `POST /api/files/sanitize-filename`.
#[derive(Debug, Serialize)]
pub struct SanitizeFilenameResponse {
    /// Sanitized filename.
    #[serde(rename = "fileName")]
    pub file_name: String,
}

/// Request body for `POST /api/files/upload`.
#[derive(Debug, Deserialize)]
pub struct UploadFileRequest {
    /// Filename for the uploaded file.
    pub name: String,
    /// Base64-encoded file data.
    pub data: String,
}

/// Response for `POST /api/files/upload`.
#[derive(Debug, Serialize)]
pub struct UploadFileResponse {
    /// Client-relative path to the uploaded file.
    pub path: String,
}

/// Request body for `POST /api/files/delete`.
#[derive(Debug, Deserialize)]
pub struct DeleteFileRequest {
    /// Relative path from user root to the file.
    pub path: String,
}

/// Request body for `POST /api/files/verify`.
#[derive(Debug, Deserialize)]
pub struct VerifyFilesRequest {
    /// List of URLs (relative paths) to verify.
    pub urls: Vec<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/files/sanitize-filename` — sanitize a filename.
///
/// Mirrors Node's `router.post('/sanitize-filename')` in `files.js:13-26`.
pub async fn sanitize_filename_handler(
    Json(body): Json<SanitizeFilenameRequest>,
) -> impl IntoResponse {
    if body.file_name.is_empty() {
        return (StatusCode::BAD_REQUEST, "No fileName specified").into_response();
    }

    let sanitized = sanitize_filename_strip(&body.file_name);
    Json(SanitizeFilenameResponse {
        file_name: sanitized,
    })
    .into_response()
}

/// `POST /api/files/upload` — upload a file (base64).
///
/// Mirrors Node's `router.post('/upload')` in `files.js:28-52`:
/// - Validates filename via `validateAssetFileName`.
/// - Decodes base64 and writes to `user/files/`.
/// - Returns `{path}` with client-relative path.
pub async fn upload_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<UploadFileRequest>,
) -> impl IntoResponse {
    if body.name.is_empty() {
        return (StatusCode::BAD_REQUEST, "No upload name specified").into_response();
    }

    if body.data.is_empty() {
        return (StatusCode::BAD_REQUEST, "No upload data specified").into_response();
    }

    // Validate filename
    let validation = validate_asset_filename(&body.name);
    if !validation.valid {
        let msg = validation.message.unwrap_or_default();
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let path_to_upload = dirs.files.join(&body.name);

    // Ensure parent directory exists
    if let Some(parent) = path_to_upload.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            tracing::error!("Failed to create directory: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    // Decode base64 leniently (mirrors Node's Buffer base64 handling)
    let file_bytes = decode_base64_loose(&body.data);

    if let Err(e) = fs::write(&path_to_upload, &file_bytes) {
        tracing::error!("Failed to write file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let url = client_relative_path(&dirs.root, &path_to_upload);
    tracing::info!("Uploaded file: {} from {}", url, user.handle);

    Json(UploadFileResponse { path: url }).into_response()
}

/// `POST /api/files/delete` — delete a file.
///
/// Mirrors Node's `router.post('/delete')` in `files.js:54-76`:
/// - Validates path is under `user/files/`.
/// - Returns 200 on success.
pub async fn delete_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteFileRequest>,
) -> impl IntoResponse {
    if body.path.is_empty() {
        return (StatusCode::BAD_REQUEST, Html("No path specified".to_string())).into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let path_to_delete = dirs.root.join(&body.path);

    // Validate path starts with files directory
    // Node uses: if (!pathToDelete.startsWith(request.user.directories.files))
    if !path_to_delete.starts_with(&dirs.files) {
        return (StatusCode::BAD_REQUEST, Html("Invalid path".to_string())).into_response();
    }

    if !path_to_delete.exists() {
        return (StatusCode::NOT_FOUND, Html("File not found".to_string())).into_response();
    }

    if let Err(e) = fs::remove_file(&path_to_delete) {
        tracing::error!("Failed to delete file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    tracing::info!("Deleted file: {} from {}", body.path, user.handle);
    "OK".into_response()
}

/// `POST /api/files/verify` — verify file existence.
///
/// Mirrors Node's `router.post('/verify')` in `files.js:78-101`:
/// - Expects `{urls: [...]}` array of relative paths.
/// - Returns `{[url]: boolean}` map.
pub async fn verify_files(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<VerifyFilesRequest>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let mut verified = serde_json::Map::new();

    for url in &body.urls {
        let path_to_verify = dirs.root.join(url);

        // Validate path starts with files directory
        if !path_to_verify.starts_with(&dirs.files) {
            tracing::warn!("File verification: Invalid path: {}", path_to_verify.display());
            continue;
        }

        let file_exists = path_to_verify.exists();
        verified.insert(url.clone(), serde_json::Value::Bool(file_exists));
    }

    Json(serde_json::Value::Object(verified)).into_response()
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Compute the client-relative path from the user root.
fn client_relative_path(root: &Path, input_path: &Path) -> String {
    match input_path.strip_prefix(root) {
        Ok(relative) => {
            let s = relative.to_string_lossy().replace('\\', "/");
            if s.starts_with('/') {
                s
            } else {
                format!("/{}", s)
            }
        }
        Err(_) => input_path.to_string_lossy().to_string(),
    }
}

/// Decode base64 input using Node's forgiving semantics.
fn decode_base64_loose(input: &str) -> Vec<u8> {
    let mut cleaned: String = input
        .chars()
        .filter(|c| {
            matches!(c,
                'A'..='Z'
                    | 'a'..='z'
                    | '0'..='9'
                    | '+'
                    | '/'
                    | '-'
                    | '_'
                    | '='
            )
        })
        .collect();

    if cleaned.contains('-') || cleaned.contains('_') {
        cleaned = cleaned.replace('-', "+").replace('_', "/");
    }

    match cleaned.len() % 4 {
        2 => cleaned.push_str("=="),
        3 => cleaned.push('='),
        1 => {
            cleaned.pop();
        }
        _ => {}
    }

    if cleaned.is_empty() {
        return Vec::new();
    }

    let engine = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );

    engine.decode(cleaned).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn sanitize_request_deserialization() {
        let json = r#"{"fileName": "hello world.png"}"#;
        let req: SanitizeFilenameRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.file_name, "hello world.png");
    }

    #[test]
    fn sanitize_response_serialization() {
        let resp = SanitizeFilenameResponse {
            file_name: "hello_world.png".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["fileName"], "hello_world.png");
    }

    #[test]
    fn upload_request_deserialization() {
        let json = r#"{"name": "test.txt", "data": "aGVsbG8="}"#;
        let req: UploadFileRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "test.txt");
        assert_eq!(req.data, "aGVsbG8=");
    }

    #[test]
    fn verify_request_deserialization() {
        let json = r#"{"urls": ["user/files/a.txt", "user/files/b.txt"]}"#;
        let req: VerifyFilesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.urls.len(), 2);
    }

    #[test]
    fn client_relative_path_computation() {
        let root = Path::new("/data/user1");
        let file = Path::new("/data/user1/user/files/test.txt");
        assert_eq!(client_relative_path(root, file), "/user/files/test.txt");
    }

    #[test]
    fn verify_files_path_validation() {
        let dir = TempDir::new().unwrap();
        let user_dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&user_dirs.files).unwrap();

        // Create a test file
        let test_file = user_dirs.files.join("test.txt");
        fs::write(&test_file, b"hello").unwrap();

        // Valid path under files/
        let valid_path = user_dirs.root.join("user/files/test.txt");
        assert!(valid_path.starts_with(&user_dirs.files));

        // File exists
        assert!(test_file.exists());

        // Invalid path (not under files/)
        let invalid_path = user_dirs.root.join("characters/test.txt");
        assert!(!invalid_path.starts_with(&user_dirs.files));
    }
}
