//! Secrets endpoints — Phase 8.
//!
//! Mirrors Node's [`src/endpoints/secrets.js`](../../../src/endpoints/secrets.js).
//!
//! ## Endpoints
//! - `POST /api/secrets/write` — write a secret.
//! - `POST /api/secrets/read` — read secret state (masked).
//! - `POST /api/secrets/view` — view all secrets (requires allowKeysExposure).
//! - `POST /api/secrets/find` — find a specific secret value.
//! - `POST /api/secrets/delete` — delete a secret.
//! - `POST /api/secrets/rotate` — rotate (activate) a secret.
//! - `POST /api/secrets/rename` — rename a secret's label.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;

const SECRETS_FILE: &str = "secrets.json";

/// Migrated marker key.
const MIGRATED_KEY: &str = "_migrated";

// ---------------------------------------------------------------------------
// Secret key constants — mirrors Node's SECRET_KEYS
// ---------------------------------------------------------------------------

/// All known secret keys.
const SECRET_KEYS: &[&str] = &[
    "api_key_horde",
    "api_key_mancer",
    "api_key_vllm",
    "api_key_aphrodite",
    "api_key_tabby",
    "api_key_openai",
    "api_key_novel",
    "api_key_claude",
    "deepl",
    "libre",
    "libre_url",
    "lingva_url",
    "api_key_openrouter",
    "api_key_ai21",
    "oneringtranslator_url",
    "deeplx_url",
    "api_key_makersuite",
    "api_key_vertexai",
    "api_key_serpapi",
    "api_key_togetherai",
    "api_key_mistralai",
    "api_key_custom",
    "api_key_ooba",
    "api_key_infermaticai",
    "api_key_dreamgen",
    "api_key_nomicai",
    "api_key_koboldcpp",
    "api_key_llamacpp",
    "api_key_cohere",
    "api_key_perplexity",
    "api_key_groq",
    "api_key_azure_tts",
    "api_key_featherless",
    "api_key_huggingface",
    "api_key_stability",
    "api_key_custom_openai_tts",
    "api_key_tavily",
    "api_key_chutes",
    "api_key_electronhub",
    "api_key_nanogpt",
    "api_key_bfl",
    "api_key_comfy_runpod",
    "api_key_falai",
    "api_key_generic",
    "api_key_deepseek",
    "api_key_serper",
    "api_key_aimlapi",
    "api_key_xai",
    "api_key_fireworks",
    "vertexai_service_account_json",
    "api_key_minimax",
    "minimax_group_id",
    "api_key_moonshot",
    "api_key_cometapi",
    "api_key_azure_openai",
    "api_key_zai",
    "api_key_siliconflow",
    "api_key_elevenlabs",
    "api_key_pollinations",
    "volcengine_app_id",
    "volcengine_access_key",
    "api_key_voyageai",
];

/// Keys safe to expose even when allowKeysExposure is false.
const EXPORTABLE_KEYS: &[&str] = &[
    "libre_url",
    "lingva_url",
    "oneringtranslator_url",
    "deeplx_url",
];

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct WriteSecretRequest {
    pub key: Option<String>,
    pub value: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FindSecretRequest {
    pub key: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteSecretRequest {
    pub key: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RotateSecretRequest {
    pub key: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RenameSecretRequest {
    pub key: Option<String>,
    pub id: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretValue {
    pub id: String,
    pub value: String,
    pub label: String,
    pub active: bool,
}

#[derive(Debug, Serialize)]
pub struct SecretState {
    pub id: String,
    pub value: String,
    pub label: String,
    pub active: bool,
}

// ---------------------------------------------------------------------------
// Secret Manager
// ---------------------------------------------------------------------------

struct SecretManager {
    file_path: std::path::PathBuf,
    backups_dir: std::path::PathBuf,
    allow_keys_exposure: bool,
}

impl SecretManager {
    fn new(data_root: &Path, handle: &str, allow_keys_exposure: bool) -> Self {
        let dirs = UserDirectories::new(data_root, handle);
        Self {
            file_path: dirs.root.join(SECRETS_FILE),
            backups_dir: dirs.backups,
            allow_keys_exposure,
        }
    }

    fn ensure_secrets_file(&self) {
        if !self.file_path.exists() {
            let _ = fs::write(&self.file_path, "{}");
        }
    }

    fn read_secrets_file(&self) -> HashMap<String, Value> {
        self.ensure_secrets_file();
        let text = match fs::read_to_string(&self.file_path) {
            Ok(text) => text,
            Err(_) => return HashMap::new(),
        };

        let parsed: HashMap<String, Value> = serde_json::from_str(&text).unwrap_or_default();
        if let Some(migrated) = self.migrate_flat_secrets_if_needed(&parsed, &text) {
            return migrated;
        }

        parsed
    }

    fn write_secrets_file(&self, secrets: &HashMap<String, Value>) {
        if let Ok(json) = serde_json::to_string_pretty(secrets) {
            let _ = fs::write(&self.file_path, json);
        }
    }

    fn migrate_flat_secrets_if_needed(
        &self,
        secrets: &HashMap<String, Value>,
        raw_contents: &str,
    ) -> Option<HashMap<String, Value>> {
        if secrets.contains_key(MIGRATED_KEY) {
            return None;
        }

        if secrets.is_empty() {
            return None;
        }

        if secrets.values().any(|v| v.is_array()) {
            return None;
        }

        let mut migrated: HashMap<String, Value> = HashMap::new();
        for (key, value) in secrets {
            if let Some(s) = value.as_str() {
                if s.trim().is_empty() {
                    continue;
                }
                let id = uuid::Uuid::new_v4().to_string();
                migrated.insert(
                    key.to_string(),
                    Value::Array(vec![serde_json::json!({
                        "id": id,
                        "value": s,
                        "label": key,
                        "active": true,
                    })]),
                );
            }
        }

        migrated.insert(MIGRATED_KEY.to_string(), Value::Array(Vec::new()));

        let _ = fs::create_dir_all(&self.backups_dir);
        let backup_path = self.backups_dir.join(format!(
            "secrets_migration_{}.json",
            chrono::Utc::now().timestamp_millis()
        ));
        let _ = fs::write(&backup_path, raw_contents);

        self.write_secrets_file(&migrated);
        tracing::info!(
            "Secrets migrated successfully, old secrets backed up to: {}",
            backup_path.display()
        );

        Some(migrated)
    }

    fn write_secret(&self, key: &str, value: &str, label: &str) -> String {
        let mut secrets = self.read_secrets_file();

        // Ensure key has an array
        let arr = secrets
            .entry(key.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));

        // Deactivate all existing secrets
        if let Some(arr) = arr.as_array_mut() {
            for item in arr.iter_mut() {
                if let Some(obj) = item.as_object_mut() {
                    obj.insert("active".to_string(), Value::Bool(false));
                }
            }
        }

        let id = uuid::Uuid::new_v4().to_string();

        let secret = serde_json::json!({
            "id": id,
            "value": value,
            "label": label,
            "active": true,
        });

        if let Some(arr) = arr.as_array_mut() {
            arr.push(secret);
        }

        self.write_secrets_file(&secrets);
        id
    }

    fn delete_secret(&self, key: &str, id: Option<&str>) {
        if !self.file_path.exists() {
            return;
        }

        let mut secrets = self.read_secrets_file();

        let arr = match secrets.get_mut(key) {
            Some(Value::Array(a)) => a,
            _ => return,
        };

        // Find target index
        let target_index = arr.iter().position(|s| {
            if let Some(id_val) = id {
                s.get("id")
                    .and_then(|v| v.as_str())
                    .map(|v| v == id_val)
                    .unwrap_or(false)
            } else {
                s.get("active")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            }
        });

        if let Some(idx) = target_index {
            arr.remove(idx);
        }

        // Reactivate first if none are active
        if !arr.is_empty() && !arr.iter().any(|s| {
            s.get("active")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        }) {
            if let Some(first) = arr.first_mut() {
                if let Some(obj) = first.as_object_mut() {
                    obj.insert("active".to_string(), Value::Bool(true));
                }
            }
        }

        // Remove key if empty
        if arr.is_empty() {
            secrets.remove(key);
        }

        self.write_secrets_file(&secrets);
    }

    fn read_secret(&self, key: &str, id: Option<&str>) -> String {
        if !self.file_path.exists() {
            return String::new();
        }

        let secrets = self.read_secrets_file();

        match secrets.get(key) {
            Some(Value::Array(arr)) if !arr.is_empty() => {
                let active = arr.iter().find(|s| {
                    if let Some(id_val) = id {
                        s.get("id")
                            .and_then(|v| v.as_str())
                            .map(|v| v == id_val)
                            .unwrap_or(false)
                    } else {
                        s.get("active")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                    }
                });
                active
                    .and_then(|s| s.get("value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            }
            _ => String::new(),
        }
    }

    fn rotate_secret(&self, key: &str, id: &str) {
        if !self.file_path.exists() {
            return;
        }

        let mut secrets = self.read_secrets_file();

        let arr = match secrets.get_mut(key) {
            Some(Value::Array(a)) => a,
            _ => return,
        };

        let target_index = arr.iter().position(|s| {
            s.get("id")
                .and_then(|v| v.as_str())
                .map(|v| v == id)
                .unwrap_or(false)
        });

        if target_index.is_none() {
            tracing::warn!("Secret with ID {} not found for key {}", id, key);
            return;
        }

        // Deactivate all
        for item in arr.iter_mut() {
            if let Some(obj) = item.as_object_mut() {
                obj.insert("active".to_string(), Value::Bool(false));
            }
        }

        // Activate target
        if let Some(idx) = target_index {
            if let Some(obj) = arr[idx].as_object_mut() {
                obj.insert("active".to_string(), Value::Bool(true));
            }
        }

        self.write_secrets_file(&secrets);
    }

    fn rename_secret(&self, key: &str, id: &str, label: &str) {
        let mut secrets = self.read_secrets_file();

        let arr = match secrets.get_mut(key) {
            Some(Value::Array(a)) => a,
            _ => return,
        };

        let target = arr.iter_mut().find(|s| {
            s.get("id")
                .and_then(|v| v.as_str())
                .map(|v| v == id)
                .unwrap_or(false)
        });

        if let Some(secret) = target {
            if let Some(obj) = secret.as_object_mut() {
                obj.insert("label".to_string(), Value::String(label.to_string()));
            }
        } else {
            tracing::warn!("Secret with ID {} not found for key {}", id, key);
            return;
        }

        self.write_secrets_file(&secrets);
    }

    fn get_masked_value(&self, value: &str, key: &str) -> String {
        if self.allow_keys_exposure || EXPORTABLE_KEYS.contains(&key) {
            return value.to_string();
        }
        let threshold = 10;
        let exposed_chars = 3;
        let placeholder = '*';
        if value.len() <= threshold {
            return std::iter::repeat(placeholder).take(threshold).collect();
        }
        let visible_end = &value[value.len() - exposed_chars..];
        let masked_middle: String =
            std::iter::repeat(placeholder).take(threshold - exposed_chars).collect();
        format!("{}{}", masked_middle, visible_end)
    }

    fn get_secret_state(&self) -> Value {
        let secrets = self.read_secrets_file();
        let mut state = serde_json::Map::new();

        for key in SECRET_KEYS {
            if *key == MIGRATED_KEY {
                continue;
            }

            match secrets.get(*key) {
                Some(Value::Array(arr)) if !arr.is_empty() => {
                    let masked: Vec<Value> = arr
                        .iter()
                        .map(|secret| {
                            let id = secret
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let value = secret
                                .get("value")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let label = secret
                                .get("label")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let active = secret
                                .get("active")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            serde_json::json!({
                                "id": id,
                                "value": self.get_masked_value(value, key),
                                "label": label,
                                "active": active,
                            })
                        })
                        .collect();
                    state.insert(key.to_string(), Value::Array(masked));
                }
                _ => {
                    state.insert(key.to_string(), Value::Null);
                }
            }
        }

        Value::Object(state)
    }

    fn get_all_secrets_flat(&self) -> HashMap<String, String> {
        let secrets = self.read_secrets_file();
        let mut result = HashMap::new();

        for (key, values) in &secrets {
            if key == MIGRATED_KEY {
                continue;
            }
            if let Some(arr) = values.as_array() {
                if !arr.is_empty() {
                    let active = arr.iter().find(|s| {
                        s.get("active")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                    });
                    if let Some(secret) = active {
                        if let Some(value) = secret.get("value").and_then(|v| v.as_str()) {
                            result.insert(key.clone(), value.to_string());
                        }
                    }
                }
            }
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/secrets/write` — write a secret.
///
/// Mirrors Node's `router.post('/write')` in `secrets.js:510-526`.
pub async fn write_secret(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<WriteSecretRequest>,
) -> Response {
    let key = match &body.key {
        Some(k) if !k.is_empty() => k.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, "Invalid key or value").into_response();
        }
    };

    let value = match &body.value {
        Some(v) => v.clone(),
        None => {
            return (StatusCode::BAD_REQUEST, "Invalid key or value").into_response();
        }
    };

    let label = body.label.as_deref().unwrap_or("Unlabeled");

    let manager = SecretManager::new(
        &state.config.data_root,
        &user.handle,
        state.config.allow_keys_exposure,
    );
    let id = manager.write_secret(&key, &value, label);

    Json(serde_json::json!({"id": id})).into_response()
}

/// `POST /api/secrets/read` — read secret state.
///
/// Mirrors Node's `router.post('/read')` in `secrets.js:528-537`.
pub async fn read_secrets(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let manager = SecretManager::new(
        &state.config.data_root,
        &user.handle,
        state.config.allow_keys_exposure,
    );
    let state_val = manager.get_secret_state();
    Json(state_val).into_response()
}

/// `POST /api/secrets/view` — view all secrets (requires allowKeysExposure).
///
/// Mirrors Node's `router.post('/view')` in `secrets.js:539-557`.
pub async fn view_secrets(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    if !state.config.allow_keys_exposure {
        tracing::error!(
            "secrets.json could not be viewed unless allowKeysExposure in config.yaml is set to true"
        );
        return StatusCode::FORBIDDEN.into_response();
    }

    let manager = SecretManager::new(
        &state.config.data_root,
        &user.handle,
        state.config.allow_keys_exposure,
    );
    let secrets = manager.get_all_secrets_flat();
    Json(secrets).into_response()
}

/// `POST /api/secrets/find` — find a specific secret value.
///
/// Mirrors Node's `router.post('/find')` in `secrets.js:559-585`.
pub async fn find_secret(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<FindSecretRequest>,
) -> Response {
    let key = match &body.key {
        Some(k) if !k.is_empty() => k.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, "Key is required").into_response();
        }
    };

    if !state.config.allow_keys_exposure && !EXPORTABLE_KEYS.contains(&key.as_str()) {
        tracing::error!(
            "Cannot fetch secrets unless allowKeysExposure in config.yaml is set to true"
        );
        return StatusCode::FORBIDDEN.into_response();
    }

    let manager = SecretManager::new(
        &state.config.data_root,
        &user.handle,
        state.config.allow_keys_exposure,
    );

    let state_val = manager.get_secret_state();
    if state_val.get(&key).and_then(|v| v.as_array()).is_none()
        && state_val.get(&key) != Some(&Value::Null)
    {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Check if the key has null (no secrets)
    if state_val.get(&key) == Some(&Value::Null) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let secret_value = manager.read_secret(&key, body.id.as_deref());
    Json(serde_json::json!({"value": secret_value})).into_response()
}

/// `POST /api/secrets/delete` — delete a secret.
///
/// Mirrors Node's `router.post('/delete')` in `secrets.js:587-603`.
pub async fn delete_secret(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteSecretRequest>,
) -> Response {
    let key = match &body.key {
        Some(k) if !k.is_empty() => k.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, "Key and ID are required").into_response();
        }
    };

    let manager = SecretManager::new(
        &state.config.data_root,
        &user.handle,
        state.config.allow_keys_exposure,
    );
    manager.delete_secret(&key, body.id.as_deref());

    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/secrets/rotate` — rotate (activate) a secret.
///
/// Mirrors Node's `router.post('/rotate')` in `secrets.js:605-621`.
pub async fn rotate_secret(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<RotateSecretRequest>,
) -> Response {
    let key = match &body.key {
        Some(k) if !k.is_empty() => k.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, "Key and ID are required").into_response();
        }
    };

    let id = match &body.id {
        Some(i) if !i.is_empty() => i.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, "Key and ID are required").into_response();
        }
    };

    let manager = SecretManager::new(
        &state.config.data_root,
        &user.handle,
        state.config.allow_keys_exposure,
    );
    manager.rotate_secret(&key, &id);

    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/secrets/rename` — rename a secret's label.
///
/// Mirrors Node's `router.post('/rename')` in `secrets.js:623-639`.
pub async fn rename_secret(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<RenameSecretRequest>,
) -> Response {
    let key = match &body.key {
        Some(k) if !k.is_empty() => k.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, "Key, ID, and label are required").into_response();
        }
    };

    let id = match &body.id {
        Some(i) if !i.is_empty() => i.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, "Key, ID, and label are required").into_response();
        }
    };

    let label = match &body.label {
        Some(l) if !l.is_empty() => l.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, "Key, ID, and label are required").into_response();
        }
    };

    let manager = SecretManager::new(
        &state.config.data_root,
        &user.handle,
        state.config.allow_keys_exposure,
    );
    manager.rename_secret(&key, &id, &label);

    StatusCode::NO_CONTENT.into_response()
}
