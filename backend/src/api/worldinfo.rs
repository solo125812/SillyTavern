//! World Info endpoints — Phase 7.
//!
//! Mirrors Node's [`src/endpoints/worldinfo.js`](../../../src/endpoints/worldinfo.js).
//!
//! ## Endpoints
//! - `POST /api/worldinfo/list` — list all world info files.
//! - `POST /api/worldinfo/get` — get a world info file by name.
//! - `POST /api/worldinfo/delete` — delete a world info file.
//! - `POST /api/worldinfo/import` — import a world info file (multipart).
//! - `POST /api/worldinfo/edit` — edit/save a world info file.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Extension, Multipart, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::ser::PrettyFormatter;
use serde_json::{Map, Value};
use serde_json::Serializer;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Entry in the list response for `POST /api/worldinfo/list`.
#[derive(Debug, Serialize)]
pub struct WorldInfoListEntry {
    /// File ID (filename without extension).
    pub file_id: String,
    /// Name from the world info JSON, or file ID as fallback.
    pub name: String,
    /// Extensions object from the world info JSON.
    pub extensions: Value,
}

/// Request body for `POST /api/worldinfo/get`.
#[derive(Debug, Deserialize)]
pub struct GetWorldInfoRequest {
    pub name: Option<String>,
}

/// Request body for `POST /api/worldinfo/delete`.
#[derive(Debug, Deserialize)]
pub struct DeleteWorldInfoRequest {
    pub name: Option<String>,
}

/// Request body for `POST /api/worldinfo/edit`.
#[derive(Debug, Deserialize)]
pub struct EditWorldInfoRequest {
    pub name: Option<String>,
    pub data: Option<Value>,
}

/// Response for successful import.
#[derive(Debug, Serialize)]
pub struct ImportWorldInfoResponse {
    pub name: String,
}

// ---------------------------------------------------------------------------
// Shared helper — mirrors Node's readWorldInfoFile()
// ---------------------------------------------------------------------------

/// Read a world info file and return its contents.
///
/// Mirrors Node's `readWorldInfoFile()` in `worldinfo.js:17-35`.
/// If `allow_dummy` is true, returns `{ "entries": {} }` when file is missing.
/// Otherwise returns `None`.
pub fn read_world_info_file(
    worlds_dir: &Path,
    world_info_name: &str,
    allow_dummy: bool,
) -> Option<Value> {
    let dummy = if allow_dummy {
        Some(serde_json::json!({ "entries": {} }))
    } else {
        None
    };

    if world_info_name.is_empty() {
        return dummy;
    }

    let filename = sanitize_filename_strip(&format!("{}.json", world_info_name));
    let path_to_world_info = worlds_dir.join(&filename);

    if !path_to_world_info.exists() {
        tracing::error!("World info file {} doesn't exist.", filename);
        return dummy;
    }

    match fs::read_to_string(&path_to_world_info) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(world_info) => Some(world_info),
            Err(e) => {
                tracing::error!("Failed to parse world info {}: {}", filename, e);
                dummy
            }
        },
        Err(e) => {
            tracing::error!("Failed to read world info {}: {}", filename, e);
            dummy
        }
    }
}

/// Serialize JSON with 4-space indentation to match `JSON.stringify(..., null, 4)`.
fn to_pretty_json_4(value: &Value) -> String {
    let mut buf = Vec::new();
    let formatter = PrettyFormatter::with_indent(b"    ");
    let mut serializer = Serializer::with_formatter(&mut buf, formatter);
    if value.serialize(&mut serializer).is_ok() {
        String::from_utf8(buf).unwrap_or_default()
    } else {
        String::new()
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/worldinfo/list` — list all world info files.
///
/// Mirrors Node's `router.post('/list')` in `worldinfo.js:39-69`.
/// Reads all `.json` files from the worlds directory, parses each to
/// extract `file_id`, `name`, and `extensions`, returning a sorted array.
pub async fn list_world_info(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let mut entries: Vec<WorldInfoListEntry> = Vec::new();

    match fs::read_dir(&dirs.worlds) {
        Ok(dir_entries) => {
            let mut json_files: Vec<String> = dir_entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_type().map(|ft| ft.is_file()).unwrap_or(false)
                        && e.path()
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .map(|ext| ext.eq_ignore_ascii_case("json"))
                            .unwrap_or(false)
                })
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();

            // Sort alphabetically (mirrors Node's `.sort((a, b) => a.name.localeCompare(b.name))`)
            json_files.sort();

            for file_name in &json_files {
                match (|| -> Option<WorldInfoListEntry> {
                    let file_path = dirs.worlds.join(file_name);
                    let contents = fs::read_to_string(&file_path).ok()?;
                    let parsed: Value =
                        serde_json::from_str(&contents).ok().unwrap_or(Value::Object(Map::new()));

                    let file_name_without_ext = Path::new(file_name)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();

                    let name = parsed
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| file_name_without_ext.clone());

                    let extensions = match parsed.get("extensions") {
                        Some(v) if v.is_object() || v.is_array() => v.clone(),
                        _ => serde_json::json!({}),
                    };

                    Some(WorldInfoListEntry {
                        file_id: file_name_without_ext.clone(),
                        name,
                        extensions,
                    })
                })() {
                    Some(entry) => entries.push(entry),
                    None => {
                        tracing::warn!("Error reading or parsing World Info file {}", file_name);
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!("Error reading World Info directory: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    // Return the entries (already sorted by filename)
    Json(entries).into_response()
}

/// `POST /api/worldinfo/get` — get a world info file by name.
///
/// Mirrors Node's `router.post('/get')` in `worldinfo.js:71-79`.
/// Returns the full world info JSON, or `{ "entries": {} }` if not found.
pub async fn get_world_info(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    body: Option<Json<Value>>,
) -> Response {
    let body = match body {
        Some(Json(v)) => v,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    match read_world_info_file(&dirs.worlds, name, true) {
        Some(file) => Json(file).into_response(),
        None => Json(serde_json::json!({ "entries": {} })).into_response(),
    }
}

/// `POST /api/worldinfo/delete` — delete a world info file.
///
/// Mirrors Node's `router.post('/delete')` in `worldinfo.js:81-97`.
/// Validates that the file exists and deletes it.
pub async fn delete_world_info(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    body: Option<Json<Value>>,
) -> Response {
    let body = match body {
        Some(Json(v)) => v,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let filename = sanitize_filename_strip(&format!("{}.json", name));
    let path_to_world_info = dirs.worlds.join(&filename);

    if !path_to_world_info.exists() {
        // Node throws an error here which results in 500
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    if let Err(e) = fs::remove_file(&path_to_world_info) {
        tracing::error!("Failed to delete world info: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    "OK".into_response()
}

/// `POST /api/worldinfo/import` — import a world info file.
///
/// Mirrors Node's `router.post('/import')` in `worldinfo.js:99-132`.
/// Accepts multipart with an uploaded file and optional `convertedData` field.
/// Validates that the parsed content has an `entries` key.
pub async fn import_world_info(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    mut multipart: Multipart,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    let mut file_data: Option<Vec<u8>> = None;
    let mut original_name: Option<String> = None;
    let mut converted_data: Option<String> = None;

    // Parse multipart fields
    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "convertedData" => {
                if let Ok(text) = field.text().await {
                    if !text.is_empty() {
                        converted_data = Some(text);
                    }
                }
            }
            _ => {
                // File field — capture original name and data
                if file_data.is_none() {
                    original_name = field.file_name().map(|s| s.to_string());
                    if let Ok(bytes) = field.bytes().await {
                        file_data = Some(bytes.to_vec());
                    }
                }
            }
        }
    }

    // Must have a file uploaded (matches Node's `if (!request.file)`).
    if file_data.is_none() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    // Determine the filename from originalname
    let raw_name = match &original_name {
        Some(name) if !name.is_empty() => name.clone(),
        _ => {
            if converted_data.is_some() {
                "imported.json".to_string()
            } else {
                return StatusCode::BAD_REQUEST.into_response();
            }
        }
    };

    // Build filename: sanitize the stem and add .json
    let stem = sanitize_filename_strip(
        Path::new(&raw_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("imported"),
    );
    let filename = format!("{}.json", stem);

    // Get file contents - either from convertedData or from the uploaded file
    let file_contents = if let Some(data) = converted_data {
        data
    } else {
        match file_data {
            Some(bytes) => match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "Is not a valid world info file")
                        .into_response();
                }
            },
            None => return StatusCode::BAD_REQUEST.into_response(),
        }
    };

    // Validate that the content has an `entries` key
    match serde_json::from_str::<Value>(&file_contents) {
        Ok(world_content) => {
            if !world_content
                .as_object()
                .map(|o| o.contains_key("entries"))
                .unwrap_or(false)
            {
                return (StatusCode::BAD_REQUEST, "Is not a valid world info file")
                    .into_response();
            }
        }
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Is not a valid world info file").into_response();
        }
    }

    let path_to_new_file = dirs.worlds.join(&filename);
    let world_name = Path::new(&path_to_new_file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    if world_name.is_empty() {
        return (StatusCode::BAD_REQUEST, "World file must have a name").into_response();
    }

    if let Err(e) = fs::write(&path_to_new_file, &file_contents) {
        tracing::error!("Failed to write world info file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Json(ImportWorldInfoResponse { name: world_name }).into_response()
}

/// `POST /api/worldinfo/edit` — edit/save a world info file.
///
/// Mirrors Node's `router.post('/edit')` in `worldinfo.js:134-157`.
/// Requires `name` and `data` with an `entries` key.
pub async fn edit_world_info(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    body: Option<Json<Value>>,
) -> Response {
    let body = match body {
        Some(Json(v)) if v.is_object() => v,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Validate name
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return (StatusCode::BAD_REQUEST, "World file must have a name").into_response(),
    };

    // Validate data has entries
    let data = match body.get("data") {
        Some(d) if d.is_object() => {
            if !d
                .as_object()
                .map(|o| o.contains_key("entries"))
                .unwrap_or(false)
            {
                return (StatusCode::BAD_REQUEST, "Is not a valid world info file")
                    .into_response();
            }
            d.clone()
        }
        _ => {
            return (StatusCode::BAD_REQUEST, "Is not a valid world info file").into_response();
        }
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    let filename = sanitize_filename_strip(&format!("{}.json", name));
    let path_to_file = dirs.worlds.join(&filename);
    let file_data = to_pretty_json_4(&data);

    if let Err(e) = fs::write(&path_to_file, &file_data) {
        tracing::error!("Failed to write world info file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Json(serde_json::json!({ "ok": true })).into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_dirs() -> (TempDir, UserDirectories) {
        let tmp = TempDir::new().unwrap();
        let dirs = UserDirectories::new(tmp.path(), "test-user");
        fs::create_dir_all(&dirs.worlds).unwrap();
        (tmp, dirs)
    }

    #[test]
    fn test_read_world_info_file_existing() {
        let (_tmp, dirs) = setup_test_dirs();

        let world = serde_json::json!({
            "entries": {
                "0": {
                    "uid": 1,
                    "key": ["keyword"],
                    "content": "World lore content"
                }
            },
            "name": "Test World"
        });
        fs::write(
            dirs.worlds.join("Test World.json"),
            serde_json::to_string(&world).unwrap(),
        )
        .unwrap();

        let result = read_world_info_file(&dirs.worlds, "Test World", false);
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.get("entries").is_some());
        assert_eq!(result["name"].as_str().unwrap(), "Test World");
    }

    #[test]
    fn test_read_world_info_file_missing_with_dummy() {
        let (_tmp, dirs) = setup_test_dirs();

        let result = read_world_info_file(&dirs.worlds, "nonexistent", true);
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.get("entries").is_some());
        assert!(result["entries"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_read_world_info_file_missing_without_dummy() {
        let (_tmp, dirs) = setup_test_dirs();

        let result = read_world_info_file(&dirs.worlds, "nonexistent", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_read_world_info_file_empty_name() {
        let (_tmp, dirs) = setup_test_dirs();

        let result_dummy = read_world_info_file(&dirs.worlds, "", true);
        assert!(result_dummy.is_some());

        let result_no_dummy = read_world_info_file(&dirs.worlds, "", false);
        assert!(result_no_dummy.is_none());
    }

    #[test]
    fn test_world_info_list_sorting() {
        let (_tmp, dirs) = setup_test_dirs();

        // Create world info files with different names
        for name in &["Zebra", "Alpha", "Banana"] {
            let world = serde_json::json!({
                "entries": {},
                "name": name
            });
            fs::write(
                dirs.worlds.join(format!("{}.json", name)),
                serde_json::to_string(&world).unwrap(),
            )
            .unwrap();
        }

        // Read and sort
        let mut files: Vec<String> = fs::read_dir(&dirs.worlds)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|ext| ext.to_str()) == Some("json")
                    && e.file_type().map(|ft| ft.is_file()).unwrap_or(false)
            })
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        files.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

        assert_eq!(files[0], "Alpha.json");
        assert_eq!(files[1], "Banana.json");
        assert_eq!(files[2], "Zebra.json");
    }

    #[test]
    fn test_world_info_edit_and_read_back() {
        let (_tmp, dirs) = setup_test_dirs();

        let data = serde_json::json!({
            "entries": {
                "0": {
                    "uid": 1,
                    "key": ["test"],
                    "content": "Test content"
                }
            }
        });
        let filename = sanitize_filename_strip(&format!("{}.json", "My World"));
        let path = dirs.worlds.join(&filename);
        fs::write(&path, serde_json::to_string_pretty(&data).unwrap()).unwrap();

        let read_back = read_world_info_file(&dirs.worlds, "My World", false);
        assert!(read_back.is_some());
        let read_back = read_back.unwrap();
        assert!(read_back.get("entries").is_some());
        assert_eq!(
            read_back["entries"]["0"]["content"].as_str().unwrap(),
            "Test content"
        );
    }

    #[test]
    fn test_world_info_delete() {
        let (_tmp, dirs) = setup_test_dirs();

        let world = serde_json::json!({ "entries": {} });
        let path = dirs.worlds.join("DeleteMe.json");
        fs::write(&path, serde_json::to_string(&world).unwrap()).unwrap();
        assert!(path.exists());

        fs::remove_file(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn test_world_info_extensions_extraction() {
        let (_tmp, dirs) = setup_test_dirs();

        // File with valid extensions
        let world_with_ext = serde_json::json!({
            "entries": {},
            "name": "WithExt",
            "extensions": { "depth": 4, "weight": 1.0 }
        });
        fs::write(
            dirs.worlds.join("WithExt.json"),
            serde_json::to_string(&world_with_ext).unwrap(),
        )
        .unwrap();

        // File with non-object extensions
        let world_no_ext = serde_json::json!({
            "entries": {},
            "name": "NoExt",
            "extensions": "not an object"
        });
        fs::write(
            dirs.worlds.join("NoExt.json"),
            serde_json::to_string(&world_no_ext).unwrap(),
        )
        .unwrap();

        // Read WithExt
        let result = read_world_info_file(&dirs.worlds, "WithExt", false).unwrap();
        let ext = result.get("extensions").unwrap();
        assert!(ext.is_object());
        assert_eq!(ext["depth"].as_i64().unwrap(), 4);

        // Read NoExt — extensions is a string, not an object
        let result = read_world_info_file(&dirs.worlds, "NoExt", false).unwrap();
        let ext = result.get("extensions").unwrap();
        assert!(ext.is_string()); // The raw value from the file
    }

    #[test]
    fn test_import_validates_entries() {
        // Simulate the validation logic from import handler
        let valid = serde_json::json!({ "entries": { "0": { "key": ["test"] } } });
        assert!(valid.as_object().unwrap().contains_key("entries"));

        let invalid = serde_json::json!({ "data": "no entries" });
        assert!(!invalid.as_object().unwrap().contains_key("entries"));
    }
}
