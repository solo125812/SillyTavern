//! Image endpoints — Phase 2 reads + Phase 3 writes.
//!
//! Mirrors Node's [`src/endpoints/images.js`](../../../src/endpoints/images.js).
//!
//! ## Endpoints
//! - `POST /api/images/list/:folder?` — list images in a subfolder (Phase 2).
//! - `POST /api/images/list` — list images (folder in body) (Phase 2).
//! - `POST /api/images/folders` — list subfolders under `user/images/` (Phase 2).
//! - `POST /api/images/upload` — upload a base64-encoded image (Phase 3).
//! - `POST /api/images/delete` — delete an image (Phase 3).

use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Extension, Path as AxumPath, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine;
use serde::Deserialize;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::media::{list_media_files, SortField, SortOrder, MEDIA_EXTENSIONS, MEDIA_IMAGE};
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::{is_path_under_parent, sanitize_filename_strip};

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Request body for `POST /api/images/list`.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct ListImagesRequest {
    /// Subfolder name under `user/images/`.
    pub folder: Option<String>,
    /// Media type flags (default: IMAGE = 1).
    #[serde(rename = "type")]
    pub media_type: Option<serde_json::Value>,
    /// Sort field: "name" or "date".
    #[serde(rename = "sortField")]
    pub sort_field: Option<String>,
    /// Sort order: "asc" or "desc".
    #[serde(rename = "sortOrder")]
    pub sort_order: Option<String>,
}

/// Request body for `POST /api/images/upload`.
#[derive(Debug, Deserialize)]
pub struct UploadImageRequest {
    /// Base64-encoded image data.
    pub image: String,
    /// File format/extension (e.g., "png", "jpg").
    pub format: String,
    /// Optional explicit filename (without extension).
    pub filename: Option<String>,
    /// Optional character name for subfolder.
    pub ch_name: Option<String>,
}

/// Request body for `POST /api/images/delete`.
#[derive(Debug, Deserialize)]
pub struct DeleteImageRequest {
    /// Relative path from user root to the image file.
    pub path: String,
}

// ---------------------------------------------------------------------------
// Handlers — Phase 2 Reads
// ---------------------------------------------------------------------------

/// `POST /api/images/list/:folder` — deprecated URL-based folder param.
pub async fn list_images_with_folder(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    AxumPath(url_folder): AxumPath<String>,
    Json(body): Json<ListImagesRequest>,
) -> impl IntoResponse {
    if body.folder.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Folder specified in both URL and body"})),
        )
            .into_response();
    }

    tracing::warn!("Deprecated: Use POST /api/images/list with folder in request body");

    let merged = ListImagesRequest {
        folder: Some(url_folder),
        media_type: body.media_type,
        sort_field: body.sort_field,
        sort_order: body.sort_order,
    };

    list_images_inner(&state, &user, merged).into_response()
}

/// `POST /api/images/list` — list images in a subfolder.
pub async fn list_images(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ListImagesRequest>,
) -> impl IntoResponse {
    list_images_inner(&state, &user, body).into_response()
}

/// Shared implementation for image listing.
fn list_images_inner(
    state: &AppState,
    user: &UserContext,
    body: ListImagesRequest,
) -> impl IntoResponse {
    let folder = match &body.folder {
        Some(f) if !f.is_empty() => f.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "No folder specified"})),
            )
                .into_response();
        }
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let sanitized_folder = sanitize_filename_strip(&folder);
    let directory_path = dirs.user_images.join(&sanitized_folder);

    let media_type = parse_media_type(body.media_type.as_ref());

    let sort = SortField::from_str_or_default(
        body.sort_field.as_deref().unwrap_or("date"),
    );
    let order = SortOrder::from_str_or_default(
        body.sort_order.as_deref().unwrap_or("asc"),
    );

    if !directory_path.exists() {
        if let Err(e) = fs::create_dir_all(&directory_path) {
            tracing::error!("Failed to create directory: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Unable to retrieve files"})),
            )
                .into_response();
        }
    }

    let mut images = list_media_files(&directory_path, sort, media_type);
    if order == SortOrder::Desc {
        images.reverse();
    }

    Json(images).into_response()
}

// ---------------------------------------------------------------------------
// Media type parsing (JS Number() semantics)
// ---------------------------------------------------------------------------

fn parse_media_type(value: Option<&serde_json::Value>) -> u32 {
    match value {
        None | Some(serde_json::Value::Null) => MEDIA_IMAGE,
        Some(v) => match parse_js_number(v) {
            Some(n) if n.is_finite() => js_to_int32(n) as u32,
            _ => 0,
        },
    }
}

fn parse_js_number(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => parse_js_number_string(s),
        serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn parse_js_number_string(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }

    let (sign, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => match trimmed.strip_prefix('+') {
            Some(rest) => (1.0, rest),
            None => (1.0, trimmed),
        },
    };

    if let Some(rest) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        return u64::from_str_radix(rest, 16).ok().map(|v| sign * v as f64);
    }
    if let Some(rest) = digits.strip_prefix("0o").or_else(|| digits.strip_prefix("0O")) {
        return u64::from_str_radix(rest, 8).ok().map(|v| sign * v as f64);
    }
    if let Some(rest) = digits.strip_prefix("0b").or_else(|| digits.strip_prefix("0B")) {
        return u64::from_str_radix(rest, 2).ok().map(|v| sign * v as f64);
    }

    if digits.eq_ignore_ascii_case("infinity") {
        return Some(sign * f64::INFINITY);
    }

    trimmed.parse::<f64>().ok()
}

fn js_to_int32(value: f64) -> i32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }

    let mut int = value.trunc() % 4_294_967_296.0;
    if int < 0.0 {
        int += 4_294_967_296.0;
    }
    if int >= 2_147_483_648.0 {
        int -= 4_294_967_296.0;
    }

    int as i32
}

/// `POST /api/images/folders` — list subfolders under `user/images/`.
pub async fn list_image_folders(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let directory_path = &dirs.user_images;

    if !directory_path.exists() {
        if let Err(e) = fs::create_dir_all(directory_path) {
            tracing::error!("Failed to create directory: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Unable to retrieve folders"})),
            )
                .into_response();
        }
    }

    let folders: Vec<String> = match fs::read_dir(directory_path) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
            })
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect(),
        Err(e) => {
            tracing::error!("Failed to read directory: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Unable to retrieve folders"})),
            )
                .into_response();
        }
    };

    Json(folders).into_response()
}

// ---------------------------------------------------------------------------
// Handlers — Phase 3 Writes
// ---------------------------------------------------------------------------

/// `POST /api/images/upload` — upload a base64-encoded image.
///
/// Mirrors Node's `router.post('/upload')` in `images.js:39-78`:
/// - Expects `{image, format, filename?, ch_name?}` in JSON body.
/// - Validates format against `MEDIA_EXTENSIONS`.
/// - If `ch_name` is provided, saves under `user/images/{ch_name}/`.
/// - Returns `{path}` with client-relative path.
pub async fn upload_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<UploadImageRequest>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    // Validate format
    if !MEDIA_EXTENSIONS.contains(&body.format.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid image format"})),
        )
            .into_response();
    }

    if body.image.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "No image data provided"})),
        )
            .into_response();
    }

    // Construct filename
    let filename = match &body.filename {
        Some(name) if !name.is_empty() => {
            let base = remove_file_extension(name);
            format!("{}.{}", base, body.format)
        }
        _ => {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            format!("{}.{}", timestamp, body.format)
        }
    };

    // Determine path: with optional character subfolder
    let sanitized_filename = sanitize_filename_strip(&filename);
    let path_to_new_file = match &body.ch_name {
        Some(ch_name) if !ch_name.is_empty() => {
            let sanitized_ch = sanitize_filename_strip(ch_name);
            dirs.user_images.join(&sanitized_ch).join(&sanitized_filename)
        }
        _ => dirs.user_images.join(&sanitized_filename),
    };

    // Ensure parent directory exists
    if let Some(parent) = path_to_new_file.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            tracing::error!("Failed to create directory: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Failed to save the image"})),
            )
                .into_response();
        }
    }

    // Decode base64 leniently (mirrors Node's Buffer.from(..., 'base64'))
    let image_bytes = decode_base64_loose(&body.image);

    if let Err(e) = fs::write(&path_to_new_file, &image_bytes) {
        tracing::error!("Failed to write image: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Failed to save the image"})),
        )
            .into_response();
    }

    // Compute client-relative path (mirrors Node's clientRelativePath)
    let relative_path = client_relative_path(&dirs.root, &path_to_new_file);

    Json(serde_json::json!({"path": relative_path})).into_response()
}

/// `POST /api/images/delete` — delete an image.
///
/// Mirrors Node's `router.post('/delete')` in `images.js:133-155`:
/// - Expects `{path}` in JSON body (relative to user root).
/// - Validates path is under `user/images/`.
/// - Returns 200 on success.
pub async fn delete_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteImageRequest>,
) -> impl IntoResponse {
    if body.path.is_empty() {
        return (StatusCode::BAD_REQUEST, "No path specified").into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let path_to_delete = dirs.root.join(&body.path);

    // Validate path is under user/images/
    if !is_path_under_parent(&dirs.user_images, &path_to_delete) {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }

    if !path_to_delete.exists() {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    }

    if let Err(e) = fs::remove_file(&path_to_delete) {
        tracing::error!("Failed to delete image: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    tracing::info!("Deleted image: {} from {}", body.path, user.handle);
    "OK".into_response()
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Compute the client-relative path from the user root.
///
/// Mirrors Node's `clientRelativePath(root, inputPath)` in `util.js:573-579`.
fn client_relative_path(root: &Path, input_path: &Path) -> String {
    match input_path.strip_prefix(root) {
        Ok(relative) => {
            // Convert path separators to forward slashes (cross-platform)
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

/// Remove file extension from a filename.
///
/// Mirrors Node's `removeFileExtension()` from `util.js`.
fn remove_file_extension(filename: &str) -> &str {
    match filename.rfind('.') {
        Some(pos) if pos > 0 => &filename[..pos],
        _ => filename,
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

    // Node's base64clean accepts URL-safe characters and pads to a multiple of 4.
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
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn list_images_from_subfolder() {
        let dir = TempDir::new().unwrap();
        let user_dirs = UserDirectories::new(dir.path(), "test-user");

        let subfolder = user_dirs.user_images.join("characters");
        fs::create_dir_all(&subfolder).unwrap();

        fs::File::create(subfolder.join("hero.png"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();
        fs::File::create(subfolder.join("villain.jpg"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();

        let images = list_media_files(&subfolder, SortField::Name, MEDIA_IMAGE);
        assert_eq!(images, vec!["hero.png", "villain.jpg"]);
    }

    #[test]
    fn list_folders_returns_directories_only() {
        let dir = TempDir::new().unwrap();
        let user_dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&user_dirs.user_images).unwrap();

        fs::create_dir(user_dirs.user_images.join("characters")).unwrap();
        fs::create_dir(user_dirs.user_images.join("portraits")).unwrap();

        fs::File::create(user_dirs.user_images.join("readme.txt"))
            .unwrap()
            .write_all(b"text")
            .unwrap();

        let entries: Vec<String> = fs::read_dir(&user_dirs.user_images)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&"characters".to_string()));
        assert!(entries.contains(&"portraits".to_string()));
    }

    #[test]
    fn list_request_deserialization() {
        let json = r#"{"folder": "test", "type": 1, "sortField": "date", "sortOrder": "desc"}"#;
        let req: ListImagesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.folder, Some("test".to_string()));
        assert_eq!(req.sort_field, Some("date".to_string()));
        assert_eq!(req.sort_order, Some("desc".to_string()));
    }

    #[test]
    fn list_request_deserialization_defaults() {
        let json = r#"{}"#;
        let req: ListImagesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.folder, None);
        assert_eq!(req.media_type, None);
        assert_eq!(req.sort_field, None);
        assert_eq!(req.sort_order, None);
    }

    #[test]
    fn parse_media_type_defaults_to_image() {
        assert_eq!(parse_media_type(None), MEDIA_IMAGE);
        assert_eq!(parse_media_type(Some(&serde_json::Value::Null)), MEDIA_IMAGE);
    }

    #[test]
    fn parse_media_type_invalid_to_zero() {
        let value = serde_json::Value::String("not-a-number".to_string());
        assert_eq!(parse_media_type(Some(&value)), 0);
        let empty = serde_json::Value::String("".to_string());
        assert_eq!(parse_media_type(Some(&empty)), 0);
    }

    #[test]
    fn parse_media_type_numeric_variants() {
        let value = serde_json::Value::String("1".to_string());
        assert_eq!(parse_media_type(Some(&value)), 1);
        let value = serde_json::Value::String("1.9".to_string());
        assert_eq!(parse_media_type(Some(&value)), 1);
        let value = serde_json::Value::Bool(true);
        assert_eq!(parse_media_type(Some(&value)), 1);
        let value = serde_json::Value::Bool(false);
        assert_eq!(parse_media_type(Some(&value)), 0);
    }

    #[test]
    fn upload_request_deserialization() {
        let json = r#"{"image": "aGVsbG8=", "format": "png"}"#;
        let req: UploadImageRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.image, "aGVsbG8=");
        assert_eq!(req.format, "png");
        assert!(req.filename.is_none());
        assert!(req.ch_name.is_none());
    }

    #[test]
    fn client_relative_path_computes_correctly() {
        let root = Path::new("/data/user1");
        let file = Path::new("/data/user1/user/images/test.png");
        let result = client_relative_path(root, file);
        assert_eq!(result, "/user/images/test.png");
    }

    #[test]
    fn remove_extension_works() {
        assert_eq!(remove_file_extension("hello.png"), "hello");
        assert_eq!(remove_file_extension("no-ext"), "no-ext");
        assert_eq!(remove_file_extension("multi.dots.png"), "multi.dots");
    }

    #[test]
    fn delete_image_path_validation() {
        let dir = TempDir::new().unwrap();
        let user_dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&user_dirs.user_images).unwrap();

        // A path under user/images/ should be valid
        let valid_path = user_dirs.root.join("user/images/test.png");
        assert!(is_path_under_parent(&user_dirs.user_images, &valid_path));

        // A path escaping user/images/ should be invalid
        let invalid_path = user_dirs.root.join("characters/test.png");
        assert!(!is_path_under_parent(&user_dirs.user_images, &invalid_path));
    }
}
