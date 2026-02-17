//! Users private endpoints — Phase 8.
//!
//! Mirrors Node's [`src/endpoints/users-private.js`](../../../src/endpoints/users-private.js).
//!
//! These endpoints require authentication (behind the auth gate).
//!
//! ## Endpoints
//! - `POST /api/users/logout` — logout.
//! - `GET /api/users/me` — get current user info.
//! - `POST /api/users/change-avatar` — change user avatar.
//! - `POST /api/users/change-password` — change user password.
//! - `POST /api/users/backup` — create a user data backup.
//! - `POST /api/users/reset-settings` — reset user settings.
//! - `POST /api/users/change-name` — change user display name.
//! - `POST /api/users/reset-step1` — start account reset (prints code to console).
//! - `POST /api/users/reset-step2` — complete account reset.

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
use serde::Deserialize;
use serde_json::Value;

use crate::api::router::AppState;
use crate::api::users_public::{
    get_password_hash,
    get_user_avatar,
    read_user,
    storage_filename_for_key,
    storage_path_for_key,
    write_user,
};
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use tempfile::NamedTempFile;
use walkdir::WalkDir;
use zip::write::FileOptions;

/// Settings filename constant.
const SETTINGS_FILE: &str = "settings.json";

/// Account reset cache TTL (5 minutes).
const RESET_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone)]
struct CodeEntry {
    code: String,
    expires_at: Instant,
}

static RESET_CACHE: OnceLock<Mutex<HashMap<String, CodeEntry>>> = OnceLock::new();

fn reset_cache() -> &'static Mutex<HashMap<String, CodeEntry>> {
    RESET_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set_reset_code(handle: &str, code: String) {
    let mut cache = reset_cache().lock().unwrap();
    cache.insert(
        handle.to_string(),
        CodeEntry {
            code,
            expires_at: Instant::now() + RESET_TTL,
        },
    );
}

fn get_reset_code(handle: &str) -> Option<String> {
    let mut cache = reset_cache().lock().unwrap();
    let entry = cache.get(handle).cloned();
    match entry {
        Some(entry) if Instant::now() <= entry.expires_at => Some(entry.code),
        _ => {
            cache.remove(handle);
            None
        }
    }
}

fn clear_reset_code(handle: &str) {
    let mut cache = reset_cache().lock().unwrap();
    cache.remove(handle);
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ChangeAvatarRequest {
    pub handle: Option<String>,
    pub avatar: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub handle: Option<String>,
    pub old_password: Option<String>,
    pub new_password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BackupRequest {
    pub handle: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResetSettingsRequest {
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChangeNameRequest {
    pub handle: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResetStep2Request {
    pub code: Option<String>,
    pub password: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/users/logout` — logout.
///
/// Mirrors Node's `router.post('/logout')` in `users-private.js:17-32`.
/// Session invalidation is handled by the Node proxy.
pub async fn logout_user() -> Response {
    // Session management is on Node side during strangler pattern.
    // Return 204 to indicate success.
    StatusCode::NO_CONTENT.into_response()
}

/// `GET /api/users/me` — get current user info.
///
/// Mirrors Node's `router.get('/me')` in `users-private.js:34-55`.
pub async fn get_me(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let user_data = match read_user(&state.config.data_root, &user.handle) {
        Some(u) => u,
        None => {
            // Fallback to user context when no storage entry exists
            let avatar = get_user_avatar(&state.config.data_root, &user.handle);
            return Json(serde_json::json!({
                "handle": user.handle,
                "name": user.name,
                "avatar": avatar,
                "admin": user.is_admin,
                "password": false,
                "created": null,
            }))
            .into_response();
        }
    };

    let avatar = get_user_avatar(&state.config.data_root, &user.handle);
    let has_password = user_data
        .get("password")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    Json(serde_json::json!({
        "handle": user.handle,
        "name": user_data.get("name").and_then(|v| v.as_str()).unwrap_or(&user.name),
        "avatar": avatar,
        "admin": user.is_admin,
        "password": has_password,
        "created": user_data.get("created").and_then(|v| v.as_u64()),
    }))
    .into_response()
}

/// `POST /api/users/change-avatar` — change user avatar.
///
/// Mirrors Node's `router.post('/change-avatar')` in `users-private.js:57-90`.
pub async fn change_avatar(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ChangeAvatarRequest>,
) -> Response {
    let handle = match &body.handle {
        Some(h) if !h.is_empty() => h.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing required fields"})),
            )
                .into_response();
        }
    };

    // Only self or admin can change avatar
    if handle != user.handle && !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "Unauthorized"})),
        )
            .into_response();
    }

    let avatar = body.avatar.as_deref().unwrap_or("");

    // Validate avatar is a data URL or empty string
    if !avatar.starts_with("data:image/") && !avatar.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid data URL"})),
        )
            .into_response();
    }

    // Check user exists
    if read_user(&state.config.data_root, &handle).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "User not found"})),
        )
            .into_response();
    }

    // Write avatar to storage
    write_avatar_to_storage(&state.config.data_root, &handle, avatar);

    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/change-password` — change user password.
///
/// Mirrors Node's `router.post('/change-password')` in `users-private.js:92-137`.
pub async fn change_password(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ChangePasswordRequest>,
) -> Response {
    let handle = match &body.handle {
        Some(h) if !h.is_empty() => h.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing required fields"})),
            )
                .into_response();
        }
    };

    if handle != user.handle && !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "Unauthorized"})),
        )
            .into_response();
    }

    let mut user_data = match read_user(&state.config.data_root, &handle) {
        Some(u) => u,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "User not found"})),
            )
                .into_response();
        }
    };

    let enabled = user_data.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    if !enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "User is disabled"})),
        )
            .into_response();
    }

    // Verify old password if not admin and user has a password
    let stored_password = user_data.get("password").and_then(|v| v.as_str()).unwrap_or("");
    let stored_salt = user_data.get("salt").and_then(|v| v.as_str()).unwrap_or("");

    if !user.is_admin && !stored_password.is_empty() {
        let old_password = body.old_password.as_deref().unwrap_or("");
        let hash = get_password_hash(old_password, stored_salt);
        if hash != stored_password {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "Incorrect password"})),
            )
                .into_response();
        }
    }

    // Set new password or clear it
    if let Some(new_password) = &body.new_password {
        if !new_password.is_empty() {
            let salt = generate_salt();
            let hash = get_password_hash(new_password, &salt);
            if let Some(obj) = user_data.as_object_mut() {
                obj.insert("password".to_string(), Value::String(hash));
                obj.insert("salt".to_string(), Value::String(salt));
            }
        } else {
            if let Some(obj) = user_data.as_object_mut() {
                obj.insert("password".to_string(), Value::String(String::new()));
                obj.insert("salt".to_string(), Value::String(String::new()));
            }
        }
    } else {
        if let Some(obj) = user_data.as_object_mut() {
            obj.insert("password".to_string(), Value::String(String::new()));
            obj.insert("salt".to_string(), Value::String(String::new()));
        }
    }

    write_user(&state.config.data_root, &handle, &user_data);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/backup` — create a user data backup archive.
///
/// Mirrors Node's `router.post('/backup')` in `users-private.js:139-158`.
/// Note: Actual archive creation requires tar/zip support.
/// Returns 501 during strangler pattern as this is better handled by Node.
pub async fn backup_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<BackupRequest>,
) -> Response {
    let handle = match &body.handle {
        Some(h) if !h.is_empty() => h.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing required fields"})),
            )
                .into_response();
        }
    };

    if handle != user.handle && !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "Unauthorized"})),
        )
            .into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &handle);
    if !dirs.root.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S%3f").to_string();
    let archive_name = format!("{}-{}.zip", handle, timestamp);

    let temp = match NamedTempFile::new() {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Failed to create temp file: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let file = match temp.reopen() {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("Failed to open temp file: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut zip = zip::ZipWriter::new(file);
    let options: FileOptions<()> = FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    for entry in WalkDir::new(&dirs.root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        let rel = match path.strip_prefix(&dirs.root) {
            Ok(r) if !r.as_os_str().is_empty() => r,
            _ => continue,
        };

        let rel_str = rel.to_string_lossy();
        if entry.file_type().is_dir() {
            if let Err(e) = zip.add_directory(rel_str.as_ref(), options) {
                tracing::error!("Failed to add directory to zip: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        } else {
            if let Err(e) = zip.start_file(rel_str.as_ref(), options) {
                tracing::error!("Failed to add file to zip: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            let mut f = match fs::File::open(path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!("Failed to open file for zip: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };
            if let Err(e) = std::io::copy(&mut f, &mut zip) {
                tracing::error!("Failed to write file to zip: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }

    if let Err(e) = zip.finish() {
        tracing::error!("Failed to finalize zip: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let bytes = match fs::read(temp.path()) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("Failed to read zip archive: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut builder = Response::builder().status(StatusCode::OK);
    builder = builder.header(axum::http::header::CONTENT_TYPE, "application/zip");
    builder = builder.header(
        axum::http::header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", archive_name),
    );

    builder
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `POST /api/users/reset-settings` — reset user settings.
///
/// Mirrors Node's `router.post('/reset-settings')` in `users-private.js:160-178`.
pub async fn reset_settings(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ResetSettingsRequest>,
) -> Response {
    // Verify password if user has one
    if let Some(user_data) = read_user(&state.config.data_root, &user.handle) {
        let stored_password = user_data.get("password").and_then(|v| v.as_str()).unwrap_or("");
        let stored_salt = user_data.get("salt").and_then(|v| v.as_str()).unwrap_or("");

        if !stored_password.is_empty() {
            let password = body.password.as_deref().unwrap_or("");
            let hash = get_password_hash(password, stored_salt);
            if hash != stored_password {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error": "Incorrect password"})),
                )
                    .into_response();
            }
        }
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let settings_path = dirs.root.join(SETTINGS_FILE);
    let _ = fs::remove_file(&settings_path);

    seed_default_settings(&state.config.server_directory, &state.config.data_root, &user.handle);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/change-name` — change user display name.
///
/// Mirrors Node's `router.post('/change-name')` in `users-private.js:180-208`.
pub async fn change_name(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ChangeNameRequest>,
) -> Response {
    let name = match &body.name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing required fields"})),
            )
                .into_response();
        }
    };

    let handle = match &body.handle {
        Some(h) if !h.is_empty() => h.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing required fields"})),
            )
                .into_response();
        }
    };

    if handle != user.handle && !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "Unauthorized"})),
        )
            .into_response();
    }

    let mut user_data = match read_user(&state.config.data_root, &handle) {
        Some(u) => u,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "User not found"})),
            )
                .into_response();
        }
    };

    if let Some(obj) = user_data.as_object_mut() {
        obj.insert("name".to_string(), Value::String(name));
    }

    write_user(&state.config.data_root, &handle, &user_data);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/reset-step1` — start account reset.
///
/// Mirrors Node's `router.post('/reset-step1')` in `users-private.js:210-222`.
pub async fn reset_step1(
    Extension(user): Extension<UserContext>,
) -> Response {
    let code: u32 = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        1000 + (seed % 9000)
    };

    tracing::info!("");
    tracing::info!("{}, your account reset code is: {}", user.name, code);
    tracing::info!("");

    set_reset_code(&user.handle, code.to_string());
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/reset-step2` — complete account reset.
///
/// Mirrors Node's `router.post('/reset-step2')` in `users-private.js:224-255`.
/// Note: Requires MFA code verification via shared state.
/// During strangler pattern, this should be proxied to Node.
pub async fn reset_step2(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ResetStep2Request>,
) -> Response {
    let code = match &body.code {
        Some(c) if !c.is_empty() => c.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing required fields"})),
            )
                .into_response();
        }
    };

    if let Some(user_data) = read_user(&state.config.data_root, &user.handle) {
        let stored_password = user_data.get("password").and_then(|v| v.as_str()).unwrap_or("");
        let stored_salt = user_data.get("salt").and_then(|v| v.as_str()).unwrap_or("");
        if !stored_password.is_empty() {
            let password = body.password.as_deref().unwrap_or("");
            let hash = get_password_hash(password, stored_salt);
            if hash != stored_password {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "Incorrect password"})),
                )
                    .into_response();
            }
        }
    }

    let cached_code = get_reset_code(&user.handle);
    if cached_code.as_deref() != Some(code.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Incorrect code"})),
        )
            .into_response();
    }

    tracing::info!("Resetting account data: {}", user.handle);
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let _ = fs::remove_dir_all(&dirs.root);

    seed_default_settings(&state.config.server_directory, &state.config.data_root, &user.handle);
    clear_reset_code(&user.handle);

    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Seed default settings file for a user if it is missing.
pub(crate) fn seed_default_settings(server_dir: &Path, data_root: &Path, handle: &str) {
    let dirs = UserDirectories::new(data_root, handle);
    let _ = fs::create_dir_all(&dirs.root);
    let default_settings = server_dir.join("default/content").join(SETTINGS_FILE);
    if default_settings.exists() {
        let target = dirs.root.join(SETTINGS_FILE);
        let _ = fs::copy(&default_settings, &target);
    }
}

/// Write avatar to node-persist storage.
fn write_avatar_to_storage(data_root: &std::path::Path, handle: &str, avatar: &str) {
    let storage_dir = crate::storage::paths::DataRootDirectories::new(data_root).storage;
    let target_key = format!("avatar:{}", handle);

    let _ = fs::create_dir_all(&storage_dir);

    if let Some(path) = storage_path_for_key(&storage_dir, &target_key) {
        let updated = serde_json::json!({
            "key": target_key,
            "value": avatar,
        });
        let _ = fs::write(&path, serde_json::to_string_pretty(&updated).unwrap_or_default());
        return;
    }

    // Find existing avatar entry or create new one
    let entries = match fs::read_dir(&storage_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }

        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                let key = parsed.get("key").and_then(|v| v.as_str()).unwrap_or("");
                if key == target_key {
                    let updated = serde_json::json!({
                        "key": target_key,
                        "value": avatar,
                    });
                    let _ = fs::write(
                        &path,
                        serde_json::to_string_pretty(&updated).unwrap_or_default(),
                    );
                    return;
                }
            }
        }
    }

    // Create new avatar entry if not found
    // Node-persist uses a sha256 hash filename
    let path = storage_dir.join(storage_filename_for_key(&target_key));
    let entry = serde_json::json!({
        "key": target_key,
        "value": avatar,
    });
    let _ = fs::write(&path, serde_json::to_string_pretty(&entry).unwrap_or_default());
}

/// Generate a random salt — mirrors Node's `getPasswordSalt()`.
fn generate_salt() -> String {
    uuid::Uuid::new_v4().to_string()
}
