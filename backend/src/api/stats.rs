//! Stats endpoints — Phase 9.
//!
//! Mirrors Node's [`src/endpoints/stats.js`](../../../src/endpoints/stats.js).
//!
//! ## Endpoints
//! - `POST /api/stats/get`      — Get the current stats object.
//! - `POST /api/stats/recreate` — Recreate stats from chat files.
//! - `POST /api/stats/update`   — Update the stats object from the client.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const STATS_FILE: &str = "stats.json";

const MONTH_NAMES: &[&str] = &[
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

// ---------------------------------------------------------------------------
// In-memory stats cache
// ---------------------------------------------------------------------------

/// Global stats cache: maps user handle -> stats object.
static STATS_CACHE: LazyLock<Mutex<HashMap<String, Value>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Global timestamps cache: maps user handle -> last save timestamp.
static TIMESTAMPS_CACHE: LazyLock<Mutex<HashMap<String, i64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ---------------------------------------------------------------------------
// Timestamp parsing
// ---------------------------------------------------------------------------

/// Parse a timestamp in various formats to milliseconds since Unix Epoch.
/// Mirrors Node's `parseTimestamp()` in `stats.js:62-117`.
fn parse_timestamp(timestamp: &Value) -> i64 {
    match timestamp {
        Value::Null => 0,
        Value::Number(n) => {
            if let Some(v) = n.as_f64() {
                if v.is_finite() && v >= 0.0 {
                    v as i64
                } else {
                    0
                }
            } else {
                0
            }
        }
        Value::String(s) => {
            if s.is_empty() {
                return 0;
            }

            // Check if it's a pure numeric string
            if s.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(v) = s.parse::<i64>() {
                    return v;
                }
            }

            // ISO 8601 format
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
                return dt.timestamp_millis();
            }

            // Try humanized format: 2024-07-12@01h31m37s123ms
            let re_humanized = regex::Regex::new(
                r"(\d{4})-(\d{1,2})-(\d{1,2})@(\d{1,2})h(\d{1,2})m(\d{1,2})s(?:(\d{1,3})ms)?"
            ).ok();
            if let Some(ref re) = re_humanized {
                if let Some(caps) = re.captures(s) {
                    let year: i32 = caps[1].parse().unwrap_or(0);
                    let month: u32 = caps[2].parse().unwrap_or(1);
                    let day: u32 = caps[3].parse().unwrap_or(1);
                    let hour: u32 = caps[4].parse().unwrap_or(0);
                    let min: u32 = caps[5].parse().unwrap_or(0);
                    let sec: u32 = caps[6].parse().unwrap_or(0);
                    let ms: u32 = caps.get(7).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);

                    if let Some(dt) = chrono::NaiveDate::from_ymd_opt(year, month, day) {
                        if let Some(t) = dt.and_hms_milli_opt(hour, min, sec, ms) {
                            return t.and_utc().timestamp_millis();
                        }
                    }
                }
            }

            // Try humanized with spaces: 2024-6-5 @14h 56m 50s 682ms
            let re_humanized_spaces = regex::Regex::new(
                r"(\d{4})-(\d{1,2})-(\d{1,2}) @(\d{1,2})h (\d{1,2})m (\d{1,2})s (\d{1,3})ms"
            ).ok();
            if let Some(ref re) = re_humanized_spaces {
                if let Some(caps) = re.captures(s) {
                    let year: i32 = caps[1].parse().unwrap_or(0);
                    let month: u32 = caps[2].parse().unwrap_or(1);
                    let day: u32 = caps[3].parse().unwrap_or(1);
                    let hour: u32 = caps[4].parse().unwrap_or(0);
                    let min: u32 = caps[5].parse().unwrap_or(0);
                    let sec: u32 = caps[6].parse().unwrap_or(0);
                    let ms: u32 = caps[7].parse().unwrap_or(0);

                    if let Some(dt) = chrono::NaiveDate::from_ymd_opt(year, month, day) {
                        if let Some(t) = dt.and_hms_milli_opt(hour, min, sec, ms) {
                            return t.and_utc().timestamp_millis();
                        }
                    }
                }
            }

            // Meridiem-based: June 19, 2023 2:20pm
            let re_meridiem = regex::Regex::new(
                r"(\w+)\s(\d{1,2}),\s(\d{4})\s(\d{1,2}):(\d{1,2})(am|pm)"
            ).ok();
            if let Some(ref re) = re_meridiem {
                if let Some(caps) = re.captures(s) {
                    let month_name = &caps[1];
                    let day: u32 = caps[2].parse().unwrap_or(1);
                    let year: i32 = caps[3].parse().unwrap_or(0);
                    let hour: u32 = caps[4].parse().unwrap_or(0);
                    let minute: u32 = caps[5].parse().unwrap_or(0);
                    let meridiem = caps[6].to_lowercase();

                    let month_num = MONTH_NAMES.iter()
                        .position(|&m| m.eq_ignore_ascii_case(month_name))
                        .map(|i| (i + 1) as u32)
                        .unwrap_or(1);

                    let hour24 = if meridiem == "pm" {
                        (hour % 12) + 12
                    } else {
                        hour % 12
                    };

                    if let Some(dt) = chrono::NaiveDate::from_ymd_opt(year, month_num, day) {
                        if let Some(t) = dt.and_hms_opt(hour24, minute, 0) {
                            return t.and_utc().timestamp_millis();
                        }
                    }
                }
            }

            0
        }
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Stats calculation helpers
// ---------------------------------------------------------------------------

/// Count words in a string (matches Node's `\b\w+\b` regex).
fn count_words(s: &str) -> usize {
    let re = regex::Regex::new(r"\b\w+\b").unwrap();
    re.find_iter(s).count()
}

/// Calculate gen time between two dates.
fn calculate_gen_time(started: &Value, finished: &Value) -> f64 {
    let start = parse_timestamp(started);
    let end = parse_timestamp(finished);
    (end - start) as f64
}

/// Calculate statistics for a single character's chat directory.
fn calculate_stats(chats_path: &Path, item: &str) -> (String, Value) {
    let char_name = item.strip_suffix(".png").unwrap_or(item);
    let chat_dir = chats_path.join(char_name);

    let mut total_gen_time: f64 = 0.0;
    let mut user_word_count: usize = 0;
    let mut non_user_word_count: usize = 0;
    let mut user_msg_count: usize = 0;
    let mut non_user_msg_count: usize = 0;
    let mut total_swipe_count: usize = 0;
    let mut chat_size: u64 = 0;
    let mut date_last_chat: i64 = 0;
    let mut date_first_chat: i64 = 253402300799999i64; // 9999-12-31

    let mut unique_gen_start_times = std::collections::HashSet::new();

    if chat_dir.exists() {
        if let Ok(entries) = fs::read_dir(&chat_dir) {
            let chats: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
                .collect();

            for chat_entry in &chats {
                let chat_path = chat_entry.path();
                let file_content = match fs::read_to_string(&chat_path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let lines: Vec<&str> = file_content.split('\n').collect();

                for line in &lines {
                    if line.is_empty() {
                        continue;
                    }

                    let json: Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    // Dedup by message hash
                    if let Some(mes) = json.get("mes").and_then(|v| v.as_str()) {
                        let mut hasher = Sha256::new();
                        hasher.update(mes.as_bytes());
                        let hash = format!("{:x}", hasher.finalize());
                        if unique_gen_start_times.contains(&hash) {
                            continue;
                        }
                        unique_gen_start_times.insert(hash);
                    }

                    // Gen time
                    if json.get("gen_started").is_some() && json.get("gen_finished").is_some() {
                        let gen_time = calculate_gen_time(
                            &json["gen_started"],
                            &json["gen_finished"],
                        );
                        total_gen_time += gen_time;

                        // If swipes but no swipe_info, estimate
                        if json.get("swipes").is_some() && json.get("swipe_info").is_none() {
                            if let Some(swipes) = json["swipes"].as_array() {
                                total_gen_time += gen_time * swipes.len() as f64;
                            }
                        }
                    }

                    // Word count
                    if let Some(mes) = json.get("mes").and_then(|v| v.as_str()) {
                        let word_count = count_words(mes);
                        let is_user = json.get("is_user").and_then(|v| v.as_bool()).unwrap_or(false);
                        if is_user {
                            user_word_count += word_count;
                            user_msg_count += 1;
                        } else {
                            non_user_word_count += word_count;
                            non_user_msg_count += 1;
                        }
                    }

                    // Swipes
                    if let Some(swipes) = json.get("swipes").and_then(|v| v.as_array()) {
                        if swipes.len() > 1 {
                            total_swipe_count += swipes.len() - 1;
                            let is_user = json.get("is_user").and_then(|v| v.as_bool()).unwrap_or(false);
                            for swipe in swipes.iter().skip(1) {
                                if let Some(text) = swipe.as_str() {
                                    let wc = count_words(text);
                                    if is_user {
                                        user_word_count += wc;
                                        user_msg_count += 1;
                                    } else {
                                        non_user_word_count += wc;
                                        non_user_msg_count += 1;
                                    }
                                }
                            }
                        }
                    }

                    // Swipe info gen times
                    if let Some(swipe_info) = json.get("swipe_info").and_then(|v| v.as_array()) {
                        for swipe in swipe_info.iter().skip(1) {
                            if swipe.get("gen_started").is_some()
                                && swipe.get("gen_finished").is_some()
                            {
                                total_gen_time += calculate_gen_time(
                                    &swipe["gen_started"],
                                    &swipe["gen_finished"],
                                );
                            }
                        }
                    }

                    // First chat time
                    let is_user = json.get("is_user").and_then(|v| v.as_bool()).unwrap_or(false);
                    if is_user {
                        if let Some(send_date) = json.get("send_date") {
                            let ts = parse_timestamp(send_date);
                            if ts > 0 && ts < date_first_chat {
                                date_first_chat = ts;
                            }
                        }
                    }
                }

                // Chat file stats
                if let Ok(meta) = fs::metadata(&chat_path) {
                    chat_size += meta.len();
                    if let Ok(modified) = meta.modified() {
                        let mtime = modified
                            .duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        date_last_chat = date_last_chat.max(mtime);
                    }
                }
            }
        }
    }

    let stats = serde_json::json!({
        "total_gen_time": total_gen_time,
        "user_word_count": user_word_count,
        "non_user_word_count": non_user_word_count,
        "user_msg_count": user_msg_count,
        "non_user_msg_count": non_user_msg_count,
        "total_swipe_count": total_swipe_count,
        "chat_size": chat_size,
        "date_last_chat": date_last_chat,
        "date_first_chat": date_first_chat,
    });

    (item.to_string(), stats)
}

/// Collect and aggregate stats for all characters.
fn collect_and_create_stats(chats_path: &Path, characters_path: &Path) -> Value {
    let png_files: Vec<String> = match fs::read_dir(characters_path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".png"))
            .collect(),
        Err(_) => Vec::new(),
    };

    let mut final_stats = serde_json::Map::new();

    for file in &png_files {
        let (key, stats) = calculate_stats(chats_path, file);
        final_stats.insert(key, stats);
    }

    final_stats.insert(
        "timestamp".to_string(),
        Value::Number(serde_json::Number::from(chrono::Utc::now().timestamp_millis())),
    );

    Value::Object(final_stats)
}

/// Recreate stats for a user and cache them.
fn recreate_stats_for_user(handle: &str, chats_path: &Path, characters_path: &Path) {
    tracing::info!("Collecting and creating stats for user: {}", handle);
    let stats = collect_and_create_stats(chats_path, characters_path);

    if let Ok(mut cache) = STATS_CACHE.lock() {
        cache.insert(handle.to_string(), stats.clone());
    }

    // Save to file
    save_stats_for_handle(handle, &stats, chats_path);
}

/// Save stats to file for a user.
fn save_stats_for_handle(handle: &str, stats: &Value, _chats_path: &Path) {
    // We need the user root path to save the file
    // The stats are saved relative to the user's root directory
    // Since we have the chats path, we can go up one level
    if let Some(user_root) = _chats_path.parent() {
        let stats_file = user_root.join(STATS_FILE);
        if let Ok(json_str) = serde_json::to_string(stats) {
            let _ = fs::write(&stats_file, json_str);
            if let Ok(mut timestamps) = TIMESTAMPS_CACHE.lock() {
                timestamps.insert(handle.to_string(), chrono::Utc::now().timestamp_millis());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/stats/get` — Get the current stats object.
///
/// Mirrors Node's `router.post('/get')` in `stats.js:444-447`.
pub async fn get_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> impl IntoResponse {
    let handle = &user.handle;

    // Try to get from cache first
    if let Ok(cache) = STATS_CACHE.lock() {
        if let Some(stats) = cache.get(handle) {
            return Json(stats.clone()).into_response();
        }
    }

    // Try to load from file
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let stats_file = dirs.root.join(STATS_FILE);
    match fs::read_to_string(&stats_file) {
        Ok(content) => {
            match serde_json::from_str::<Value>(&content) {
                Ok(stats) => {
                    // Cache it
                    if let Ok(mut cache) = STATS_CACHE.lock() {
                        cache.insert(handle.to_string(), stats.clone());
                    }
                    Json(stats).into_response()
                }
                Err(_) => Json(serde_json::json!({})).into_response(),
            }
        }
        Err(_) => Json(serde_json::json!({})).into_response(),
    }
}

/// `POST /api/stats/recreate` — Recreate stats from chat files.
///
/// Mirrors Node's `router.post('/recreate')` in `stats.js:452-460`.
pub async fn recreate_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    recreate_stats_for_user(&user.handle, &dirs.chats, &dirs.characters);
    (StatusCode::OK, "OK")
}

/// `POST /api/stats/update` — Update the stats object from the client.
///
/// Mirrors Node's `router.post('/update')` in `stats.js:465-469`.
pub async fn update_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if body.is_null() {
        return (StatusCode::BAD_REQUEST, "Bad Request").into_response();
    }

    // Set timestamp and cache
    let mut stats = body;
    stats["timestamp"] = Value::Number(
        serde_json::Number::from(chrono::Utc::now().timestamp_millis()),
    );

    if let Ok(mut cache) = STATS_CACHE.lock() {
        cache.insert(user.handle.clone(), stats);
    }

    // Persist to disk (matches Node's periodic save behavior)
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    if let Ok(cache) = STATS_CACHE.lock() {
        if let Some(current) = cache.get(&user.handle) {
            save_stats_for_handle(&user.handle, current, &dirs.chats);
        }
    }

    (StatusCode::OK, "OK").into_response()
}
