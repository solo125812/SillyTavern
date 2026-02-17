//! Background endpoints — Phase 2 reads + Phase 3 writes.
//!
//! Mirrors Node's [`src/endpoints/backgrounds.js`](../../../src/endpoints/backgrounds.js).
//!
//! ## Endpoints
//! - `POST /api/backgrounds/all` — return `{images, config}` with thumbnail dimensions (Phase 2).
//! - `POST /api/backgrounds/upload` — upload a background via multipart (Phase 3).
//! - `POST /api/backgrounds/delete` — delete a background (Phase 3).
//! - `POST /api/backgrounds/rename` — rename a background (Phase 3).

use std::fs;
use std::sync::Arc;

use axum::{
    extract::{Extension, Multipart, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::api::router::AppState;
use crate::api::thumbnails::{invalidate_thumbnail, ThumbnailType};
use crate::http::middleware::UserContext;
use crate::storage::media::list_image_files;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Response for `POST /api/backgrounds/all`.
#[derive(Debug, Serialize)]
pub struct BackgroundsAllResponse {
    /// List of background image filenames.
    pub images: Vec<String>,
    /// Thumbnail configuration with dimensions.
    pub config: ThumbnailConfig,
}

/// Thumbnail dimension configuration sent to the client.
#[derive(Debug, Serialize)]
pub struct ThumbnailConfig {
    /// Thumbnail width in pixels.
    pub width: u32,
    /// Thumbnail height in pixels.
    pub height: u32,
}

/// Request body for `POST /api/backgrounds/delete`.
#[derive(Debug, Deserialize)]
pub struct DeleteBackgroundRequest {
    /// Background filename to delete.
    pub bg: String,
}

/// Request body for `POST /api/backgrounds/rename`.
#[derive(Debug, Deserialize)]
pub struct RenameBackgroundRequest {
    /// Old background filename.
    pub old_bg: String,
    /// New background filename.
    pub new_bg: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/backgrounds/all` — return background list with thumbnail config.
///
/// Mirrors Node's `router.post('/all')` in `backgrounds.js:14-18`.
pub async fn get_all_backgrounds(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let images = list_image_files(&dirs.backgrounds);
    let (width, height) = state.config.thumbnail_dimensions.bg;
    let config = ThumbnailConfig { width, height };
    Json(BackgroundsAllResponse { images, config })
}

/// `POST /api/backgrounds/upload` — upload a background via multipart.
///
/// Mirrors Node's `router.post('/upload')` in `backgrounds.js:77-99`:
/// - Multipart with file field; uses the original filename.
/// - Writes to `backgrounds/` directory (can overwrite existing).
/// - Invalidates background thumbnail.
/// - Returns the filename as plain text.
pub async fn upload_background(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    let mut file_data: Option<Vec<u8>> = None;
    let mut original_name: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        if matches!(field.name(), Some("file" | "avatar")) {
            original_name = field.file_name().map(|s| s.to_string());
            match field.bytes().await {
                Ok(bytes) => file_data = Some(bytes.to_vec()),
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            }
        }
    }

    let image_data = match file_data {
        Some(data) if !data.is_empty() => data,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let filename = match original_name {
        Some(name) if !name.is_empty() => name,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Ensure backgrounds directory exists
    if let Err(e) = fs::create_dir_all(&dirs.backgrounds) {
        tracing::error!("Failed to create backgrounds directory: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let dest = dirs.backgrounds.join(&filename);
    if let Err(e) = fs::write(&dest, &image_data) {
        tracing::error!("Failed to write background: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    invalidate_thumbnail(&dirs, ThumbnailType::Bg, &filename);

    // Note: metadata generation deferred (requires image-metadata module in Phase 9)

    Html(filename).into_response()
}

/// `POST /api/backgrounds/delete` — delete a background.
///
/// Mirrors Node's `router.post('/delete')` in `backgrounds.js:20-45`:
/// - Validates filename via `getFileNameValidationFunction('bg')`.
/// - Sanitize comparison: rejects if `sanitize(bg) != bg`.
/// - Deletes the file.
/// - Invalidates background thumbnail.
/// - Removes image metadata (deferred to Phase 9).
pub async fn delete_background(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteBackgroundRequest>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    // Sanitize comparison — mirrors Node: if (bg !== sanitize(bg))
    let sanitized = sanitize_filename_strip(&body.bg);
    if sanitized != body.bg {
        tracing::error!("Malicious bg name prevented");
        return StatusCode::FORBIDDEN.into_response();
    }

    let file_path = dirs.backgrounds.join(&sanitized);

    if !file_path.exists() {
        tracing::error!("BG file not found");
        return StatusCode::BAD_REQUEST.into_response();
    }

    if let Err(e) = fs::remove_file(&file_path) {
        tracing::error!("Failed to delete background: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    invalidate_thumbnail(&dirs, ThumbnailType::Bg, &body.bg);

    // Note: metadata removal deferred (requires image-metadata module in Phase 9)

    Html("ok".to_string()).into_response()
}

/// `POST /api/backgrounds/rename` — rename a background.
///
/// Mirrors Node's `router.post('/rename')` in `backgrounds.js:47-75`:
/// - Sanitizes both old and new filenames.
/// - Checks old file exists and new file does NOT exist.
/// - Copies old to new, deletes old.
/// - Invalidates old background thumbnail.
/// - Renames image metadata (deferred to Phase 9).
pub async fn rename_background(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<RenameBackgroundRequest>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    let old_sanitized = sanitize_filename_strip(&body.old_bg);
    let new_sanitized = sanitize_filename_strip(&body.new_bg);

    let old_path = dirs.backgrounds.join(&old_sanitized);
    let new_path = dirs.backgrounds.join(&new_sanitized);

    if !old_path.exists() {
        tracing::error!("BG file not found");
        return StatusCode::BAD_REQUEST.into_response();
    }

    if new_path.exists() {
        tracing::error!("New BG file already exists");
        return StatusCode::BAD_REQUEST.into_response();
    }

    // Copy then delete (mirrors Node: fs.copyFileSync + fs.unlinkSync)
    if let Err(e) = fs::copy(&old_path, &new_path) {
        tracing::error!("Failed to copy background: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    if let Err(e) = fs::remove_file(&old_path) {
        tracing::error!("Failed to remove old background: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    invalidate_thumbnail(&dirs, ThumbnailType::Bg, &body.old_bg);

    // Note: metadata rename deferred (requires image-metadata module in Phase 9)

    Html("ok".to_string()).into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn backgrounds_all_response_shape() {
        let resp = BackgroundsAllResponse {
            images: vec!["bg1.png".to_string(), "bg2.jpg".to_string()],
            config: ThumbnailConfig {
                width: 160,
                height: 90,
            },
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["images"].is_array());
        assert_eq!(json["images"][0], "bg1.png");
        assert_eq!(json["images"][1], "bg2.jpg");
        assert_eq!(json["config"]["width"], 160);
        assert_eq!(json["config"]["height"], 90);
    }

    #[test]
    fn list_backgrounds_from_directory() {
        let dir = TempDir::new().unwrap();
        let user_dirs = UserDirectories::new(dir.path(), "test-user");

        // Create backgrounds directory
        fs::create_dir_all(&user_dirs.backgrounds).unwrap();

        // Create background files
        fs::File::create(user_dirs.backgrounds.join("forest.png"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();
        fs::File::create(user_dirs.backgrounds.join("city.jpg"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();

        let images = list_image_files(&user_dirs.backgrounds);
        assert_eq!(images, vec!["city.jpg", "forest.png"]);
    }

    #[test]
    fn delete_background_removes_file() {
        let dir = TempDir::new().unwrap();
        let user_dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&user_dirs.backgrounds).unwrap();

        let bg_file = user_dirs.backgrounds.join("sunset.png");
        fs::File::create(&bg_file)
            .unwrap()
            .write_all(b"fake bg")
            .unwrap();
        assert!(bg_file.exists());

        fs::remove_file(&bg_file).unwrap();
        assert!(!bg_file.exists());
    }

    #[test]
    fn rename_background_copies_and_removes() {
        let dir = TempDir::new().unwrap();
        let user_dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&user_dirs.backgrounds).unwrap();

        let old_file = user_dirs.backgrounds.join("old.png");
        let new_file = user_dirs.backgrounds.join("new.png");
        fs::File::create(&old_file)
            .unwrap()
            .write_all(b"bg data")
            .unwrap();

        fs::copy(&old_file, &new_file).unwrap();
        fs::remove_file(&old_file).unwrap();

        assert!(!old_file.exists());
        assert!(new_file.exists());
        assert_eq!(fs::read_to_string(&new_file).unwrap(), "bg data");
    }

    #[test]
    fn delete_request_deserialization() {
        let json = r#"{"bg": "sunset.png"}"#;
        let req: DeleteBackgroundRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.bg, "sunset.png");
    }

    #[test]
    fn rename_request_deserialization() {
        let json = r#"{"old_bg": "old.png", "new_bg": "new.png"}"#;
        let req: RenameBackgroundRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.old_bg, "old.png");
        assert_eq!(req.new_bg, "new.png");
    }
}
