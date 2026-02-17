//! Phase 5 — Character write endpoints.
//!
//! Implements create, edit, edit-avatar, edit-attribute, merge-attributes,
//! rename, duplicate, delete, import, and export.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Multipart, Query, State},
    http::{header::USER_AGENT, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    Extension, Json,
};
use regex::RegexBuilder;
use serde::Serialize;
use serde_json::Value;
use tracing::{error, info};

use crate::api::router::AppState;
use crate::api::avatars::{CropParams, UploadAvatarQuery};
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::png;
use crate::api::thumbnails::{invalidate_thumbnail, ThumbnailType};
use crate::storage::sanitize::sanitize_filename_strip;
use crate::config::AppConfig;

use super::helpers::*;
use super::imports::{import_from_byaf, import_from_charx};

fn parse_crop_query(query: &UploadAvatarQuery) -> Option<CropParams> {
    query
        .crop
        .as_ref()
        .and_then(|s| serde_json::from_str::<CropParams>(s).ok())
}

fn should_bust_cache(config: &AppConfig, user_agent: Option<&str>) -> bool {
    if !config.cache_buster_enabled {
        return false;
    }

    let pattern = config.cache_buster_user_agent_pattern.trim();
    if pattern.is_empty() {
        return true;
    }

    let ua = user_agent.unwrap_or("");
    match RegexBuilder::new(pattern).case_insensitive(true).build() {
        Ok(regex) => regex.is_match(ua),
        Err(err) => {
            tracing::warn!("Invalid cacheBuster.userAgentPattern: {}", err);
            true
        }
    }
}


// ---------------------------------------------------------------------------
// POST /api/characters/create
// ---------------------------------------------------------------------------

/// `POST /api/characters/create`
///
/// Mirrors Node's `router.post('/create')` in `characters.js:1015-1045`.
/// Multipart form: optional `avatar` file, other fields are character data.
pub async fn create_character(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Query(query): Query<UploadAvatarQuery>,
    multipart: Multipart,
) -> impl IntoResponse {
    let (mut body, image_data, mut crop) = match parse_character_multipart(multipart).await {
        Ok(parsed) => parsed,
        Err(status) => return status.into_response(),
    };
    if crop.is_none() {
        crop = parse_crop_query(&query);
    }
    if image_data.is_none() {
        crop = None;
    }

    if body.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    // Sanitize character name (mirrors Node's sanitize() in /create)
    let ch_name_raw = body.get("ch_name").and_then(|v| v.as_str()).unwrap_or("");
    let ch_name = sanitize_filename_strip(ch_name_raw);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("ch_name".to_string(), Value::String(ch_name.clone()));
    }
    // Format character data
    let char_json_val = chara_format_data(&body);
    let char_json = serde_json::to_string(&char_json_val).unwrap_or_default();

    // Determine filename
    let file_name_input = body
        .get("file_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    if let Some(name) = file_name_input {
        if contains_forbidden_chars(name) {
            return StatusCode::BAD_REQUEST.into_response();
        }
    }
    let file_name = file_name_input
        .map(|s| s.to_string())
        .unwrap_or_else(|| get_png_name(&ch_name, &dirs.characters));

    let avatar_name = format!("{}.png", file_name);

    // Read image source
    let img = match read_image_source(image_data.as_deref(), &state.config.server_directory) {
        Ok(data) => data,
        Err(status) => return status.into_response(),
    };

    // Write character
    if let Err(status) = write_and_save_character(&img, &char_json, &file_name, &dirs.characters, crop.as_ref()) {
        return status.into_response();
    }

    // Create chats directory for this character
    let chats_dir = dirs.chats.join(&file_name);
    let _ = fs::create_dir_all(&chats_dir);

    Html(avatar_name).into_response()
}

// ---------------------------------------------------------------------------
// POST /api/characters/edit
// ---------------------------------------------------------------------------

/// `POST /api/characters/edit`
///
/// Mirrors Node's `router.post('/edit')` in `characters.js:1080-1138`.
pub async fn edit_character(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Query(query): Query<UploadAvatarQuery>,
    headers: HeaderMap,
    multipart: Multipart,
) -> impl IntoResponse {
    let (body, image_data, mut crop) = match parse_character_multipart(multipart).await {
        Ok(parsed) => parsed,
        Err(status) => return status.into_response(),
    };
    if crop.is_none() {
        crop = parse_crop_query(&query);
    }
    if image_data.is_none() {
        crop = None;
    }

    if body.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return (StatusCode::BAD_REQUEST, "Error: no response body detected").into_response();
    }

    let ch_name = body.get("ch_name").and_then(|v| v.as_str()).unwrap_or("");
    if ch_name.is_empty() || ch_name == "." {
        return (StatusCode::BAD_REQUEST, "Error: invalid name.").into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    let avatar_url = body.get("avatar_url").and_then(|v| v.as_str()).unwrap_or("");
    if avatar_url.is_empty() || contains_forbidden_chars(avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let target_file = Path::new(&avatar_url).file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&avatar_url)
        .to_string();

    // Format character data
    let mut char_json_val = chara_format_data(&body);
    if let Some(chat_val) = body.get("chat") {
        set_val(&mut char_json_val, "chat", chat_val.clone());
    } else if let Some(obj) = char_json_val.as_object_mut() {
        obj.remove("chat");
    }
    if let Some(cd_val) = body.get("create_date") {
        set_val(&mut char_json_val, "create_date", cd_val.clone());
    } else if let Some(obj) = char_json_val.as_object_mut() {
        obj.remove("create_date");
    }

    let char_json = serde_json::to_string(&char_json_val).unwrap_or_default();

    let avatar_path = dirs.characters.join(avatar_url);

    // Determine image source
    let has_new_image = image_data.is_some();
    let img = if let Some(ref data) = image_data {
        data.clone()
    } else {
        // Read existing character's PNG
        match fs::read(&avatar_path) {
            Ok(data) => data,
            Err(_) => match read_image_source(None, &state.config.server_directory) {
                Ok(data) => data,
                Err(status) => return status.into_response(),
            },
        }
    };

    // Write character
    if let Err(status) = write_and_save_character(&img, &char_json, &target_file, &dirs.characters, crop.as_ref()) {
        return status.into_response();
    }

    // Invalidate thumbnail if avatar changed
    if has_new_image {
        invalidate_thumbnail(&dirs, ThumbnailType::Avatar, avatar_url);
    }

    let mut response = (StatusCode::OK, "OK").into_response();
    if has_new_image {
        let user_agent = headers.get(USER_AGENT).and_then(|v| v.to_str().ok());
        if should_bust_cache(&state.config, user_agent) {
            response.headers_mut().insert(
                "clear-site-data",
                HeaderValue::from_static("\"cache\""),
            );
        }
    }

    response
}

// ---------------------------------------------------------------------------
// POST /api/characters/edit-avatar
// ---------------------------------------------------------------------------

/// `POST /api/characters/edit-avatar`
///
/// Mirrors Node's `router.post('/edit-avatar')` in `characters.js:1140-1177`.
pub async fn edit_character_avatar(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Query(query): Query<UploadAvatarQuery>,
    headers: HeaderMap,
    multipart: Multipart,
) -> impl IntoResponse {
    let (body, image_data, mut crop) = match parse_character_multipart(multipart).await {
        Ok(parsed) => parsed,
        Err(status) => return status.into_response(),
    };
    if crop.is_none() {
        crop = parse_crop_query(&query);
    }

    let avatar_url = body.get("avatar_url").and_then(|v| v.as_str()).unwrap_or("");
    if avatar_url.is_empty() || contains_forbidden_chars(avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let image_data = match image_data {
        Some(data) if !data.is_empty() => data,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let avatar_path = dirs.characters.join(avatar_url);

    // Read existing character data from the old PNG
    let char_json = match png::read_character_data(&avatar_path) {
        Ok(data) => data,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let target_file = Path::new(&avatar_url).file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&avatar_url)
        .to_string();

    // Write with new avatar image
    if let Err(status) = write_and_save_character(&image_data, &char_json, &target_file, &dirs.characters, crop.as_ref()) {
        return status.into_response();
    }

    invalidate_thumbnail(&dirs, ThumbnailType::Avatar, avatar_url);

    let mut response = (StatusCode::OK, "OK").into_response();
    let user_agent = headers.get(USER_AGENT).and_then(|v| v.to_str().ok());
    if should_bust_cache(&state.config, user_agent) {
        response.headers_mut().insert(
            "clear-site-data",
            HeaderValue::from_static("\"cache\""),
        );
    }

    response
}

// ---------------------------------------------------------------------------
// POST /api/characters/edit-attribute
// ---------------------------------------------------------------------------

/// `POST /api/characters/edit-attribute`
///
/// Mirrors Node's `router.post('/edit-attribute')` in `characters.js:1196-1240`.
pub async fn edit_character_attribute(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<EditAttributeRequest>,
) -> impl IntoResponse {
    if body.ch_name.as_deref().unwrap_or("").is_empty() || body.ch_name.as_deref().unwrap_or("") == "." {
        return (StatusCode::BAD_REQUEST, "Error: invalid name.").into_response();
    }
    if body.avatar_url.is_empty() || contains_forbidden_chars(&body.avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    if body.field == "json_data" {
        return (StatusCode::BAD_REQUEST, "Error: cannot edit json_data field.").into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let avatar_path = dirs.characters.join(&body.avatar_url);

    // Read existing character data
    let char_data_str = match png::read_character_data(&avatar_path) {
        Ok(data) => data,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let mut char: Value = match serde_json::from_str(&char_data_str) {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let data_obj = match char.get("data").and_then(|d| d.as_object()) {
        Some(obj) => obj,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // Check field exists on char or char.data
    let field_exists = char.get(&body.field).is_some() || data_obj.contains_key(&body.field);
    if !field_exists {
        return (StatusCode::BAD_REQUEST, "Error: invalid field.").into_response();
    }

    // Set the value on both top-level and data.* if applicable
    set_val(&mut char, &body.field, body.value.clone());
    set_nested_value(&mut char, &["data", &body.field], body.value);

    let char_json = serde_json::to_string(&char).unwrap_or_default();

    // Read image and rewrite
    let image_data = match fs::read(&avatar_path) {
        Ok(data) => data,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let target_file = Path::new(&body.avatar_url).file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&body.avatar_url)
        .to_string();

    if let Err(status) = write_and_save_character(&image_data, &char_json, &target_file, &dirs.characters, None) {
        return status.into_response();
    }

    (StatusCode::OK, "OK").into_response()
}

// ---------------------------------------------------------------------------
// POST /api/characters/merge-attributes
// ---------------------------------------------------------------------------

/// `POST /api/characters/merge-attributes`
///
/// Mirrors Node's `router.post('/merge-attributes')` in `characters.js:1242-1308`.
pub async fn merge_character_attributes(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(mut update): Json<Value>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let avatar_url = update
        .get("avatar")
        .and_then(|v| v.as_str())
        .or_else(|| update.get("avatar_url").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_default();
    if avatar_url.is_empty() || contains_forbidden_chars(&avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let avatar_path = dirs.characters.join(&avatar_url);

    // Read existing character data
    let char_data_str = match png::read_character_data(&avatar_path) {
        Ok(data) => data,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let mut char: Value = match serde_json::from_str(&char_data_str) {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // Deep merge update into existing, excluding json_data
    if let Some(obj) = update.as_object_mut() {
        obj.remove("json_data");
    }
    if let Some(obj) = char.as_object_mut() {
        obj.remove("json_data");
    }

    let merged = deep_merge(&char, &update);
    let mut validator = TavernCardValidator::new(&merged);
    if validator.validate().is_none() {
        let name = merged.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let error_val = validator
            .last_error()
            .map(|s| Value::String(s.to_string()))
            .unwrap_or(Value::Null);
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": format!("Validation failed for {}", name),
                "error": error_val,
            })),
        )
            .into_response();
    }
    let char_json = serde_json::to_string(&merged).unwrap_or_default();

    // Read image and rewrite
    let image_data = match fs::read(&avatar_path) {
        Ok(data) => data,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let target_file = Path::new(&avatar_url).file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&avatar_url)
        .to_string();

    if let Err(status) = write_and_save_character(&image_data, &char_json, &target_file, &dirs.characters, None) {
        return status.into_response();
    }

    (StatusCode::OK, "OK").into_response()
}

// ---------------------------------------------------------------------------
// POST /api/characters/rename
// ---------------------------------------------------------------------------

/// `POST /api/characters/rename`
///
/// Mirrors Node's `router.post('/rename')` in `characters.js:1047-1078`.
pub async fn rename_character(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<RenameRequest>,
) -> impl IntoResponse {
    if body.avatar_url.is_empty() || contains_forbidden_chars(&body.avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let new_name = sanitize_filename_strip(&body.new_name);
    if new_name.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    let old_internal_name = Path::new(&body.avatar_url)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let new_internal_name = get_png_name(&new_name, &dirs.characters);
    let new_avatar_name = format!("{}.png", new_internal_name);

    let old_avatar_path = dirs.characters.join(&body.avatar_url);
    let old_chats_path = dirs.chats.join(&old_internal_name);
    let new_chats_path = dirs.chats.join(&new_internal_name);

    // Read existing character data
    let char_data_str = match png::read_character_data(&old_avatar_path) {
        Ok(data) => data,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let mut char: Value = match serde_json::from_str(&char_data_str) {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if char.get("spec").is_some() {
        png::normalise_to_v2(&mut char);
    } else {
        convert_v1_to_v2(&mut char, &dirs.worlds);
    }

    // Update name in character data
    set_val(&mut char, "name", Value::String(new_name.clone()));
    set_nested_value(&mut char, &["data", "name"], Value::String(new_name.clone()));

    let char_json = serde_json::to_string(&char).unwrap_or_default();

    // Read image from old file
    let image_data = match fs::read(&old_avatar_path) {
        Ok(data) => data,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // Write to new file
    if let Err(status) = write_and_save_character(&image_data, &char_json, &new_internal_name, &dirs.characters, None) {
        return status.into_response();
    }

    // Move chats directory
    if old_chats_path.exists() {
        let _ = fs::rename(&old_chats_path, &new_chats_path);
    }

    // Remove old avatar file
    if old_avatar_path.exists() && old_internal_name != new_internal_name {
        let _ = fs::remove_file(&old_avatar_path);
    }

    invalidate_thumbnail(&dirs, ThumbnailType::Avatar, &body.avatar_url);

    Json(serde_json::json!({"avatar": new_avatar_name})).into_response()
}

// ---------------------------------------------------------------------------
// POST /api/characters/duplicate
// ---------------------------------------------------------------------------

/// `POST /api/characters/duplicate`
///
/// Mirrors Node's `router.post('/duplicate')` in `characters.js:1310-1352`.
pub async fn duplicate_character(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DuplicateRequest>,
) -> impl IntoResponse {
    if body.avatar_url.is_empty() || contains_forbidden_chars(&body.avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let sanitized = sanitize_filename_strip(&body.avatar_url);
    let avatar_path = dirs.characters.join(&sanitized);

    if !avatar_path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let stem = Path::new(&sanitized)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let ext = Path::new(&sanitized)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{}", s))
        .unwrap_or_else(|| ".png".to_string());

    let parts: Vec<&str> = stem.split('_').collect();
    let mut suffix = 1;
    let base_name = if parts.len() > 1 {
        let last = parts[parts.len() - 1];
        if let Ok(num) = last.parse::<u32>() {
            suffix = num + 1;
            parts[..parts.len() - 1].join("_")
        } else {
            stem.to_string()
        }
    } else {
        stem.to_string()
    };

    let mut new_name = format!("{}_{}", base_name, suffix);
    let mut new_path = dirs.characters.join(format!("{}{}", new_name, ext));
    while new_path.exists() {
        suffix += 1;
        new_name = format!("{}_{}", base_name, suffix);
        new_path = dirs.characters.join(format!("{}{}", new_name, ext));
    }

    // Simple file copy — preserves the embedded character data
    if let Err(e) = fs::copy(&avatar_path, &new_path) {
        error!("Failed to duplicate character: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let new_file = new_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&format!("{}{}", new_name, ext))
        .to_string();
    Json(serde_json::json!({"path": new_file})).into_response()
}

// ---------------------------------------------------------------------------
// POST /api/characters/delete
// ---------------------------------------------------------------------------

/// `POST /api/characters/delete`
///
/// Mirrors Node's `router.post('/delete')` in `characters.js:1354-1392`.
pub async fn delete_character(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteRequest>,
) -> impl IntoResponse {
    if body.avatar_url.is_empty() || contains_forbidden_chars(&body.avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let sanitized_avatar = sanitize_filename_strip(&body.avatar_url);
    if sanitized_avatar != body.avatar_url {
        return StatusCode::FORBIDDEN.into_response();
    }
    let avatar_path = dirs.characters.join(&body.avatar_url);

    if !avatar_path.exists() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    // Delete the character PNG
    if let Err(e) = fs::remove_file(&avatar_path) {
        error!("Failed to delete character file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    invalidate_thumbnail(&dirs, ThumbnailType::Avatar, &body.avatar_url);

    // Optionally delete chat history
    let internal_name = Path::new(&body.avatar_url)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if internal_name.is_empty() {
        return StatusCode::FORBIDDEN.into_response();
    }

    if body.delete_chats {
        let sanitized_dir = sanitize_filename_strip(internal_name);
        if sanitized_dir.is_empty() {
            return StatusCode::FORBIDDEN.into_response();
        }
        let chats_dir = dirs.chats.join(&sanitized_dir);
        if chats_dir.exists() {
            if let Err(err) = fs::remove_dir_all(&chats_dir) {
                error!("Failed to delete chats for character: {}", err);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }

    (StatusCode::OK, "OK").into_response()
}

// ---------------------------------------------------------------------------
// POST /api/characters/import
// ---------------------------------------------------------------------------

/// `POST /api/characters/import`
///
/// Mirrors Node's `router.post('/import')` in `characters.js:1405-1548`.
/// Supports `png`, `json`, and `yaml` file types.
/// Supports `charx` and `byaf` imports.
pub async fn import_character(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    multipart: Multipart,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut log_file_type = String::new();
    let mut log_size: Option<usize> = None;

    let response: Response = match parse_character_multipart(multipart).await {
        Ok((body, file_data, _crop)) => {
            let file_type = body.get("file_type").and_then(|v| v.as_str()).unwrap_or("");
            log_file_type = file_type.to_string();

            match file_data {
                Some(data) if !data.is_empty() => {
                    log_size = Some(data.len());
                    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

                    // Preserved name from the original request
                    let preserved_name = body
                        .get("preserved_name")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .and_then(|s| Path::new(s).file_stem().and_then(|s| s.to_str()))
                        .map(|s| s.to_string());

                    let result: Result<String, Response> = match file_type {
                        "png" => import_from_png(&data, &dirs, preserved_name.as_deref(), &state).await,
                        "json" => import_from_json(&data, &dirs, preserved_name.as_deref(), &state).await,
                        "yaml" | "yml" => import_from_yaml(&data, &dirs, preserved_name.as_deref(), &state).await,
                        "charx" => import_from_charx(&data, &dirs, preserved_name.as_deref(), &state).await,
                        "byaf" => import_from_byaf(&data, &dirs, preserved_name.as_deref(), &state, &body).await,
                        _ => Err((StatusCode::BAD_REQUEST, "Unknown file type").into_response()),
                    };

                    match result {
                        Ok(file_name) => {
                            if let Some(preserved) = preserved_name {
                                invalidate_thumbnail(&dirs, ThumbnailType::Avatar, &format!("{}.png", preserved));
                            }
                            Json(serde_json::json!({"file_name": file_name})).into_response()
                        }
                        Err(resp) => resp,
                    }
                }
                _ => StatusCode::BAD_REQUEST.into_response(),
            }
        }
        Err(status) => status.into_response(),
    };

    let status = response.status().as_u16();
    info!(
        "character import: user={} type={} bytes={} status={} elapsed_ms={}",
        user.handle,
        log_file_type,
        log_size.unwrap_or(0),
        status,
        start.elapsed().as_millis()
    );

    response
}

async fn import_from_png(
    file_data: &[u8],
    dirs: &UserDirectories,
    preserved_name: Option<&str>,
    _state: &Arc<AppState>,
) -> Result<String, Response> {
    // Read embedded character data
    let char_data_str = match png::read_character_data_from_bytes(file_data) {
        Ok(data) => data,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "No character data in PNG").into_response()),
    };

    let mut char_val: Value = match serde_json::from_str(&char_data_str) {
        Ok(v) => v,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "Invalid character JSON").into_response()),
    };

    let name_from_card = char_val
        .get("data")
        .and_then(|d| d.get("name"))
        .and_then(|v| v.as_str())
        .or_else(|| char_val.get("name").and_then(|v| v.as_str()))
        .unwrap_or("Unknown");
    let name_for_file = sanitize_filename_strip(name_from_card);
    let file_name = preserved_name
        .map(|s| s.to_string())
        .unwrap_or_else(|| get_png_name(&name_for_file, &dirs.characters));
    set_val(&mut char_val, "name", Value::String(name_for_file.clone()));

    // Process the character
    if char_val.get("spec").is_some() {
        crate::api::sprites::import_risu_sprites(dirs, &mut char_val);
        unset_private_fields(&mut char_val);
        png::normalise_to_v2(&mut char_val);
    } else {
        let sanitized = name_for_file.clone();
        if let Some(notes) = char_val.get("creator_notes").and_then(|v| v.as_str()) {
            let cleaned = notes.replace("Creator's notes go here.", "");
            set_val(&mut char_val, "creator_notes", Value::String(cleaned));
        }
        let base = serde_json::json!({
            "name": sanitized,
            "description": char_val.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            "creatorcomment": char_val.get("creatorcomment").and_then(|v| v.as_str())
                .or_else(|| char_val.get("creator_notes").and_then(|v| v.as_str()))
                .unwrap_or(""),
            "personality": char_val.get("personality").and_then(|v| v.as_str()).unwrap_or(""),
            "first_mes": char_val.get("first_mes").and_then(|v| v.as_str()).unwrap_or(""),
            "avatar": "none",
            "chat": format!("{} - {}", sanitized, png::humanized_date_time()),
            "mes_example": char_val.get("mes_example").and_then(|v| v.as_str()).unwrap_or(""),
            "scenario": char_val.get("scenario").and_then(|v| v.as_str()).unwrap_or(""),
            "create_date": chrono::Utc::now().to_rfc3339(),
            "talkativeness": char_val.get("talkativeness").cloned().unwrap_or(Value::from(0.5)),
            "creator": char_val.get("creator").and_then(|v| v.as_str()).unwrap_or(""),
            "tags": char_val.get("tags").cloned().unwrap_or(Value::String(String::new())),
        });
        char_val = convert_to_v2(&base);
    }

    set_val(&mut char_val, "create_date", Value::String(chrono::Utc::now().to_rfc3339()));

    let char_json = serde_json::to_string(&char_val).unwrap_or_default();

    // Write to the PNG (reuse original PNG image)
    if let Err(status) = write_and_save_character(file_data, &char_json, &file_name, &dirs.characters, None) {
        return Err(status.into_response());
    }

    Ok(file_name)
}

async fn import_from_json(
    file_data: &[u8],
    dirs: &UserDirectories,
    preserved_name: Option<&str>,
    state: &Arc<AppState>,
) -> Result<String, Response> {
    let json_str = match std::str::from_utf8(file_data) {
        Ok(s) => s,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "Invalid UTF-8").into_response()),
    };

    let mut json_data: Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "Invalid JSON").into_response()),
    };

    if let Some(char_data) = json_data.get("char_json").cloned() {
        json_data = char_data;
    }

    if json_data.get("spec").is_some() {
        crate::api::sprites::import_risu_sprites(dirs, &mut json_data);
        unset_private_fields(&mut json_data);
        png::normalise_to_v2(&mut json_data);
        set_val(&mut json_data, "create_date", Value::String(chrono::Utc::now().to_rfc3339()));

        let name = json_data
            .get("data")
            .and_then(|d| d.get("name"))
            .and_then(|v| v.as_str())
            .or_else(|| json_data.get("name").and_then(|v| v.as_str()))
            .unwrap_or("Unknown");
        let file_name = preserved_name
            .map(|s| s.to_string())
            .unwrap_or_else(|| get_png_name(name, &dirs.characters));

        let char_json = serde_json::to_string(&json_data).unwrap_or_default();
        let default_img = match read_image_source(None, &state.config.server_directory) {
            Ok(data) => data,
            Err(status) => return Err(status.into_response()),
        };
        if let Err(status) = write_and_save_character(&default_img, &char_json, &file_name, &dirs.characters, None) {
            return Err(status.into_response());
        }
        return Ok(file_name);
    }

    if let Some(name) = json_data.get("name").and_then(|v| v.as_str()) {
        let sanitized = sanitize_filename_strip(name);
        if let Some(notes) = json_data.get("creator_notes").and_then(|v| v.as_str()) {
            let cleaned = notes.replace("Creator's notes go here.", "");
            json_data.as_object_mut().map(|o| o.insert("creator_notes".to_string(), Value::String(cleaned)));
        }

        let file_name = preserved_name
            .map(|s| s.to_string())
            .unwrap_or_else(|| get_png_name(&sanitized, &dirs.characters));
        let base = serde_json::json!({
            "name": sanitized,
            "description": json_data.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            "creatorcomment": json_data.get("creatorcomment").and_then(|v| v.as_str())
                .or_else(|| json_data.get("creator_notes").and_then(|v| v.as_str()))
                .unwrap_or(""),
            "personality": json_data.get("personality").and_then(|v| v.as_str()).unwrap_or(""),
            "first_mes": json_data.get("first_mes").and_then(|v| v.as_str()).unwrap_or(""),
            "avatar": "none",
            "chat": format!("{} - {}", sanitized, png::humanized_date_time()),
            "mes_example": json_data.get("mes_example").and_then(|v| v.as_str()).unwrap_or(""),
            "scenario": json_data.get("scenario").and_then(|v| v.as_str()).unwrap_or(""),
            "create_date": chrono::Utc::now().to_rfc3339(),
            "talkativeness": json_data.get("talkativeness").cloned().unwrap_or(Value::from(0.5)),
            "creator": json_data.get("creator").and_then(|v| v.as_str()).unwrap_or(""),
            "tags": json_data.get("tags").cloned().unwrap_or(Value::String(String::new())),
        });
        let char_val = convert_to_v2(&base);
        let char_json = serde_json::to_string(&char_val).unwrap_or_default();
        let default_img = match read_image_source(None, &state.config.server_directory) {
            Ok(data) => data,
            Err(status) => return Err(status.into_response()),
        };
        if let Err(status) = write_and_save_character(&default_img, &char_json, &file_name, &dirs.characters, None) {
            return Err(status.into_response());
        }
        return Ok(file_name);
    }

    if let Some(char_name) = json_data.get("char_name").and_then(|v| v.as_str()) {
        let sanitized = sanitize_filename_strip(char_name);
        if let Some(notes) = json_data.get("creator_notes").and_then(|v| v.as_str()) {
            let cleaned = notes.replace("Creator's notes go here.", "");
            json_data.as_object_mut().map(|o| o.insert("creator_notes".to_string(), Value::String(cleaned)));
        }

        let file_name = preserved_name
            .map(|s| s.to_string())
            .unwrap_or_else(|| get_png_name(&sanitized, &dirs.characters));
        let base = serde_json::json!({
            "name": sanitized,
            "description": json_data.get("char_persona").and_then(|v| v.as_str()).unwrap_or(""),
            "creatorcomment": json_data.get("creatorcomment").and_then(|v| v.as_str())
                .or_else(|| json_data.get("creator_notes").and_then(|v| v.as_str()))
                .unwrap_or(""),
            "personality": "",
            "first_mes": json_data.get("char_greeting").and_then(|v| v.as_str()).unwrap_or(""),
            "avatar": "none",
            "chat": format!("{} - {}", json_data.get("name").and_then(|v| v.as_str()).unwrap_or("undefined"), png::humanized_date_time()),
            "mes_example": json_data.get("example_dialogue").and_then(|v| v.as_str()).unwrap_or(""),
            "scenario": json_data.get("world_scenario").and_then(|v| v.as_str()).unwrap_or(""),
            "create_date": chrono::Utc::now().to_rfc3339(),
            "talkativeness": json_data.get("talkativeness").cloned().unwrap_or(Value::from(0.5)),
            "creator": json_data.get("creator").and_then(|v| v.as_str()).unwrap_or(""),
            "tags": json_data.get("tags").cloned().unwrap_or(Value::String(String::new())),
        });
        let char_val = convert_to_v2(&base);
        let char_json = serde_json::to_string(&char_val).unwrap_or_default();
        let default_img = match read_image_source(None, &state.config.server_directory) {
            Ok(data) => data,
            Err(status) => return Err(status.into_response()),
        };
        if let Err(status) = write_and_save_character(&default_img, &char_json, &file_name, &dirs.characters, None) {
            return Err(status.into_response());
        }
        return Ok(file_name);
    }

    Err((StatusCode::BAD_REQUEST, "Invalid JSON").into_response())
}

async fn import_from_yaml(
    file_data: &[u8],
    dirs: &UserDirectories,
    preserved_name: Option<&str>,
    state: &Arc<AppState>,
) -> Result<String, Response> {
    let yaml_str = match std::str::from_utf8(file_data) {
        Ok(s) => s,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "Invalid UTF-8").into_response()),
    };

    let yaml_data: Value = match serde_yaml::from_str(yaml_str) {
        Ok(v) => v,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "Invalid YAML").into_response()),
    };

    let name = yaml_data.get("name").and_then(|v| v.as_str()).unwrap_or("Unknown");
    let sanitized_name = sanitize_filename_strip(name);
    let file_name = preserved_name
        .map(|s| s.to_string())
        .unwrap_or_else(|| get_png_name(&sanitized_name, &dirs.characters));

    let base = serde_json::json!({
        "name": sanitized_name,
        "description": yaml_data.get("context").and_then(|v| v.as_str()).unwrap_or(""),
        "first_mes": yaml_data.get("greeting").and_then(|v| v.as_str()).unwrap_or(""),
        "create_date": chrono::Utc::now().to_rfc3339(),
        "chat": format!("{} - {}", sanitized_name, png::humanized_date_time()),
        "personality": "",
        "creatorcomment": "",
        "avatar": "none",
        "mes_example": "",
        "scenario": "",
        "talkativeness": 0.5,
        "creator": "",
        "tags": "",
    });

    let char_val = convert_to_v2(&base);
    let char_json = serde_json::to_string(&char_val).unwrap_or_default();

    // Write to default avatar
    let default_img = match read_image_source(None, &state.config.server_directory) {
        Ok(data) => data,
        Err(status) => return Err(status.into_response()),
    };

    if let Err(status) = write_and_save_character(&default_img, &char_json, &file_name, &dirs.characters, None) {
        return Err(status.into_response());
    }

    Ok(file_name)
}

// ---------------------------------------------------------------------------
// POST /api/characters/export
// ---------------------------------------------------------------------------

/// `POST /api/characters/export`
///
/// Mirrors Node's `router.post('/export')` in `characters.js:1505-1548`.
/// Supports `png` and `json` export formats.
pub async fn export_character(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ExportRequest>,
) -> impl IntoResponse {
    if body.avatar_url.is_empty() || contains_forbidden_chars(&body.avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let sanitized = sanitize_filename_strip(&body.avatar_url);
    let avatar_path = dirs.characters.join(&sanitized);

    if !avatar_path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    match body.format.as_str() {
        "png" => {
            // Read the raw PNG bytes
            let raw_png = match fs::read(&avatar_path) {
                Ok(data) => data,
                Err(e) => {
                    error!("Failed to read character PNG for export: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };

            // Read the embedded data, strip private fields, re-embed
            if let Ok(char_data_str) = png::read_character_data_from_bytes(&raw_png) {
                if let Ok(mut char_val) = serde_json::from_str::<Value>(&char_data_str) {
                    unset_private_fields(&mut char_val);
                    let clean_json = serde_json::to_string(&char_val).unwrap_or_default();
                    if let Ok(clean_png) = png::write_character_data(&raw_png, &clean_json) {
                        let file_stem = Path::new(&body.avatar_url)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("character");
                        return (
                            StatusCode::OK,
                            [
                                ("content-type", "image/png"),
                                ("content-disposition", &format!("attachment; filename=\"{}.png\"", file_stem)),
                            ],
                            clean_png,
                        ).into_response();
                    }
                }
            }

            // Fallback: return raw PNG as-is
            let file_stem = Path::new(&sanitized)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("character");
            (
                StatusCode::OK,
                [
                    ("content-type", "image/png"),
                    ("content-disposition", &format!("attachment; filename=\"{}.png\"", file_stem)),
                ],
                raw_png,
            ).into_response()
        }
        "json" => {
            // Read character data, strip private fields, return as JSON
            let char_data_str = match png::read_character_data(&avatar_path) {
                Ok(data) => data,
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            };

            let mut char_val: Value = match serde_json::from_str(&char_data_str) {
                Ok(v) => v,
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            };

            // Convert to V2 if needed
            if char_val.get("spec").is_some() {
                png::normalise_to_v2(&mut char_val);
            } else {
                convert_v1_to_v2(&mut char_val, &dirs.worlds);
            }

            unset_private_fields(&mut char_val);

            let file_stem = Path::new(&sanitized)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("character");

            let mut json_buf = Vec::new();
            let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
            let mut ser = serde_json::Serializer::with_formatter(&mut json_buf, formatter);
            if char_val.serialize(&mut ser).is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            let json_bytes = String::from_utf8(json_buf).unwrap_or_default();
            (
                StatusCode::OK,
                [
                    ("content-type", "application/json; charset=utf-8"),
                    ("content-disposition", &format!("attachment; filename=\"{}.json\"", file_stem)),
                ],
                json_bytes,
            ).into_response()
        }
        _ => (StatusCode::BAD_REQUEST, "Unsupported export format").into_response(),
    }
}
