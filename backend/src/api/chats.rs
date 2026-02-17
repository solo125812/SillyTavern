//! Phase 6 — Chat endpoints.
//!
//! Mirrors `src/endpoints/chats.js` from the Node server.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Extension, Multipart, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::error;

use crate::api::characters::helpers::{
    contains_forbidden_chars, file_mtime_ms,
};
use crate::api::characters::reads::get_chat_info;
use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::jsonl::{
    check_chat_integrity, get_preview_message, read_jsonl_file,
    BackupThrottle, CHAT_BACKUPS_PREFIX,
};
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename;

// ---------------------------------------------------------------------------
// Lazy-initialised backup throttle (one per process)
// ---------------------------------------------------------------------------

use std::sync::LazyLock;

/// Global backup throttle shared across all requests.
/// Interval is set on first use from config; defaults to 10s.
static BACKUP_THROTTLE: LazyLock<Arc<BackupThrottle>> = LazyLock::new(|| {
    // The throttle interval is read from config at app startup.
    // We default to 10_000ms here; the actual value is passed per-call.
    Arc::new(BackupThrottle::new(10_000))
});

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SaveChatRequest {
    pub avatar_url: Option<String>,
    pub file_name: Option<String>,
    pub chat: Option<Vec<Value>>,
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Deserialize)]
pub struct GetChatRequest {
    pub avatar_url: Option<String>,
    pub file_name: Option<String>,
}

#[derive(Deserialize)]
pub struct RenameChatRequest {
    pub avatar_url: Option<String>,
    pub original_file: Option<String>,
    pub renamed_file: Option<String>,
    #[serde(default)]
    pub is_group: Option<bool>,
}

#[derive(Deserialize)]
pub struct DeleteChatRequest {
    pub avatar_url: Option<String>,
    pub chatfile: Option<String>,
}

#[derive(Deserialize)]
pub struct ExportChatRequest {
    pub file: Option<String>,
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub is_group: Option<bool>,
    pub format: Option<String>,
    pub exportfilename: Option<String>,
}

#[derive(Deserialize)]
pub struct SearchChatRequest {
    pub query: Option<String>,
    pub avatar_url: Option<String>,
    pub group_id: Option<String>,
}

#[derive(Deserialize)]
pub struct RecentChatRequest {
    #[serde(default)]
    pub pinned: Option<Vec<PinnedChat>>,
    pub max: Option<Value>,
    #[serde(default)]
    pub metadata: Option<bool>,
}

#[derive(Deserialize, Clone)]
pub struct PinnedChat {
    pub file_name: Option<String>,
    pub avatar: Option<String>,
    pub group: Option<String>,
}

#[derive(Deserialize)]
pub struct GroupIdRequest {
    pub id: Option<String>,
}

#[derive(Deserialize)]
pub struct GroupSaveRequest {
    pub id: Option<String>,
    pub chat: Option<Vec<Value>>,
    #[serde(default)]
    pub force: Option<bool>,
}

// ---------------------------------------------------------------------------
// Import format converters
// ---------------------------------------------------------------------------

fn import_ooba_chat(user_name: &str, character_name: &str, data: &Value) -> Option<String> {
    let header = json!({"chat_metadata": {}, "user_name": "unused", "character_name": "unused"});
    let mut chat = vec![serde_json::to_string(&header).ok()?];
    let now = chrono::Utc::now().to_rfc3339();

    for arr in data.get("data_visible")?.as_array()? {
        let arr = arr.as_array()?;
        if let Some(user_msg) = arr.first().and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            chat.push(serde_json::to_string(&json!({
                "name": user_name, "is_user": true, "send_date": now, "mes": user_msg, "extra": {}
            })).unwrap_or_default());
        }
        if let Some(char_msg) = arr.get(1).and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            chat.push(serde_json::to_string(&json!({
                "name": character_name, "is_user": false, "send_date": now, "mes": char_msg, "extra": {}
            })).unwrap_or_default());
        }
    }
    Some(chat.join("\n"))
}

fn import_agnai_chat(user_name: &str, character_name: &str, data: &Value) -> Option<String> {
    let header = json!({"chat_metadata": {}, "user_name": "unused", "character_name": "unused"});
    let mut chat = vec![serde_json::to_string(&header).ok()?];
    let now = chrono::Utc::now().to_rfc3339();

    for message in data.get("messages")?.as_array()? {
        let is_user = message.get("userId").is_some();
        let mes = message.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        chat.push(serde_json::to_string(&json!({
            "name": if is_user { user_name } else { character_name },
            "is_user": is_user, "send_date": now, "mes": mes, "extra": {}
        })).unwrap_or_default());
    }
    Some(chat.join("\n"))
}

fn import_cai_chat(user_name: &str, character_name: &str, data: &Value) -> Option<Vec<String>> {
    let histories = data.pointer("/histories/histories")?.as_array()?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = Vec::new();

    for history in histories {
        let header = json!({"chat_metadata": {}, "user_name": "unused", "character_name": "unused"});
        let mut chat = vec![serde_json::to_string(&header).ok()?];
        if let Some(msgs) = history.get("msgs").and_then(|v| v.as_array()) {
            for msg in msgs {
                let is_human = msg.pointer("/src/is_human").and_then(|v| v.as_bool()).unwrap_or(false);
                let text = msg.get("text").and_then(|v| v.as_str()).unwrap_or("");
                chat.push(serde_json::to_string(&json!({
                    "name": if is_human { user_name } else { character_name },
                    "is_user": is_human, "send_date": now, "mes": text, "extra": {}
                })).unwrap_or_default());
            }
        }
        result.push(chat.join("\n"));
    }
    Some(result)
}

fn import_kobold_lite_chat(data: &Value) -> Option<String> {
    let input_token = "{{[INPUT]}}";
    let output_token = "{{[OUTPUT]}}";
    let user_name = data.pointer("/savedsettings/chatname")?.as_str().unwrap_or("User");
    let char_raw = data.pointer("/savedsettings/chatopponent")?.as_str().unwrap_or("Character");
    let character_name = char_raw.split("||$||").next().unwrap_or(char_raw);
    let now = chrono::Utc::now().to_rfc3339();

    let header = json!({"chat_metadata": {}, "user_name": "unused", "character_name": "unused"});
    let mut chat = vec![serde_json::to_string(&header).ok()?];

    let process = |msg: &str| -> String {
        let is_user = msg.contains(input_token);
        let cleaned = msg.replace(input_token, "").replace(output_token, "");
        serde_json::to_string(&json!({
            "name": if is_user { user_name } else { character_name },
            "is_user": is_user, "mes": cleaned.trim(), "send_date": now, "extra": {}
        })).unwrap_or_default()
    };

    if let Some(prompt) = data.get("prompt").and_then(|v| v.as_str()) {
        chat.push(process(prompt));
    }
    for action in data.get("actions")?.as_array()? {
        if let Some(msg) = action.as_str() {
            chat.push(process(msg));
        }
    }
    Some(chat.join("\n"))
}

fn import_risu_chat(user_name: &str, character_name: &str, data: &Value) -> Option<String> {
    let header = json!({"chat_metadata": {}, "user_name": "unused", "character_name": "unused"});
    let mut chat = vec![serde_json::to_string(&header).ok()?];

    for message in data.pointer("/data/message")?.as_array()? {
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let is_user = role == "user";
        let name = message.get("name").and_then(|v| v.as_str())
            .unwrap_or(if is_user { user_name } else { character_name });
        let time = message.get("time").and_then(|v| v.as_u64()).unwrap_or_else(|| {
            chrono::Utc::now().timestamp_millis() as u64
        });
        let send_date = chrono::DateTime::from_timestamp_millis(time as i64)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        let mes = message.get("data").and_then(|v| v.as_str()).unwrap_or("");

        chat.push(serde_json::to_string(&json!({
            "name": name, "is_user": is_user, "send_date": send_date, "mes": mes, "extra": {}
        })).unwrap_or_default());
    }
    Some(chat.join("\n"))
}

fn flatten_chub_chat(lines: &[&str]) -> String {
    lines.iter().map(|line| {
        match serde_json::from_str::<Value>(line) {
            Ok(mut data) => {
                // Flatten mes.message → mes
                if let Some(inner) = data.pointer("/mes/message").cloned() {
                    data["mes"] = inner;
                }
                // Flatten swipes[].message → swipes[]
                if let Some(swipes) = data.get("swipes").and_then(|v| v.as_array()).cloned() {
                    let flat: Vec<Value> = swipes.iter().map(|s| {
                        if let Some(msg) = s.get("message") { msg.clone() } else { s.clone() }
                    }).collect();
                    data["swipes"] = Value::Array(flat);
                }
                serde_json::to_string(&data).unwrap_or_else(|_| line.to_string())
            }
            Err(_) => line.to_string(),
        }
    }).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------------------
// Core save helper (mirrors trySaveChat)
// ---------------------------------------------------------------------------

fn try_save_chat(
    chat_data: &[Value],
    file_path: &Path,
    skip_integrity: bool,
    handle: &str,
    card_name: &str,
    backup_dir: &Path,
    config: &crate::config::AppConfig,
) -> Result<(), Response> {
    let jsonl_data: String = chat_data
        .iter()
        .map(|m| serde_json::to_string(m).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    let do_check = config.chat_backup_check_integrity && !skip_integrity;
    if do_check {
        if let Some(slug) = chat_data
            .first()
            .and_then(|h| h.pointer("/chat_metadata/integrity"))
            .and_then(|v| v.as_str())
        {
            if !check_chat_integrity(file_path, slug) {
                error!("Chat integrity check failed for {:?}", file_path);
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "integrity"})),
                ).into_response());
            }
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = file_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Write the chat file atomically
    crate::storage::atomic::atomic_write_file(file_path, jsonl_data.as_bytes())
        .map_err(|e| {
            error!("Failed to write chat file {:?}: {}", file_path, e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })?;

    // Throttled backup
    if config.chat_backup_enabled {
        BACKUP_THROTTLE
            .clone()
            .try_backup(
                handle,
                backup_dir,
                card_name,
                &jsonl_data,
                CHAT_BACKUPS_PREFIX,
                config.chat_backup_num_per_chat,
                config.chat_backup_max_total,
                config.chat_backup_throttle_ms,
            );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Endpoint handlers
// ---------------------------------------------------------------------------

/// `POST /api/chats/save` — Save a chat with integrity check + backup.
pub async fn save_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<SaveChatRequest>,
) -> Response {
    let avatar_url = match &body.avatar_url {
        Some(u) if !u.is_empty() => u.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    if contains_forbidden_chars(avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let card_name = avatar_url.trim_end_matches(".png");
    let file_name = match &body.file_name {
        Some(f) if !f.is_empty() => f.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let chat_data = match &body.chat {
        Some(c) => c,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "The request's body.chat is not an array."}))).into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let chat_filename = format!("{}.jsonl", file_name);
    let chat_file_path = dirs.chats.join(card_name).join(sanitize_filename(&chat_filename, ""));
    let skip = body.force.unwrap_or(false);

    match try_save_chat(chat_data, &chat_file_path, skip, &user.handle, card_name, &dirs.backups, &state.config) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(resp) => resp,
    }
}

/// `POST /api/chats/get` — Get chat data as JSON array.
pub async fn get_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GetChatRequest>,
) -> Response {
    let avatar_url = match &body.avatar_url {
        Some(u) if !u.is_empty() => u.as_str(),
        _ => return Json(json!({})).into_response(),
    };
    if contains_forbidden_chars(avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let dir_name = avatar_url.trim_end_matches(".png");
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let directory_path = dirs.chats.join(dir_name);

    if !directory_path.exists() {
        let _ = fs::create_dir_all(&directory_path);
        return Json(json!({})).into_response();
    }

    let file_name = match &body.file_name {
        Some(f) if !f.is_empty() => f.as_str(),
        _ => return Json(json!({})).into_response(),
    };

    let chat_filename = format!("{}.jsonl", file_name);
    let chat_file_path = directory_path.join(sanitize_filename(&chat_filename, ""));
    let data = read_jsonl_file(&chat_file_path);
    Json(json!(data)).into_response()
}

/// `POST /api/chats/rename` — Rename a chat file.
pub async fn rename_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<RenameChatRequest>,
) -> Response {
    let original = match &body.original_file {
        Some(f) if !f.is_empty() => f.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let renamed = match &body.renamed_file {
        Some(f) if !f.is_empty() => f.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let is_group = body.is_group.unwrap_or(false);

    let folder = if is_group {
        dirs.group_chats.clone()
    } else {
        let avatar_url = match &body.avatar_url {
            Some(u) if !u.is_empty() => u.as_str(),
            _ => return StatusCode::BAD_REQUEST.into_response(),
        };
        if contains_forbidden_chars(avatar_url) {
            return StatusCode::BAD_REQUEST.into_response();
        }
        dirs.chats.join(avatar_url.trim_end_matches(".png"))
    };

    let original_path = folder.join(sanitize_filename(original, ""));
    let renamed_path = folder.join(sanitize_filename(renamed, ""));
    let sanitized_name = renamed_path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    if !original_path.exists() || renamed_path.exists() {
        error!("Either source or destination files are not available");
        return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
    }

    // Copy + delete (mirrors Node behavior)
    if let Err(e) = fs::copy(&original_path, &renamed_path) {
        error!("Error renaming chat file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let _ = fs::remove_file(&original_path);

    Json(json!({"ok": true, "sanitizedFileName": sanitized_name})).into_response()
}

/// `POST /api/chats/delete` — Delete a chat file.
pub async fn delete_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteChatRequest>,
) -> Response {
    let avatar_url = match &body.avatar_url {
        Some(u) if !u.is_empty() => u.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    if contains_forbidden_chars(avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut chatfile = body.chatfile.unwrap_or_default();
    if chatfile.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !chatfile.contains('.') {
        chatfile.push_str(".jsonl");
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let dir_name = avatar_url.trim_end_matches(".png");
    let chat_file_path = dirs.chats.join(dir_name).join(sanitize_filename(&chatfile, ""));

    match fs::remove_file(&chat_file_path) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => {
            error!("The chat file was not deleted: {}", e);
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

/// `POST /api/chats/export` — Export chat (JSONL raw or plaintext).
pub async fn export_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ExportChatRequest>,
) -> Response {
    let file = match &body.file {
        Some(f) if !f.is_empty() => f.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let is_group = body.is_group.unwrap_or(false);
    if !is_group {
        if body.avatar_url.as_ref().map(|u| u.is_empty()).unwrap_or(true) {
            return StatusCode::BAD_REQUEST.into_response();
        }
    }
    if let Some(ref avatar_url) = body.avatar_url {
        if !avatar_url.is_empty() && contains_forbidden_chars(avatar_url) {
            return StatusCode::BAD_REQUEST.into_response();
        }
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let folder = if is_group {
        dirs.group_chats.clone()
    } else {
        let avatar_url = body.avatar_url.as_deref().unwrap_or("");
        dirs.chats.join(avatar_url.trim_end_matches(".png"))
    };

    let filename = folder.join(file);
    let export_name = body.exportfilename.as_deref().unwrap_or("undefined");

    if !filename.exists() {
        return (StatusCode::NOT_FOUND, Json(json!({
            "message": format!("Could not find JSONL file to export. Source chat file: {}.", filename.display())
        }))).into_response();
    }

    let format = body.format.as_deref();

    if format == Some("jsonl") {
        match fs::read_to_string(&filename) {
            Ok(raw) => Json(json!({
                "message": format!("Chat saved to {}", export_name),
                "result": raw
            })).into_response(),
            Err(e) => {
                error!("Could not read JSONL file to export: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                    "message": format!("Could not read JSONL file to export. Source chat file: {}.", filename.display())
                }))).into_response()
            }
        }
    } else {
        // Plaintext export
        match fs::read_to_string(&filename) {
            Ok(raw) => {
                let mut buffer = String::new();
                for line in raw.lines() {
                    if let Ok(data) = serde_json::from_str::<Value>(line) {
                        if data.get("is_system").and_then(|v| v.as_bool()).unwrap_or(false) {
                            continue;
                        }
                        if let Some(mes) = data.get("mes").and_then(|v| v.as_str()) {
                            let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let display_text = data.pointer("/extra/display_text")
                                .and_then(|v| v.as_str())
                                .unwrap_or(mes);
                            let cleaned = display_text.replace("\r\n", "\n");
                            buffer.push_str(&format!("{}: {}\n\n", name, cleaned));
                        }
                    }
                }
                Json(json!({
                    "message": format!("Chat saved to {}", export_name),
                    "result": buffer
                })).into_response()
            }
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
        }
    }
}

/// `POST /api/chats/import` — Import a chat (multipart: file + fields).
pub async fn import_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    mut multipart: Multipart,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let mut file_data: Option<Vec<u8>> = None;
    let mut file_type = String::new();
    let mut avatar_url = String::new();
    let mut character_name = String::new();
    let mut user_name = "User".to_string();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "avatar_url" => { avatar_url = field.text().await.unwrap_or_default(); }
            "file_type" => { file_type = field.text().await.unwrap_or_default(); }
            "character_name" => { character_name = field.text().await.unwrap_or_default(); }
            "user_name" => { user_name = field.text().await.unwrap_or(user_name); }
            _ => {
                if file_data.is_none() {
                    file_data = field.bytes().await.ok().map(|b| b.to_vec());
                }
            }
        }
    }

    let file_bytes = match file_data {
        Some(b) => b,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    if contains_forbidden_chars(&avatar_url) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let avatar = avatar_url.trim_end_matches(".png");
    if avatar.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let char_name = if character_name.is_empty() { "Character" } else { &character_name };
    if file_type.is_empty() {
        return Json(json!({"error": true})).into_response();
    }
    let format = file_type.as_str();
    let data_str = String::from_utf8_lossy(&file_bytes).to_string();
    let mut file_names: Vec<String> = Vec::new();
    let chat_dir = dirs.chats.join(avatar);
    let _ = fs::create_dir_all(&chat_dir);

    if format == "json" {
        let json_data: Value = match serde_json::from_str(&data_str) {
            Ok(v) => v,
            Err(_) => return Json(json!({"error": true})).into_response(),
        };
        let chats: Vec<String> = if json_data.get("savedsettings").is_some() {
            import_kobold_lite_chat(&json_data).map(|c| vec![c]).unwrap_or_default()
        } else if json_data.get("histories").is_some() {
            import_cai_chat(&user_name, char_name, &json_data).unwrap_or_default()
        } else if json_data.get("data_visible").and_then(|v| v.as_array()).is_some() {
            import_ooba_chat(&user_name, char_name, &json_data).map(|c| vec![c]).unwrap_or_default()
        } else if json_data.get("messages").and_then(|v| v.as_array()).is_some() {
            import_agnai_chat(&user_name, char_name, &json_data).map(|c| vec![c]).unwrap_or_default()
        } else if json_data.get("type").and_then(|v| v.as_str()) == Some("risuChat") {
            import_risu_chat(&user_name, char_name, &json_data).map(|c| vec![c]).unwrap_or_default()
        } else {
            return Json(json!({"error": true})).into_response();
        };
        if chats.is_empty() { return Json(json!({"error": true})).into_response(); }
        for chat in chats {
            let fname = format!("{} - {} imported.jsonl", char_name, crate::storage::png::humanized_date_time());
            let fpath = chat_dir.join(&fname);
            file_names.push(fname);
            let _ = crate::storage::atomic::atomic_write_file(&fpath, chat.as_bytes());
        }
    } else if format == "jsonl" {
        let lines: Vec<&str> = data_str.lines().collect();
        if lines.is_empty() { return Json(json!({"error": true})).into_response(); }
        if let Ok(header) = serde_json::from_str::<Value>(lines[0]) {
            if header.get("user_name").is_none() && header.get("name").is_none() && header.get("chat_metadata").is_none() {
                return Json(json!({"error": true})).into_response();
            }
        } else { return Json(json!({"error": true})).into_response(); }
        let flattened = flatten_chub_chat(&lines);
        let fname = format!("{} - {} imported.jsonl", char_name, crate::storage::png::humanized_date_time());
        let fpath = chat_dir.join(&fname);
        file_names.push(fname);
        if flattened != data_str {
            let _ = crate::storage::atomic::atomic_write_file(&fpath, flattened.as_bytes());
        } else {
            let _ = fs::write(&fpath, &file_bytes);
        }
    } else {
        return Json(json!({"error": true})).into_response();
    }
    Json(json!({"res": true, "fileNames": file_names})).into_response()
}

/// `POST /api/chats/group/import` — Import group chat (multipart file).
pub async fn group_import_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    mut multipart: Multipart,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let mut file_data: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if file_data.is_none() { file_data = field.bytes().await.ok().map(|b| b.to_vec()); }
    }
    let file_bytes = match file_data {
        Some(b) => b,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    let chatname = crate::storage::png::humanized_date_time();
    let _ = fs::create_dir_all(&dirs.group_chats);
    let new_path = dirs.group_chats.join(format!("{}.jsonl", chatname));
    match fs::write(&new_path, &file_bytes) {
        Ok(()) => Json(json!({"res": chatname})).into_response(),
        Err(e) => { error!("Failed to write group chat import: {}", e); Json(json!({"error": true})).into_response() }
    }
}

/// `POST /api/chats/group/get` — Get group chat data.
pub async fn group_get_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GroupIdRequest>,
) -> Response {
    let id = match &body.id {
        Some(id) if !id.is_empty() => id.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let path = dirs.group_chats.join(sanitize_filename(&format!("{}.jsonl", id), ""));
    Json(json!(read_jsonl_file(&path))).into_response()
}

/// `POST /api/chats/group/info` — Get group chat metadata.
pub async fn group_info_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GroupIdRequest>,
) -> Response {
    let id = match &body.id {
        Some(id) if !id.is_empty() => id.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let path = dirs.group_chats.join(sanitize_filename(&format!("{}.jsonl", id), ""));
    match get_chat_info(&path, false) {
        Ok(info) => Json(serde_json::to_value(info).unwrap_or(json!({}))).into_response(),
        Err(e) => { error!("Error getting group chat info: {}", e); StatusCode::INTERNAL_SERVER_ERROR.into_response() }
    }
}

/// `POST /api/chats/group/delete` — Delete group chat file.
pub async fn group_delete_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GroupIdRequest>,
) -> Response {
    let id = match &body.id {
        Some(id) if !id.is_empty() => id.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let path = dirs.group_chats.join(sanitize_filename(&format!("{}.jsonl", id), ""));
    match fs::remove_file(&path) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => { error!("The group chat file was not deleted: {}", e); StatusCode::BAD_REQUEST.into_response() }
    }
}

/// `POST /api/chats/group/save` — Save group chat with integrity check.
pub async fn group_save_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GroupSaveRequest>,
) -> Response {
    let id = match &body.id {
        Some(id) if !id.is_empty() => id.as_str(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let chat_data = match &body.chat {
        Some(c) => c,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "The request's body.chat is not an array."}))).into_response(),
    };
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let path = dirs.group_chats.join(sanitize_filename(&format!("{}.jsonl", id), ""));
    let skip = body.force.unwrap_or(false);
    match try_save_chat(chat_data, &path, skip, &user.handle, id, &dirs.backups, &state.config) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(resp) => resp,
    }
}

/// `POST /api/chats/search` — Search chats for text matches.
pub async fn search_chat(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<SearchChatRequest>,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let query = body.query.as_deref().unwrap_or("");
    let fragments: Vec<String> = if query.is_empty() {
        Vec::new()
    } else {
        query.trim().to_lowercase().split_whitespace().map(String::from).collect()
    };

    let chat_files: Vec<PathBuf> = if let Some(group_id) = &body.group_id {
        // Find the target group and its chat IDs
        let groups_dir = &dirs.groups;
        let mut target_chats: Vec<String> = Vec::new();
        if let Ok(entries) = fs::read_dir(groups_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Ok(data) = fs::read_to_string(&p) {
                        if let Ok(gd) = serde_json::from_str::<Value>(&data) {
                            if gd.get("id").and_then(|v| v.as_str()) == Some(group_id) {
                                if let Some(chats) = gd.get("chats").and_then(|v| v.as_array()) {
                                    target_chats = chats.iter().filter_map(|v| v.as_str().map(String::from)).collect();
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }
        target_chats.iter()
            .map(|id| dirs.group_chats.join(format!("{}.jsonl", id)))
            .filter(|p| p.exists())
            .collect()
    } else {
        let avatar_url = match &body.avatar_url {
            Some(u) if !u.is_empty() => u.as_str(),
            _ => return Json(json!([])).into_response(),
        };
        let dir_name = avatar_url.trim_end_matches(".png");
        let directory_path = dirs.chats.join(dir_name);
        if !directory_path.exists() {
            return Json(json!([])).into_response();
        }
        fs::read_dir(&directory_path)
            .ok()
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default()
    };

    let has_text_match = |texts: &[String]| -> bool {
        if fragments.is_empty() { return true; }
        fragments.iter().all(|frag| {
            texts.iter().any(|t| t.to_lowercase().contains(frag.as_str()))
        })
    };

    let mut results: Vec<Value> = Vec::new();
    for chat_file in &chat_files {
        match get_chat_info(chat_file, false) {
            Ok(info) => {
                let file_id_match = has_text_match(&[info.file_id.clone()]);
                // Check message content match by reading the file
                let mut msg_match = false;
                if !query.is_empty() {
                    let all_messages = read_jsonl_file(chat_file);
                    let texts: Vec<String> = all_messages.iter()
                        .filter_map(|v| v.get("mes").and_then(|m| m.as_str()).map(String::from))
                        .collect();
                    msg_match = has_text_match(&texts);
                }
                let has_match = msg_match || file_id_match;

                if info.file_name.is_empty() { continue; }
                if !query.is_empty() && info.chat_items == 0 && !has_match { continue; }
                if query.is_empty() || has_match {
                    results.push(json!({
                        "file_name": info.file_id,
                        "file_size": info.file_size,
                        "message_count": info.chat_items,
                        "last_mes": info.last_mes,
                        "preview_message": get_preview_message(&info.mes),
                    }));
                }
            }
            Err(_) => continue,
        }
    }

    Json(json!(results)).into_response()
}

/// `POST /api/chats/recent` — List recent chats across characters and groups.
pub async fn recent_chats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<RecentChatRequest>,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let pinned = body.pinned.as_deref().unwrap_or(&[]);
    let with_metadata = body.metadata.unwrap_or(false);

    struct ChatFile {
        png_file: Option<String>,
        group_id: Option<String>,
        file_path: PathBuf,
        mtime: f64,
    }

    let mut all_chats: Vec<ChatFile> = Vec::new();

    // Character chats
    if let Ok(entries) = fs::read_dir(&dirs.characters) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("png") {
                let png_name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                let chat_dir_name = png_name.trim_end_matches(".png");
                let chats_path = dirs.chats.join(chat_dir_name);
                if chats_path.is_dir() {
                    if let Ok(chat_entries) = fs::read_dir(&chats_path) {
                        for ce in chat_entries.flatten() {
                            let cp = ce.path();
                            if cp.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                                if let Ok(meta) = fs::metadata(&cp) {
                                    all_chats.push(ChatFile {
                                        png_file: Some(png_name.clone()),
                                        group_id: None,
                                        file_path: cp,
                                        mtime: file_mtime_ms(&meta),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Group chats
    if let Ok(entries) = fs::read_dir(&dirs.groups) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(data) = fs::read_to_string(&p) {
                    if let Ok(gd) = serde_json::from_str::<Value>(&data) {
                        let gid = gd.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        if let Some(chats) = gd.get("chats").and_then(|v| v.as_array()) {
                            for chat in chats {
                                if let Some(chat_id) = chat.as_str() {
                                    let fp = dirs.group_chats.join(format!("{}.jsonl", chat_id));
                                    if fp.exists() {
                                        if let Ok(meta) = fs::metadata(&fp) {
                                            all_chats.push(ChatFile {
                                                png_file: None,
                                                group_id: Some(gid.clone()),
                                                file_path: fp,
                                                mtime: file_mtime_ms(&meta),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Root-level chats
    if let Ok(entries) = fs::read_dir(&dirs.chats) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Ok(meta) = fs::metadata(&p) {
                    all_chats.push(ChatFile {
                        png_file: None,
                        group_id: None,
                        file_path: p,
                        mtime: file_mtime_ms(&meta),
                    });
                }
            }
        }
    }

    // Sort: pinned first, then by mtime descending
    let is_pinned = |cf: &ChatFile| -> bool {
        let fname = cf.file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        pinned.iter().any(|p| {
            let name_match = p.file_name.as_deref() == Some(fname);
            let avatar_match = p.avatar.as_deref() == cf.png_file.as_deref();
            let group_match = p.group.as_deref() == cf.group_id.as_deref();
            name_match && (avatar_match || group_match)
        })
    };

    all_chats.sort_by(|a, b| {
        let ap = is_pinned(a);
        let bp = is_pinned(b);
        match (ap, bp) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => b.mtime.partial_cmp(&a.mtime).unwrap_or(std::cmp::Ordering::Equal),
        }
    });

    let max_val = match &body.max {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(u64::MAX),
        Some(Value::String(s)) => s.parse::<u64>().unwrap_or(u64::MAX),
        _ => u64::MAX,
    };
    let limit = (max_val as usize).saturating_add(pinned.len());
    let recent = &all_chats[..std::cmp::min(limit, all_chats.len())];

    let mut results: Vec<Value> = Vec::new();
    for cf in recent {
        let mut extra = serde_json::Map::new();
        if let Some(ref png) = cf.png_file {
            extra.insert("avatar".to_string(), json!(png));
        }
        if let Some(ref gid) = cf.group_id {
            extra.insert("group".to_string(), json!(gid));
        }

        match get_chat_info(&cf.file_path, with_metadata) {
            Ok(info) => {
                let mut v = serde_json::to_value(&info).unwrap_or(json!({}));
                if let Some(obj) = v.as_object_mut() {
                    for (k, val) in extra {
                        obj.insert(k, val);
                    }
                }
                if v.get("file_name").and_then(|f| f.as_str()).unwrap_or("").is_empty() {
                    continue;
                }
                results.push(v);
            }
            Err(_) => continue,
        }
    }

    Json(json!(results)).into_response()
}
