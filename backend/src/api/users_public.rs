//! Users public endpoints — Phase 8.
//!
//! Mirrors Node's [`src/endpoints/users-public.js`](../../../src/endpoints/users-public.js).
//!
//! These endpoints are mounted **before** the auth gate in Node.
//! In the Rust sidecar, they still receive user context from the proxy
//! but don't strictly require authentication for all operations.
//!
//! ## Endpoints
//! - `POST /api/users/list` — list enabled users (or 204 if discreet login).
//! - `POST /api/users/login` — login a user.
//! - `POST /api/users/recover-step1` — start password recovery (prints code to console).
//! - `POST /api/users/recover-step2` — complete password recovery.

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{ConnectInfo, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::router::AppState;
use crate::storage::paths::DataRootDirectories;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub handle: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecoverStep1Request {
    pub handle: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverStep2Request {
    pub handle: Option<String>,
    pub code: Option<String>,
    pub new_password: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UserViewModel {
    pub handle: String,
    pub name: String,
    pub created: Option<u64>,
    pub avatar: String,
    pub password: bool,
}

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

/// User storage key prefix — mirrors Node's `KEY_PREFIX`.
const KEY_PREFIX: &str = "user:";

/// Login rate limit — 5 points / 60 seconds (mirrors Node).
const LOGIN_RATE_POINTS: u32 = 5;
const LOGIN_RATE_WINDOW: Duration = Duration::from_secs(60);

/// Recover rate limit — 5 points / 300 seconds (mirrors Node).
const RECOVER_RATE_POINTS: u32 = 5;
const RECOVER_RATE_WINDOW: Duration = Duration::from_secs(300);

/// MFA cache TTL (5 minutes).
const MFA_TTL: Duration = Duration::from_secs(5 * 60);

// ---------------------------------------------------------------------------
// Rate limiting + MFA cache
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RateEntry {
    remaining: u32,
    reset_at: Instant,
}

#[derive(Debug)]
struct RateLimiter {
    points: u32,
    window: Duration,
    entries: Mutex<HashMap<String, RateEntry>>,
}

impl RateLimiter {
    fn new(points: u32, window: Duration) -> Self {
        Self {
            points,
            window,
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn consume(&self, key: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.entry(key.to_string()).or_insert_with(|| RateEntry {
            remaining: self.points,
            reset_at: now + self.window,
        });

        if now >= entry.reset_at {
            entry.remaining = self.points;
            entry.reset_at = now + self.window;
        }

        if entry.remaining == 0 {
            return Err(entry.reset_at.saturating_duration_since(now));
        }

        entry.remaining = entry.remaining.saturating_sub(1);
        Ok(())
    }

    fn reset(&self, key: &str) {
        let mut entries = self.entries.lock().unwrap();
        entries.remove(key);
    }
}

#[derive(Debug, Clone)]
struct CodeEntry {
    code: String,
    expires_at: Instant,
}

static LOGIN_LIMITER: OnceLock<RateLimiter> = OnceLock::new();
static RECOVER_LIMITER: OnceLock<RateLimiter> = OnceLock::new();
static MFA_CACHE: OnceLock<Mutex<HashMap<String, CodeEntry>>> = OnceLock::new();

fn login_limiter() -> &'static RateLimiter {
    LOGIN_LIMITER.get_or_init(|| RateLimiter::new(LOGIN_RATE_POINTS, LOGIN_RATE_WINDOW))
}

fn recover_limiter() -> &'static RateLimiter {
    RECOVER_LIMITER.get_or_init(|| RateLimiter::new(RECOVER_RATE_POINTS, RECOVER_RATE_WINDOW))
}

fn mfa_cache() -> &'static Mutex<HashMap<String, CodeEntry>> {
    MFA_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read all user records from the node-persist storage.
///
/// Node-persist stores items as individual JSON files in `_storage/`.
/// Each file has `{ "key": "user:handle", "value": { ... } }` structure.
pub(crate) fn read_all_users(data_root: &Path) -> Vec<Value> {
    let storage_dir = DataRootDirectories::new(data_root).storage;
    let mut users = Vec::new();

    let entries = match fs::read_dir(&storage_dir) {
        Ok(entries) => entries,
        Err(_) => return users,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }

        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                let key = parsed.get("key").and_then(|v| v.as_str()).unwrap_or("");
                if key.starts_with(KEY_PREFIX) {
                    if let Some(value) = parsed.get("value") {
                        users.push(value.clone());
                    }
                }
            }
        }
    }

    users
}

/// Read a single user record by handle.
pub(crate) fn read_user(data_root: &Path, handle: &str) -> Option<Value> {
    let storage_dir = DataRootDirectories::new(data_root).storage;
    let target_key = format!("{}{}", KEY_PREFIX, handle);

    if let Some(path) = storage_path_for_key(&storage_dir, &target_key) {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                let key = parsed.get("key").and_then(|v| v.as_str()).unwrap_or("");
                if key == target_key {
                    return parsed.get("value").cloned();
                }
            }
        }
    }

    let entries = match fs::read_dir(&storage_dir) {
        Ok(entries) => entries,
        Err(_) => return None,
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
                    return parsed.get("value").cloned();
                }
            }
        }
    }

    None
}

/// Write a user record back to node-persist storage.
pub(crate) fn write_user(data_root: &Path, handle: &str, user: &Value) {
    let storage_dir = DataRootDirectories::new(data_root).storage;
    let target_key = format!("{}{}", KEY_PREFIX, handle);

    let _ = fs::create_dir_all(&storage_dir);

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
                        "value": user,
                    });
                    let _ = fs::write(&path, serde_json::to_string_pretty(&updated).unwrap_or_default());
                    return;
                }
            }
        }
    }

    // If not found, write to the node-persist hashed filename
    let path = storage_dir.join(storage_filename_for_key(&target_key));
    let updated = serde_json::json!({
        "key": target_key,
        "value": user,
    });
    let _ = fs::write(&path, serde_json::to_string_pretty(&updated).unwrap_or_default());
}

/// Get user avatar data URL from node-persist storage.
pub(crate) fn get_user_avatar(data_root: &Path, handle: &str) -> String {
    let storage_dir = DataRootDirectories::new(data_root).storage;
    let target_key = format!("avatar:{}", handle);

    if let Some(path) = storage_path_for_key(&storage_dir, &target_key) {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                let key = parsed.get("key").and_then(|v| v.as_str()).unwrap_or("");
                if key == target_key {
                    return parsed
                        .get("value")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                }
            }
        }
    }

    let entries = match fs::read_dir(&storage_dir) {
        Ok(entries) => entries,
        Err(_) => return String::new(),
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
                    return parsed
                        .get("value")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                }
            }
        }
    }

    String::new()
}

/// Compute password hash — mirrors Node's `getPasswordHash()`.
/// Node uses: `crypto.createHash('sha256').update(password + salt).digest('hex')`
pub(crate) fn get_password_hash(password: &str, salt: &str) -> String {
    
    let hasher = sha256_digest(format!("{}{}", password, salt).as_bytes());
    hasher
}

/// SHA-256 hex digest.
fn sha256_digest(data: &[u8]) -> String {
    // Use a simple SHA-256 implementation via the crypto primitives
    // Since we don't have a sha2 crate, we'll compute it differently
    // For now, use a command or built-in approach
    // Actually, let's use ring or compute manually. Since we need to add
    // a dependency, let's use a simple approach with the existing deps.
    // We'll add sha2 to Cargo.toml.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

pub(crate) fn storage_filename_for_key(key: &str) -> String {
    sha256_digest(key.as_bytes())
}

pub(crate) fn storage_path_for_key(storage_dir: &Path, key: &str) -> Option<PathBuf> {
    let path = storage_dir.join(storage_filename_for_key(key));
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

fn normalize_ip(addr: SocketAddr) -> String {
    match addr.ip() {
        std::net::IpAddr::V6(v6) => v6.to_ipv4().map(|v4| v4.to_string()).unwrap_or_else(|| v6.to_string()),
        std::net::IpAddr::V4(v4) => v4.to_string(),
    }
}

fn get_client_ip(
    headers: &HeaderMap,
    connect_info: SocketAddr,
    prefer_real_ip_header: bool,
) -> String {
    if prefer_real_ip_header {
        if let Some(real_ip) = headers
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
        {
            return real_ip.to_string();
        }
    }

    normalize_ip(connect_info)
}

fn set_mfa_code(handle: &str, code: String) {
    let mut cache = mfa_cache().lock().unwrap();
    cache.insert(
        handle.to_string(),
        CodeEntry {
            code,
            expires_at: Instant::now() + MFA_TTL,
        },
    );
}

fn get_mfa_code(handle: &str) -> Option<String> {
    let mut cache = mfa_cache().lock().unwrap();
    let entry = cache.get(handle).cloned();
    match entry {
        Some(entry) if Instant::now() <= entry.expires_at => Some(entry.code),
        _ => {
            cache.remove(handle);
            None
        }
    }
}

fn clear_mfa_code(handle: &str) {
    let mut cache = mfa_cache().lock().unwrap();
    cache.remove(handle);
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/users/list` — list enabled users.
///
/// Mirrors Node's `router.post('/list')` in `users-public.js:26-57`.
/// Returns 204 when discreet login is enabled.
pub async fn list_users(
    State(state): State<Arc<AppState>>,
) -> Response {
    if state.config.enable_discreet_login {
        return StatusCode::NO_CONTENT.into_response();
    }

    let users = read_all_users(&state.config.data_root);

    let mut view_models: Vec<Value> = users
        .iter()
        .filter(|u| {
            u.get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .map(|u| {
            let handle = u.get("handle").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let avatar = get_user_avatar(&state.config.data_root, &handle);
            serde_json::json!({
                "handle": handle,
                "name": u.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                "created": u.get("created").and_then(|v| v.as_u64()),
                "avatar": avatar,
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

/// `POST /api/users/login` — login a user.
///
/// Mirrors Node's `router.post('/login')` in `users-public.js:59-105`.
/// Note: Session management is handled by the Node proxy. This endpoint
/// validates credentials and returns the handle on success.
pub async fn login_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<LoginRequest>,
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

    let ip = get_client_ip(
        &headers,
        addr,
        state.config.prefer_real_ip_header,
    );

    if let Err(_retry_after) = login_limiter().consume(&ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "Too many attempts. Try again later or recover your password."
            })),
        )
            .into_response();
    }

    let user = match read_user(&state.config.data_root, &handle) {
        Some(u) => u,
        None => {
            tracing::error!("Login failed: User {} not found", handle);
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "Incorrect credentials"})),
            )
                .into_response();
        }
    };

    let enabled = user.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    if !enabled {
        tracing::warn!("Login failed: User {} is disabled", handle);
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "User is disabled"})),
        )
            .into_response();
    }

    let stored_password = user.get("password").and_then(|v| v.as_str()).unwrap_or("");
    let stored_salt = user.get("salt").and_then(|v| v.as_str()).unwrap_or("");

    if !stored_password.is_empty() {
        let input_password = body.password.as_deref().unwrap_or("");
        let hash = get_password_hash(input_password, stored_salt);
        if hash != stored_password {
            tracing::warn!("Login failed: Incorrect password for {}", handle);
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "Incorrect credentials"})),
            )
                .into_response();
        }
    }

    login_limiter().reset(&ip);

    Json(serde_json::json!({"handle": handle})).into_response()
}

/// `POST /api/users/recover-step1` — start password recovery.
///
/// Mirrors Node's `router.post('/recover-step1')` in `users-public.js:107-145`.
/// Generates a 4-digit code and prints it to the server console.
pub async fn recover_step1(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<RecoverStep1Request>,
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

    let ip = get_client_ip(
        &headers,
        addr,
        state.config.prefer_real_ip_header,
    );

    if let Err(_retry_after) = recover_limiter().consume(&ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "Too many attempts. Try again later or contact your admin."
            })),
        )
            .into_response();
    }

    let user = match read_user(&state.config.data_root, &handle) {
        Some(u) => u,
        None => {
            tracing::error!("Recover step 1 failed: User {} not found", handle);
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "User not found"})),
            )
                .into_response();
        }
    };

    let enabled = user.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    if !enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "User is disabled"})),
        )
            .into_response();
    }

    let name = user.get("name").and_then(|v| v.as_str()).unwrap_or("");

    // Generate a random 4-digit code
    let code: u32 = rand_code();
    tracing::info!("");
    tracing::info!("{}, your password recovery code is: {}", name, code);
    tracing::info!("");

    set_mfa_code(&handle, code.to_string());

    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/users/recover-step2` — complete password recovery.
///
/// Mirrors Node's `router.post('/recover-step2')` in `users-public.js:147-199`.
/// Note: In the strangler pattern, this is primarily handled by Node.
/// The Rust implementation provides the endpoint structure for post-cutover.
pub async fn recover_step2(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<RecoverStep2Request>,
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

    let _code = match &body.code {
        Some(c) if !c.is_empty() => c.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing required fields"})),
            )
                .into_response();
        }
    };

    let user = match read_user(&state.config.data_root, &handle) {
        Some(u) => u,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "User not found"})),
            )
                .into_response();
        }
    };

    let enabled = user.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    if !enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "User is disabled"})),
        )
            .into_response();
    }

    let ip = get_client_ip(
        &headers,
        addr,
        state.config.prefer_real_ip_header,
    );

    let cached_code = get_mfa_code(&handle);
    if cached_code.as_deref() != body.code.as_deref() {
        if let Err(_retry_after) = recover_limiter().consume(&ip) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "error": "Too many attempts. Try again later or contact your admin."
                })),
            )
                .into_response();
        }

        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "Incorrect code"})),
        )
            .into_response();
    }

    let mut user_data = user;
    if let Some(new_password) = &body.new_password {
        if !new_password.is_empty() {
            let salt = uuid::Uuid::new_v4().to_string();
            let hash = get_password_hash(new_password, &salt);
            if let Some(obj) = user_data.as_object_mut() {
                obj.insert("password".to_string(), Value::String(hash));
                obj.insert("salt".to_string(), Value::String(salt));
            }
        } else if let Some(obj) = user_data.as_object_mut() {
            obj.insert("password".to_string(), Value::String(String::new()));
            obj.insert("salt".to_string(), Value::String(String::new()));
        }
    } else if let Some(obj) = user_data.as_object_mut() {
        obj.insert("password".to_string(), Value::String(String::new()));
        obj.insert("salt".to_string(), Value::String(String::new()));
    }

    write_user(&state.config.data_root, &handle, &user_data);
    recover_limiter().reset(&ip);
    clear_mfa_code(&handle);

    StatusCode::NO_CONTENT.into_response()
}

/// Generate a random 4-digit code (1000-9999).
fn rand_code() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    1000 + (seed % 9000)
}
