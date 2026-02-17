//! JSONL (JSON Lines) read/write helpers and chat backup utilities.
//!
//! Provides helpers for the `.jsonl` chat format used by SillyTavern,
//! including integrity checking, throttled backups, and old backup cleanup.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::{debug, error, warn};

use crate::storage::sanitize::sanitize_filename;

// ---------------------------------------------------------------------------
// JSONL read / write
// ---------------------------------------------------------------------------

/// Read a `.jsonl` file and parse each line as JSON.
///
/// Mirrors Node's `getChatData()` from `chats.js:498-511`.
/// Lines that fail to parse are silently skipped.
pub fn read_jsonl_file(path: &Path) -> Vec<Value> {
    let data = match fs::read_to_string(path) {
        Ok(d) => d,
        Err(e) => {
            warn!("File not found or unreadable: {}: {}", path.display(), e);
            return Vec::new();
        }
    };

    if data.is_empty() {
        return Vec::new();
    }

    data.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Serialize a slice of JSON values to JSONL format and write atomically.
///
/// Uses `write-file-atomic`-compatible atomic writing from `atomic.rs`.
pub fn write_jsonl_file(path: &Path, values: &[Value]) -> std::io::Result<()> {
    let data = values
        .iter()
        .map(|v| serde_json::to_string(v).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    crate::storage::atomic::atomic_write_file(path, data.as_bytes())
}

/// Read and parse the first line of a JSONL file.
///
/// Used for fast integrity slug checks without reading the entire file.
/// Mirrors Node's `readFirstLine()` usage in `checkChatIntegrity()`.
pub fn read_first_line(path: &Path) -> Option<Value> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;

    if first_line.is_empty() {
        return None;
    }

    serde_json::from_str(&first_line).ok()
}

// ---------------------------------------------------------------------------
// Timestamp generation (for backup filenames)
// ---------------------------------------------------------------------------

/// Generate a timestamp string for backup filenames.
///
/// Format: `YYYYMMDD-HHMMSS` — matches Node's `generateTimestamp()` from `util.js:625-634`.
pub fn generate_timestamp() -> String {
    let now = chrono::Local::now();
    now.format("%Y%m%d-%H%M%S").to_string()
}

// ---------------------------------------------------------------------------
// Chat integrity checking
// ---------------------------------------------------------------------------

/// Check if the chat file's integrity slug matches the expected value.
///
/// Mirrors Node's `checkChatIntegrity()` from `chats.js:315-334`:
/// - If the file doesn't exist, returns `true` (assume intact).
/// - If the header has no integrity metadata, returns `true` (skip validation).
/// - Otherwise, compares the slug strings.
pub fn check_chat_integrity(file_path: &Path, integrity_slug: &str) -> bool {
    if !file_path.exists() {
        return true;
    }

    let header = match read_first_line(file_path) {
        Some(h) => h,
        None => return true,
    };

    let chat_integrity = header
        .pointer("/chat_metadata/integrity")
        .and_then(|v| v.as_str());

    match chat_integrity {
        Some(slug) => slug == integrity_slug,
        None => {
            debug!(
                "File {:?} has no integrity metadata for slug {:?}, skipping validation",
                file_path, integrity_slug
            );
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Backup logic
// ---------------------------------------------------------------------------

/// The default backup file prefix for chats.
pub const CHAT_BACKUPS_PREFIX: &str = "chat_";

/// Remove old backup files matching a prefix, keeping at most `limit` files.
///
/// Mirrors Node's `removeOldBackups()` from `util.js:643-660`.
/// Files are sorted by modification time; the oldest are deleted first.
pub fn remove_old_backups(directory: &Path, prefix: &str, limit: usize) {
    let entries = match fs::read_dir(directory) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut files: Vec<(std::path::PathBuf, f64)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if name.starts_with(prefix) && path.is_file() {
                let mtime = path
                    .metadata()
                    .ok()?
                    .modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_secs_f64();
                Some((path, mtime))
            } else {
                None
            }
        })
        .collect();

    if files.len() <= limit {
        return;
    }

    // Sort by mtime ascending (oldest first)
    files.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    while files.len() > limit {
        if let Some((oldest, _)) = files.first() {
            if let Err(e) = fs::remove_file(oldest) {
                warn!("Failed to remove old backup {:?}: {}", oldest, e);
            }
        }
        files.remove(0);
    }
}

/// Write a chat backup file and clean up old backups.
///
/// Mirrors Node's `backupChat()` from `chats.js:40-60`.
/// The filename is sanitized and formatted as `{prefix}{name}_{timestamp}.jsonl`.
pub fn backup_chat(
    directory: &Path,
    name: &str,
    data: &str,
    backup_prefix: &str,
    num_per_chat: usize,
    max_total: i32,
) {
    if !directory.exists() {
        error!(
            "Chat backup directory does not exist: {}",
            directory.display()
        );
        return;
    }

    // Sanitize name: replace non-alphanumeric with underscores, lowercase
    let sanitized = sanitize_filename(name, "")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .to_lowercase();

    let backup_filename = format!(
        "{}{}_{}.jsonl",
        backup_prefix,
        sanitized,
        generate_timestamp()
    );
    let backup_path = directory.join(&backup_filename);

    if let Err(e) = fs::write(&backup_path, data) {
        error!("Could not backup chat for {}: {}", name, e);
        return;
    }

    // Clean up per-chat backups
    let per_chat_prefix = format!("{}{}_", backup_prefix, sanitized);
    remove_old_backups(directory, &per_chat_prefix, num_per_chat);

    // Clean up total chat backups if configured
    if max_total >= 0 {
        remove_old_backups(directory, backup_prefix, max_total as usize);
    }
}

// ---------------------------------------------------------------------------
// Throttled backup
// ---------------------------------------------------------------------------

/// Per-user throttled backup writer.
///
/// Mirrors the lodash `throttle(backupChat, throttleInterval)` pattern
/// from `chats.js:72-77`. Only allows one backup per user within the
/// configured throttle interval.
pub struct BackupThrottle {
    entries: Mutex<HashMap<String, ThrottleEntry>>,
    interval_ms: AtomicU64,
}

impl BackupThrottle {
    /// Create a new throttle with the given interval in milliseconds.
    pub fn new(interval_ms: u64) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            interval_ms: AtomicU64::new(interval_ms),
        }
    }

    /// Update the throttle interval (in milliseconds).
    pub fn set_interval(&self, interval_ms: u64) {
        self.interval_ms.store(interval_ms, Ordering::Relaxed);
    }

    /// Attempt a throttled backup. Returns `true` if the backup was performed,
    /// `false` if throttled (skipped).
    pub fn try_backup(
        self: &std::sync::Arc<Self>,
        handle: &str,
        directory: &Path,
        name: &str,
        data: &str,
        backup_prefix: &str,
        num_per_chat: usize,
        max_total: i32,
        interval_ms: u64,
    ) -> bool {
        self.set_interval(interval_ms);

        let now = Instant::now();
        let interval = Duration::from_millis(interval_ms);
        let mut map = self.entries.lock().unwrap();

        let entry = map
            .entry(handle.to_string())
            .or_insert_with(|| ThrottleEntry {
                last_call: now.checked_sub(interval).unwrap_or(now),
                pending: None,
                scheduled: false,
            });

        if now.duration_since(entry.last_call) >= interval {
            entry.last_call = now;
            entry.pending = None;
            entry.scheduled = false;
            drop(map); // release lock before I/O
            backup_chat(directory, name, data, backup_prefix, num_per_chat, max_total);
            return true;
        }

        // Within interval: store pending backup for trailing call.
        entry.pending = Some(PendingBackup {
            directory: directory.to_path_buf(),
            name: name.to_string(),
            data: data.to_string(),
            backup_prefix: backup_prefix.to_string(),
            num_per_chat,
            max_total,
        });

        if !entry.scheduled {
            entry.scheduled = true;
            let delay = interval
                .checked_sub(now.duration_since(entry.last_call))
                .unwrap_or(Duration::from_millis(0));
            let throttle = std::sync::Arc::clone(self);
            let handle = handle.to_string();
            drop(map);

            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    tokio::time::sleep(delay).await;
                    throttle.run_trailing(handle).await;
                });
            } else {
                // No runtime (e.g., unit tests) — skip scheduling.
            }
            return false;
        }

        false
    }
}

/// Per-user throttle entry.
struct ThrottleEntry {
    last_call: Instant,
    pending: Option<PendingBackup>,
    scheduled: bool,
}

/// Pending backup data for trailing execution.
struct PendingBackup {
    directory: PathBuf,
    name: String,
    data: String,
    backup_prefix: String,
    num_per_chat: usize,
    max_total: i32,
}

impl BackupThrottle {
    async fn run_trailing(self: std::sync::Arc<Self>, handle: String) {
        let pending = {
            let mut map = self.entries.lock().unwrap();
            if let Some(entry) = map.get_mut(&handle) {
                entry.scheduled = false;
                entry.pending.take()
            } else {
                None
            }
        };

        if let Some(p) = pending {
            backup_chat(
                &p.directory,
                &p.name,
                &p.data,
                &p.backup_prefix,
                p.num_per_chat,
                p.max_total,
            );
            let mut map = self.entries.lock().unwrap();
            if let Some(entry) = map.get_mut(&handle) {
                entry.last_call = Instant::now();
            }
        }
    }
}

/// Preview message helper — truncate a message for search results.
///
/// Mirrors Node's `getPreviewMessage()` from `chats.js:84-94`.
pub fn get_preview_message(last_message: &str) -> String {
    const MAX_LEN: usize = 400;

    if last_message.is_empty() {
        return String::new();
    }

    if last_message.len() > MAX_LEN {
        format!("...{}", &last_message[last_message.len() - MAX_LEN..])
    } else {
        last_message.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_read_write_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.jsonl");

        let values = vec![
            json!({"user_name": "User", "character_name": "Char", "chat_metadata": {}}),
            json!({"name": "User", "mes": "Hello!", "is_user": true}),
            json!({"name": "Char", "mes": "Hi there!", "is_user": false}),
        ];

        write_jsonl_file(&file, &values).unwrap();

        let read_back = read_jsonl_file(&file);
        assert_eq!(read_back.len(), 3);
        assert_eq!(read_back[0]["user_name"], "User");
        assert_eq!(read_back[1]["mes"], "Hello!");
        assert_eq!(read_back[2]["mes"], "Hi there!");
    }

    #[test]
    fn test_read_jsonl_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("empty.jsonl");
        fs::write(&file, "").unwrap();

        let result = read_jsonl_file(&file);
        assert!(result.is_empty());
    }

    #[test]
    fn test_read_jsonl_nonexistent() {
        let result = read_jsonl_file(Path::new("/nonexistent/path.jsonl"));
        assert!(result.is_empty());
    }

    #[test]
    fn test_read_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chat.jsonl");

        let content = format!(
            "{}\n{}",
            json!({"chat_metadata": {"integrity": "abc123"}, "user_name": "User"}),
            json!({"name": "User", "mes": "Hello"})
        );
        fs::write(&file, content).unwrap();

        let header = read_first_line(&file).unwrap();
        assert_eq!(
            header.pointer("/chat_metadata/integrity").unwrap(),
            "abc123"
        );
    }

    #[test]
    fn test_generate_timestamp_format() {
        let ts = generate_timestamp();
        // Should be format YYYYMMDD-HHMMSS (15 chars)
        assert_eq!(ts.len(), 15);
        assert!(ts.contains('-'));
    }

    #[test]
    fn test_check_integrity_matching() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chat.jsonl");

        let header = json!({"chat_metadata": {"integrity": "test_slug"}, "user_name": "User"});
        fs::write(&file, serde_json::to_string(&header).unwrap()).unwrap();

        assert!(check_chat_integrity(&file, "test_slug"));
    }

    #[test]
    fn test_check_integrity_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chat.jsonl");

        let header = json!({"chat_metadata": {"integrity": "slug_a"}, "user_name": "User"});
        fs::write(&file, serde_json::to_string(&header).unwrap()).unwrap();

        assert!(!check_chat_integrity(&file, "slug_b"));
    }

    #[test]
    fn test_check_integrity_missing_file() {
        assert!(check_chat_integrity(
            Path::new("/nonexistent/chat.jsonl"),
            "any_slug"
        ));
    }

    #[test]
    fn test_check_integrity_no_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chat.jsonl");

        let header = json!({"chat_metadata": {}, "user_name": "User"});
        fs::write(&file, serde_json::to_string(&header).unwrap()).unwrap();

        assert!(check_chat_integrity(&file, "any_slug"));
    }

    #[test]
    fn test_backup_chat_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        fs::create_dir_all(&backup_dir).unwrap();

        backup_chat(&backup_dir, "TestChar", "line1\nline2", CHAT_BACKUPS_PREFIX, 50, -1);

        let files: Vec<_> = fs::read_dir(&backup_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        assert_eq!(files.len(), 1);
        assert!(files[0].starts_with("chat_testchar_"));
        assert!(files[0].ends_with(".jsonl"));
    }

    #[test]
    fn test_remove_old_backups() {
        let dir = tempfile::tempdir().unwrap();

        // Create 5 backup files with slight time differences
        for i in 0..5 {
            let path = dir.path().join(format!("chat_test_{}.jsonl", i));
            fs::write(&path, format!("data_{}", i)).unwrap();
            // Small sleep to ensure different mtimes
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        remove_old_backups(dir.path(), "chat_test_", 2);

        let remaining: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.starts_with("chat_test_"))
                    .unwrap_or(false)
            })
            .collect();

        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn test_get_preview_message_short() {
        assert_eq!(get_preview_message("Hello!"), "Hello!");
    }

    #[test]
    fn test_get_preview_message_empty() {
        assert_eq!(get_preview_message(""), "");
    }

    #[test]
    fn test_get_preview_message_truncation() {
        let long_message = "x".repeat(500);
        let preview = get_preview_message(&long_message);
        assert!(preview.starts_with("..."));
        assert_eq!(preview.len(), 403); // "..." (3) + 400 chars
    }

    #[test]
    fn test_throttle_blocks_rapid_calls() {
        let throttle = std::sync::Arc::new(BackupThrottle::new(60_000)); // 60 second throttle
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path()).unwrap();

        let first = throttle.try_backup(
            "user1",
            dir.path(),
            "test",
            "data",
            "chat_",
            50,
            -1,
            60_000,
        );
        let second = throttle.try_backup(
            "user1",
            dir.path(),
            "test",
            "data",
            "chat_",
            50,
            -1,
            60_000,
        );

        assert!(first);
        assert!(!second); // Should be throttled
    }

    #[test]
    fn test_throttle_allows_different_users() {
        let throttle = std::sync::Arc::new(BackupThrottle::new(60_000));
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path()).unwrap();

        let first = throttle.try_backup(
            "user1",
            dir.path(),
            "test",
            "data",
            "chat_",
            50,
            -1,
            60_000,
        );
        let second = throttle.try_backup(
            "user2",
            dir.path(),
            "test",
            "data",
            "chat_",
            50,
            -1,
            60_000,
        );

        assert!(first);
        assert!(second); // Different user, not throttled
    }
}
