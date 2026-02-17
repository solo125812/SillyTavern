//! Presets endpoints — Phase 8.
//!
//! Mirrors Node's [`src/endpoints/presets.js`](../../../src/endpoints/presets.js).
//!
//! ## Endpoints
//! - `POST /api/presets/save` — save a preset.
//! - `POST /api/presets/delete` — delete a preset.
//! - `POST /api/presets/restore` — restore a default preset.

use std::fs;
use std::path::{Path, PathBuf};
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
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetSaveRequest {
    pub name: Option<String>,
    pub preset: Option<Value>,
    pub api_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetDeleteRequest {
    pub name: Option<String>,
    pub api_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetRestoreRequest {
    pub name: Option<String>,
    pub api_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the folder and extension for preset settings based on the API source ID.
/// Mirrors Node's `getPresetSettingsByAPI()`.
fn get_preset_settings_by_api(
    api_id: &str,
    dirs: &UserDirectories,
) -> (Option<PathBuf>, &'static str) {
    match api_id {
        "kobold" | "koboldhorde" => (Some(dirs.kobold_ai_settings.clone()), ".json"),
        "novel" => (Some(dirs.novel_ai_settings.clone()), ".json"),
        "textgenerationwebui" => (Some(dirs.text_gen_settings.clone()), ".json"),
        "openai" => (Some(dirs.open_ai_settings.clone()), ".json"),
        "instruct" => (Some(dirs.instruct.clone()), ".json"),
        "context" => (Some(dirs.context.clone()), ".json"),
        "sysprompt" => (Some(dirs.sysprompt.clone()), ".json"),
        "reasoning" => (Some(dirs.reasoning.clone()), ".json"),
        _ => (None, ".json"),
    }
}

/// Get default presets from the content directory.
/// Mirrors Node's `getDefaultPresets()` from `content-manager.js`.
fn get_default_presets(
    server_dir: &Path,
    dirs: &UserDirectories,
) -> Vec<DefaultPreset> {
    let mut presets = Vec::new();
    let content_dir = server_dir.join("default/content");
    let scaffold_dir = server_dir.join("default/scaffold");

    // Load content index from both directories
    let mut content_index: Vec<ContentItem> = Vec::new();

    for (dir, index_path) in [
        (&scaffold_dir, scaffold_dir.join("index.json")),
        (&content_dir, content_dir.join("index.json")),
    ] {
        if index_path.exists() {
            if let Ok(text) = fs::read_to_string(&index_path) {
                if let Ok(items) = serde_json::from_str::<Vec<ContentItem>>(&text) {
                    for mut item in items {
                        item.folder = Some(dir.to_string_lossy().to_string());
                        content_index.push(item);
                    }
                }
            }
        }
    }

    for item in &content_index {
        let is_preset_type = item.content_type.ends_with("_preset")
            || ["instruct", "context", "sysprompt", "reasoning"]
                .contains(&item.content_type.as_str());

        if is_preset_type {
            let name = Path::new(&item.filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            let target_folder = get_target_by_type(&item.content_type, dirs);

            presets.push(DefaultPreset {
                name,
                filename: item.filename.clone(),
                folder: target_folder.map(|p| p.to_string_lossy().to_string()),
                source_folder: item.folder.clone(),
            });
        }
    }

    presets
}

/// Get the target directory for a content type.
fn get_target_by_type(content_type: &str, dirs: &UserDirectories) -> Option<PathBuf> {
    match content_type {
        "kobold_preset" => Some(dirs.kobold_ai_settings.clone()),
        "openai_preset" => Some(dirs.open_ai_settings.clone()),
        "novel_preset" => Some(dirs.novel_ai_settings.clone()),
        "textgen_preset" => Some(dirs.text_gen_settings.clone()),
        "instruct" => Some(dirs.instruct.clone()),
        "context" => Some(dirs.context.clone()),
        "sysprompt" => Some(dirs.sysprompt.clone()),
        "reasoning" => Some(dirs.reasoning.clone()),
        _ => None,
    }
}

/// Get a default preset file from the content directory.
fn get_default_preset_file(server_dir: &Path, filename: &str) -> Option<Value> {
    let content_path = server_dir.join("default/content").join(filename);
    if content_path.exists() {
        if let Ok(text) = fs::read_to_string(&content_path) {
            return serde_json::from_str(&text).ok();
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct ContentItem {
    filename: String,
    #[serde(rename = "type")]
    content_type: String,
    #[serde(skip)]
    folder: Option<String>,
}

#[allow(dead_code)]
struct DefaultPreset {
    name: String,
    filename: String,
    folder: Option<String>,
    source_folder: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/presets/save` — save a preset.
///
/// Mirrors Node's `router.post('/save')` in `presets.js:42-58`.
pub async fn save_preset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<PresetSaveRequest>,
) -> Response {
    let name = match &body.name {
        Some(n) => sanitize_filename_strip(n),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    if name.is_empty() || body.preset.is_none() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let api_id = body.api_id.as_deref().unwrap_or("");
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let (folder, extension) = get_preset_settings_by_api(api_id, &dirs);

    let folder = match folder {
        Some(f) => f,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let filename = format!("{}{}", name, extension);
    let fullpath = folder.join(&filename);

    // Ensure directory exists
    let _ = fs::create_dir_all(&folder);

    // Write preset as pretty-printed JSON
    let content = match serde_json::to_string_pretty(body.preset.as_ref().unwrap()) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to serialize preset: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if let Err(e) = fs::write(&fullpath, content) {
        tracing::error!("Failed to write preset: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Json(serde_json::json!({"name": name})).into_response()
}

/// `POST /api/presets/delete` — delete a preset.
///
/// Mirrors Node's `router.post('/delete')` in `presets.js:60-81`.
pub async fn delete_preset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<PresetDeleteRequest>,
) -> Response {
    let name = match &body.name {
        Some(n) => sanitize_filename_strip(n),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    if name.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let api_id = body.api_id.as_deref().unwrap_or("");
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let (folder, extension) = get_preset_settings_by_api(api_id, &dirs);

    let folder = match folder {
        Some(f) => f,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let filename = format!("{}{}", name, extension);
    let fullpath = folder.join(&filename);

    if fullpath.exists() {
        if let Err(e) = fs::remove_file(&fullpath) {
            tracing::error!("Failed to delete preset: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        StatusCode::OK.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// `POST /api/presets/restore` — restore a default preset.
///
/// Mirrors Node's `router.post('/restore')` in `presets.js:83-103`.
pub async fn restore_preset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<PresetRestoreRequest>,
) -> Response {
    let name = match &body.name {
        Some(n) => sanitize_filename_strip(n),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let api_id = body.api_id.as_deref().unwrap_or("");
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let (folder, _) = get_preset_settings_by_api(api_id, &dirs);

    let default_presets = get_default_presets(&state.config.server_directory, &dirs);

    let default_preset = default_presets.iter().find(|p| {
        p.name == name
            && folder
                .as_ref()
                .map(|f| {
                    p.folder
                        .as_ref()
                        .map(|pf| pf == &f.to_string_lossy().to_string())
                        .unwrap_or(false)
                })
                .unwrap_or(false)
    });

    let mut result = serde_json::json!({"isDefault": false, "preset": {}});

    if let Some(preset) = default_preset {
        result["isDefault"] = Value::Bool(true);
        if let Some(file_content) =
            get_default_preset_file(&state.config.server_directory, &preset.filename)
        {
            result["preset"] = file_content;
        }
    }

    Json(result).into_response()
}
