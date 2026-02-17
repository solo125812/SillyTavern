//! Group endpoints — Phase 7.
//!
//! Mirrors Node's [`src/endpoints/groups.js`](../../../src/endpoints/groups.js).
//!
//! ## Endpoints
//! - `POST /api/groups/all` — list all groups with chat stats.
//! - `POST /api/groups/create` — create a new group.
//! - `POST /api/groups/edit` — edit an existing group.
//! - `POST /api/groups/delete` — delete a group and its chats.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde::Serialize;
use serde_json::ser::PrettyFormatter;
use serde_json::{Map, Value};
use serde_json::Serializer;

use crate::api::characters::helpers::contains_forbidden_chars;
use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Request body for `POST /api/groups/edit`.
/// The full group JSON is sent in the body; `id` is required.
#[derive(Debug, Deserialize)]
pub struct EditGroupRequest {
    /// Group ID (required for edit).
    pub id: Option<String>,
    /// All remaining fields are passed through.
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

/// Request body for `POST /api/groups/create`.
#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub name: Option<String>,
    pub members: Option<Vec<Value>>,
    pub avatar_url: Option<Value>,
    pub allow_self_responses: Option<bool>,
    pub activation_strategy: Option<Value>,
    pub generation_mode: Option<Value>,
    pub disabled_members: Option<Vec<Value>>,
    pub fav: Option<Value>,
    pub chat_id: Option<String>,
    pub chats: Option<Vec<String>>,
    pub auto_mode_delay: Option<Value>,
    pub generation_mode_join_prefix: Option<String>,
    pub generation_mode_join_suffix: Option<String>,
    /// Deprecated metadata keys — will be warned and stripped.
    pub chat_metadata: Option<Value>,
    pub past_metadata: Option<Value>,
}

/// Request body for `POST /api/groups/delete`.
#[derive(Debug, Deserialize)]
pub struct DeleteGroupRequest {
    pub id: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Warn on deprecated metadata keys and remove them from a JSON value.
/// Mirrors Node's `warnOnGroupMetadata()`.
fn warn_on_group_metadata(group_data: &mut Value) {
    if let Some(obj) = group_data.as_object_mut() {
        for key in &["chat_metadata", "past_metadata"] {
            if obj.contains_key(*key) {
                let id = obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                tracing::warn!(
                    "Group JSON data for \"{}\" contains deprecated key \"{}\".",
                    id,
                    key
                );
                obj.remove(*key);
            }
        }
    }
}

/// Get file creation time as milliseconds since epoch.
fn birthtime_ms(path: &Path) -> f64 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.created().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Get file modification time as milliseconds since epoch.
fn mtime_ms(path: &Path) -> f64 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Get file size in bytes.
fn file_size(path: &Path) -> u64 {
    fs::metadata(path).ok().map(|m| m.len()).unwrap_or(0)
}

/// Match JS nullish coalescing (`??`) semantics.
fn coalesce_non_null(value: Option<&Value>, default: Value) -> Value {
    match value {
        Some(v) if !v.is_null() => v.clone(),
        _ => default,
    }
}

/// Match JS truthiness (`!!value`) semantics.
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|v| v != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) => true,
        Value::Object(_) => true,
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

/// `POST /api/groups/all` — list all groups with chat statistics.
///
/// Mirrors Node's `router.post('/all')` in `groups.js:113-155`.
/// Reads all `.json` files from the groups directory, enriches each with
/// `date_added`, `create_date`, `date_last_chat`, and `chat_size` from
/// the associated group chat files.
pub async fn get_all_groups(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    // Ensure groups directory exists
    if !dirs.groups.exists() {
        let _ = fs::create_dir_all(&dirs.groups);
    }

    // Read group JSON files
    let group_files: Vec<String> = match fs::read_dir(&dirs.groups) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|ext| ext.to_str()) == Some("json")
                    && e.file_type().map(|ft| ft.is_file()).unwrap_or(false)
            })
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect(),
        Err(e) => {
            tracing::error!("Failed to read groups directory: {}", e);
            return Json(serde_json::json!([])).into_response();
        }
    };

    // Read group chat JSONL filenames
    let chat_files: Vec<String> = match fs::read_dir(&dirs.group_chats) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl")
                    && e.file_type().map(|ft| ft.is_file()).unwrap_or(false)
            })
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect(),
        Err(_) => Vec::new(),
    };

    let mut groups: Vec<Value> = Vec::new();

    for file in &group_files {
        match (|| -> Option<Value> {
            let file_path = dirs.groups.join(file);
            let contents = fs::read_to_string(&file_path).ok()?;
            let mut group: Value = serde_json::from_str(&contents).ok()?;

            // Add file creation dates
            let added = birthtime_ms(&file_path);
            let obj = group.as_object_mut()?;

            // Ensure avatar_url is always present (frontend's isValidImageUrl
            // crashes with Object.keys(undefined) if it's missing)
            if !obj.contains_key("avatar_url") {
                obj.insert("avatar_url".to_string(), Value::String(String::new()));
            }

            obj.insert("date_added".to_string(), serde_json::json!(added));

            // Create ISO date string from birthtime
            let create_date = if added > 0.0 {
                let secs = (added / 1000.0) as i64;
                let nanos = ((added % 1000.0) * 1_000_000.0) as u32;
                chrono::DateTime::from_timestamp(secs, nanos)
                    .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            obj.insert("create_date".to_string(), serde_json::json!(create_date));

            // Calculate chat stats
            let mut chat_size: u64 = 0;
            let mut date_last_chat: f64 = 0.0;

            if let Some(chats) = obj.get("chats").and_then(|v| v.as_array()) {
                for chat_file in &chat_files {
                    let chat_stem = Path::new(chat_file)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    let is_member = chats.iter().any(|c| c.as_str() == Some(chat_stem));
                    if is_member {
                        let chat_path = dirs.group_chats.join(chat_file);
                        chat_size += file_size(&chat_path);
                        let mtime = mtime_ms(&chat_path);
                        if mtime > date_last_chat {
                            date_last_chat = mtime;
                        }
                    }
                }
            }

            obj.insert("date_last_chat".to_string(), serde_json::json!(date_last_chat));
            obj.insert("chat_size".to_string(), serde_json::json!(chat_size));

            Some(group)
        })() {
            Some(group) => groups.push(group),
            None => {
                tracing::error!("Failed to parse group file: {}", file);
            }
        }
    }

    Json(groups).into_response()
}

/// `POST /api/groups/create` — create a new group.
///
/// Mirrors Node's `router.post('/create')` in `groups.js:157-189`.
/// Group ID is `Date.now()` as a string. Returns the complete group metadata.
pub async fn create_group(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    body: Option<Json<Value>>,
) -> Response {
    let mut body = match body {
        Some(Json(v)) => v,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    if body.is_null() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    // Warn on deprecated metadata keys (mirrors Node's warnOnGroupMetadata on request body)
    warn_on_group_metadata(&mut body);

    // Generate ID from current timestamp (milliseconds since epoch)
    let id = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string());

    let obj = body.as_object();

    // Build group metadata — mirrors Node's field extraction with defaults
    let mut group_map = Map::new();
    group_map.insert("id".to_string(), Value::String(id.clone()));
    group_map.insert(
        "name".to_string(),
        coalesce_non_null(
            obj.and_then(|o| o.get("name")),
            Value::String("New Group".to_string()),
        ),
    );
    group_map.insert(
        "members".to_string(),
        coalesce_non_null(obj.and_then(|o| o.get("members")), Value::Array(Vec::new())),
    );
    group_map.insert(
        "allow_self_responses".to_string(),
        Value::Bool(
            obj.and_then(|o| o.get("allow_self_responses"))
                .map(js_truthy)
                .unwrap_or(false),
        ),
    );
    group_map.insert(
        "activation_strategy".to_string(),
        coalesce_non_null(obj.and_then(|o| o.get("activation_strategy")), Value::from(0)),
    );
    group_map.insert(
        "generation_mode".to_string(),
        coalesce_non_null(obj.and_then(|o| o.get("generation_mode")), Value::from(0)),
    );
    group_map.insert(
        "disabled_members".to_string(),
        coalesce_non_null(
            obj.and_then(|o| o.get("disabled_members")),
            Value::Array(Vec::new()),
        ),
    );
    group_map.insert(
        "chat_id".to_string(),
        coalesce_non_null(
            obj.and_then(|o| o.get("chat_id")),
            Value::String(id.clone()),
        ),
    );
    group_map.insert(
        "chats".to_string(),
        coalesce_non_null(
            obj.and_then(|o| o.get("chats")),
            Value::Array(vec![Value::String(id.clone())]),
        ),
    );
    group_map.insert(
        "auto_mode_delay".to_string(),
        coalesce_non_null(obj.and_then(|o| o.get("auto_mode_delay")), Value::from(5)),
    );
    group_map.insert(
        "generation_mode_join_prefix".to_string(),
        coalesce_non_null(
            obj.and_then(|o| o.get("generation_mode_join_prefix")),
            Value::String(String::new()),
        ),
    );
    group_map.insert(
        "generation_mode_join_suffix".to_string(),
        coalesce_non_null(
            obj.and_then(|o| o.get("generation_mode_join_suffix")),
            Value::String(String::new()),
        ),
    );

    if let Some(obj) = obj {
        if obj.contains_key("avatar_url") {
            group_map.insert(
                "avatar_url".to_string(),
                obj.get("avatar_url").cloned().unwrap_or(Value::Null),
            );
        }
        if obj.contains_key("fav") {
            group_map.insert(
                "fav".to_string(),
                obj.get("fav").cloned().unwrap_or(Value::Null),
            );
        }
    }

    let group_metadata = Value::Object(group_map);

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    // Ensure groups directory exists
    if !dirs.groups.exists() {
        let _ = fs::create_dir_all(&dirs.groups);
    }

    let filename = sanitize_filename_strip(&format!("{}.json", id));
    let path_to_file = dirs.groups.join(&filename);
    let file_data = to_pretty_json_4(&group_metadata);

    if let Err(e) = fs::write(&path_to_file, &file_data) {
        tracing::error!("Failed to write group file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Json(group_metadata).into_response()
}

/// `POST /api/groups/edit` — edit an existing group.
///
/// Mirrors Node's `router.post('/edit')` in `groups.js:191-202`.
/// Validates `id` field, sanitizes filename, writes the full body as the
/// group JSON.
pub async fn edit_group(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    body: Option<Json<Value>>,
) -> Response {
    let mut body = match body {
        Some(Json(v)) if v.is_object() => v,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let id = match body.get("id").and_then(|v| v.as_str()) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Validate filename (mirrors getFileNameValidationFunction('id'))
    if contains_forbidden_chars(&id) {
        tracing::error!("Malicious group id prevented");
        return StatusCode::BAD_REQUEST.into_response();
    }

    // Warn on deprecated metadata keys
    warn_on_group_metadata(&mut body);

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let filename = sanitize_filename_strip(&format!("{}.json", id));
    let path_to_file = dirs.groups.join(&filename);
    let file_data = to_pretty_json_4(&body);

    if let Err(e) = fs::write(&path_to_file, &file_data) {
        tracing::error!("Failed to write group file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Json(serde_json::json!({ "ok": true })).into_response()
}

/// `POST /api/groups/delete` — delete a group and its chats.
///
/// Mirrors Node's `router.post('/delete')` in `groups.js:204-235`.
/// Reads the group file to find associated chats, deletes each chat JSONL,
/// then deletes the group JSON file.
pub async fn delete_group(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    body: Option<Json<Value>>,
) -> Response {
    let body = match body {
        Some(Json(v)) if v.is_object() => v,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let id = match body.get("id").and_then(|v| v.as_str()) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Validate filename (mirrors getFileNameValidationFunction('id'))
    if contains_forbidden_chars(&id) {
        tracing::error!("Malicious group id prevented");
        return StatusCode::BAD_REQUEST.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let filename = sanitize_filename_strip(&format!("{}.json", id));
    let path_to_group = dirs.groups.join(&filename);

    // Try to delete group chats
    match fs::read_to_string(&path_to_group) {
        Ok(contents) => {
            if let Ok(group) = serde_json::from_str::<Value>(&contents) {
                if let Some(chats) = group.get("chats").and_then(|v| v.as_array()) {
                    for chat in chats {
                        if let Some(chat_id) = chat.as_str() {
                            tracing::info!("Deleting group chat {}", chat_id);
                            let chat_filename =
                                sanitize_filename_strip(&format!("{}.jsonl", chat_id));
                            let chat_path = dirs.group_chats.join(&chat_filename);
                            if chat_path.exists() {
                                let _ = fs::remove_file(&chat_path);
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!(
                "Could not delete group chats. Clean them up manually. Error: {}",
                e
            );
        }
    }

    // Delete the group file itself
    if path_to_group.exists() {
        let _ = fs::remove_file(&path_to_group);
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
        fs::create_dir_all(&dirs.groups).unwrap();
        fs::create_dir_all(&dirs.group_chats).unwrap();
        (tmp, dirs)
    }

    #[test]
    fn test_warn_on_group_metadata_strips_deprecated_keys() {
        let mut data = serde_json::json!({
            "id": "123",
            "name": "Test Group",
            "chat_metadata": {"key": "value"},
            "past_metadata": {"old": "data"},
        });
        warn_on_group_metadata(&mut data);
        assert!(data.get("chat_metadata").is_none());
        assert!(data.get("past_metadata").is_none());
        assert_eq!(data.get("id").unwrap().as_str().unwrap(), "123");
        assert_eq!(data.get("name").unwrap().as_str().unwrap(), "Test Group");
    }

    #[test]
    fn test_warn_on_group_metadata_no_deprecated_keys() {
        let mut data = serde_json::json!({
            "id": "456",
            "name": "Clean Group",
        });
        warn_on_group_metadata(&mut data);
        assert_eq!(data.get("id").unwrap().as_str().unwrap(), "456");
    }

    #[test]
    fn test_group_create_and_read() {
        let (_tmp, dirs) = setup_test_dirs();

        // Write a group file
        let group = serde_json::json!({
            "id": "1000",
            "name": "Test Group",
            "members": ["char1", "char2"],
            "chats": ["1000"],
            "chat_id": "1000",
            "allow_self_responses": false,
            "activation_strategy": 0,
            "generation_mode": 0,
            "disabled_members": [],
            "auto_mode_delay": 5,
            "generation_mode_join_prefix": "",
            "generation_mode_join_suffix": "",
        });
        let path = dirs.groups.join("1000.json");
        fs::write(&path, serde_json::to_string_pretty(&group).unwrap()).unwrap();

        // Verify it can be read back
        let contents = fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed["name"].as_str().unwrap(), "Test Group");
        assert_eq!(parsed["members"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_group_delete_with_chats() {
        let (_tmp, dirs) = setup_test_dirs();

        // Create a group with chats
        let group = serde_json::json!({
            "id": "2000",
            "name": "Delete Me",
            "chats": ["2000", "2001"],
        });
        let group_path = dirs.groups.join("2000.json");
        fs::write(&group_path, serde_json::to_string(&group).unwrap()).unwrap();

        // Create chat files
        let chat1_path = dirs.group_chats.join("2000.jsonl");
        let chat2_path = dirs.group_chats.join("2001.jsonl");
        fs::write(&chat1_path, "{}").unwrap();
        fs::write(&chat2_path, "{}").unwrap();

        assert!(chat1_path.exists());
        assert!(chat2_path.exists());

        // Simulate delete: read group, delete chats, delete group
        let contents = fs::read_to_string(&group_path).unwrap();
        let parsed: Value = serde_json::from_str(&contents).unwrap();
        if let Some(chats) = parsed["chats"].as_array() {
            for chat in chats {
                if let Some(chat_id) = chat.as_str() {
                    let chat_filename = sanitize_filename_strip(&format!("{}.jsonl", chat_id));
                    let chat_path = dirs.group_chats.join(&chat_filename);
                    if chat_path.exists() {
                        fs::remove_file(&chat_path).unwrap();
                    }
                }
            }
        }
        fs::remove_file(&group_path).unwrap();

        assert!(!chat1_path.exists());
        assert!(!chat2_path.exists());
        assert!(!group_path.exists());
    }

    #[test]
    fn test_birthtime_and_mtime() {
        let tmp = TempDir::new().unwrap();
        let test_file = tmp.path().join("test.txt");
        fs::write(&test_file, "hello").unwrap();

        let birth = birthtime_ms(&test_file);
        let mtime = mtime_ms(&test_file);
        let size = file_size(&test_file);

        // Both should be positive (file was just created)
        assert!(birth > 0.0 || mtime > 0.0);
        assert_eq!(size, 5); // "hello" is 5 bytes
    }

    #[test]
    fn test_nonexistent_path_returns_zero() {
        let path = Path::new("/nonexistent/path/to/file.txt");
        assert_eq!(birthtime_ms(path), 0.0);
        assert_eq!(mtime_ms(path), 0.0);
        assert_eq!(file_size(path), 0);
    }
}
