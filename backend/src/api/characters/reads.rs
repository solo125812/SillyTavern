//! Phase 4 — Character read endpoints.
//!
//! Implements `POST /api/characters/all`, `/get`, and `/chats`.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, warn};

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::png;

use super::helpers::*;

// ---------------------------------------------------------------------------
// POST /api/characters/all
// ---------------------------------------------------------------------------

/// Response envelope when listing all characters fails with overflow.
#[derive(Serialize)]
struct CharacterListError {
    overflow: bool,
    error: bool,
}

/// `POST /api/characters/all` — List all characters for the current user.
///
/// Reads every `.png` file in the user's characters directory, extracts
/// the embedded character card JSON, enriches it with file stats and chat
/// metadata, and returns the full array.
pub async fn get_all_characters(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let shallow = state.config.lazy_load_characters;

    match list_characters(&dirs, shallow) {
        Ok(characters) => Json(characters).into_response(),
        Err(e) => {
            error!("Failed to list characters: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CharacterListError {
                    overflow: false,
                    error: true,
                }),
            )
                .into_response()
        }
    }
}

/// List all valid character cards from the characters directory.
///
/// Uses rayon for parallel processing, mirroring Node's `Promise.all()`.
fn list_characters(dirs: &UserDirectories, shallow: bool) -> Result<Vec<Value>, std::io::Error> {
    let char_dir = &dirs.characters;

    if !char_dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(char_dir)?;

    // Collect PNG filenames first
    let png_files: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("png") {
                path.file_name().and_then(|n| n.to_str()).map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    // Process in parallel using rayon (mirrors Node's Promise.all)
    let characters: Vec<Value> = png_files
        .par_iter()
        .filter_map(|filename| {
            match process_character(filename, dirs) {
                Ok(character) => {
                    if character.get("name").and_then(|v| v.as_str()).is_some() {
                        if shallow {
                            to_shallow(&character)
                        } else {
                            Some(character)
                        }
                    } else {
                        warn!("Character {} has no 'name' field, skipping", filename);
                        None
                    }
                }
                Err(e) => {
                    warn!("Could not process character {}: {}", filename, e);
                    None
                }
            }
        })
        .collect();

    Ok(characters)
}

/// Process a single character PNG file, returning enriched JSON.
///
/// Mirrors Node's `processCharacter()`:
/// 1. Read character card JSON from PNG tEXt chunks.
/// 2. Parse and normalise to V2 format.
/// 3. Add `avatar`, `date_added`, `create_date`, `chat_size`,
///    `date_last_chat`, `data_size` fields.
fn process_character(filename: &str, dirs: &UserDirectories) -> Result<Value, String> {
    let img_path = dirs.characters.join(filename);

    // Read and parse character data from PNG
    let img_data = png::read_character_data(&img_path)
        .map_err(|e| format!("Failed to read character data: {}", e))?;

    let mut json_object: Value =
        serde_json::from_str(&img_data).map_err(|e| format!("Invalid JSON: {}", e))?;

    // Normalise to V2 format (convert legacy cards if needed)
    if json_object.get("spec").is_some() {
        png::normalise_to_v2(&mut json_object);
    } else {
        convert_v1_to_v2(&mut json_object, &dirs.worlds);
    }

    // Set avatar field
    json_object["avatar"] = Value::String(filename.to_string());

    // Store raw json_data
    json_object["json_data"] = Value::String(img_data);

    // File stats
    let char_stat = fs::metadata(&img_path)
        .map_err(|e| format!("Failed to stat {}: {}", filename, e))?;

    // date_added uses ctime (creation time) on supported platforms
    let date_added = file_ctime_ms(&char_stat);
    json_object["date_added"] = Value::from(date_added);

    // create_date: use existing value or fall back to file creation time
    if json_object.get("create_date").and_then(|v| v.as_str()).is_none() {
        let iso = timestamp_to_iso(date_added);
        json_object["create_date"] = Value::String(iso);
    }

    // Chat statistics
    let char_name = filename.strip_suffix(".png").unwrap_or(filename);
    let chats_directory = dirs.chats.join(char_name);
    let (chat_size, date_last_chat) = calculate_chat_size(&chats_directory);
    json_object["chat_size"] = Value::from(chat_size);
    json_object["date_last_chat"] = Value::from(date_last_chat);

    // Data size
    let data_size = calculate_data_size(&json_object);
    json_object["data_size"] = Value::from(data_size);

    Ok(json_object)
}

// ---------------------------------------------------------------------------
// POST /api/characters/get
// ---------------------------------------------------------------------------

/// Request body for `POST /api/characters/get`.
#[derive(Deserialize)]
pub struct GetCharacterRequest {
    pub avatar_url: Option<String>,
}

/// `POST /api/characters/get` — Get a single character's full data.
pub async fn get_character(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GetCharacterRequest>,
) -> Response {
    let avatar_url = match &body.avatar_url {
        Some(url) if !url.is_empty() => url,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    if contains_forbidden_chars(avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let file_path = dirs.characters.join(avatar_url);

    if !file_path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    match process_character(avatar_url, &dirs) {
        Ok(data) => Json(data).into_response(),
        Err(e) => {
            error!("Failed to get character {}: {}", avatar_url, e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /api/characters/chats
// ---------------------------------------------------------------------------

/// Request body for `POST /api/characters/chats`.
#[derive(Deserialize)]
pub struct CharacterChatsRequest {
    pub avatar_url: Option<String>,
    pub simple: Option<bool>,
    pub metadata: Option<bool>,
}

/// Simplified chat file entry (when `simple` is true).
#[derive(Serialize)]
struct SimpleChatFile {
    file_name: String,
    file_id: String,
}

/// Full chat info entry.
#[derive(Serialize)]
pub(crate) struct ChatInfo {
    #[serde(rename = "match")]
    pub(crate) is_match: bool,
    pub(crate) file_id: String,
    pub(crate) file_name: String,
    pub(crate) file_size: String,
    pub(crate) chat_items: usize,
    pub(crate) mes: String,
    pub(crate) last_mes: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) chat_metadata: Option<Value>,
}

/// Error response for chats endpoint.
#[derive(Serialize)]
struct ChatsError {
    error: bool,
}

/// `POST /api/characters/chats` — List chats for a character.
pub async fn get_character_chats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<CharacterChatsRequest>,
) -> Response {
    let avatar_url = match &body.avatar_url {
        Some(url) if !url.is_empty() => url,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    if contains_forbidden_chars(avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let character_directory = avatar_url.replace(".png", "");
    let chats_directory = dirs.chats.join(&character_directory);

    if !chats_directory.exists() {
        return Json(ChatsError { error: true }).into_response();
    }

    let files = match fs::read_dir(&chats_directory) {
        Ok(entries) => entries,
        Err(e) => {
            error!("Failed to read chats directory: {}", e);
            return Json(ChatsError { error: true }).into_response();
        }
    };

    // Collect .jsonl files
    let jsonl_files: Vec<String> = files
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    if jsonl_files.is_empty() {
        return Json(Vec::<Value>::new()).into_response();
    }

    // Simple mode: just return file names and IDs
    if body.simple.unwrap_or(false) {
        let simple_list: Vec<SimpleChatFile> = jsonl_files
            .iter()
            .map(|file| {
                let file_id = Path::new(file)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                SimpleChatFile {
                    file_name: file.clone(),
                    file_id,
                }
            })
            .collect();
        return Json(simple_list).into_response();
    }

    // Full mode: read chat info from each file
    let with_metadata = body.metadata.unwrap_or(false);
    let mut chat_data: Vec<ChatInfo> = Vec::new();

    for file in &jsonl_files {
        let path_to_file = chats_directory.join(file);
        match get_chat_info(&path_to_file, with_metadata) {
            Ok(info) => {
                if !info.file_name.is_empty() {
                    chat_data.push(info);
                }
            }
            Err(e) => {
                warn!("Failed to read chat info for {}: {}", file, e);
            }
        }
    }

    Json(chat_data).into_response()
}

/// Read chat information from a JSONL file.
///
/// Mirrors Node's `getChatInfo()`:
/// - Parse file name, size, item count.
/// - Read the last line for the last message.
/// - Optionally read first line for chat metadata.
pub(crate) fn get_chat_info(path_to_file: &Path, with_metadata: bool) -> Result<ChatInfo, std::io::Error> {
    let metadata = fs::metadata(path_to_file)?;
    let file_size = format_bytes(metadata.len());

    let parsed_name = path_to_file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let file_id = path_to_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let mut info = ChatInfo {
        is_match: false,
        file_id,
        file_name: parsed_name,
        file_size,
        chat_items: 0,
        mes: "[The chat is empty]".to_string(),
        last_mes: Value::from(file_mtime_ms(&metadata)),
        chat_metadata: None,
    };

    if metadata.len() == 0 {
        return Ok(info);
    }

    let file = fs::File::open(path_to_file)?;
    let reader = BufReader::new(file);

    let mut last_line = String::new();
    let mut item_counter: usize = 0;
    let mut chat_meta: Option<Value> = None;

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue,
        };

        if with_metadata && item_counter == 0 {
            if let Ok(parsed) = serde_json::from_str::<Value>(&line) {
                if parsed.get("chat_metadata").map_or(false, |v| v.is_object()) {
                    chat_meta = parsed.get("chat_metadata").cloned();
                }
            }
        }

        item_counter += 1;
        last_line = line;
    }

    if !last_line.is_empty() {
        if let Ok(json_data) = serde_json::from_str::<Value>(&last_line) {
            let has_valid_data = json_data.get("name").is_some()
                || json_data.get("character_name").is_some()
                || json_data.get("chat_metadata").is_some();

            if has_valid_data {
                info.chat_items = item_counter.saturating_sub(1);
                info.mes = json_data
                    .get("mes")
                    .and_then(|v| v.as_str())
                    .unwrap_or("[The message is empty]")
                    .to_string();

                // last_mes: use send_date if available, otherwise mtime ISO
                info.last_mes = match json_data.get("send_date") {
                    Some(sd) if sd.is_string() => sd.clone(),
                    _ => {
                        let iso = timestamp_to_iso(file_mtime_ms(&metadata));
                        Value::String(iso)
                    }
                };

                info.is_match = true;
                if with_metadata {
                    info.chat_metadata = chat_meta;
                }
                return Ok(info);
            }
        }
    }

    // Invalid or corrupted chat file — return empty to be filtered out
    info.file_name.clear();
    info.file_id.clear();
    Ok(info)
}
