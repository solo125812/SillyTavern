//! Settings endpoints — Phase 8.
//!
//! Mirrors Node's [`src/endpoints/settings.js`](../../../src/endpoints/settings.js).
//!
//! ## Endpoints
//! - `POST /api/settings/get` — get settings + preset bundles.
//! - `POST /api/settings/save` — save settings.
//! - `POST /api/settings/get-snapshots` — list settings snapshots.
//! - `POST /api/settings/load-snapshot` — load a snapshot.
//! - `POST /api/settings/make-snapshot` — create a snapshot.
//! - `POST /api/settings/restore-snapshot` — restore a snapshot.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::router::AppState;
use crate::api::themes::read_and_parse_from_directory;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

/// Settings filename constant — mirrors Node's `SETTINGS_FILE`.
const SETTINGS_FILE: &str = "settings.json";

/// Autosave interval (10 minutes) — mirrors Node's throttle.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(10 * 60);

static AUTOSAVE_TRACKER: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SnapshotRequest {
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SnapshotEntry {
    pub date: f64,
    pub name: String,
    pub size: u64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the backup file prefix for a user's settings.
pub fn get_settings_backup_file_prefix(handle: &str) -> String {
    format!("settings_{}_", handle)
}

/// Generate a timestamp string matching Node's `generateTimestamp()`.
fn generate_timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S%3f").to_string()
}

/// Read presets from a directory — mirrors Node's `readPresetsFromDirectory()`.
///
/// Returns `(file_contents, file_names)` where file_contents are raw JSON strings
/// (validated as parseable JSON) and file_names are the filenames with extension
/// optionally removed.
fn read_presets_from_directory(
    directory: &Path,
    remove_file_extension: bool,
) -> (Vec<String>, Vec<String>) {
    let mut file_contents: Vec<String> = Vec::new();
    let mut file_names: Vec<String> = Vec::new();

    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return (file_contents, file_names),
    };

    let mut files: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().map(|ft| ft.is_file()).unwrap_or(false)
        })
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| {
            Path::new(name)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
        })
        .collect();

    // Sort by name (localeCompare equivalent)
    files.sort();

    for item in &files {
        let path = directory.join(item);
        match fs::read_to_string(&path) {
            Ok(content) => {
                // Validate JSON
                if serde_json::from_str::<Value>(&content).is_ok() {
                    file_contents.push(content);
                    if remove_file_extension {
                        let name = Path::new(item)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or(item)
                            .to_string();
                        file_names.push(name);
                    } else {
                        file_names.push(item.clone());
                    }
                } else {
                    tracing::warn!("{} is not valid JSON", item);
                }
            }
            Err(_) => continue,
        }
    }

    (file_contents, file_names)
}

/// Remove old backups, keeping at most 50 (matches Node's `removeOldBackups`).
fn remove_old_backups(backups_dir: &Path, prefix: &str) {
    let entries = match fs::read_dir(backups_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    let mut backup_files: Vec<(String, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(prefix)
        })
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((name, modified))
        })
        .collect();

    // Sort by time, newest first
    backup_files.sort_by(|a, b| b.1.cmp(&a.1));

    // Keep at most 50
    if backup_files.len() > 50 {
        for (name, _) in &backup_files[50..] {
            let _ = fs::remove_file(backups_dir.join(name));
        }
    }
}

/// Check if a backup would be a duplicate of the latest.
fn is_duplicate_backup(backups_dir: &Path, prefix: &str, source_file: &Path) -> bool {
    let latest = get_latest_backup(backups_dir, prefix);
    match latest {
        Some(latest_path) => are_files_equal(&latest_path, source_file),
        None => false,
    }
}

/// Get the latest backup file for a user.
fn get_latest_backup(backups_dir: &Path, prefix: &str) -> Option<std::path::PathBuf> {
    let entries = fs::read_dir(backups_dir).ok()?;

    let mut backup_files: Vec<(std::path::PathBuf, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(prefix)
        })
        .filter_map(|e| {
            let path = e.path();
            let ctime = e.metadata().ok()?.modified().ok()?;
            Some((path, ctime))
        })
        .collect();

    backup_files.sort_by(|a, b| b.1.cmp(&a.1));
    backup_files.first().map(|(p, _)| p.clone())
}

/// Compare two files for content equality.
fn are_files_equal(file1: &Path, file2: &Path) -> bool {
    if !file1.exists() || !file2.exists() {
        return false;
    }
    match (fs::read(file1), fs::read(file2)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Backup user settings — mirrors Node's `backupUserSettings()`.
fn backup_user_settings(data_root: &Path, handle: &str, prevent_duplicates: bool) {
    let dirs = UserDirectories::new(data_root, handle);

    if !dirs.root.exists() {
        return;
    }

    let source_file = dirs.root.join(SETTINGS_FILE);
    let prefix = get_settings_backup_file_prefix(handle);

    if prevent_duplicates && is_duplicate_backup(&dirs.backups, &prefix, &source_file) {
        return;
    }

    if !source_file.exists() {
        return;
    }

    // Ensure backups directory exists
    let _ = fs::create_dir_all(&dirs.backups);

    let backup_file = dirs.backups.join(format!(
        "{}{}.json",
        prefix,
        generate_timestamp()
    ));

    if let Err(e) = fs::copy(&source_file, &backup_file) {
        tracing::error!("Failed to backup settings: {}", e);
        return;
    }

    remove_old_backups(&dirs.backups, &format!("settings_{}", handle));
}

/// Trigger autosave backup with throttling.
fn trigger_auto_save(data_root: &Path, handle: &str) {
    let tracker = AUTOSAVE_TRACKER.get_or_init(|| Mutex::new(HashMap::new()));
    let mut tracker = tracker.lock().unwrap();
    let now = Instant::now();

    let should_run = match tracker.get(handle) {
        Some(last) => now.duration_since(*last) >= AUTOSAVE_INTERVAL,
        None => true,
    };

    if should_run {
        tracker.insert(handle.to_string(), now);
        backup_user_settings(data_root, handle, true);
    }
}

/// Backup settings for all known users (mirrors Node's `backupSettings()`).
pub fn backup_settings_for_all_users(data_root: &Path) {
    let users = crate::api::users_public::read_all_users(data_root);
    for user in users {
        if let Some(handle) = user.get("handle").and_then(|v| v.as_str()) {
            backup_user_settings(data_root, handle, true);
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/settings/get` — get settings + preset bundles + flags.
///
/// Mirrors Node's `router.post('/get')` in `settings.js:215-286`.
/// Returns raw settings JSON string plus preset bundles for all providers,
/// world names, themes, moving UI presets, quick reply presets, instruct,
/// context, sysprompt, reasoning presets, and feature flags.
pub async fn get_settings(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    // Read settings file as raw string
    let settings = match fs::read_to_string(dirs.root.join(SETTINGS_FILE)) {
        Ok(s) => s,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // NovelAI Settings
    let (novelai_settings, novelai_setting_names) =
        read_presets_from_directory(&dirs.novel_ai_settings, true);

    // OpenAI Settings
    let (openai_settings, openai_setting_names) =
        read_presets_from_directory(&dirs.open_ai_settings, true);

    // TextGenerationWebUI Settings
    let (textgenerationwebui_presets, textgenerationwebui_preset_names) =
        read_presets_from_directory(&dirs.text_gen_settings, true);

    // KoboldAI Settings
    let (koboldai_settings, koboldai_setting_names) =
        read_presets_from_directory(&dirs.kobold_ai_settings, true);

    // World names
    let world_names: Vec<String> = match fs::read_dir(&dirs.worlds) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|name| {
                    Path::new(name)
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext.eq_ignore_ascii_case("json"))
                        .unwrap_or(false)
                })
                .map(|name| {
                    Path::new(&name)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string()
                })
                .collect();
            names.sort();
            names
        }
        Err(_) => Vec::new(),
    };

    let themes = read_and_parse_from_directory(&dirs.themes, ".json");
    let moving_ui_presets = read_and_parse_from_directory(&dirs.moving_ui, ".json");
    let quick_reply_presets = read_and_parse_from_directory(&dirs.quick_replies, ".json");
    let instruct = read_and_parse_from_directory(&dirs.instruct, ".json");
    let context = read_and_parse_from_directory(&dirs.context, ".json");
    let sysprompt = read_and_parse_from_directory(&dirs.sysprompt, ".json");
    let reasoning = read_and_parse_from_directory(&dirs.reasoning, ".json");

    // Build response — Note: `settings` is a raw JSON string, not parsed.
    // Node sends it as a raw string property in the response object.
    let response = serde_json::json!({
        "settings": settings,
        "koboldai_settings": koboldai_settings,
        "koboldai_setting_names": koboldai_setting_names,
        "world_names": world_names,
        "novelai_settings": novelai_settings,
        "novelai_setting_names": novelai_setting_names,
        "openai_settings": openai_settings,
        "openai_setting_names": openai_setting_names,
        "textgenerationwebui_presets": textgenerationwebui_presets,
        "textgenerationwebui_preset_names": textgenerationwebui_preset_names,
        "themes": themes,
        "movingUIPresets": moving_ui_presets,
        "quickReplyPresets": quick_reply_presets,
        "instruct": instruct,
        "context": context,
        "sysprompt": sysprompt,
        "reasoning": reasoning,
        "enable_extensions": state.config.enable_extensions,
        "enable_extensions_auto_update": state.config.enable_extensions_auto_update,
        "enable_accounts": state.config.enable_accounts,
    });

    Json(response).into_response()
}

/// `POST /api/settings/save` — save settings.
///
/// Mirrors Node's `router.post('/save')` in `settings.js:202-212`.
pub async fn save_settings(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let path_to_settings = dirs.root.join(SETTINGS_FILE);

    // Write as pretty-printed JSON with 4-space indent
    let content = match serde_json::to_string_pretty(&body) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to serialize settings: {}", e);
            return Json(serde_json::json!({"error": e.to_string()})).into_response();
        }
    };

    if let Err(e) = fs::write(&path_to_settings, content) {
        tracing::error!("Failed to write settings: {}", e);
        return Json(serde_json::json!({"error": e.to_string()})).into_response();
    }

    trigger_auto_save(&state.config.data_root, &user.handle);

    Json(serde_json::json!({"result": "ok"})).into_response()
}

/// `POST /api/settings/get-snapshots` — list settings snapshots.
///
/// Mirrors Node's `router.post('/get-snapshots')` in `settings.js:288-304`.
pub async fn get_snapshots(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let prefix = get_settings_backup_file_prefix(&user.handle);

    let entries = match fs::read_dir(&dirs.backups) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::error!("Failed to read backups directory: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let snapshots: Vec<SnapshotEntry> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&prefix)
        })
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let stat = e.metadata().ok()?;
            let ctime = stat
                .created()
                .or_else(|_| stat.modified())
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs_f64()
                * 1000.0; // Convert to milliseconds like Node's ctimeMs
            Some(SnapshotEntry {
                date: ctime,
                name,
                size: stat.len(),
            })
        })
        .collect();

    // No particular sort needed — Node doesn't sort the result
    Json(snapshots).into_response()
}

/// `POST /api/settings/load-snapshot` — load a snapshot.
///
/// Mirrors Node's `router.post('/load-snapshot')` in `settings.js:306-328`.
pub async fn load_snapshot(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<SnapshotRequest>,
) -> Response {
    let prefix = get_settings_backup_file_prefix(&user.handle);

    let name = match &body.name {
        Some(n) if !n.is_empty() && n.starts_with(&prefix) => n.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Invalid snapshot name"})),
            )
                .into_response();
        }
    };

    // Validate filename
    let sanitized = sanitize_filename_strip(&name);
    if sanitized != name {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid snapshot name"})),
        )
            .into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let snapshot_path = dirs.backups.join(&name);

    if !snapshot_path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    match fs::read_to_string(&snapshot_path) {
        Ok(content) => content.into_response(),
        Err(e) => {
            tracing::error!("Failed to read snapshot: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/settings/make-snapshot` — create a snapshot.
///
/// Mirrors Node's `router.post('/make-snapshot')` in `settings.js:330-338`.
pub async fn make_snapshot(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    backup_user_settings(&state.config.data_root, &user.handle, false);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/settings/restore-snapshot` — restore a snapshot over current settings.
///
/// Mirrors Node's `router.post('/restore-snapshot')` in `settings.js:340-364`.
pub async fn restore_snapshot(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<SnapshotRequest>,
) -> Response {
    let prefix = get_settings_backup_file_prefix(&user.handle);

    let name = match &body.name {
        Some(n) if !n.is_empty() && n.starts_with(&prefix) => n.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Invalid snapshot name"})),
            )
                .into_response();
        }
    };

    let sanitized = sanitize_filename_strip(&name);
    if sanitized != name {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid snapshot name"})),
        )
            .into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let snapshot_path = dirs.backups.join(&name);

    if !snapshot_path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let path_to_settings = dirs.root.join(SETTINGS_FILE);

    // Remove current settings and copy snapshot
    let _ = fs::remove_file(&path_to_settings);
    if let Err(e) = fs::copy(&snapshot_path, &path_to_settings) {
        tracing::error!("Failed to restore snapshot: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}
