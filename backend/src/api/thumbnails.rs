//! Thumbnail endpoint — Phase 2 + Phase 4 generation.
//!
//! Mirrors Node's [`src/endpoints/thumbnails.js`](../../../src/endpoints/thumbnails.js).
//!
//! ## Endpoints
//! - `GET /thumbnail` — serve cached thumbnail or generate on cache miss.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Extension, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use image::imageops::FilterType;
use image::DynamicImage;
use serde::Deserialize;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::atomic::atomic_write_file;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

// ---------------------------------------------------------------------------
// Constants (matching Node's thumbnails.js)
// ---------------------------------------------------------------------------

/// File extensions that are skipped for thumbnail generation (served as-is).
/// Mirrors Node's `SKIPPED_EXTENSIONS` in `thumbnails.js:17`.
const SKIPPED_EXTENSIONS: &[&str] = &[
    ".apng", ".mp4", ".webm", ".avi", ".mkv", ".flv", ".gif",
];

/// Thumbnail type — maps to different source/cache directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailType {
    Bg,
    Avatar,
    Persona,
}

impl ThumbnailType {
    /// Parse from query parameter string.
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "bg" => Some(ThumbnailType::Bg),
            "avatar" => Some(ThumbnailType::Avatar),
            "persona" => Some(ThumbnailType::Persona),
            _ => None,
        }
    }

    /// Get the thumbnail cache folder for this type.
    fn thumbnail_folder<'a>(&self, dirs: &'a UserDirectories) -> &'a Path {
        match self {
            ThumbnailType::Bg => &dirs.thumbnails_bg,
            ThumbnailType::Avatar => &dirs.thumbnails_avatar,
            ThumbnailType::Persona => &dirs.thumbnails_persona,
        }
    }

    /// Get the original images folder for this type.
    fn original_folder<'a>(&self, dirs: &'a UserDirectories) -> &'a Path {
        match self {
            ThumbnailType::Bg => &dirs.backgrounds,
            ThumbnailType::Avatar => &dirs.characters,
            ThumbnailType::Persona => &dirs.avatars,
        }
    }
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Query parameters for `GET /thumbnail`.
#[derive(Debug, Deserialize)]
pub struct ThumbnailQuery {
    /// Filename to get thumbnail for.
    pub file: Option<String>,
    /// Thumbnail type: "bg", "avatar", or "persona".
    #[serde(rename = "type")]
    pub thumb_type: Option<String>,
    /// Whether animated formats should be served as originals.
    pub animated: Option<String>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `GET /thumbnail` — serve cached thumbnail or generate on cache miss.
///
/// Mirrors Node's `publicRouter.get('/')` in `thumbnails.js:249-309`:
/// 1. Validates `file` and `type` query params.
/// 2. Sanitizes filename and rejects if sanitized != original.
/// 3. If thumbnails disabled, serves original.
/// 4. If animated format and `animated=true`, serves original.
/// 5. GIFs always serve original.
/// 6. If cached thumbnail exists and is newer than original, serves it.
/// 7. Otherwise generates a thumbnail, caches it, and serves it.
/// 8. Returns 404 if file not found.
pub async fn thumbnail_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Query(query): Query<ThumbnailQuery>,
) -> Response {
    // Validate required query params
    let raw_file = match &query.file {
        Some(f) if !f.is_empty() => f.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let type_str = match &query.thumb_type {
        Some(t) => t.as_str(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    let thumb_type = match ThumbnailType::from_str(type_str) {
        Some(t) => t,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Sanitize filename — reject if sanitized doesn't match original
    let sanitized = sanitize_filename_strip(raw_file);
    if sanitized != raw_file {
        return StatusCode::FORBIDDEN.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    // Helper: serve original file
    let serve_original = |file: &str, dirs: &UserDirectories| -> Response {
        let folder = thumb_type.original_folder(dirs);
        let path_to_original = folder.join(file);
        let path_to_original = match path_to_original.canonicalize() {
            Ok(p) => p,
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        };
        if !path_to_original.exists() {
            return StatusCode::NOT_FOUND.into_response();
        }
        serve_file(&path_to_original)
    };

    // Check if thumbnails are enabled (config or default true)
    let thumbnails_enabled = state.config.thumbnails_enabled;

    if !thumbnails_enabled {
        return serve_original(&sanitized, &dirs);
    }

    let animated_enabled = query.animated.as_deref() == Some("true");
    let file_ext = Path::new(&sanitized)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();

    let is_animated_format = SKIPPED_EXTENSIONS.contains(&file_ext.as_str());

    // Serve original for animated formats when animated flag is set
    if animated_enabled && is_animated_format {
        return serve_original(&sanitized, &dirs);
    }

    // GIFs always serve original
    if file_ext == ".gif" {
        return serve_original(&sanitized, &dirs);
    }

    // Check if cached thumbnail exists and is still fresh
    let thumbnail_folder = thumb_type.thumbnail_folder(&dirs);
    let cached_path = thumbnail_folder.join(&sanitized);
    let original_folder = thumb_type.original_folder(&dirs);
    let original_path = original_folder.join(&sanitized);

    if cached_path.exists() {
        // Check if original image was updated after thumbnail creation
        let needs_regen = match (original_path.metadata(), cached_path.metadata()) {
            (Ok(orig_meta), Ok(cache_meta)) => {
                match (orig_meta.modified(), cache_meta.modified()) {
                    (Ok(orig_mtime), Ok(cache_mtime)) => orig_mtime > cache_mtime,
                    _ => false,
                }
            }
            _ => false,
        };

        if !needs_regen {
            return serve_file(&cached_path);
        }
    }

    // Generate thumbnail on cache miss (or stale cache)
    let dims = match thumb_type {
        ThumbnailType::Bg => state.config.thumbnail_dimensions.bg,
        ThumbnailType::Avatar => state.config.thumbnail_dimensions.avatar,
        ThumbnailType::Persona => state.config.thumbnail_dimensions.persona,
    };

    match generate_thumbnail(
        &original_path,
        &cached_path,
        thumb_type,
        dims,
        state.config.thumbnail_quality,
        state.config.thumbnail_pngformat,
    ) {
        Some(_) => serve_file(&cached_path),
        None => serve_original(&sanitized, &dirs),
    }
}

/// Serve a file with appropriate content-type based on extension.
fn serve_file(path: &Path) -> Response {
    match std::fs::read(path) {
        Ok(contents) => {
            let mime = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();
            let headers = [(axum::http::header::CONTENT_TYPE, mime)];
            (StatusCode::OK, headers, contents).into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Thumbnail Generation
// ---------------------------------------------------------------------------

/// Generate a thumbnail for an image file.
///
/// Mirrors Node's `generateThumbnail()` in `thumbnails.js:103-144`:
/// - Skips animated formats (WebP with ANIM chunks, APNG with acTL chunks).
/// - Calls `process_single_image()` for the actual resize.
/// - Returns `Some(())` on success, `None` on skip/failure.
fn generate_thumbnail(
    original_path: &Path,
    cached_path: &Path,
    thumb_type: ThumbnailType,
    dims: (u32, u32),
    quality: u8,
    png_format: bool,
) -> Option<()> {
    // Check the file exists
    if !original_path.exists() {
        return None;
    }

    // Read the raw bytes for animated format detection
    let raw_bytes = std::fs::read(original_path).ok()?;

    // Skip animated WebP (has ANIM/ANMF chunks)
    if is_animated_webp(&raw_bytes) {
        return None;
    }

    // Skip animated PNG (APNG — has acTL chunk)
    if is_animated_apng(&raw_bytes) {
        return None;
    }

    process_single_image(&raw_bytes, cached_path, thumb_type, dims, quality, png_format)
}

/// Process a single image into a thumbnail.
///
/// Mirrors Node's `processSingleImage()` in `thumbnails.js:159-242`:
/// - For `bg`: proportional resize maintaining aspect ratio to target pixel area.
/// - For `avatar`/`persona`: cover resize to fixed dimensions.
/// - Encodes as JPEG (default) or PNG based on config.
/// - Writes atomically to the cached path.
fn process_single_image(
    raw_bytes: &[u8],
    cached_path: &Path,
    thumb_type: ThumbnailType,
    dims: (u32, u32),
    quality: u8,
    png_format: bool,
) -> Option<()> {
    let img = image::load_from_memory(raw_bytes).ok()?;
    let (target_w, target_h) = dims;

    let resized = match thumb_type {
        ThumbnailType::Bg => {
            // Proportional resize: maintain aspect ratio to target pixel area
            // Mirrors Node: `image.resize({ w: newWidth, h: newHeight })`
            let (src_w, src_h) = (img.width() as f64, img.height() as f64);
            let target_area = target_w as f64 * target_h as f64;
            let src_area = src_w * src_h;

            if src_area <= target_area {
                // Image already small enough
                img
            } else {
                let scale = (target_area / src_area).sqrt();
                let new_w = (src_w * scale).round().max(1.0) as u32;
                let new_h = (src_h * scale).round().max(1.0) as u32;
                img.resize(new_w, new_h, FilterType::Triangle)
            }
        }
        ThumbnailType::Avatar | ThumbnailType::Persona => {
            // Cover resize: fill exact dimensions, crop overflow
            cover_resize(&img, target_w, target_h)
        }
    };

    // Encode the thumbnail
    let buffer = if png_format {
        let mut buf = Cursor::new(Vec::new());
        resized.write_to(&mut buf, image::ImageFormat::Png).ok()?;
        buf.into_inner()
    } else {
        let mut buf = Cursor::new(Vec::new());
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        resized.write_with_encoder(encoder).ok()?;
        buf.into_inner()
    };

    // Ensure the cache directory exists
    if let Some(parent) = cached_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Write atomically
    atomic_write_file(cached_path, &buffer).ok()?;

    Some(())
}

/// "Cover" resize — resize to fill exact dimensions, cropping overflow.
///
/// Mirrors Jimp's `image.cover({w, h})` behavior.
fn cover_resize(img: &DynamicImage, target_w: u32, target_h: u32) -> DynamicImage {
    if target_w == 0 || target_h == 0 {
        return img.clone();
    }

    let src_w = img.width() as f64;
    let src_h = img.height() as f64;
    let target_w_f = target_w as f64;
    let target_h_f = target_h as f64;

    let scale = (target_w_f / src_w).max(target_h_f / src_h);
    let scaled_w = (src_w * scale).round() as u32;
    let scaled_h = (src_h * scale).round() as u32;

    let resized = img.resize_exact(scaled_w, scaled_h, FilterType::Lanczos3);

    let crop_x = (scaled_w.saturating_sub(target_w)) / 2;
    let crop_y = (scaled_h.saturating_sub(target_h)) / 2;
    resized.crop_imm(crop_x, crop_y, target_w, target_h)
}

// ---------------------------------------------------------------------------
// Animated format detection
// ---------------------------------------------------------------------------

/// Check if raw bytes represent an animated WebP (has ANIM or ANMF chunks).
///
/// Mirrors Node's `isAnimatedWebP()` in `image-metadata.js`.
fn is_animated_webp(data: &[u8]) -> bool {
    // WebP files start with "RIFF" + size + "WEBP"
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return false;
    }
    // Search for ANIM or ANMF chunk headers
    let search = &data[12..];
    for window in search.windows(4) {
        if window == b"ANIM" || window == b"ANMF" {
            return true;
        }
    }
    false
}

/// Check if raw bytes represent an animated PNG (APNG — has acTL chunk).
///
/// Mirrors Node's `isAnimatedApng()` in `image-metadata.js`.
fn is_animated_apng(data: &[u8]) -> bool {
    // PNG starts with 8-byte signature
    if data.len() < 8 || &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return false;
    }
    // Search for "acTL" chunk type marker
    for window in data[8..].windows(4) {
        if window == b"acTL" {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Invalidation helper (used by write endpoints in Phase 3)
// ---------------------------------------------------------------------------

/// Remove a cached thumbnail from disk.
///
/// Mirrors Node's `invalidateThumbnail()` in `thumbnails.js:83-92`.
pub fn invalidate_thumbnail(dirs: &UserDirectories, thumb_type: ThumbnailType, file: &str) {
    let folder = thumb_type.thumbnail_folder(dirs);
    let sanitized = sanitize_filename_strip(file);
    let path = folder.join(sanitized);
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_type_parsing() {
        assert_eq!(ThumbnailType::from_str("bg"), Some(ThumbnailType::Bg));
        assert_eq!(ThumbnailType::from_str("avatar"), Some(ThumbnailType::Avatar));
        assert_eq!(ThumbnailType::from_str("persona"), Some(ThumbnailType::Persona));
        assert_eq!(ThumbnailType::from_str("invalid"), None);
        assert_eq!(ThumbnailType::from_str(""), None);
    }

    #[test]
    fn thumbnail_type_folders() {
        let dirs = UserDirectories::new(Path::new("/data"), "test-user");
        let bg = ThumbnailType::Bg;
        assert_eq!(bg.thumbnail_folder(&dirs), Path::new("/data/test-user/thumbnails/bg"));
        assert_eq!(bg.original_folder(&dirs), Path::new("/data/test-user/backgrounds"));

        let avatar = ThumbnailType::Avatar;
        assert_eq!(avatar.thumbnail_folder(&dirs), Path::new("/data/test-user/thumbnails/avatar"));
        assert_eq!(avatar.original_folder(&dirs), Path::new("/data/test-user/characters"));

        let persona = ThumbnailType::Persona;
        assert_eq!(persona.thumbnail_folder(&dirs), Path::new("/data/test-user/thumbnails/persona"));
        assert_eq!(persona.original_folder(&dirs), Path::new("/data/test-user/User Avatars"));
    }

    #[test]
    fn skipped_extensions_contains_common_formats() {
        assert!(SKIPPED_EXTENSIONS.contains(&".gif"));
        assert!(SKIPPED_EXTENSIONS.contains(&".mp4"));
        assert!(SKIPPED_EXTENSIONS.contains(&".webm"));
        assert!(SKIPPED_EXTENSIONS.contains(&".apng"));
        assert!(!SKIPPED_EXTENSIONS.contains(&".png"));
        assert!(!SKIPPED_EXTENSIONS.contains(&".jpg"));
    }

    #[test]
    fn invalidate_thumbnail_removes_cached_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "user");

        // Create the thumbnail directory and a cached file
        std::fs::create_dir_all(dirs.thumbnails_bg.clone()).unwrap();
        let cached = dirs.thumbnails_bg.join("test.png");
        std::fs::write(&cached, b"fake thumbnail").unwrap();
        assert!(cached.exists());

        invalidate_thumbnail(&dirs, ThumbnailType::Bg, "test.png");
        assert!(!cached.exists());
    }

    #[test]
    fn invalidate_thumbnail_noop_when_no_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "user");
        // Should not panic even if directory doesn't exist
        invalidate_thumbnail(&dirs, ThumbnailType::Avatar, "nonexistent.png");
    }

    /// Create a minimal test PNG of given dimensions.
    fn make_test_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(
            width, height,
            image::Rgba([100, 150, 200, 255]),
        );
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn thumbnail_generation_bg_proportional_resize() {
        let dir = tempfile::TempDir::new().unwrap();
        let original = dir.path().join("bg.png");
        let cached = dir.path().join("cached_bg.png");

        // Create a large source image (1920x1080)
        let png = make_test_png(1920, 1080);
        std::fs::write(&original, &png).unwrap();

        let result = generate_thumbnail(
            &original, &cached, ThumbnailType::Bg,
            (160, 90), 95, false,
        );
        assert!(result.is_some());
        assert!(cached.exists());

        // Verify the thumbnail is smaller than original
        let thumb_bytes = std::fs::read(&cached).unwrap();
        assert!(thumb_bytes.len() < png.len());
    }

    #[test]
    fn thumbnail_generation_avatar_cover_resize() {
        let dir = tempfile::TempDir::new().unwrap();
        let original = dir.path().join("avatar.png");
        let cached = dir.path().join("cached_avatar.png");

        let png = make_test_png(512, 768);
        std::fs::write(&original, &png).unwrap();

        let result = generate_thumbnail(
            &original, &cached, ThumbnailType::Avatar,
            (96, 144), 95, true, // PNG format
        );
        assert!(result.is_some());
        assert!(cached.exists());

        let decoded = image::load_from_memory(&std::fs::read(&cached).unwrap()).unwrap();
        assert_eq!(decoded.width(), 96);
        assert_eq!(decoded.height(), 144);
    }

    #[test]
    fn thumbnail_skips_animated_webp() {
        // Construct a minimal animated WebP-like header
        let mut data = Vec::new();
        data.extend(b"RIFF");
        data.extend(&100u32.to_le_bytes());
        data.extend(b"WEBP");
        data.extend(b"ANIM"); // animated chunk marker
        data.extend(b"\x00\x00\x00\x00");

        assert!(is_animated_webp(&data));
    }

    #[test]
    fn thumbnail_skips_animated_apng() {
        // PNG signature + acTL marker
        let mut data = Vec::new();
        data.extend(b"\x89PNG\r\n\x1a\n");
        data.extend(b"\x00\x00\x00\x08IHDR"); // fake IHDR
        data.extend(&[0u8; 17]);
        data.extend(b"\x00\x00\x00\x08acTL"); // acTL chunk = animated

        assert!(is_animated_apng(&data));
    }

    #[test]
    fn non_animated_webp_passes() {
        let mut data = Vec::new();
        data.extend(b"RIFF");
        data.extend(&100u32.to_le_bytes());
        data.extend(b"WEBP");
        data.extend(b"VP8 "); // static webp
        data.extend(&[0u8; 20]);

        assert!(!is_animated_webp(&data));
    }

    #[test]
    fn non_png_data_not_apng() {
        assert!(!is_animated_apng(b"not a png file at all"));
    }

    #[test]
    fn thumbnail_cache_hit_no_regen() {
        let dir = tempfile::TempDir::new().unwrap();
        let original = dir.path().join("test.png");
        let cached = dir.path().join("cached_test.png");

        let png = make_test_png(200, 200);
        std::fs::write(&original, &png).unwrap();

        // Generate the thumbnail first
        generate_thumbnail(
            &original, &cached, ThumbnailType::Bg,
            (160, 90), 95, false,
        );
        assert!(cached.exists());

        let first_contents = std::fs::read(&cached).unwrap();

        // Generate again — should produce same result (cache is fresh)
        // Since we're testing the generation function directly (not the handler),
        // a second call will regenerate. The handler checks mtime.
        generate_thumbnail(
            &original, &cached, ThumbnailType::Bg,
            (160, 90), 95, false,
        );
        let second_contents = std::fs::read(&cached).unwrap();
        assert_eq!(first_contents, second_contents);
    }

    #[test]
    fn process_bg_small_image_not_upscaled() {
        // Image smaller than target area should not be upscaled
        let png = make_test_png(50, 50);
        let dir = tempfile::TempDir::new().unwrap();
        let cached = dir.path().join("small.jpg");

        let result = process_single_image(
            &png, &cached, ThumbnailType::Bg,
            (160, 90), 95, false,
        );
        assert!(result.is_some());

        // Decode and verify dimensions match original (no upscale)
        let decoded = image::load_from_memory(&std::fs::read(&cached).unwrap()).unwrap();
        assert_eq!(decoded.width(), 50);
        assert_eq!(decoded.height(), 50);
    }
}

