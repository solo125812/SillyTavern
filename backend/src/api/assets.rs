//! Asset endpoints — Phase 3.
//!
//! Mirrors Node's [`src/endpoints/assets.js`](../../../src/endpoints/assets.js).
//!
//! ## Endpoints
//! - `POST /api/assets/get` — list assets by category.
//! - `POST /api/assets/download` — download an asset from URL.
//! - `POST /api/assets/delete` — delete an asset.
//! - `POST /api/assets/character` — list per-character assets.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Extension, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    Json,
};
use serde::Deserialize;
use url::Url;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::{sanitize_filename_strip, validate_asset_filename};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Valid asset categories — mirrors Node's `VALID_CATEGORIES`.
const VALID_CATEGORIES: &[&str] = &["bgm", "ambient", "blip", "live2d", "vrm", "character", "temp"];

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Request body for `POST /api/assets/download`.
#[derive(Debug, Deserialize)]
pub struct DownloadAssetRequest {
    /// URL to download the asset from.
    pub url: String,
    /// Asset category (bgm, ambient, blip, live2d, vrm, character, temp).
    pub category: String,
    /// Filename for the downloaded asset.
    pub filename: String,
}

/// Request body for `POST /api/assets/delete`.
#[derive(Debug, Deserialize)]
pub struct DeleteAssetRequest {
    /// Asset category.
    pub category: String,
    /// Filename to delete.
    pub filename: String,
}

/// Query params for `POST /api/assets/character`.
#[derive(Debug, Deserialize)]
pub struct CharacterAssetsQuery {
    /// Character name.
    pub name: Option<String>,
    /// Asset category.
    pub category: Option<String>,
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Validate that a category is in the allowed list.
fn validate_category(input: &str) -> Option<&str> {
    VALID_CATEGORIES.iter().find(|&&c| c == input).copied()
}

/// Extract hostname from a URL. Returns empty string if parsing fails.
fn get_host_from_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default()
}

/// Check if a host is in the whitelist.
fn is_host_whitelisted(host: &str, whitelist: &[String]) -> bool {
    whitelist.iter().any(|allowed| allowed == host)
}

/// Ensure that asset category folders exist.
///
/// Mirrors Node's `ensureFoldersExist()` in `assets.js:83-95`.
fn ensure_folders_exist(dirs: &UserDirectories) {
    for category in VALID_CATEGORIES {
        let category_path = dirs.assets.join(category);
        if category_path.exists() && !category_path.is_dir() {
            let _ = fs::remove_file(&category_path);
        }
        if !category_path.exists() {
            let _ = fs::create_dir_all(&category_path);
        }
    }
}

/// Recursively get all files in a directory.
///
/// Mirrors Node's `getFiles()` in `assets.js:59-77`.
fn get_files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !dir.exists() {
        return files;
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return files,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            files.extend(get_files_recursive(&path));
        } else {
            files.push(path);
        }
    }

    files
}

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

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/assets/get` — list all assets by category.
///
/// Mirrors Node's `router.post('/get')` in `assets.js:107-181`:
/// - Returns a map of categories to file lists.
/// - Skips "temp" category.
/// - Special handling for live2d (model JSON files) and vrm (model + animation).
/// - Other categories list files, excluding `.placeholder`.
pub async fn get_assets(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let folder_path = &dirs.assets;
    let mut output = serde_json::Map::new();

    if folder_path.exists() && folder_path.is_dir() {
        ensure_folders_exist(&dirs);

        let folders: Vec<String> = match fs::read_dir(folder_path) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect(),
            Err(_) => Vec::new(),
        };

        for folder in &folders {
            if folder == "temp" {
                continue;
            }

            if folder == "live2d" {
                let mut live2d_files = Vec::new();
                let live2d_folder = folder_path.join(folder);
                let files = get_files_recursive(&live2d_folder);
                for file in files {
                    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.contains("model") && name.ends_with(".json") {
                        live2d_files.push(serde_json::Value::String(
                            client_relative_path(&dirs.root, &file),
                        ));
                    }
                }
                output.insert(folder.clone(), serde_json::Value::Array(live2d_files));
                continue;
            }

            if folder == "vrm" {
                let mut vrm_output = serde_json::Map::new();

                // Models
                let model_folder = folder_path.join("vrm/model");
                let mut models = Vec::new();
                for file in get_files_recursive(&model_folder) {
                    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name != ".placeholder" {
                        models.push(serde_json::Value::String(
                            client_relative_path(&dirs.root, &file),
                        ));
                    }
                }
                vrm_output.insert("model".to_string(), serde_json::Value::Array(models));

                // Animations
                let animation_folder = folder_path.join("vrm/animation");
                let mut animations = Vec::new();
                for file in get_files_recursive(&animation_folder) {
                    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name != ".placeholder" {
                        animations.push(serde_json::Value::String(
                            client_relative_path(&dirs.root, &file),
                        ));
                    }
                }
                vrm_output.insert("animation".to_string(), serde_json::Value::Array(animations));

                output.insert(folder.clone(), serde_json::Value::Object(vrm_output));
                continue;
            }

            // Other categories (bgm, ambient, blip)
            let category_path = folder_path.join(folder);
            let mut files_list = Vec::new();
            if let Ok(entries) = fs::read_dir(&category_path) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name != ".placeholder" {
                        files_list.push(serde_json::Value::String(format!(
                            "assets/{}/{}",
                            folder, name
                        )));
                    }
                }
            }
            output.insert(folder.clone(), serde_json::Value::Array(files_list));
        }
    }

    Json(serde_json::Value::Object(output))
}

/// `POST /api/assets/download` — download an asset from URL.
///
/// Mirrors Node's `router.post('/download')` in `assets.js:191-252`:
/// - Validates category and filename.
/// - Downloads to temp, then moves to category folder.
/// - For "character" category, returns file content directly.
pub async fn download_asset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DownloadAssetRequest>,
) -> impl IntoResponse {
    // Validate URL
    if Url::parse(&body.url).is_err() {
        tracing::warn!("Asset download failed: Must be a valid URL");
        return (StatusCode::BAD_REQUEST, "Bad Request").into_response();
    }

    // Enforce import host whitelist
    let host = get_host_from_url(&body.url);
    if !is_host_whitelisted(&host, &state.config.whitelist_import_domains) {
        tracing::error!(
            "Received an import for \"{}\", but site is not whitelisted. This domain must be added to the config key \"whitelistImportDomains\" to allow import from this source.",
            host
        );
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    let category = match validate_category(&body.category) {
        Some(c) => c,
        None => {
            tracing::error!("Bad request: unsupported asset category.");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    ensure_folders_exist(&dirs);

    // Validate filename
    let validation = validate_asset_filename(&body.filename);
    if !validation.valid {
        let msg = validation.message.unwrap_or_default();
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    let temp_path = dirs.assets.join("temp").join(&body.filename);
    let file_path = dirs.assets.join(category).join(&body.filename);

    tracing::info!("Request received to download {} to {}", body.url, file_path.display());

    // Download to temp
    let response = match reqwest::get(&body.url).await {
        Ok(resp) if resp.status().is_success() => resp,
        Ok(resp) => {
            tracing::error!("Unexpected response: {}", resp.status());
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Err(e) => {
            tracing::error!("Download failed: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("Failed to read response body: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Delete if previous download failed
    if temp_path.exists() {
        let _ = fs::remove_file(&temp_path);
    }

    if let Err(e) = fs::write(&temp_path, &bytes) {
        tracing::error!("Failed to write temp file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // For "character" category, return file content directly
    if category == "character" {
        let content_type = mime_guess::from_path(&temp_path)
            .first_or_octet_stream()
            .to_string();
        match fs::read(&temp_path) {
            Ok(file_content) => {
                let _ = fs::remove_file(&temp_path);
                return (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, content_type)],
                    file_content,
                )
                    .into_response();
            }
            Err(e) => {
                tracing::error!("Failed to read temp file: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }

    // Move into asset place
    tracing::info!("Download finished, moving file from {} to {}",
        temp_path.display(), file_path.display());

    if let Err(e) = fs::copy(&temp_path, &file_path) {
        tracing::error!("Failed to copy asset: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let _ = fs::remove_file(&temp_path);
    "OK".into_response()
}

/// `POST /api/assets/delete` — delete an asset.
///
/// Mirrors Node's `router.post('/delete')` in `assets.js:262-303`.
pub async fn delete_asset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteAssetRequest>,
) -> impl IntoResponse {
    let category = match validate_category(&body.category) {
        Some(c) => c,
        None => {
            tracing::error!("Bad request: unsupported asset category.");
            return (StatusCode::BAD_REQUEST, "Bad Request").into_response();
        }
    };

    let validation = validate_asset_filename(&body.filename);
    if !validation.valid {
        let msg = validation.message.unwrap_or_default();
        return (StatusCode::BAD_REQUEST, Html(msg)).into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let file_path = dirs.assets.join(category).join(&body.filename);

    tracing::info!("Request received to delete {} {}", category, file_path.display());

    if file_path.exists() {
        if let Err(e) = fs::remove_file(&file_path) {
            tracing::error!("Failed to delete asset: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        tracing::info!("Asset deleted.");
    } else {
        tracing::error!("Asset not found.");
        return (StatusCode::BAD_REQUEST, "Bad Request").into_response();
    }

    "OK".into_response()
}

/// `POST /api/assets/character` — list per-character assets.
///
/// Mirrors Node's `router.post('/character')` in `assets.js:314-370`:
/// - Query params: `name` (character), `category`.
/// - Returns array of asset paths.
pub async fn character_assets(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Query(query): Query<CharacterAssetsQuery>,
) -> impl IntoResponse {
    let name_raw = match &query.name {
        Some(n) => n.clone(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let input_category = match &query.category {
        Some(c) => c.clone(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Sanitize character name (for backwards compatibility, don't reject, just sanitize)
    let name = sanitize_filename_strip(&name_raw);

    let category = match validate_category(&input_category) {
        Some(c) => c,
        None => {
            tracing::error!("Bad request: unsupported asset category.");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let folder_path = dirs.characters.join(&name).join(category);

    let mut output: Vec<String> = Vec::new();

    if folder_path.exists() && folder_path.is_dir() {
        // Live2d assets
        if category == "live2d" {
            if let Ok(entries) = fs::read_dir(&folder_path) {
                for entry in entries.filter_map(|e| e.ok()) {
                    if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                        continue;
                    }
                    let model_folder = entry.file_name().to_string_lossy().to_string();
                    let live2d_model_path = folder_path.join(&model_folder);
                    if let Ok(files) = fs::read_dir(&live2d_model_path) {
                        for file in files.filter_map(|e| e.ok()) {
                            let fname = file.file_name().to_string_lossy().to_string();
                            if fname.contains("model") && fname.ends_with(".json") {
                                output.push(format!(
                                    "characters/{}/{}/{}/{}",
                                    name, category, model_folder, fname
                                ));
                            }
                        }
                    }
                }
            }
            return Json(output).into_response();
        }

        // Other assets
        if let Ok(entries) = fs::read_dir(&folder_path) {
            for entry in entries.filter_map(|e| e.ok()) {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname != ".placeholder" {
                    output.push(format!("/characters/{}/{}/{}", name, category, fname));
                }
            }
        }
    }

    Json(output).into_response()
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
    fn validate_category_accepts_valid() {
        assert_eq!(validate_category("bgm"), Some("bgm"));
        assert_eq!(validate_category("ambient"), Some("ambient"));
        assert_eq!(validate_category("character"), Some("character"));
        assert_eq!(validate_category("temp"), Some("temp"));
    }

    #[test]
    fn validate_category_rejects_invalid() {
        assert_eq!(validate_category("invalid"), None);
        assert_eq!(validate_category(""), None);
    }

    #[test]
    fn ensure_folders_creates_categories() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&dirs.assets).unwrap();

        ensure_folders_exist(&dirs);

        for category in VALID_CATEGORIES {
            assert!(dirs.assets.join(category).exists());
            assert!(dirs.assets.join(category).is_dir());
        }
    }

    #[test]
    fn get_files_recursive_finds_nested_files() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("a/b");
        fs::create_dir_all(&sub).unwrap();

        fs::File::create(dir.path().join("top.txt"))
            .unwrap()
            .write_all(b"1")
            .unwrap();
        fs::File::create(sub.join("deep.txt"))
            .unwrap()
            .write_all(b"2")
            .unwrap();

        let files = get_files_recursive(dir.path());
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn download_request_deserialization() {
        let json = r#"{"url": "https://example.com/file.mp3", "category": "bgm", "filename": "song.mp3"}"#;
        let req: DownloadAssetRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.url, "https://example.com/file.mp3");
        assert_eq!(req.category, "bgm");
        assert_eq!(req.filename, "song.mp3");
    }

    #[test]
    fn delete_request_deserialization() {
        let json = r#"{"category": "bgm", "filename": "song.mp3"}"#;
        let req: DeleteAssetRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.category, "bgm");
        assert_eq!(req.filename, "song.mp3");
    }

    #[test]
    fn character_assets_query_deserialization() {
        let json = r#"{"name": "Alice", "category": "bgm"}"#;
        let query: CharacterAssetsQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.name, Some("Alice".to_string()));
        assert_eq!(query.category, Some("bgm".to_string()));
    }
}
