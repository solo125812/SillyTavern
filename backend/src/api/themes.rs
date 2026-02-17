//! Themes endpoints — Phase 2 list helper + Phase 3 writes.
//!
//! Mirrors Node's [`src/endpoints/themes.js`](../../../src/endpoints/themes.js)
//! and the list helper from [`src/endpoints/settings.js`](../../../src/endpoints/settings.js).
//!
//! ## Endpoints
//! - `POST /api/themes/save` — save a theme (Phase 3).
//! - `POST /api/themes/delete` — delete a theme (Phase 3).
//!
//! ## Helper
//! - `list_themes()` — read/parse theme JSON files for `/api/settings/get` (Phase 8).

use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Request body for `POST /api/themes/save` and `POST /api/themes/delete`.
///
/// The full body is saved as-is for `/save`; only `name` is needed for `/delete`.
#[derive(Debug, Deserialize)]
pub struct ThemeRequest {
    /// Theme name (used as filename).
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// List helper (Phase 2)
// ---------------------------------------------------------------------------

/// Read and parse JSON files from a directory.
///
/// Mirrors Node's `readAndParseFromDirectory()` in `settings.js:49-68`.
pub fn read_and_parse_from_directory(
    directory: &Path,
    extension: &str,
) -> Vec<serde_json::Value> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut files: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
        })
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| {
            Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{}", e) == extension)
                .unwrap_or(false)
        })
        .collect();

    files.sort();

    let mut parsed = Vec::new();
    for file in &files {
        let path = directory.join(file);
        match fs::read_to_string(&path) {
            Ok(contents) => {
                if extension == ".json" {
                    match serde_json::from_str::<serde_json::Value>(&contents) {
                        Ok(value) => parsed.push(value),
                        Err(_) => continue,
                    }
                } else {
                    parsed.push(serde_json::Value::String(contents));
                }
            }
            Err(_) => continue,
        }
    }

    parsed
}

/// List theme files from the themes directory.
pub fn list_themes(themes_dir: &Path) -> Vec<serde_json::Value> {
    read_and_parse_from_directory(themes_dir, ".json")
}

// ---------------------------------------------------------------------------
// Handlers (Phase 3)
// ---------------------------------------------------------------------------

/// `POST /api/themes/save` — save a theme.
///
/// Mirrors Node's `router.post('/save')` in `themes.js:10-19`:
/// - Expects JSON body with `name` field.
/// - Writes the entire body as pretty-printed JSON to `themes/{name}.json`.
pub async fn save_theme(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let sanitized_name = sanitize_filename_strip(&format!("{}.json", name));
    let filename = dirs.themes.join(&sanitized_name);

    // Ensure themes directory exists
    if let Err(e) = fs::create_dir_all(&dirs.themes) {
        tracing::error!("Failed to create themes directory: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Write body as pretty-printed JSON (mirrors Node: JSON.stringify(body, null, 4))
    let content = match to_pretty_json_4(&body) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to serialize theme: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if let Err(e) = fs::write(&filename, content) {
        tracing::error!("Failed to write theme: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    "OK".into_response()
}

/// `POST /api/themes/delete` — delete a theme.
///
/// Mirrors Node's `router.post('/delete')` in `themes.js:21-38`:
/// - Expects `{name}` in JSON body.
/// - Deletes `themes/{name}.json`.
pub async fn delete_theme(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ThemeRequest>,
) -> impl IntoResponse {
    let name = match &body.name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let sanitized_name = sanitize_filename_strip(&format!("{}.json", name));
    let filename = dirs.themes.join(&sanitized_name);

    if !filename.exists() {
        tracing::error!("Theme file not found: {}", filename.display());
        return StatusCode::NOT_FOUND.into_response();
    }

    if let Err(e) = fs::remove_file(&filename) {
        tracing::error!("Failed to delete theme: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    "OK".into_response()
}

fn to_pretty_json_4(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut serializer)?;
    Ok(String::from_utf8(buf).unwrap_or_default())
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
    fn read_and_parse_json_files() {
        let dir = TempDir::new().unwrap();

        let theme1 = serde_json::json!({"name": "dark", "color": "#000"});
        let theme2 = serde_json::json!({"name": "light", "color": "#fff"});

        fs::File::create(dir.path().join("dark.json"))
            .unwrap()
            .write_all(serde_json::to_string_pretty(&theme1).unwrap().as_bytes())
            .unwrap();
        fs::File::create(dir.path().join("light.json"))
            .unwrap()
            .write_all(serde_json::to_string_pretty(&theme2).unwrap().as_bytes())
            .unwrap();
        fs::File::create(dir.path().join("readme.txt"))
            .unwrap()
            .write_all(b"not a theme")
            .unwrap();

        let result = read_and_parse_from_directory(dir.path(), ".json");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["name"], "dark");
        assert_eq!(result[1]["name"], "light");
    }

    #[test]
    fn read_and_parse_skips_invalid_json() {
        let dir = TempDir::new().unwrap();

        fs::File::create(dir.path().join("good.json"))
            .unwrap()
            .write_all(b"{\"name\": \"valid\"}")
            .unwrap();
        fs::File::create(dir.path().join("bad.json"))
            .unwrap()
            .write_all(b"this is not json {{{")
            .unwrap();

        let result = read_and_parse_from_directory(dir.path(), ".json");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "valid");
    }

    #[test]
    fn read_and_parse_empty_directory() {
        let dir = TempDir::new().unwrap();
        let result = read_and_parse_from_directory(dir.path(), ".json");
        assert!(result.is_empty());
    }

    #[test]
    fn read_and_parse_nonexistent_directory() {
        let result =
            read_and_parse_from_directory(Path::new("/nonexistent/dir/for/test"), ".json");
        assert!(result.is_empty());
    }

    #[test]
    fn read_and_parse_non_json_extension() {
        let dir = TempDir::new().unwrap();

        fs::File::create(dir.path().join("preset1.txt"))
            .unwrap()
            .write_all(b"raw text content")
            .unwrap();
        fs::File::create(dir.path().join("preset2.txt"))
            .unwrap()
            .write_all(b"more content")
            .unwrap();

        let result = read_and_parse_from_directory(dir.path(), ".txt");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].as_str().unwrap(), "raw text content");
        assert_eq!(result[1].as_str().unwrap(), "more content");
    }

    #[test]
    fn list_themes_returns_parsed_json() {
        let dir = TempDir::new().unwrap();

        let theme = serde_json::json!({"name": "monokai", "background": "#272822"});
        fs::File::create(dir.path().join("monokai.json"))
            .unwrap()
            .write_all(serde_json::to_string(&theme).unwrap().as_bytes())
            .unwrap();

        let result = list_themes(dir.path());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "monokai");
    }

    #[test]
    fn save_and_delete_theme() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&dirs.themes).unwrap();

        let body = serde_json::json!({"name": "test-theme", "color": "#ff0000"});
        let sanitized = sanitize_filename_strip(&format!("{}.json", "test-theme"));
        let filename = dirs.themes.join(&sanitized);

        // Save
        let content = serde_json::to_string_pretty(&body).unwrap();
        fs::write(&filename, &content).unwrap();
        assert!(filename.exists());

        // Verify content
        let read_back: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&filename).unwrap()).unwrap();
        assert_eq!(read_back["name"], "test-theme");
        assert_eq!(read_back["color"], "#ff0000");

        // Delete
        fs::remove_file(&filename).unwrap();
        assert!(!filename.exists());
    }
}
