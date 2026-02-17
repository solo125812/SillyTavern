//! Sprite endpoints — Phase 3.
//!
//! Mirrors Node's [`src/endpoints/sprites.js`](../../../src/endpoints/sprites.js).
//!
//! ## Endpoints
//! - `GET /api/sprites/get` — list sprite files for a character.
//! - `POST /api/sprites/delete` — delete a sprite.
//! - `POST /api/sprites/upload-zip` — upload sprites from a ZIP (multipart).
//! - `POST /api/sprites/upload` — upload a single sprite (multipart).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Extension, Multipart, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::atomic::atomic_write_file;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Query params for `GET /api/sprites/get`.
#[derive(Debug, Deserialize)]
pub struct GetSpritesQuery {
    /// Character name (may include `/` for subfolder).
    pub name: String,
}

/// A single sprite entry returned by `GET /api/sprites/get`.
#[derive(Debug, Serialize)]
pub struct SpriteEntry {
    /// Extracted label from the filename (e.g., "joy" from "joy-1.png").
    pub label: String,
    /// Client-facing path with cache-busting timestamp.
    pub path: String,
}

/// Request body for `POST /api/sprites/delete`.
#[derive(Debug, Deserialize)]
pub struct DeleteSpriteRequest {
    /// Sprite label to match against filenames.
    pub label: Option<String>,
    /// Character name (may include `/` for subfolder).
    pub name: String,
    /// Explicit sprite name to delete (overrides label if provided).
    #[serde(rename = "spriteName")]
    pub sprite_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Get the path to the sprites folder for a character.
///
/// Mirrors Node's `getSpritesPath()` in `sprites.js:18-38`:
/// - If `is_subfolder`, splits name on `/` and sanitizes each part.
/// - Otherwise sanitizes the name and joins with `characters/`.
pub(crate) fn get_sprites_path(dirs: &UserDirectories, name: &str, is_subfolder: bool) -> Option<PathBuf> {
    if is_subfolder {
        let parts: Vec<&str> = name.splitn(2, '/').collect();
        if parts.len() < 2 {
            return None;
        }
        let character_name = sanitize_filename_strip(parts[0]);
        let subfolder_name = sanitize_filename_strip(parts[1]);
        if character_name.is_empty() || subfolder_name.is_empty() {
            return None;
        }
        Some(dirs.characters.join(&character_name).join(&subfolder_name))
    } else {
        let sanitized = sanitize_filename_strip(name);
        if sanitized.is_empty() {
            return None;
        }
        Some(dirs.characters.join(&sanitized))
    }
}

/// Import base64 sprites from RisuAI character data.
///
/// Mirrors Node's `importRisuSprites()` in `sprites.js:48-113`.
pub fn import_risu_sprites(dirs: &UserDirectories, data: &mut Value) {
    let name = data
        .get("data")
        .and_then(|d| d.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if name.is_none() {
        return;
    }
    let name = name.unwrap();
    let risu = match data
        .get_mut("data")
        .and_then(|d| d.get_mut("extensions"))
        .and_then(|e| e.get_mut("risuai"))
    {
        Some(val) => val,
        None => return,
    };

    let mut images: Vec<(String, String)> = Vec::new();
    for key in ["additionalAssets", "emotions"] {
        if let Some(arr) = risu.get(key).and_then(|v| v.as_array()) {
            for entry in arr {
                if let Some(pair) = entry.as_array() {
                    if pair.len() < 2 {
                        continue;
                    }
                    let label = pair.get(0).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let data = pair.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if label.is_empty() || data.is_empty() {
                        continue;
                    }
                    images.push((label, data));
                }
            }
        }
    }

    if images.is_empty() {
        return;
    }

    let sprites_path = match get_sprites_path(dirs, &name, false) {
        Some(p) => p,
        None => return,
    };

    if let Err(err) = fs::create_dir_all(&sprites_path) {
        tracing::error!("RisuAI: Failed to create sprites directory: {}", err);
        return;
    }

    if !sprites_path.is_dir() {
        return;
    }

    let mut existing = HashSet::new();
    if let Ok(entries) = fs::read_dir(&sprites_path) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if !file_type.is_file() {
                    continue;
                }
            }
            if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                existing.insert(stem.to_string());
            }
        }
    }

    for (label, data) in images {
        if existing.contains(&label) {
            continue;
        }

        let filename = format!("{}.png", label);
        let sanitized = sanitize_filename_strip(&filename);
        if sanitized.is_empty() {
            continue;
        }

        let decoded = match BASE64.decode(data.as_bytes()) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };

        let target = sprites_path.join(&sanitized);
        if atomic_write_file(&target, &decoded).is_ok() {
            existing.insert(label);
        }
    }

    if let Some(obj) = risu.as_object_mut() {
        obj.remove("additionalAssets");
        obj.remove("emotions");
    }
}

/// Check if a file has an image MIME type.
fn is_image_file(filename: &str) -> bool {
    let mime = mime_guess::from_path(filename).first_or_octet_stream();
    mime.type_() == mime_guess::mime::IMAGE
}

/// Extract the label from a sprite filename.
///
/// Mirrors Node's regex: `/^(.+?)(?:[-\\.].*?)?$/`
/// Examples: "joy.png" → "joy", "joy-1.png" → "joy", "joy.expressive.png" → "joy"
fn extract_label(filename: &str) -> String {
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename)
        .to_lowercase();

    // Find the first dash or dot in the stem to split the label
    if let Some(pos) = stem.find(|c: char| c == '-' || c == '.') {
        stem[..pos].to_string()
    } else {
        stem
    }
}

/// Format a sprite mtime in Node's cache-buster style: YYYYMMDDHHMMSS (UTC).
fn format_sprite_timestamp(time: std::time::SystemTime) -> String {
    let dt: DateTime<Utc> = time.into();
    dt.format("%Y%m%d%H%M%S").to_string()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/sprites/get` — list sprite files for a character.
///
/// Mirrors Node's `router.get('/get')` in `sprites.js:118-151`:
/// - Query param `name` is the character name.
/// - Returns array of `{label, path}` entries.
pub async fn get_sprites(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Query(query): Query<GetSpritesQuery>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let name = &query.name;
    let is_subfolder = name.contains('/');
    let sprites_path = get_sprites_path(&dirs, name, is_subfolder);

    let mut sprites: Vec<SpriteEntry> = Vec::new();

    if let Some(ref sp) = sprites_path {
        if sp.exists() && sp.is_dir() {
            if let Ok(entries) = fs::read_dir(sp) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    if !is_image_file(&file_name) {
                        continue;
                    }

                    let mtime = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(format_sprite_timestamp);

                    let label = extract_label(&file_name);
                    let path = match mtime {
                        Some(ts) => format!("/characters/{}/{}?t={}", name, file_name, ts),
                        None => format!("/characters/{}/{}", name, file_name),
                    };

                    sprites.push(SpriteEntry { label, path });
                }
            }
        }
    }

    Json(sprites)
}

/// `POST /api/sprites/delete` — delete a sprite.
///
/// Mirrors Node's `router.post('/delete')` in `sprites.js:153-185`:
/// - Finds files whose stem matches `spriteName` or `label`.
/// - Deletes matching files.
pub async fn delete_sprite(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteSpriteRequest>,
) -> impl IntoResponse {
    let sprite_name = body
        .sprite_name
        .as_deref()
        .or(body.label.as_deref())
        .unwrap_or("");

    if sprite_name.is_empty() || body.name.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let is_subfolder = body.name.contains('/');
    let sprites_path = match get_sprites_path(&dirs, &body.name, is_subfolder) {
        Some(p) => p,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    if !sprites_path.exists() || !sprites_path.is_dir() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let entries = match fs::read_dir(&sprites_path) {
        Ok(entries) => entries,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let file_name = entry.file_name().to_string_lossy().to_string();
        let stem = Path::new(&file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if stem == sprite_name {
            let _ = fs::remove_file(entry.path());
        }
    }

    "OK".into_response()
}

/// `POST /api/sprites/upload-zip` — upload sprites from a ZIP file (multipart).
///
/// Mirrors Node's `router.post('/upload-zip')` in `sprites.js:187-238`:
/// 1. Receives multipart form with `file` field (ZIP) and `name` field (character name).
/// 2. Extracts all images from the ZIP (skips `__MACOSX/` entries and non-image MIME types).
/// 3. Writes each extracted image to the character's sprites folder.
/// 4. Returns `{count: N}` with the number of images extracted.
pub async fn upload_sprite_zip(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    let mut zip_data: Option<Vec<u8>> = None;
    let mut name: Option<String> = None;

    // Parse multipart fields
    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "file" | "avatar" => {
                match field.bytes().await {
                    Ok(bytes) => zip_data = Some(bytes.to_vec()),
                    Err(_) => return StatusCode::BAD_REQUEST.into_response(),
                }
            }
            "name" => {
                if let Ok(text) = field.text().await {
                    name = Some(text);
                }
            }
            _ => {}
        }
    }

    let zip_bytes = match zip_data {
        Some(data) if !data.is_empty() => data,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let name_val = match &name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let is_subfolder = name_val.contains('/');
    let sprites_path = match get_sprites_path(&dirs, &name_val, is_subfolder) {
        Some(p) => p,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Create sprites folder if needed
    if let Err(e) = fs::create_dir_all(&sprites_path) {
        tracing::error!("Failed to create sprites directory: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Extract images from ZIP — mirrors Node's getImageBuffers()
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Failed to open ZIP archive: {}", e);
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let mut count = 0u32;
    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Skip directories
        if entry.is_dir() {
            continue;
        }

        let entry_name = match entry.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };

        let entry_str = entry_name.to_string_lossy().to_string();

        // Skip __MACOSX entries (matches Node's getImageBuffers filter)
        if entry_str.starts_with("__MACOSX") {
            continue;
        }

        // Only extract image files (matches Node's mime.lookup check)
        let mime = mime_guess::from_path(&entry_str).first_or_octet_stream();
        if mime.type_() != mime_guess::mime::IMAGE {
            continue;
        }

        // Read entry data
        let mut data = Vec::new();
        if std::io::Read::read_to_end(&mut entry, &mut data).is_err() {
            continue;
        }

        // Use the basename of the entry as the filename (matches Node's path.parse().base)
        let basename = Path::new(&entry_str)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&entry_str);
        let sanitized = sanitize_filename_strip(basename);
        if sanitized.is_empty() {
            continue;
        }

        let target_path = sprites_path.join(&sanitized);
        if let Err(e) = atomic_write_file(&target_path, &data) {
            tracing::warn!("Failed to write sprite from ZIP: {}", e);
            continue;
        }

        count += 1;
    }

    Json(serde_json::json!({"count": count})).into_response()
}

/// `POST /api/sprites/upload` — upload a single sprite (multipart).
///
/// Mirrors Node's `router.post('/upload')` in `sprites.js:240-290`:
/// - Multipart with file + `label` + `name` fields.
/// - Removes existing sprite with same label/spriteName.
/// - Writes new sprite to character's sprites folder.
pub async fn upload_sprite(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    let mut file_data: Option<Vec<u8>> = None;
    let mut original_name: Option<String> = None;
    let mut label: Option<String> = None;
    let mut name: Option<String> = None;
    let mut sprite_name: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "file" | "avatar" => {
                original_name = field.file_name().map(|s| s.to_string());
                match field.bytes().await {
                    Ok(bytes) => file_data = Some(bytes.to_vec()),
                    Err(_) => return StatusCode::BAD_REQUEST.into_response(),
                }
            }
            "label" => {
                if let Ok(text) = field.text().await {
                    label = Some(text);
                }
            }
            "name" => {
                if let Ok(text) = field.text().await {
                    name = Some(text);
                }
            }
            "spriteName" => {
                if let Ok(text) = field.text().await {
                    sprite_name = Some(text);
                }
            }
            _ => {}
        }
    }

    let file_bytes = match file_data {
        Some(data) if !data.is_empty() => data,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let label_val = match &label {
        Some(l) if !l.is_empty() => l.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let name_val = match &name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let effective_sprite_name = sprite_name
        .as_deref()
        .unwrap_or(&label_val);

    let is_subfolder = name_val.contains('/');
    let sprites_path = match get_sprites_path(&dirs, &name_val, is_subfolder) {
        Some(p) => p,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Create sprites folder if needed
    if let Err(e) = fs::create_dir_all(&sprites_path) {
        tracing::error!("Failed to create sprites directory: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    if !sprites_path.is_dir() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Remove existing sprite with same name
    if let Ok(entries) = fs::read_dir(&sprites_path) {
        for entry in entries.filter_map(|e| e.ok()) {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let stem = Path::new(&file_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if stem == effective_sprite_name {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    // Determine filename: spriteName + original extension
    let ext = original_name
        .as_deref()
        .and_then(|n| Path::new(n).extension())
        .and_then(|e| e.to_str())
        .unwrap_or("png");
    let filename = format!("{}.{}", effective_sprite_name, ext);
    let sanitized = sanitize_filename_strip(&filename);
    let path_to_file = sprites_path.join(&sanitized);

    if let Err(e) = atomic_write_file(&path_to_file, &file_bytes) {
        tracing::error!("Failed to write sprite: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Json(serde_json::json!({"ok": true})).into_response()
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
    fn extract_label_from_filename() {
        assert_eq!(extract_label("joy.png"), "joy");
        assert_eq!(extract_label("joy-1.png"), "joy");
        assert_eq!(extract_label("joy.expressive.png"), "joy");
        assert_eq!(extract_label("anger.jpg"), "anger");
        assert_eq!(extract_label("simple"), "simple");
    }

    #[test]
    fn get_sprites_path_normal() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");

        let path = get_sprites_path(&dirs, "MyCharacter", false);
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.ends_with("characters/MyCharacter"));
    }

    #[test]
    fn get_sprites_path_subfolder() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");

        let path = get_sprites_path(&dirs, "MyCharacter/emotions", true);
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.ends_with("characters/MyCharacter/emotions"));
    }

    #[test]
    fn get_sprites_path_empty_name() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");

        assert!(get_sprites_path(&dirs, "", false).is_none());
    }

    #[test]
    fn get_sprites_path_empty_subfolder() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");

        // Both parts must be non-empty
        assert!(get_sprites_path(&dirs, "/subfolder", true).is_none());
        assert!(get_sprites_path(&dirs, "char/", true).is_none());
    }

    #[test]
    fn delete_sprite_removes_matching_file() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");
        let sprites_dir = dirs.characters.join("TestChar");
        fs::create_dir_all(&sprites_dir).unwrap();

        // Create sprite files
        fs::File::create(sprites_dir.join("joy.png"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();
        fs::File::create(sprites_dir.join("anger.png"))
            .unwrap()
            .write_all(b"fake")
            .unwrap();

        // Delete "joy" sprite
        let entries = fs::read_dir(&sprites_dir).unwrap();
        for entry in entries.filter_map(|e| e.ok()) {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let stem = Path::new(&file_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if stem == "joy" {
                fs::remove_file(entry.path()).unwrap();
            }
        }

        assert!(!sprites_dir.join("joy.png").exists());
        assert!(sprites_dir.join("anger.png").exists());
    }

    #[test]
    fn is_image_file_check() {
        assert!(is_image_file("test.png"));
        assert!(is_image_file("test.jpg"));
        assert!(is_image_file("test.gif"));
        assert!(is_image_file("test.webp"));
        assert!(!is_image_file("test.txt"));
        assert!(!is_image_file("test.json"));
    }

    #[test]
    fn sprite_timestamp_format_matches_node_style() {
        use std::time::{Duration, UNIX_EPOCH};

        assert_eq!(format_sprite_timestamp(UNIX_EPOCH), "19700101000000");
        assert_eq!(
            format_sprite_timestamp(UNIX_EPOCH + Duration::from_secs(1)),
            "19700101000001"
        );
    }

    /// Helper: create an in-memory ZIP archive with given entries.
    fn create_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as IoWrite;
        let buf = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(buf);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn zip_extracts_images_only() {
        let zip_bytes = create_test_zip(&[
            ("joy.png", b"fake-png-data"),
            ("anger.jpg", b"fake-jpg-data"),
            ("readme.txt", b"not an image"),
        ]);

        let dir = TempDir::new().unwrap();
        let out_dir = dir.path().join("sprites");
        fs::create_dir_all(&out_dir).unwrap();

        let cursor = std::io::Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();

        let mut count = 0u32;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            if entry.is_dir() { continue; }
            let entry_name = entry.enclosed_name().unwrap().to_path_buf();
            let entry_str = entry_name.to_string_lossy().to_string();

            let mime = mime_guess::from_path(&entry_str).first_or_octet_stream();
            if mime.type_() != mime_guess::mime::IMAGE { continue; }

            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
            let basename = Path::new(&entry_str).file_name().unwrap().to_str().unwrap();
            fs::write(out_dir.join(basename), &data).unwrap();
            count += 1;
        }

        assert_eq!(count, 2);
        assert!(out_dir.join("joy.png").exists());
        assert!(out_dir.join("anger.jpg").exists());
        assert!(!out_dir.join("readme.txt").exists());
    }

    #[test]
    fn zip_skips_macosx_entries() {
        let zip_bytes = create_test_zip(&[
            ("joy.png", b"real"),
            ("__MACOSX/._joy.png", b"macosx-metadata"),
            ("__MACOSX/.DS_Store", b"macosx-ds"),
        ]);

        let cursor = std::io::Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();

        let mut names: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            let entry = archive.by_index(i).unwrap();
            let entry_name = entry.enclosed_name().unwrap().to_path_buf();
            let entry_str = entry_name.to_string_lossy().to_string();
            if entry_str.starts_with("__MACOSX") { continue; }
            names.push(entry_str);
        }

        assert_eq!(names, vec!["joy.png"]);
    }
}
