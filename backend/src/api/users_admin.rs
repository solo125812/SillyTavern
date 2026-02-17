//! Users admin endpoints — Phase 8.
//!
//! Mirrors Node's [`src/endpoints/users-admin.js`](../../../src/endpoints/users-admin.js).
//!
//! All endpoints require admin privileges.
//!
//! ## Endpoints
//! - `POST /api/users/get` — get all users (admin view).
//! - `POST /api/users/disable` — disable a user.
//! - `POST /api/users/enable` — enable a user.
//! - `POST /api/users/promote` — promote user to admin.
//! - `POST /api/users/demote` — demote user from admin.
//! - `POST /api/users/create` — create a new user.
//! - `POST /api/users/delete` — delete a user.
//! - `POST /api/users/slugify` — slugify text for handle generation.

use std::fs;
use std::sync::Arc;

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
    read_all_users,
    read_user,
    storage_filename_for_key,
    write_user,
};
use crate::api::users_private::seed_default_settings;
use crate::http::middleware::UserContext;
use crate::storage::paths::{DataRootDirectories, UserDirectories};

/// Default user handle — mirrors Node's `DEFAULT_USER.handle`.
const DEFAULT_USER_HANDLE: &str = "default-user";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct HandleRequest {
    pub handle: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub handle: Option<String>,
    pub name: Option<String>,
    pub password: Option<String>,
    pub admin: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteUserRequest {
    pub handle: Option<String>,
    pub purge: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct SlugifyRequest {
    pub text: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Slugify text — mirrors Node's `slugify()` in `users-admin.js:32-34`.
/// Converts to lowercase, trims, replaces non-alphanumeric with hyphens,
/// removes leading/trailing hyphens. Uses deburr equivalent for diacritics.
fn slugify(text: &str) -> String {
    let lowered = text.to_lowercase().trim().to_string();

    // Simple deburr — strip combining diacritical marks via Unicode normalization
    let deburred = unicode_deburr(&lowered);

    // Replace non-alphanumeric sequences with hyphens
    let re = regex::Regex::new(r"[^a-z0-9]+").unwrap();
    let slugged = re.replace_all(&deburred, "-").to_string();

    // Remove leading/trailing hyphens
    slugged.trim_matches('-').to_string()
}

/// Simple deburr — approximates lodash.deburr by removing diacritical marks.
fn unicode_deburr(input: &str) -> String {
    // Use Unicode NFD decomposition and strip combining marks (category Mn)
    
    let nfd: String = input.chars().collect();

    // Simple approach: map common accented chars, pass through others
    nfd.chars()
        .filter_map(|c| {
            if c.is_ascii() {
                Some(c)
            } else {
                // Try to decompose common characters
                match c {
                    'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => Some('a'),
                    'è' | 'é' | 'ê' | 'ë' => Some('e'),
                    'ì' | 'í' | 'î' | 'ï' => Some('i'),
                    'ò' | 'ó' | 'ô' | 'õ' | 'ö' => Some('o'),
                    'ù' | 'ú' | 'û' | 'ü' => Some('u'),
                    'ñ' => Some('n'),
                    'ç' => Some('c'),
                    'ý' | 'ÿ' => Some('y'),
                    'ß' => Some('s'),
                    'ð' => Some('d'),
                    'þ' => Some('t'),
                    'æ' => Some('a'),
                    'ø' => Some('o'),
                    _ => Some(c), // Keep as-is, will be replaced by regex
                }
            }
        })
        .collect()
}

/// Check if the current user is an admin.
fn require_admin(user: &UserContext) -> Option<Response> {
    if !user.is_admin {
        Some(
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "Forbidden: Admin access required"})),
            )
                .into_response(),
        )
    } else {
        None
    }
}

/// Write a new user entry to node-persist storage.
fn create_user_in_storage(data_root: &std::path::Path, handle: &str, user: &Value) {
    write_user(data_root, handle, user);
}

/// Remove a user entry from node-persist storage.
fn remove_user_from_storage(data_root: &std::path::Path, handle: &str) {
    let storage_dir = DataRootDirectories::new(data_root).storage;
    let target_key = format!("user:{}", handle);

    let direct_path = storage_dir.join(storage_filename_for_key(&target_key));
    if direct_path.exists() {
        let _ = fs::remove_file(&direct_path);
        return;
    }

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
                    let _ = fs::remove_file(&path);
                    return;
                }
            }
        }
    }
}

/// Get all user handles from storage.
fn get_all_user_handles(data_root: &std::path::Path) -> Vec<String> {
    read_all_users(data_root)
        .iter()
        .filter_map(|u| u.get("handle").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/users/get` — get all users (admin view).
///
/// Mirrors Node's `router.post('/get')` in `users-admin.js:36-64`.
pub async fn admin_get_users(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    if let Some(resp) = require_admin(&user) {
        return resp;
    }

    let users = read_all_users(&state.config.data_root);

    let mut view_models: Vec<Value> = users
        .iter()
        .map(|u| {
            let handle = u
                .get("handle")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let avatar = get_user_avatar(&state.config.data_root, &handle);
            serde_json::json!({
                "handle": handle,
                "name": u.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                "avatar": avatar,
                "admin": u.get("admin").and_then(|v| v.as_bool()).unwrap_or(false),
                "enabled": u.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
                "created": u.get("created").and_then(|v| v.as_u64()),
                "password": u.get("password").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false),
            })
        })
        .collect();

    // Sort by created time
    view_models.sort_by(|a, b| {
        let a_created = a.get("created").and_then(|v| v.as_u64()).unwrap_or(0);
        let b_created = b.get("created").and_then(|v| v.as_u64()).unwrap_or(0);
        a_created.cmp(&b_created)
    });

    Json(view_models).into_response()
}

/// `POST /api/users/disable` — disable a user.
///
/// Mirrors Node's `router.post('/disable')` in `users-admin.js:66-93`.
pub async fn disable_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<HandleRequest>,
) -> Response {
    if let Some(resp) = require_admin(&user) {
        return resp;
    }

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

    if handle == user.handle {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Cannot disable yourself"})),
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
        obj.insert("enabled".to_string(), Value::Bool(false));
    }

    write_user(&state.config.data_root, &handle, &user_data);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/enable` — enable a user.
///
/// Mirrors Node's `router.post('/enable')` in `users-admin.js:95-117`.
pub async fn enable_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<HandleRequest>,
) -> Response {
    if let Some(resp) = require_admin(&user) {
        return resp;
    }

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
        obj.insert("enabled".to_string(), Value::Bool(true));
    }

    write_user(&state.config.data_root, &handle, &user_data);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/promote` — promote user to admin.
///
/// Mirrors Node's `router.post('/promote')` in `users-admin.js:119-141`.
pub async fn promote_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<HandleRequest>,
) -> Response {
    if let Some(resp) = require_admin(&user) {
        return resp;
    }

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
        obj.insert("admin".to_string(), Value::Bool(true));
    }

    write_user(&state.config.data_root, &handle, &user_data);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/demote` — demote user from admin.
///
/// Mirrors Node's `router.post('/demote')` in `users-admin.js:143-170`.
pub async fn demote_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<HandleRequest>,
) -> Response {
    if let Some(resp) = require_admin(&user) {
        return resp;
    }

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

    if handle == user.handle {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Cannot demote yourself"})),
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
        obj.insert("admin".to_string(), Value::Bool(false));
    }

    write_user(&state.config.data_root, &handle, &user_data);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/create` — create a new user.
///
/// Mirrors Node's `router.post('/create')` in `users-admin.js:172-217`.
pub async fn create_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<CreateUserRequest>,
) -> Response {
    if let Some(resp) = require_admin(&user) {
        return resp;
    }

    let raw_handle = match &body.handle {
        Some(h) if !h.is_empty() => h.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing required fields"})),
            )
                .into_response();
        }
    };

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

    let handle = slugify(&raw_handle);
    if handle.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid handle"})),
        )
            .into_response();
    }

    let handles = get_all_user_handles(&state.config.data_root);
    if handles.iter().any(|h| h == &handle) {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "User already exists"})),
        )
            .into_response();
    }

    let salt = uuid::Uuid::new_v4().to_string();
    let password = match &body.password {
        Some(p) if !p.is_empty() => get_password_hash(p, &salt),
        _ => String::new(),
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let new_user = serde_json::json!({
        "handle": handle,
        "name": name,
        "created": now,
        "password": password,
        "salt": salt,
        "admin": body.admin.unwrap_or(false),
        "enabled": true,
    });

    create_user_in_storage(&state.config.data_root, &handle, &new_user);

    // Create user directories
    tracing::info!("Creating data directories for {}", handle);
    let dirs = UserDirectories::new(&state.config.data_root, &handle);
    let _ = fs::create_dir_all(&dirs.root);
    seed_default_settings(&state.config.server_directory, &state.config.data_root, &handle);

    Json(serde_json::json!({"handle": handle})).into_response()
}

/// `POST /api/users/delete` — delete a user.
///
/// Mirrors Node's `router.post('/delete')` in `users-admin.js:219-249`.
pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteUserRequest>,
) -> Response {
    if let Some(resp) = require_admin(&user) {
        return resp;
    }

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

    if handle == user.handle {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Cannot delete yourself"})),
        )
            .into_response();
    }

    if handle == DEFAULT_USER_HANDLE {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Sorry, but the default user cannot be deleted. It is required as a fallback."})),
        )
            .into_response();
    }

    remove_user_from_storage(&state.config.data_root, &handle);

    if body.purge.unwrap_or(false) {
        let dirs = UserDirectories::new(&state.config.data_root, &handle);
        tracing::info!("Deleting data directories for {}", handle);
        let _ = fs::remove_dir_all(&dirs.root);
    }

    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/slugify` — slugify text.
///
/// Mirrors Node's `router.post('/slugify')` in `users-admin.js:251-265`.
pub async fn slugify_text(
    Extension(user): Extension<UserContext>,
    Json(body): Json<SlugifyRequest>,
) -> Response {
    if !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "Forbidden: Admin access required"})),
        )
            .into_response();
    }

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing required fields"})),
            )
                .into_response();
        }
    };

    let result = slugify(&text);
    result.into_response()
}
