//! Avatar endpoints — Phase 2 reads + Phase 3 writes.
//!
//! Mirrors Node's [`src/endpoints/avatars.js`](../../../src/endpoints/avatars.js).
//!
//! ## Endpoints
//! - `POST /api/avatars/get` — return list of avatar image files (Phase 2).
//! - `POST /api/avatars/upload` — upload a new avatar (Phase 3).
//! - `POST /api/avatars/delete` — delete an avatar (Phase 3).

use std::fs;
use std::io::Cursor;
use std::sync::Arc;

use axum::{
    extract::{Extension, Multipart, Query, State},
    http::{header::USER_AGENT, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    Json,
};
use image::imageops::FilterType;
use image::DynamicImage;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

use crate::api::router::AppState;
use crate::api::thumbnails::{invalidate_thumbnail, ThumbnailType};
use crate::config::AppConfig;
use crate::http::middleware::UserContext;
use crate::storage::atomic::atomic_write_file;
use crate::storage::media::list_image_files;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

/// Avatar width in pixels. Matches Node's `AVATAR_WIDTH` in `constants.js:356`.
const AVATAR_WIDTH: u32 = 512;
/// Avatar height in pixels. Matches Node's `AVATAR_HEIGHT` in `constants.js:357`.
const AVATAR_HEIGHT: u32 = 768;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Request body for `POST /api/avatars/delete`.
#[derive(Debug, Deserialize)]
pub struct DeleteAvatarRequest {
    /// Avatar filename to delete.
    pub avatar: String,
}

/// Response for `POST /api/avatars/upload`.
#[derive(Debug, Serialize)]
pub struct UploadAvatarResponse {
    /// Path (filename) of the uploaded avatar.
    pub path: String,
}

/// Query parameters for `POST /api/avatars/upload`.
#[derive(Debug, Deserialize, Default)]
pub struct UploadAvatarQuery {
    /// Optional crop JSON string (parsed from query param).
    pub crop: Option<String>,
}

/// Crop parameters parsed from the `crop` query parameter.
///
/// Mirrors Node's `Crop` typedef used in `applyAvatarCropResize()`.
#[derive(Debug, Deserialize, Clone)]
pub struct CropParams {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub want_resize: bool,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/avatars/get` — return avatar file list.
///
/// Mirrors Node's `router.post('/get')` in `avatars.js:17-20`:
/// - Reads the `User Avatars/` directory
/// - Returns a JSON array of image filenames sorted by name
pub async fn get_avatars(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let images = list_image_files(&dirs.avatars);
    Json(images)
}

/// `POST /api/avatars/delete` — delete an avatar file.
///
/// Mirrors Node's `router.post('/delete')` in `avatars.js:22-38`:
/// 1. Validates filename (rejects path separators, null bytes).
/// 2. Sanitizes and compares — rejects if sanitized != original.
/// 3. Deletes the file from `User Avatars/`.
/// 4. Invalidates the persona thumbnail cache.
/// 5. Returns `{result: "ok"}` on success, 404 if not found.
pub async fn delete_avatar(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteAvatarRequest>,
) -> impl IntoResponse {
    // Validate filename — reject path separators and null bytes
    if contains_forbidden_chars(&body.avatar) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    // Sanitize and compare
    let sanitized = sanitize_filename_strip(&body.avatar);
    if sanitized != body.avatar {
        tracing::error!("Malicious avatar name prevented");
        return StatusCode::FORBIDDEN.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let file_path = dirs.avatars.join(&sanitized);

    if file_path.exists() {
        if let Err(e) = fs::remove_file(&file_path) {
            tracing::error!("Failed to delete avatar: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        invalidate_thumbnail(&dirs, ThumbnailType::Persona, &sanitized);
        return Json(serde_json::json!({"result": "ok"})).into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

/// `POST /api/avatars/upload` — upload a new avatar via multipart.
///
/// Mirrors Node's `router.post('/upload')` in `avatars.js:41-65`:
/// 1. Receives multipart form with `avatar` file field and optional `overwrite_name` text field.
/// 2. Reads image data from the `avatar` field.
/// 3. If `overwrite_name` is provided, invalidates persona thumbnail and uses that name.
/// 4. Otherwise generates a timestamp-based filename.
/// 5. Writes the file to `User Avatars/`.
/// 6. Returns `{path: filename}`.
///
/// Note: Image processing (crop/resize via Jimp equivalent) is deferred.
/// The raw image bytes are written as-is for now.
pub async fn upload_avatar(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Query(_query): Query<UploadAvatarQuery>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    // Ensure avatar directory exists
    if let Err(e) = fs::create_dir_all(&dirs.avatars) {
        tracing::error!("Failed to create avatars directory: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let mut avatar_data: Option<Vec<u8>> = None;
    let mut overwrite_name: Option<String> = None;

    // Parse multipart fields
    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "avatar" => {
                match field.bytes().await {
                    Ok(bytes) => avatar_data = Some(bytes.to_vec()),
                    Err(e) => {
                        tracing::error!("Failed to read avatar field: {}", e);
                        return StatusCode::BAD_REQUEST.into_response();
                    }
                }
            }
            "overwrite_name" => {
                match field.text().await {
                    Ok(text) if !text.is_empty() => {
                        if contains_forbidden_chars(&text) {
                            return StatusCode::BAD_REQUEST.into_response();
                        }
                        overwrite_name = Some(text);
                    }
                    _ => {}
                }
            }
            _ => {
                // Skip unknown fields
            }
        }
    }

    let image_data = match avatar_data {
        Some(data) if !data.is_empty() => data,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    // If overwriting, invalidate the old thumbnail
    if let Some(ref name) = overwrite_name {
        let sanitized_name = sanitize_filename_strip(name);
        invalidate_thumbnail(&dirs, ThumbnailType::Persona, &sanitized_name);
    }

    // Determine filename
    let filename = match &overwrite_name {
        Some(name) => sanitize_filename_strip(name),
        None => {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            format!("{}.png", timestamp)
        }
    };

    let path_to_new_file = dirs.avatars.join(&filename);

    // Parse crop parameters from query string (Node: `tryParse(request.query.crop)`)
    let crop = _query
        .crop
        .as_deref()
        .and_then(|s| serde_json::from_str::<CropParams>(s).ok());

    // Apply crop/resize processing — mirrors Node's applyAvatarCropResize
    let processed = apply_avatar_crop_resize(&image_data, crop.as_ref());

    // Write the processed image atomically
    if let Err(e) = atomic_write_file(&path_to_new_file, &processed) {
        tracing::error!("Error uploading user avatar: {}", e);
        return (StatusCode::BAD_REQUEST, "Is not a valid image").into_response();
    }

    let mut response = Json(UploadAvatarResponse { path: filename }).into_response();

    if overwrite_name.is_some() {
        let user_agent = headers.get(USER_AGENT).and_then(|v| v.to_str().ok());
        if should_bust_cache(&state.config, user_agent) {
            response.headers_mut().insert(
                "clear-site-data",
                HeaderValue::from_static("\"cache\""),
            );
        }
    }

    response
}

// ---------------------------------------------------------------------------
// Image Processing
// ---------------------------------------------------------------------------

/// Apply avatar crop and resize, producing a PNG buffer.
///
/// Mirrors Node's `applyAvatarCropResize()` in `characters.js:283-306`:
/// 1. If `crop` is provided, crop the image to the specified region.
/// 2. If `crop.want_resize` is true, final size is `AVATAR_WIDTH × AVATAR_HEIGHT`.
/// 3. Apply "cover" resize (resize + center-crop to fill exact dimensions).
/// 4. Encode as PNG.
///
/// On failure (unsupported format like APNG), returns the raw bytes as-is,
/// matching Node's `tryReadImage()` catch block.
pub fn apply_avatar_crop_resize(data: &[u8], crop: Option<&CropParams>) -> Vec<u8> {
    let img = match image::load_from_memory(data) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("Failed to decode image for crop/resize, writing raw: {}", e);
            return data.to_vec();
        }
    };

    let (final_w, final_h, img) = match crop {
        Some(c) => {
            // Apply crop
            let x = c.x.max(0.0) as u32;
            let y = c.y.max(0.0) as u32;
            let w = c.width.max(1.0) as u32;
            let h = c.height.max(1.0) as u32;

            // Clamp crop region to image bounds
            let img_w = img.width();
            let img_h = img.height();
            let x = x.min(img_w.saturating_sub(1));
            let y = y.min(img_h.saturating_sub(1));
            let w = w.min(img_w.saturating_sub(x));
            let h = h.min(img_h.saturating_sub(y));

            let cropped = img.crop_imm(x, y, w, h);

            if c.want_resize {
                (AVATAR_WIDTH, AVATAR_HEIGHT, cropped)
            } else {
                (w, h, cropped)
            }
        }
        None => {
            let w = img.width();
            let h = img.height();
            (w, h, img)
        }
    };

    // Apply "cover" resize: resize to fill the target, then crop center.
    let covered = cover_resize(&img, final_w, final_h);

    // Encode as PNG
    let mut buf = Cursor::new(Vec::new());
    if let Err(e) = covered.write_to(&mut buf, image::ImageFormat::Png) {
        tracing::warn!("Failed to encode avatar as PNG, writing raw: {}", e);
        return data.to_vec();
    }
    buf.into_inner()
}

/// "Cover" resize — resize to fill exact dimensions, cropping overflow.
///
/// Mirrors Jimp's `image.cover({w, h})` behavior:
/// 1. Scale so both dimensions are >= target.
/// 2. Center-crop to exact target size.
fn cover_resize(img: &DynamicImage, target_w: u32, target_h: u32) -> DynamicImage {
    if target_w == 0 || target_h == 0 {
        return img.clone();
    }

    let src_w = img.width() as f64;
    let src_h = img.height() as f64;
    let target_w_f = target_w as f64;
    let target_h_f = target_h as f64;

    // Scale factor to cover the target (both dims >= target)
    let scale = (target_w_f / src_w).max(target_h_f / src_h);
    let scaled_w = (src_w * scale).round() as u32;
    let scaled_h = (src_h * scale).round() as u32;

    let resized = img.resize_exact(scaled_w, scaled_h, FilterType::Lanczos3);

    // Center crop to exact target
    let crop_x = (scaled_w.saturating_sub(target_w)) / 2;
    let crop_y = (scaled_h.saturating_sub(target_h)) / 2;
    resized.crop_imm(crop_x, crop_y, target_w, target_h)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if a string contains forbidden characters (path separators, null bytes).
///
/// Mirrors Node's `getFileNameValidationFunction()` in
/// [`src/middleware/validateFileName.js`](../../../src/middleware/validateFileName.js):
/// - Rejects `/` and `\0` on all platforms
/// - Also rejects `\` on Windows (but we check it everywhere for safety)
fn contains_forbidden_chars(s: &str) -> bool {
    s.contains('/') || s.contains('\\') || s.contains('\0')
}

/// Check if cache busting should be applied for this request.
///
/// Mirrors Node's `cacheBuster.shouldBust()` logic:
/// - Disabled when `cacheBuster.enabled` is false.
/// - If no user-agent pattern is configured, bust for all requests.
/// - Invalid regex patterns fall back to matching all.
fn should_bust_cache(config: &AppConfig, user_agent: Option<&str>) -> bool {
    if !config.cache_buster_enabled {
        return false;
    }

    let pattern = config.cache_buster_user_agent_pattern.trim();
    if pattern.is_empty() {
        return true;
    }

    let ua = user_agent.unwrap_or("");
    match RegexBuilder::new(pattern).case_insensitive(true).build() {
        Ok(regex) => regex.is_match(ua),
        Err(err) => {
            tracing::warn!("Invalid cacheBuster.userAgentPattern: {}", err);
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn test_config(cache_buster_enabled: bool, pattern: &str) -> AppConfig {
        AppConfig {
            data_root: PathBuf::from("./data"),
            listen_address: "127.0.0.1:0".to_string(),
            server_directory: PathBuf::from("."),
            log_level: "info".to_string(),
            backend_header: false,
            thumbnails_enabled: true,
            thumbnail_dimensions: crate::config::ThumbnailDimensions {
                bg: (160, 90),
                avatar: (96, 144),
                persona: (96, 144),
            },
            cache_buster_enabled,
            cache_buster_user_agent_pattern: pattern.to_string(),
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

    #[test]
    fn list_avatars_from_directory() {
        let dir = TempDir::new().unwrap();
        let user_dirs = UserDirectories::new(dir.path(), "test-user");

        // Create avatar directory
        fs::create_dir_all(&user_dirs.avatars).unwrap();

        // Create some avatar files
        fs::File::create(user_dirs.avatars.join("alice.png"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();
        fs::File::create(user_dirs.avatars.join("bob.jpg"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();
        // Non-image file should be excluded
        fs::File::create(user_dirs.avatars.join("notes.txt"))
            .unwrap()
            .write_all(b"text")
            .unwrap();

        let images = list_image_files(&user_dirs.avatars);
        assert_eq!(images, vec!["alice.png", "bob.jpg"]);
    }

    #[test]
    fn list_avatars_empty_directory() {
        let dir = TempDir::new().unwrap();
        let user_dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&user_dirs.avatars).unwrap();

        let images = list_image_files(&user_dirs.avatars);
        assert!(images.is_empty());
    }

    #[test]
    fn delete_avatar_removes_file() {
        let dir = TempDir::new().unwrap();
        let user_dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&user_dirs.avatars).unwrap();

        let avatar_path = user_dirs.avatars.join("test.png");
        fs::write(&avatar_path, b"fake image data").unwrap();
        assert!(avatar_path.exists());

        // Simulate delete logic
        let filename = "test.png";
        let sanitized = sanitize_filename_strip(filename);
        assert_eq!(sanitized, filename);
        let file_path = user_dirs.avatars.join(&sanitized);
        fs::remove_file(&file_path).unwrap();
        assert!(!avatar_path.exists());
    }

    #[test]
    fn delete_avatar_rejects_malicious_name() {
        let sanitized = sanitize_filename_strip("../../../etc/passwd");
        assert_ne!(sanitized, "../../../etc/passwd");
    }

    #[test]
    fn forbidden_chars_detected() {
        assert!(contains_forbidden_chars("path/to/file"));
        assert!(contains_forbidden_chars("path\\to\\file"));
        assert!(contains_forbidden_chars("file\0name"));
        assert!(!contains_forbidden_chars("valid-file.png"));
        assert!(!contains_forbidden_chars("my avatar.png"));
    }

    #[test]
    fn upload_response_serialization() {
        let resp = UploadAvatarResponse {
            path: "12345.png".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["path"], "12345.png");
    }

    #[test]
    fn upload_generates_timestamp_filename() {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let filename = format!("{}.png", timestamp);
        assert!(filename.ends_with(".png"));
        assert!(filename.len() > 4);
    }

    #[test]
    fn cache_buster_disabled_returns_false() {
        let config = test_config(false, "");
        assert!(!should_bust_cache(&config, Some("Mozilla/5.0")));
    }

    #[test]
    fn cache_buster_enabled_no_pattern_returns_true() {
        let config = test_config(true, "");
        assert!(should_bust_cache(&config, Some("Mozilla/5.0")));
    }

    #[test]
    fn cache_buster_pattern_matches_user_agent() {
        let config = test_config(true, "firefox");
        assert!(should_bust_cache(&config, Some("Mozilla/5.0 Firefox/120.0")));
        assert!(!should_bust_cache(&config, Some("Mozilla/5.0 Chrome/120.0")));
    }

    #[test]
    fn cache_buster_invalid_pattern_returns_true() {
        let config = test_config(true, "(");
        assert!(should_bust_cache(&config, Some("Mozilla/5.0")));
    }

    /// Create a minimal valid PNG image buffer of the given dimensions.
    fn make_test_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([128, 64, 32, 255]),
        );
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn avatar_crop_resize_no_crop_preserves_dimensions() {
        let png = make_test_png(200, 300);
        let result = apply_avatar_crop_resize(&png, None);
        let decoded = image::load_from_memory(&result).unwrap();
        // Without crop, cover resize to original dimensions = same size
        assert_eq!(decoded.width(), 200);
        assert_eq!(decoded.height(), 300);
    }

    #[test]
    fn avatar_crop_resize_with_crop_no_resize() {
        let png = make_test_png(400, 600);
        let crop = CropParams {
            x: 50.0, y: 50.0, width: 200.0, height: 300.0,
            want_resize: false,
        };
        let result = apply_avatar_crop_resize(&png, Some(&crop));
        let decoded = image::load_from_memory(&result).unwrap();
        // Crop to 200x300, no resize
        assert_eq!(decoded.width(), 200);
        assert_eq!(decoded.height(), 300);
    }

    #[test]
    fn avatar_crop_resize_with_want_resize() {
        let png = make_test_png(1024, 1024);
        let crop = CropParams {
            x: 0.0, y: 0.0, width: 800.0, height: 800.0,
            want_resize: true,
        };
        let result = apply_avatar_crop_resize(&png, Some(&crop));
        let decoded = image::load_from_memory(&result).unwrap();
        assert_eq!(decoded.width(), AVATAR_WIDTH);
        assert_eq!(decoded.height(), AVATAR_HEIGHT);
    }

    #[test]
    fn avatar_crop_resize_unsupported_format_fallback() {
        // Pass invalid image data — should fall back to raw bytes
        let garbage = b"this is not an image";
        let result = apply_avatar_crop_resize(garbage, None);
        assert_eq!(result, garbage);
    }

    #[test]
    fn cover_resize_produces_exact_dimensions() {
        let img = image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_pixel(100, 200, image::Rgba([0, 0, 0, 255])),
        );
        let covered = cover_resize(&img, 50, 50);
        assert_eq!(covered.width(), 50);
        assert_eq!(covered.height(), 50);
    }

    #[test]
    fn cover_resize_zero_target_returns_clone() {
        let img = image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_pixel(100, 100, image::Rgba([0, 0, 0, 255])),
        );
        let covered = cover_resize(&img, 0, 0);
        assert_eq!(covered.width(), 100);
        assert_eq!(covered.height(), 100);
    }

    #[test]
    fn crop_params_deserialization() {
        let json = r#"{"x":10,"y":20,"width":100,"height":200,"want_resize":true}"#;
        let crop: CropParams = serde_json::from_str(json).unwrap();
        assert_eq!(crop.x, 10.0);
        assert_eq!(crop.y, 20.0);
        assert_eq!(crop.width, 100.0);
        assert_eq!(crop.height, 200.0);
        assert!(crop.want_resize);
    }

    #[test]
    fn crop_params_want_resize_defaults_false() {
        let json = r#"{"x":0,"y":0,"width":100,"height":100}"#;
        let crop: CropParams = serde_json::from_str(json).unwrap();
        assert!(!crop.want_resize);
    }
}
