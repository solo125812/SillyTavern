//! Shared helper utilities for character data processing.
//!
//! Contains formatting, type coercion, deep merge, and file-stat helpers
//! used by both the read and write endpoint modules.

use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Map, Value};
use tracing::{error, warn};

use crate::storage::png;
use crate::api::avatars::{apply_avatar_crop_resize, CropParams};
use crate::storage::atomic::atomic_write_file;
use axum::extract::Multipart;
use axum::http::StatusCode;

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

/// Validate avatar URLs to match Node's `validateAvatarUrlMiddleware`.
pub(crate) fn contains_forbidden_chars(input: &str) -> bool {
    if input.contains('/') || input.contains('\0') {
        return true;
    }
    if cfg!(windows) && input.contains('\\') {
        return true;
    }
    false
}

/// Convert legacy (spec-less) character cards to Spec V2 format.
///
/// Mirrors `getCharaCardV2()` → `convertToV2()` → `charaFormatData()` in Node.
pub(super) fn convert_v1_to_v2(value: &mut Value, worlds_dir: &Path) {
    let original = value.clone();
    let mut char_obj = original.clone();

    if let Value::Object(map) = &mut char_obj {
        map.remove("json_data");
    }

    let name = original
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let description = js_or_value(original.get("description"), Value::String(String::new()));
    let personality = js_or_value(original.get("personality"), Value::String(String::new()));
    let scenario = js_or_value(original.get("scenario"), Value::String(String::new()));
    let first_mes = js_or_value(original.get("first_mes"), Value::String(String::new()));
    let mes_example = js_or_value(original.get("mes_example"), Value::String(String::new()));
    let creator_notes = js_or_value(original.get("creatorcomment"), Value::String(String::new()));
    let talkativeness = js_or_value(original.get("talkativeness"), Value::from(0.5));
    let fav_bool = is_string_true(original.get("fav"));
    let tags = parse_tags(original.get("tags"));
    let creator = js_or_value(original.get("creator"), Value::String(String::new()));

    let depth_value = parse_depth_prompt_depth(original.get("depth_prompt_depth"));
    let depth_prompt = coalesce_non_null(
        original.get("depth_prompt_prompt"),
        Value::String(String::new()),
    );
    let depth_role = coalesce_non_null(
        original.get("depth_prompt_role"),
        Value::String("system".to_string()),
    );

    // World name from the original card
    let world_name = original
        .get("world")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Spec V1 fields
    set_nested_value(&mut char_obj, &["name"], Value::String(name.clone()));
    set_nested_value(&mut char_obj, &["description"], description.clone());
    set_nested_value(&mut char_obj, &["personality"], personality.clone());
    set_nested_value(&mut char_obj, &["scenario"], scenario.clone());
    set_nested_value(&mut char_obj, &["first_mes"], first_mes.clone());
    set_nested_value(&mut char_obj, &["mes_example"], mes_example.clone());

    // Old ST extension fields
    set_nested_value(&mut char_obj, &["creatorcomment"], creator_notes.clone());
    set_nested_value(&mut char_obj, &["avatar"], Value::String("none".to_string()));
    set_nested_value(
        &mut char_obj,
        &["chat"],
        Value::String(format!("{} - {}", name, png::humanized_date_time())),
    );
    set_nested_value(&mut char_obj, &["talkativeness"], talkativeness.clone());
    set_nested_value(&mut char_obj, &["fav"], Value::Bool(fav_bool));
    set_nested_value(&mut char_obj, &["tags"], tags.clone());

    // Spec V2 fields
    set_nested_value(&mut char_obj, &["spec"], Value::String("chara_card_v2".to_string()));
    set_nested_value(&mut char_obj, &["spec_version"], Value::String("2.0".to_string()));
    set_nested_value(&mut char_obj, &["data", "name"], Value::String(name));
    set_nested_value(&mut char_obj, &["data", "description"], description);
    set_nested_value(&mut char_obj, &["data", "personality"], personality);
    set_nested_value(&mut char_obj, &["data", "scenario"], scenario);
    set_nested_value(&mut char_obj, &["data", "first_mes"], first_mes);
    set_nested_value(&mut char_obj, &["data", "mes_example"], mes_example);

    // New V2 fields
    set_nested_value(&mut char_obj, &["data", "creator_notes"], creator_notes);
    set_nested_value(&mut char_obj, &["data", "system_prompt"], Value::String(String::new()));
    set_nested_value(
        &mut char_obj,
        &["data", "post_history_instructions"],
        Value::String(String::new()),
    );
    set_nested_value(&mut char_obj, &["data", "tags"], tags);
    set_nested_value(&mut char_obj, &["data", "creator"], creator);
    set_nested_value(
        &mut char_obj,
        &["data", "character_version"],
        Value::String(String::new()),
    );
    set_nested_value(
        &mut char_obj,
        &["data", "alternate_greetings"],
        Value::Array(Vec::new()),
    );

    // ST extension fields to V2 object
    set_nested_value(
        &mut char_obj,
        &["data", "extensions", "talkativeness"],
        talkativeness,
    );
    set_nested_value(
        &mut char_obj,
        &["data", "extensions", "fav"],
        Value::Bool(fav_bool),
    );
    set_nested_value(
        &mut char_obj,
        &["data", "extensions", "world"],
        Value::String(world_name.clone()),
    );

    // Spec extension: depth prompt
    set_nested_value(
        &mut char_obj,
        &["data", "extensions", "depth_prompt", "prompt"],
        depth_prompt,
    );
    set_nested_value(
        &mut char_obj,
        &["data", "extensions", "depth_prompt", "depth"],
        Value::from(depth_value),
    );
    set_nested_value(
        &mut char_obj,
        &["data", "extensions", "depth_prompt", "role"],
        depth_role,
    );

    // World info → character_book integration (mirrors Node characters.js:629-646)
    if !world_name.is_empty() {
        match read_world_info_file(worlds_dir, &world_name) {
            Ok(Some(file_data)) => {
                if let Some(original_data) = file_data.get("originalData") {
                    // File was imported — save it directly as character book
                    set_nested_value(
                        &mut char_obj,
                        &["data", "character_book"],
                        original_data.clone(),
                    );
                } else if let Some(entries) = file_data.get("entries") {
                    // File was not imported — convert entries to character book
                    let book = convert_world_info_to_character_book(&world_name, entries);
                    set_nested_value(&mut char_obj, &["data", "character_book"], book);
                }
            }
            Ok(None) => {} // World info file not found, skip silently
            Err(e) => {
                warn!(
                    "Failed to read world info file: {}. Character book will not be available.",
                    e
                );
            }
        }
    }

    // Deep merge extensions from original data (mirrors Node characters.js:648-656)
    if let Some(ext_val) = original.get("extensions") {
        if let Some(ext_str) = ext_val.as_str() {
            match serde_json::from_str::<Value>(ext_str) {
                Ok(parsed_extensions) => {
                    let current_extensions = char_obj
                        .get("data")
                        .and_then(|d| d.get("extensions"))
                        .cloned()
                        .unwrap_or(Value::Object(Map::new()));
                    let merged = deep_merge(&current_extensions, &parsed_extensions);
                    set_nested_value(&mut char_obj, &["data", "extensions"], merged);
                }
                Err(_) => {
                    warn!("Failed to parse extensions JSON");
                }
            }
        } else if ext_val.is_object() {
            // If extensions is already an object, deep merge directly
            let current_extensions = char_obj
                .get("data")
                .and_then(|d| d.get("extensions"))
                .cloned()
                .unwrap_or(Value::Object(Map::new()));
            let merged = deep_merge(&current_extensions, ext_val);
            set_nested_value(&mut char_obj, &["data", "extensions"], merged);
        }
    }

    // Preserve existing chat/create_date when present (nullish coalescing in Node)
    if let Some(chat_value) = original.get("chat") {
        if !chat_value.is_null() {
            set_nested_value(&mut char_obj, &["chat"], chat_value.clone());
        }
    }
    if let Some(create_date_value) = original.get("create_date") {
        if !create_date_value.is_null() {
            set_nested_value(&mut char_obj, &["create_date"], create_date_value.clone());
        }
    } else {
        // Node's getCharaCardV2 hoists create_date when missing.
        set_nested_value(&mut char_obj, &["create_date"], Value::String(chrono::Utc::now().to_rfc3339()));
    }

    *value = char_obj;
}

/// Read a world info JSON file from the worlds directory.
///
/// Mirrors Node's `readWorldInfoFile()` (worldinfo.js:17-35).
pub(super) fn read_world_info_file(worlds_dir: &Path, world_name: &str) -> Result<Option<Value>, String> {
    let sanitized = sanitize_filename::sanitize(world_name);
    let file_path = worlds_dir.join(format!("{}.json", sanitized));

    if !file_path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read world info file: {}", e))?;
    let data: Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse world info JSON: {}", e))?;

    Ok(Some(data))
}

/// Convert world info entries to a character book structure.
///
/// Mirrors Node's `convertWorldInfoToCharacterBook()` (characters.js:665-724).
pub(super) fn convert_world_info_to_character_book(name: &str, entries: &Value) -> Value {
    let mut result_entries = Vec::new();

    if let Value::Object(entries_map) = entries {
        for (_index, entry) in entries_map {
            let mut original_entry = Map::new();

            // Core fields
            original_entry.insert("id".to_string(), entry.get("uid").cloned().unwrap_or(Value::Null));
            original_entry.insert("keys".to_string(), entry.get("key").cloned().unwrap_or(Value::Array(vec![])));
            original_entry.insert("secondary_keys".to_string(), entry.get("keysecondary").cloned().unwrap_or(Value::Array(vec![])));
            original_entry.insert("comment".to_string(), entry.get("comment").cloned().unwrap_or(Value::String(String::new())));
            original_entry.insert("content".to_string(), entry.get("content").cloned().unwrap_or(Value::String(String::new())));
            original_entry.insert("constant".to_string(), entry.get("constant").cloned().unwrap_or(Value::Bool(false)));
            original_entry.insert("selective".to_string(), entry.get("selective").cloned().unwrap_or(Value::Bool(false)));
            original_entry.insert("insertion_order".to_string(), entry.get("order").cloned().unwrap_or(Value::from(100)));

            // enabled = !disable
            let enabled = match entry.get("disable") {
                Some(Value::Bool(b)) => Value::Bool(!b),
                _ => Value::Bool(true),
            };
            original_entry.insert("enabled".to_string(), enabled);

            // position mapping
            let position_str = match entry.get("position").and_then(|v| v.as_i64()) {
                Some(0) => "before_char",
                _ => "after_char",
            };
            original_entry.insert("position".to_string(), Value::String(position_str.to_string()));
            original_entry.insert("use_regex".to_string(), Value::Bool(true));

            // Extensions sub-object with all ST-specific fields
            let mut ext = Map::new();
            // Spread entry.extensions first
            if let Some(Value::Object(entry_ext)) = entry.get("extensions") {
                for (k, v) in entry_ext {
                    ext.insert(k.clone(), v.clone());
                }
            }
            ext.insert("position".to_string(), entry.get("position").cloned().unwrap_or(Value::from(0)));
            ext.insert("exclude_recursion".to_string(), entry.get("excludeRecursion").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("display_index".to_string(), entry.get("displayIndex").cloned().unwrap_or(Value::from(0)));
            ext.insert("probability".to_string(), entry.get("probability").cloned().unwrap_or(Value::Null));
            ext.insert("useProbability".to_string(), entry.get("useProbability").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("depth".to_string(), entry.get("depth").cloned().unwrap_or(Value::from(4)));
            ext.insert("selectiveLogic".to_string(), entry.get("selectiveLogic").cloned().unwrap_or(Value::from(0)));
            ext.insert("outlet_name".to_string(), entry.get("outletName").cloned().unwrap_or(Value::String(String::new())));
            ext.insert("group".to_string(), entry.get("group").cloned().unwrap_or(Value::String(String::new())));
            ext.insert("group_override".to_string(), entry.get("groupOverride").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("group_weight".to_string(), entry.get("groupWeight").cloned().unwrap_or(Value::Null));
            ext.insert("prevent_recursion".to_string(), entry.get("preventRecursion").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("delay_until_recursion".to_string(), entry.get("delayUntilRecursion").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("scan_depth".to_string(), entry.get("scanDepth").cloned().unwrap_or(Value::Null));
            ext.insert("match_whole_words".to_string(), entry.get("matchWholeWords").cloned().unwrap_or(Value::Null));
            ext.insert("use_group_scoring".to_string(), entry.get("useGroupScoring").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("case_sensitive".to_string(), entry.get("caseSensitive").cloned().unwrap_or(Value::Null));
            ext.insert("automation_id".to_string(), entry.get("automationId").cloned().unwrap_or(Value::String(String::new())));
            ext.insert("role".to_string(), entry.get("role").cloned().unwrap_or(Value::from(0)));
            ext.insert("vectorized".to_string(), entry.get("vectorized").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("sticky".to_string(), entry.get("sticky").cloned().unwrap_or(Value::Null));
            ext.insert("cooldown".to_string(), entry.get("cooldown").cloned().unwrap_or(Value::Null));
            ext.insert("delay".to_string(), entry.get("delay").cloned().unwrap_or(Value::Null));
            ext.insert("match_persona_description".to_string(), entry.get("matchPersonaDescription").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("match_character_description".to_string(), entry.get("matchCharacterDescription").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("match_character_personality".to_string(), entry.get("matchCharacterPersonality").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("match_character_depth_prompt".to_string(), entry.get("matchCharacterDepthPrompt").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("match_scenario".to_string(), entry.get("matchScenario").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("match_creator_notes".to_string(), entry.get("matchCreatorNotes").cloned().unwrap_or(Value::Bool(false)));
            ext.insert("triggers".to_string(), entry.get("triggers").cloned().unwrap_or(Value::Array(vec![])));
            ext.insert("ignore_budget".to_string(), entry.get("ignoreBudget").cloned().unwrap_or(Value::Bool(false)));

            original_entry.insert("extensions".to_string(), Value::Object(ext));
            result_entries.push(Value::Object(original_entry));
        }
    }

    serde_json::json!({
        "entries": result_entries,
        "name": name
    })
}

/// Recursively deep-merge two JSON values (source into target).
///
/// Mirrors Node's `deepMerge()` (util.js:494-510):
/// - Objects: recursively merge keys
/// - Non-objects: source overwrites target
pub(super) fn deep_merge(target: &Value, source: &Value) -> Value {
    match (target, source) {
        (Value::Object(target_map), Value::Object(source_map)) => {
            let mut output = target_map.clone();
            for (key, source_val) in source_map {
                if source_val.is_object() {
                    if let Some(target_val) = target_map.get(key) {
                        output.insert(key.clone(), deep_merge(target_val, source_val));
                    } else {
                        output.insert(key.clone(), source_val.clone());
                    }
                } else {
                    output.insert(key.clone(), source_val.clone());
                }
            }
            Value::Object(output)
        }
        _ => source.clone(),
    }
}

/// Convert a full character object to a shallow representation.
pub(super) fn to_shallow(character: &Value) -> Option<Value> {
    let name = character.get("name")?.as_str()?.to_string();
    let mut out = Map::new();

    out.insert("shallow".to_string(), Value::Bool(true));
    out.insert("name".to_string(), Value::String(name));

    for key in [
        "avatar",
        "chat",
        "fav",
        "date_added",
        "create_date",
        "date_last_chat",
        "chat_size",
        "data_size",
        "tags",
    ] {
        if let Some(value) = character.get(key) {
            out.insert(key.to_string(), value.clone());
        }
    }

    let mut data = Map::new();
    let data_src = character.get("data").unwrap_or(&Value::Null);
    data.insert(
        "name".to_string(),
        data_src
            .get("name")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new())),
    );
    data.insert(
        "character_version".to_string(),
        data_src
            .get("character_version")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new())),
    );
    data.insert(
        "creator".to_string(),
        data_src
            .get("creator")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new())),
    );
    data.insert(
        "creator_notes".to_string(),
        data_src
            .get("creator_notes")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new())),
    );
    data.insert(
        "tags".to_string(),
        data_src
            .get("tags")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    );

    let mut extensions = Map::new();
    let fav = data_src
        .get("extensions")
        .and_then(|v| v.get("fav"))
        .cloned()
        .unwrap_or_else(|| Value::Bool(false));
    extensions.insert("fav".to_string(), fav);
    data.insert("extensions".to_string(), Value::Object(extensions));

    out.insert("data".to_string(), Value::Object(data));

    Some(Value::Object(out))
}

pub(super) fn parse_tags(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(s)) => {
            let tags: Vec<Value> = s
                .split(',')
                .map(|t| t.trim())
                .filter(|t| !t.is_empty())
                .map(|t| Value::String(t.to_string()))
                .collect();
            Value::Array(tags)
        }
        Some(v) if js_truthy(v) => v.clone(),
        _ => Value::Array(Vec::new()),
    }
}

pub(super) fn is_string_true(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::String(s)) if s == "true")
}

pub(super) fn parse_depth_prompt_depth(value: Option<&Value>) -> f64 {
    let parsed = match value {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        Some(Value::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    };

    match parsed {
        Some(v) if v.is_finite() => v,
        _ => 4.0,
    }
}

pub(super) fn js_or_value(value: Option<&Value>, default: Value) -> Value {
    match value {
        Some(v) if js_truthy(v) => v.clone(),
        _ => default,
    }
}

pub(super) fn coalesce_non_null(value: Option<&Value>, default: Value) -> Value {
    match value {
        Some(v) if !v.is_null() => v.clone(),
        _ => default,
    }
}

pub(super) fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|v| v != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) => true,
        Value::Object(_) => true,
    }
}

pub(super) fn set_nested_value(target: &mut Value, path: &[&str], value: Value) {
    if path.is_empty() {
        return;
    }
    let mut current = target;
    for (idx, key) in path.iter().enumerate() {
        let is_last = idx == path.len() - 1;
        if is_last {
            let map = ensure_object(current);
            map.insert(key.to_string(), value);
            return;
        }
        let map = ensure_object(current);
        current = map
            .entry(key.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}

pub(super) fn ensure_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value.as_object_mut().unwrap()
}

pub(super) fn js_string_length(value: &Value) -> usize {
    js_string(value).len()
}

pub(super) fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(arr) => js_array_to_string(arr),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

pub(super) fn js_array_to_string(values: &[Value]) -> String {
    let parts: Vec<String> = values
        .iter()
        .map(|v| match v {
            Value::Null => String::new(),
            _ => js_string(v),
        })
        .collect();
    parts.join(",")
}

/// Calculate total chat size and last chat timestamp for a character's chats directory.
///
/// Mirrors Node's `calculateChatSize()`.
pub(super) fn calculate_chat_size(chat_dir: &Path) -> (u64, f64) {
    let mut chat_size: u64 = 0;
    let mut date_last_chat: f64 = 0.0;

    if chat_dir.exists() {
        if let Ok(entries) = fs::read_dir(chat_dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        chat_size += meta.len();
                        let mtime = file_mtime_ms(&meta);
                        if mtime > date_last_chat {
                            date_last_chat = mtime;
                        }
                    }
                }
            }
        }
    }

    (chat_size, date_last_chat)
}

/// Calculate the total string length of a data object's values.
///
/// Mirrors Node's `calculateDataSize()`.
pub(super) fn calculate_data_size(json: &Value) -> usize {
    match json.get("data") {
        Some(data) if data.is_object() => data
            .as_object()
            .unwrap()
            .values()
            .map(js_string_length)
            .sum(),
        _ => 0,
    }
}

/// Format bytes into a human-readable string (matching Node's `bytes` package).
///
/// The npm `bytes` package uses 1024-based units with up to 2 decimal places,
/// trimming trailing zeros. No space between number and unit.
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0B".to_string();
    }

    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    // Format with 2 decimal places, then trim trailing zeros and dot
    let formatted = format!("{:.2}", size);
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    format!("{}{}", trimmed, UNITS[unit_idx])
}

/// Get file modification time in milliseconds (matching Node's `stat.mtimeMs`).
pub(crate) fn file_mtime_ms(metadata: &fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Get file creation time in milliseconds (matching Node's `stat.ctimeMs`).
#[cfg(unix)]
pub(super) fn file_ctime_ms(metadata: &fs::Metadata) -> f64 {
    use std::os::unix::fs::MetadataExt;
    // On Unix, ctime is "change time" but Node uses it for date_added.
    // ctimeMs = ctime * 1000 + ctime_nsec / 1_000_000
    let secs = metadata.ctime() as f64;
    let nsecs = metadata.ctime_nsec() as f64;
    secs * 1000.0 + nsecs / 1_000_000.0
}

#[cfg(not(unix))]
pub(super) fn file_ctime_ms(metadata: &fs::Metadata) -> f64 {
    // Prefer creation time on non-Unix platforms; fall back to modified time.
    metadata
        .created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or_else(|| file_mtime_ms(metadata))
}

/// Convert a millisecond timestamp to ISO 8601 string.
pub(crate) fn timestamp_to_iso(ms: f64) -> String {
    let secs = (ms / 1000.0) as i64;
    let nsecs = ((ms % 1000.0) * 1_000_000.0) as u32;
    match chrono::DateTime::from_timestamp(secs, nsecs) {
        Some(dt) => dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        None => chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    }
}
// ---------------------------------------------------------------------------
// Phase 5 — Character Writes
// ---------------------------------------------------------------------------

/// Default avatar image path relative to server root.
/// Mirrors Node's `DEFAULT_AVATAR_PATH = './public/img/ai4.png'`.
pub(super) const DEFAULT_AVATAR_REL: &str = "public/img/ai4.png";

/// Avatar width in pixels (mirrors Node `AVATAR_WIDTH`).
#[allow(dead_code)]
pub(super) const AVATAR_WIDTH: u32 = 512;
/// Avatar height in pixels (mirrors Node `AVATAR_HEIGHT`).
#[allow(dead_code)]
pub(super) const AVATAR_HEIGHT: u32 = 768;

// ---------------------------------------------------------------------------
// Phase 5 request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct EditAttributeRequest {
    pub avatar_url: String,
    #[serde(default)]
    pub ch_name: Option<String>,
    pub field: String,
    pub value: Value,
}

#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub avatar_url: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct DuplicateRequest {
    pub avatar_url: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    pub avatar_url: String,
    #[serde(default)]
    pub delete_chats: bool,
}

#[derive(Debug, Deserialize)]
pub struct ExportRequest {
    pub format: String,
    pub avatar_url: String,
}

// ---------------------------------------------------------------------------
// Phase 5 helpers
// ---------------------------------------------------------------------------

/// Generate a unique PNG filename for a character.
///
/// Mirrors Node's `getPngName()` in `characters.js:1393-1405`:
/// - Start with the provided name (caller is responsible for sanitization).
/// - If `{name}.png` already exists, try `{name}1.png`, `{name}2.png`, etc.
pub(super) fn get_png_name(name: &str, chars_dir: &Path) -> String {
    let base_name = name.to_string();
    let mut file = base_name.clone();
    let mut i = 1;
    while chars_dir.join(format!("{}.png", file)).exists() {
        file = format!("{}{}", base_name, i);
        i += 1;
    }
    file
}

/// Remove private fields from a character object.
///
/// Mirrors Node's `unsetPrivateFields()` in `characters.js:499-503`:
/// - Set `fav = false`
/// - Set `data.extensions.fav = false`
/// - Remove `chat`
pub(super) fn unset_private_fields(char: &mut Value) {
    if let Some(obj) = char.as_object_mut() {
        obj.insert("fav".to_string(), Value::Bool(false));
        obj.remove("chat");
        set_nested_value(char, &["data", "extensions", "fav"], Value::Bool(false));
    }
}

/// Format character data to Spec V2 from a flat request body.
///
/// Mirrors Node's `charaFormatData()` in `characters.js:566-658`.
/// Takes the flat form fields from create/edit and produces a full V2 character object.
pub(super) fn chara_format_data(data: &Value) -> Value {
    // Start with existing json_data if present, else empty object
    let mut char = data
        .get("json_data")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or_else(|| Value::Object(Map::new()));

    // Remove json_data to prevent recursive saving
    if let Some(obj) = char.as_object_mut() {
        obj.remove("json_data");
    }

    let ch_name = data
        .get("ch_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let description = str_or_empty(data, "description");
    let personality = str_or_empty(data, "personality");
    let scenario = str_or_empty(data, "scenario");
    let first_mes = str_or_empty(data, "first_mes");
    let mes_example = str_or_empty(data, "mes_example");
    let creator_notes = str_or_empty(data, "creator_notes");

    let fav = data
        .get("fav")
        .and_then(|v| v.as_str())
        .map(|s| s == "true")
        .unwrap_or(false);

    let talkativeness = data
        .get("talkativeness")
        .and_then(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        })
        .unwrap_or(0.5);

    let tags = parse_tags(data.get("tags"));

    let alternate_greetings = match data.get("alternate_greetings") {
        Some(Value::Array(arr)) => Value::Array(arr.clone()),
        Some(Value::String(s)) => Value::Array(vec![Value::String(s.clone())]),
        _ => Value::Array(Vec::new()),
    };

    let chat_name = format!("{} - {}", ch_name, png::humanized_date_time());

    // Spec V1 fields
    set_val(&mut char, "name", Value::String(ch_name.to_string()));
    set_val(&mut char, "description", Value::String(description.to_string()));
    set_val(&mut char, "personality", Value::String(personality.to_string()));
    set_val(&mut char, "scenario", Value::String(scenario.to_string()));
    set_val(&mut char, "first_mes", Value::String(first_mes.to_string()));
    set_val(&mut char, "mes_example", Value::String(mes_example.to_string()));
    set_val(&mut char, "creatorcomment", Value::String(creator_notes.to_string()));
    set_val(&mut char, "avatar", Value::String("none".to_string()));
    set_val(&mut char, "chat", Value::String(chat_name));
    set_val(&mut char, "talkativeness", Value::from(talkativeness));
    set_val(&mut char, "fav", Value::Bool(fav));
    set_val(&mut char, "tags", tags.clone());

    // Spec V2 envelope
    set_val(&mut char, "spec", Value::String("chara_card_v2".to_string()));
    set_val(&mut char, "spec_version", Value::String("2.0".to_string()));

    set_nested_value(&mut char, &["data", "name"], Value::String(ch_name.to_string()));
    set_nested_value(&mut char, &["data", "description"], Value::String(description.to_string()));
    set_nested_value(&mut char, &["data", "personality"], Value::String(personality.to_string()));
    set_nested_value(&mut char, &["data", "scenario"], Value::String(scenario.to_string()));
    set_nested_value(&mut char, &["data", "first_mes"], Value::String(first_mes.to_string()));
    set_nested_value(&mut char, &["data", "mes_example"], Value::String(mes_example.to_string()));
    set_nested_value(&mut char, &["data", "creator_notes"], Value::String(creator_notes.to_string()));
    set_nested_value(&mut char, &["data", "system_prompt"], Value::String(str_or_empty(data, "system_prompt").to_string()));
    set_nested_value(&mut char, &["data", "post_history_instructions"], Value::String(str_or_empty(data, "post_history_instructions").to_string()));
    set_nested_value(&mut char, &["data", "tags"], tags);
    set_nested_value(&mut char, &["data", "creator"], Value::String(str_or_empty(data, "creator").to_string()));
    set_nested_value(&mut char, &["data", "character_version"], Value::String(str_or_empty(data, "character_version").to_string()));
    set_nested_value(&mut char, &["data", "alternate_greetings"], alternate_greetings);

    // Extensions
    set_nested_value(&mut char, &["data", "extensions", "talkativeness"], Value::from(talkativeness));
    set_nested_value(&mut char, &["data", "extensions", "fav"], Value::Bool(fav));
    set_nested_value(&mut char, &["data", "extensions", "world"], Value::String(str_or_empty(data, "world").to_string()));

    // Depth prompt
    let depth_value = parse_depth_prompt_depth(data.get("depth_prompt_depth"));
    let role_value = data
        .get("depth_prompt_role")
        .and_then(|v| v.as_str())
        .unwrap_or("system")
        .to_string();
    set_nested_value(&mut char, &["data", "extensions", "depth_prompt", "prompt"], Value::String(str_or_empty(data, "depth_prompt_prompt").to_string()));
    set_nested_value(&mut char, &["data", "extensions", "depth_prompt", "depth"], Value::from(depth_value));
    set_nested_value(&mut char, &["data", "extensions", "depth_prompt", "role"], Value::String(role_value));

    // Merge client-provided extensions
    if let Some(ext_str) = data.get("extensions").and_then(|v| v.as_str()) {
        if let Ok(ext_val) = serde_json::from_str::<Value>(ext_str) {
            let existing = char.get("data")
                .and_then(|d| d.get("extensions"))
                .cloned()
                .unwrap_or(Value::Object(Map::new()));
            let merged = deep_merge(&existing, &ext_val);
            set_nested_value(&mut char, &["data", "extensions"], merged);
        }
    }

    char
}

/// Convert a V1 character card to V2 format.
///
/// Mirrors Node's `convertToV2()` in `characters.js:464-494`.
pub(super) fn convert_to_v2(v1: &Value) -> Value {
    let json_data = serde_json::to_string(v1).unwrap_or_default();
    let form = serde_json::json!({
        "json_data": json_data,
        "ch_name": str_or_empty(v1, "name"),
        "description": str_or_empty(v1, "description"),
        "personality": str_or_empty(v1, "personality"),
        "scenario": str_or_empty(v1, "scenario"),
        "first_mes": str_or_empty(v1, "first_mes"),
        "mes_example": str_or_empty(v1, "mes_example"),
        "creator_notes": str_or_empty(v1, "creatorcomment"),
        "talkativeness": v1.get("talkativeness").cloned().unwrap_or(Value::from(0.5)),
        "fav": v1.get("fav").cloned().unwrap_or(Value::Bool(false)),
        "creator": v1.get("creator").cloned().unwrap_or(Value::String(String::new())),
        "tags": v1.get("tags").cloned().unwrap_or(Value::String(String::new())),
        "depth_prompt_prompt": v1.get("depth_prompt_prompt").cloned().unwrap_or(Value::String(String::new())),
        "depth_prompt_depth": v1.get("depth_prompt_depth").cloned().unwrap_or(Value::String(String::new())),
        "depth_prompt_role": v1.get("depth_prompt_role").cloned().unwrap_or(Value::String(String::new())),
    });

    let mut char = chara_format_data(&form);
    if let Some(chat_val) = v1.get("chat") {
        set_val(&mut char, "chat", chat_val.clone());
    } else {
        let name = str_or_empty(v1, "name");
        set_val(
            &mut char,
            "chat",
            Value::String(format!("{} - {}", name, png::humanized_date_time())),
        );
    }
    if let Some(create_date_val) = v1.get("create_date") {
        set_val(&mut char, "create_date", create_date_val.clone());
    }

    char
}

/// Read an image source: either from disk or the default avatar.
pub(super) fn read_image_source(image_source: Option<&[u8]>, server_dir: &Path) -> Result<Vec<u8>, StatusCode> {
    if let Some(data) = image_source {
        if !data.is_empty() {
            return Ok(data.to_vec());
        }
    }
    // Fall back to default avatar
    let default_path = server_dir.join(DEFAULT_AVATAR_REL);
    fs::read(&default_path).map_err(|e| {
        error!("Failed to read default avatar at {:?}: {}", default_path, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// Write character data to a PNG file atomically.
///
/// Mirrors Node's `writeCharacterData()` in `characters.js:220-269`:
/// 1. Read image (from buffer or default avatar).
/// 2. Apply crop/resize if specified.
/// 3. Embed character JSON in PNG tEXt chunks.
/// 4. Atomically write to disk.
pub(super) fn write_and_save_character(
    image_data: &[u8],
    char_json: &str,
    target_name: &str,
    chars_dir: &Path,
    crop: Option<&CropParams>,
) -> Result<(), StatusCode> {
    // Apply crop/resize
    let processed = apply_avatar_crop_resize(image_data, crop);

    // Embed character data in PNG
    let png_with_data = png::write_character_data(&processed, char_json)
        .map_err(|e| {
            error!("Failed to write character data to PNG: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Ensure characters directory exists
    fs::create_dir_all(chars_dir).map_err(|e| {
        error!("Failed to create characters directory: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Write atomically
    let output_path = chars_dir.join(format!("{}.png", target_name));
    atomic_write_file(&output_path, &png_with_data).map_err(|e| {
        error!("Failed to write character file: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(())
}

pub(super) fn str_or_empty<'a>(obj: &'a Value, key: &str) -> &'a str {
    obj.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

pub(super) fn set_val(obj: &mut Value, key: &str, val: Value) {
    if let Some(map) = obj.as_object_mut() {
        map.insert(key.to_string(), val);
    }
}

/// Parse multipart form into (json_body, optional_image_bytes, optional_crop).
pub(super) async fn parse_character_multipart(
    mut multipart: Multipart,
) -> Result<(Value, Option<Vec<u8>>, Option<CropParams>), StatusCode> {
    let mut json_body = Value::Object(Map::new());
    let mut image_data: Option<Vec<u8>> = None;
    let mut crop: Option<CropParams> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "avatar" => {
                match field.bytes().await {
                    Ok(bytes) if !bytes.is_empty() => image_data = Some(bytes.to_vec()),
                    _ => {}
                }
            }
            "crop" => {
                if let Ok(text) = field.text().await {
                    crop = serde_json::from_str(&text).ok();
                }
            }
            _ => {
                // All other fields are treated as JSON body fields
                if let Ok(text) = field.text().await {
                    if let Some(obj) = json_body.as_object_mut() {
                        // Try to parse as JSON value, fall back to string
                        let val = serde_json::from_str::<Value>(&text)
                            .unwrap_or(Value::String(text));
                        obj.insert(field_name, val);
                    }
                }
            }
        }
    }

    Ok((json_body, image_data, crop))
}

// ---------------------------------------------------------------------------
// TavernCardValidator (merge-attributes)
// ---------------------------------------------------------------------------

pub(super) struct TavernCardValidator<'a> {
    card: &'a Value,
    last_error: Option<String>,
}

impl<'a> TavernCardValidator<'a> {
    pub(super) fn new(card: &'a Value) -> Self {
        Self { card, last_error: None }
    }

    pub(super) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub(super) fn validate(&mut self) -> Option<u8> {
        self.last_error = None;

        if self.validate_v1() {
            return Some(1);
        }
        if self.validate_v2() {
            return Some(2);
        }
        if self.validate_v3() {
            return Some(3);
        }
        None
    }

    fn validate_v1(&mut self) -> bool {
        let required = ["name", "description", "personality", "scenario", "first_mes", "mes_example"];
        for field in required {
            if self.card.get(field).is_none() {
                self.last_error = Some(field.to_string());
                return false;
            }
        }
        true
    }

    fn validate_v2(&mut self) -> bool {
        if self.card.get("spec").and_then(|v| v.as_str()) != Some("chara_card_v2") {
            self.last_error = Some("spec".to_string());
            return false;
        }
        if self.card.get("spec_version").and_then(|v| v.as_str()) != Some("2.0") {
            self.last_error = Some("spec_version".to_string());
            return false;
        }

        let data = match self.card.get("data") {
            Some(d) => d,
            None => {
                self.last_error = Some("No tavern card data found".to_string());
                return false;
            }
        };

        let required = [
            "name",
            "description",
            "personality",
            "scenario",
            "first_mes",
            "mes_example",
            "creator_notes",
            "system_prompt",
            "post_history_instructions",
            "alternate_greetings",
            "tags",
            "creator",
            "character_version",
            "extensions",
        ];

        for field in required {
            if data.get(field).is_none() {
                self.last_error = Some(format!("data.{}", field));
                return false;
            }
        }

        let alternates_ok = data.get("alternate_greetings").and_then(|v| v.as_array()).is_some();
        let tags_ok = data.get("tags").and_then(|v| v.as_array()).is_some();
        let extensions_ok = data.get("extensions").and_then(|v| v.as_object()).is_some();

        alternates_ok && tags_ok && extensions_ok && self.validate_character_book_v2(data)
    }

    fn validate_character_book_v2(&mut self, data: &Value) -> bool {
        let book = match data.get("character_book") {
            Some(v) => v,
            None => return true,
        };

        let required = ["extensions", "entries"];
        for field in required {
            if book.get(field).is_none() {
                self.last_error = Some(format!("data.character_book.{}", field));
                return false;
            }
        }

        let entries_ok = book.get("entries").and_then(|v| v.as_array()).is_some();
        let ext_ok = book.get("extensions").and_then(|v| v.as_object()).is_some();
        entries_ok && ext_ok
    }

    fn validate_v3(&mut self) -> bool {
        if self.card.get("spec").and_then(|v| v.as_str()) != Some("chara_card_v3") {
            self.last_error = Some("spec".to_string());
            return false;
        }
        let version = self
            .card
            .get("spec_version")
            .and_then(|v| v.as_f64())
            .or_else(|| {
                self.card
                    .get("spec_version")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
            })
            .unwrap_or(f64::NAN);

        if version < 3.0 || version >= 4.0 {
            self.last_error = Some("spec_version".to_string());
            return false;
        }

        let data = self.card.get("data");
        if data.is_none() || !data.unwrap().is_object() {
            self.last_error = Some("No tavern card data found".to_string());
            return false;
        }

        true
    }
}
