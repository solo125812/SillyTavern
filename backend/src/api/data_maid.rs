//! Data Maid endpoints — Phase 10.
//!
//! Mirrors Node's [`src/endpoints/data-maid.js`](../../../src/endpoints/data-maid.js).
//!
//! ## Endpoints
//! - `POST /api/data-maid/report` — generate orphan-file report + token.
//! - `POST /api/data-maid/finalize` — invalidate a clean-up token.
//! - `GET  /api/data-maid/view` — serve a file by hash (requires token).
//! - `POST /api/data-maid/delete` — delete files by hashes (requires token).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::fs;

use crate::api::router::AppState;
use crate::api::settings::get_settings_backup_file_prefix;
use crate::http::middleware::UserContext;
use crate::storage::jsonl::CHAT_BACKUPS_PREFIX;
use crate::storage::paths::UserDirectories;

// ---------------------------------------------------------------------------
// Static token store
// ---------------------------------------------------------------------------

/// In-memory token store — mirrors Node's `DataMaidService.TOKENS`.
static TOKENS: LazyLock<Mutex<HashMap<String, TokenEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A path entry with its SHA-256 hash.
#[derive(Debug, Clone)]
struct PathEntry {
    path: PathBuf,
    hash: String,
}

/// Token entry storing the user handle and all deletable paths.
#[derive(Debug, Clone)]
struct TokenEntry {
    handle: String,
    paths: Vec<PathEntry>,
}

/// Raw report — absolute paths grouped by category.
struct RawReport {
    images: Vec<PathBuf>,
    files: Vec<PathBuf>,
    chats: Vec<PathBuf>,
    group_chats: Vec<PathBuf>,
    avatar_thumbnails: Vec<PathBuf>,
    background_thumbnails: Vec<PathBuf>,
    persona_thumbnails: Vec<PathBuf>,
    chat_backups: Vec<PathBuf>,
    settings_backups: Vec<PathBuf>,
}

/// Sanitized record — sent to the client.
#[derive(Debug, Clone, Serialize)]
struct SanitizedRecord {
    name: String,
    hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mtime: Option<f64>,
}

/// Sanitized report — sent to the client.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SanitizedReport {
    images: Vec<SanitizedRecord>,
    files: Vec<SanitizedRecord>,
    chats: Vec<SanitizedRecord>,
    group_chats: Vec<SanitizedRecord>,
    avatar_thumbnails: Vec<SanitizedRecord>,
    background_thumbnails: Vec<SanitizedRecord>,
    persona_thumbnails: Vec<SanitizedRecord>,
    chat_backups: Vec<SanitizedRecord>,
    settings_backups: Vec<SanitizedRecord>,
}

/// Request for finalize/delete.
#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub token: Option<String>,
    pub hashes: Option<Vec<String>>,
}

/// Query params for GET /view.
#[derive(Debug, Deserialize)]
pub struct ViewQuery {
    pub token: Option<String>,
    pub hash: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// SHA-256 hex digest of a string.
fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Check if `child` is under `parent` (after normalization).
fn is_path_under_parent(parent: &Path, child: &Path) -> bool {
    // Use canonicalize if possible, fall back to starts_with on the raw path.
    let parent_canon = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
    let child_canon = child.canonicalize().unwrap_or_else(|_| child.to_path_buf());
    child_canon.starts_with(&parent_canon)
}

/// Sanitize a single file path into a `SanitizedRecord`.
async fn sanitize_record(file_path: &Path, with_parent: bool) -> SanitizedRecord {
    let stat = fs::metadata(file_path).await.ok();
    SanitizedRecord {
        name: file_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        hash: sha256_hex(&file_path.to_string_lossy()),
        parent: if with_parent {
            file_path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
        } else {
            None
        },
        size: stat.as_ref().map(|s| s.len()),
        mtime: stat.as_ref().and_then(|s| {
            s.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs_f64() * 1000.0)
            })
        }),
    }
}

/// Try to parse a JSON string, returning `None` on failure.
fn try_parse(s: &str) -> Option<Value> {
    serde_json::from_str(s).ok()
}

// ---------------------------------------------------------------------------
// DataMaidService
// ---------------------------------------------------------------------------

/// Service for detecting and managing loose user data files.
struct DataMaidService {
    handle: String,
    dirs: UserDirectories,
}

impl DataMaidService {
    fn new(handle: String, dirs: UserDirectories) -> Self {
        Self { handle, dirs }
    }

    /// Generate a full report of orphaned files.
    async fn generate_report(&self) -> RawReport {
        RawReport {
            images: self.collect_images().await,
            files: self.collect_files().await,
            chats: self.collect_chats().await,
            group_chats: self.collect_group_chats().await,
            avatar_thumbnails: self.collect_avatar_thumbnails().await,
            background_thumbnails: self.collect_background_thumbnails().await,
            persona_thumbnails: self.collect_persona_thumbnails().await,
            chat_backups: self.collect_chat_backups().await,
            settings_backups: self.collect_settings_backups().await,
        }
    }

    /// Sanitize the report for client consumption.
    async fn sanitize_report(&self, report: &RawReport) -> SanitizedReport {
        SanitizedReport {
            images: Self::sanitize_vec(&report.images, true).await,
            files: Self::sanitize_vec(&report.files, false).await,
            chats: Self::sanitize_vec(&report.chats, true).await,
            group_chats: Self::sanitize_vec(&report.group_chats, false).await,
            avatar_thumbnails: Self::sanitize_vec(&report.avatar_thumbnails, false).await,
            background_thumbnails: Self::sanitize_vec(&report.background_thumbnails, false).await,
            persona_thumbnails: Self::sanitize_vec(&report.persona_thumbnails, false).await,
            chat_backups: Self::sanitize_vec(&report.chat_backups, false).await,
            settings_backups: Self::sanitize_vec(&report.settings_backups, false).await,
        }
    }

    async fn sanitize_vec(paths: &[PathBuf], with_parent: bool) -> Vec<SanitizedRecord> {
        let mut out = Vec::with_capacity(paths.len());
        for p in paths {
            out.push(sanitize_record(p, with_parent).await);
        }
        out
    }

    /// Generate a token, replacing any existing token for the same user handle.
    fn generate_token(handle: &str, report: &RawReport) -> String {
        let mut tokens = TOKENS.lock().unwrap();

        // Remove existing tokens for this user.
        tokens.retain(|_, entry| entry.handle != handle);

        let token = uuid::Uuid::new_v4().to_string().replace('-', "")
            + &uuid::Uuid::new_v4().to_string().replace('-', "");

        let mut all_paths = Vec::new();
        for path_list in [
            &report.images,
            &report.files,
            &report.chats,
            &report.group_chats,
            &report.avatar_thumbnails,
            &report.background_thumbnails,
            &report.persona_thumbnails,
            &report.chat_backups,
            &report.settings_backups,
        ] {
            for p in path_list {
                all_paths.push(PathEntry {
                    hash: sha256_hex(&p.to_string_lossy()),
                    path: p.clone(),
                });
            }
        }

        tokens.insert(
            token.clone(),
            TokenEntry {
                handle: handle.to_string(),
                paths: all_paths,
            },
        );

        token
    }

    // -----------------------------------------------------------------------
    // Collectors
    // -----------------------------------------------------------------------

    /// Collect loose images — files in `user/images` not referenced by any chat.
    async fn collect_images(&self) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let Ok(messages) = self
            .parse_all_chats(|v| {
                let extra = &v["extra"];
                !extra["image"].is_null()
                    || !extra["video"].is_null()
                    || extra["image_swipes"].is_array()
                    || extra["media"].is_array()
            })
            .await
        else {
            return result;
        };

        let mut known = HashSet::new();
        for msg in &messages {
            let extra = &msg["extra"];
            if let Some(s) = extra["image"].as_str() {
                known.insert(s.to_string());
            }
            if let Some(s) = extra["video"].as_str() {
                known.insert(s.to_string());
            }
            if let Some(arr) = extra["image_swipes"].as_array() {
                for v in arr {
                    if let Some(s) = v.as_str() {
                        known.insert(s.to_string());
                    }
                }
            }
            if let Some(arr) = extra["media"].as_array() {
                for v in arr {
                    if let Some(s) = v["url"].as_str() {
                        known.insert(s.to_string());
                    }
                }
            }
        }

        // Also check chat metadata for chat_backgrounds.
        if let Ok(metadata) = self
            .parse_all_metadata(|v| {
                v["chat_backgrounds"].is_array()
                    && v["chat_backgrounds"]
                        .as_array()
                        .map_or(false, |a| !a.is_empty())
            })
            .await
        {
            for meta in &metadata {
                if let Some(bgs) = meta["chat_backgrounds"].as_array() {
                    for bg in bgs {
                        if let Some(s) = bg.as_str() {
                            known.insert(s.to_string());
                        }
                    }
                }
            }
        }

        // Build full paths from relative references, skip URLs and data URIs.
        let mut known_full = HashSet::new();
        for img in &known {
            if img.starts_with("http") || img.starts_with("data:") {
                continue;
            }
            let full = self.dirs.root.join(img);
            if let Ok(norm) = std::fs::canonicalize(&full) {
                known_full.insert(norm);
            } else {
                // Use the joined path as-is for comparison.
                known_full.insert(full);
            }
        }

        // Scan user images directory.
        if let Ok(mut rd) = fs::read_dir(&self.dirs.user_images).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_file() && !known_full.contains(&path) {
                        result.push(path);
                    } else if ft.is_dir() {
                        // Scan one level of subdirectories.
                        if let Ok(mut sub) = fs::read_dir(&path).await {
                            while let Ok(Some(sub_entry)) = sub.next_entry().await {
                                let sub_path = sub_entry.path();
                                if let Ok(sub_ft) = sub_entry.file_type().await {
                                    if sub_ft.is_file() && !known_full.contains(&sub_path) {
                                        result.push(sub_path);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        result
    }

    /// Collect loose user files not referenced by chats, metadata, or settings.
    async fn collect_files(&self) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let mut known = HashSet::new();

        // Collect from chat messages.
        if let Ok(messages) = self
            .parse_all_chats(|v| {
                let extra = &v["extra"];
                !extra["file"]["url"].is_null()
                    || (extra["files"].is_array()
                        && extra["files"]
                            .as_array()
                            .map_or(false, |a| !a.is_empty()))
            })
            .await
        {
            for msg in &messages {
                let extra = &msg["extra"];
                if let Some(s) = extra["file"]["url"].as_str() {
                    known.insert(s.to_string());
                }
                if let Some(arr) = extra["files"].as_array() {
                    for f in arr {
                        if let Some(s) = f["url"].as_str() {
                            known.insert(s.to_string());
                        }
                    }
                }
            }
        }

        // Collect from chat metadata (attachments).
        if let Ok(metadata) = self
            .parse_all_metadata(|v| {
                v["attachments"].is_array()
                    && v["attachments"]
                        .as_array()
                        .map_or(false, |a| !a.is_empty())
            })
            .await
        {
            for meta in &metadata {
                if let Some(atts) = meta["attachments"].as_array() {
                    for att in atts {
                        if let Some(s) = att["url"].as_str() {
                            known.insert(s.to_string());
                        }
                    }
                }
            }
        }

        // Collect from settings file.
        let settings_path = self.dirs.root.join("settings.json");
        if let Ok(content) = fs::read_to_string(&settings_path).await {
            if let Some(settings) = try_parse(&content) {
                if let Some(atts) = settings["extension_settings"]["attachments"].as_array() {
                    for f in atts {
                        if let Some(s) = f["url"].as_str() {
                            known.insert(s.to_string());
                        }
                    }
                }
                if let Some(obj) = settings["extension_settings"]["character_attachments"].as_object() {
                    for files in obj.values() {
                        if let Some(arr) = files.as_array() {
                            for f in arr {
                                if let Some(s) = f["url"].as_str() {
                                    known.insert(s.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        // Build full paths.
        let mut known_full = HashSet::new();
        for file_ref in &known {
            let full = self.dirs.root.join(file_ref);
            if let Ok(norm) = std::fs::canonicalize(&full) {
                known_full.insert(norm);
            } else {
                known_full.insert(full);
            }
        }

        // Scan files directory.
        if let Ok(mut rd) = fs::read_dir(&self.dirs.files).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_file() && !known_full.contains(&path) {
                        result.push(path);
                    }
                }
            }
        }

        result
    }

    /// Collect loose character chats — chat folders without matching character PNGs.
    async fn collect_chats(&self) -> Vec<PathBuf> {
        let mut result = Vec::new();

        // Get known character names (from .png files).
        let mut known_folders = HashSet::new();
        if let Ok(mut rd) = fs::read_dir(&self.dirs.characters).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_file() {
                        if let Some(ext) = path.extension() {
                            if ext == "png" {
                                if let Some(stem) = path.file_stem() {
                                    known_folders.insert(stem.to_string_lossy().into_owned());
                                }
                            }
                        }
                    }
                }
            }
        }

        // Scan chat folders.
        if let Ok(mut rd) = fs::read_dir(&self.dirs.chats).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_dir() {
                        let folder_name = entry.file_name().to_string_lossy().into_owned();
                        if !known_folders.contains(&folder_name) {
                            // Collect all .jsonl files in this orphaned folder.
                            let folder_path = entry.path();
                            if let Ok(mut sub) = fs::read_dir(&folder_path).await {
                                while let Ok(Some(sub_entry)) = sub.next_entry().await {
                                    let sub_path = sub_entry.path();
                                    if sub_path.extension().map_or(false, |e| e == "jsonl") {
                                        if let Ok(sub_ft) = sub_entry.file_type().await {
                                            if sub_ft.is_file() {
                                                result.push(sub_path);
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

        result
    }

    /// Collect loose group chats — JSONL files not referenced by group definitions.
    async fn collect_group_chats(&self) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let mut known = HashSet::new();

        // Read group definitions to collect known chat IDs.
        if let Ok(mut rd) = fs::read_dir(&self.dirs.groups).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "json") {
                    if let Ok(content) = fs::read_to_string(&path).await {
                        if let Some(group) = try_parse(&content) {
                            if let Some(chat_id) = group["chat_id"].as_str() {
                                known.insert(chat_id.to_string());
                            }
                            if let Some(chats) = group["chats"].as_array() {
                                for c in chats {
                                    if let Some(s) = c.as_str() {
                                        known.insert(s.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Scan group chats directory for orphaned JSONL files.
        if let Ok(mut rd) = fs::read_dir(&self.dirs.group_chats).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "jsonl") {
                    if let Ok(ft) = entry.file_type().await {
                        if ft.is_file() {
                            let stem = path
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            if !known.contains(&stem) {
                                result.push(path);
                            }
                        }
                    }
                }
            }
        }

        result
    }

    /// Collect loose avatar thumbnails.
    async fn collect_avatar_thumbnails(&self) -> Vec<PathBuf> {
        self.collect_loose_thumbnails(&self.dirs.characters, &self.dirs.thumbnails_avatar)
            .await
    }

    /// Collect loose background thumbnails.
    async fn collect_background_thumbnails(&self) -> Vec<PathBuf> {
        self.collect_loose_thumbnails(&self.dirs.backgrounds, &self.dirs.thumbnails_bg)
            .await
    }

    /// Collect loose persona thumbnails.
    async fn collect_persona_thumbnails(&self) -> Vec<PathBuf> {
        self.collect_loose_thumbnails(&self.dirs.avatars, &self.dirs.thumbnails_persona)
            .await
    }

    /// Generic thumbnail collector — finds thumbnails without matching source files.
    async fn collect_loose_thumbnails(
        &self,
        source_dir: &Path,
        thumbnail_dir: &Path,
    ) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let mut known = HashSet::new();

        if let Ok(mut rd) = fs::read_dir(source_dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_file() {
                        known.insert(entry.file_name().to_string_lossy().into_owned());
                    }
                }
            }
        }

        if let Ok(mut rd) = fs::read_dir(thumbnail_dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_file() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if !known.contains(&name) {
                            result.push(entry.path());
                        }
                    }
                }
            }
        }

        result
    }

    /// Collect chat backups.
    async fn collect_chat_backups(&self) -> Vec<PathBuf> {
        self.collect_backup_files(CHAT_BACKUPS_PREFIX).await
    }

    /// Collect settings backups.
    async fn collect_settings_backups(&self) -> Vec<PathBuf> {
        let prefix = get_settings_backup_file_prefix(&self.handle);
        self.collect_backup_files(&prefix).await
    }

    /// Generic backup file collector.
    async fn collect_backup_files(&self, prefix: &str) -> Vec<PathBuf> {
        let mut result = Vec::new();
        if let Ok(mut rd) = fs::read_dir(&self.dirs.backups).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_file() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if name.starts_with(prefix) {
                            result.push(entry.path());
                        }
                    }
                }
            }
        }
        result
    }

    // -----------------------------------------------------------------------
    // Chat / metadata parsing
    // -----------------------------------------------------------------------

    /// Parse all JSONL chat files and return messages matching `filter_fn`.
    async fn parse_all_chats(
        &self,
        filter_fn: impl Fn(&Value) -> bool,
    ) -> Result<Vec<Value>, ()> {
        let mut all = Vec::new();

        // Group chats.
        if let Ok(mut rd) = fs::read_dir(&self.dirs.group_chats).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "jsonl") {
                    if let Ok(ft) = entry.file_type().await {
                        if ft.is_file() {
                            let msgs = Self::parse_chat_file(&path).await;
                            all.extend(msgs.into_iter().filter(|m| filter_fn(m)));
                        }
                    }
                }
            }
        }

        // Character chats.
        if let Ok(mut rd) = fs::read_dir(&self.dirs.chats).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_dir() {
                        let dir_path = entry.path();
                        if let Ok(mut sub) = fs::read_dir(&dir_path).await {
                            while let Ok(Some(sub_entry)) = sub.next_entry().await {
                                let sub_path = sub_entry.path();
                                if sub_path.extension().map_or(false, |e| e == "jsonl") {
                                    if let Ok(sub_ft) = sub_entry.file_type().await {
                                        if sub_ft.is_file() {
                                            let msgs = Self::parse_chat_file(&sub_path).await;
                                            all.extend(
                                                msgs.into_iter().filter(|m| filter_fn(m)),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(all)
    }

    /// Parse all chat metadata from group definitions and JSONL first lines.
    async fn parse_all_metadata(
        &self,
        filter_fn: impl Fn(&Value) -> bool,
    ) -> Result<Vec<Value>, ()> {
        let mut all = Vec::new();

        // Group definition files.
        if let Ok(mut rd) = fs::read_dir(&self.dirs.groups).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "json") {
                    if let Ok(content) = fs::read_to_string(&path).await {
                        if let Some(group) = try_parse(&content) {
                            if let Some(cm) = group.get("chat_metadata") {
                                if filter_fn(cm) {
                                    tracing::warn!(
                                        "Found group chat metadata in group definition — deprecated behavior."
                                    );
                                    all.push(cm.clone());
                                }
                            }
                            if let Some(past) = group.get("past_metadata") {
                                if let Some(obj) = past.as_object() {
                                    tracing::warn!(
                                        "Found group past chat metadata in group definition — deprecated behavior."
                                    );
                                    for v in obj.values() {
                                        if filter_fn(v) {
                                            all.push(v.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Group chat JSONL first lines.
        if let Ok(mut rd) = fs::read_dir(&self.dirs.group_chats).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "jsonl") {
                    let msgs = Self::parse_chat_file(&path).await;
                    if let Some(first) = msgs.first() {
                        if let Some(cm) = first.get("chat_metadata") {
                            if filter_fn(cm) {
                                all.push(cm.clone());
                            }
                        }
                    }
                }
            }
        }

        // Character chat JSONL first lines.
        if let Ok(mut rd) = fs::read_dir(&self.dirs.chats).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if let Ok(ft) = entry.file_type().await {
                    if ft.is_dir() {
                        let dir_path = entry.path();
                        if let Ok(mut sub) = fs::read_dir(&dir_path).await {
                            while let Ok(Some(sub_entry)) = sub.next_entry().await {
                                let sub_path = sub_entry.path();
                                if sub_path.extension().map_or(false, |e| e == "jsonl") {
                                    let msgs = Self::parse_chat_file(&sub_path).await;
                                    if let Some(first) = msgs.first() {
                                        if let Some(cm) = first.get("chat_metadata") {
                                            if filter_fn(cm) {
                                                all.push(cm.clone());
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

        Ok(all)
    }

    /// Parse a single JSONL chat file into a list of values.
    async fn parse_chat_file(path: &Path) -> Vec<Value> {
        match fs::read_to_string(path).await {
            Ok(content) => content
                .lines()
                .filter_map(|line| try_parse(line))
                .collect(),
            Err(e) => {
                tracing::error!("Error reading chat file {:?}: {}", path, e);
                Vec::new()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/data-maid/report` — generate orphan-file report + token.
///
/// Mirrors Node's `router.post('/report')` in `data-maid.js:673-690`.
pub async fn generate_report(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let service = DataMaidService::new(user.handle.clone(), dirs);

    let raw = service.generate_report().await;
    let report = service.sanitize_report(&raw).await;
    let token = DataMaidService::generate_token(&user.handle, &raw);

    Json(serde_json::json!({ "report": report, "token": token })).into_response()
}

/// `POST /api/data-maid/finalize` — invalidate a clean-up token.
///
/// Mirrors Node's `router.post('/finalize')` in `data-maid.js:692-719`.
pub async fn finalize_token(
    State(_state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<TokenRequest>,
) -> Response {
    let Some(token) = body.token else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let mut tokens = TOKENS.lock().unwrap();

    let Some(entry) = tokens.get(&token) else {
        return StatusCode::FORBIDDEN.into_response();
    };

    if entry.handle != user.handle {
        return StatusCode::FORBIDDEN.into_response();
    }

    tokens.remove(&token);
    StatusCode::NO_CONTENT.into_response()
}

/// `GET /api/data-maid/view` — serve a file by hash (requires token).
///
/// Mirrors Node's `router.get('/view')` in `data-maid.js:721-768`.
pub async fn view_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Query(query): Query<ViewQuery>,
) -> Response {
    let user_dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let (Some(token), Some(hash)) = (query.token, query.hash) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    // Scope the lock so it's dropped before any async I/O.
    let path = {
        let tokens = TOKENS.lock().unwrap();
        let Some(entry) = tokens.get(&token) else {
            return StatusCode::FORBIDDEN.into_response();
        };
        if entry.handle != user.handle {
            return StatusCode::FORBIDDEN.into_response();
        }
        let Some(file_entry) = entry.paths.iter().find(|e| e.hash == hash) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if !is_path_under_parent(&user_dirs.root, &file_entry.path) {
            tracing::warn!(
                "Attempted access to a file outside of the user directory: {:?}",
                file_entry.path
            );
            return StatusCode::FORBIDDEN.into_response();
        }
        file_entry.path.clone()
    };

    if !path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    match fs::read(&path).await {
        Ok(data) => {
            let mime = mime_guess::from_path(&path)
                .first_raw()
                .unwrap_or("text/plain");
            ([(axum::http::header::CONTENT_TYPE, mime)], data).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `POST /api/data-maid/delete` — delete files by hashes (requires token).
///
/// Mirrors Node's `router.post('/delete')` in `data-maid.js:770-816`.
pub async fn delete_files(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<TokenRequest>,
) -> Response {
    let user_dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let Some(token) = body.token else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let hashes = match body.hashes {
        Some(h) if !h.is_empty() => h,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Scope the lock so it's dropped before any async I/O.
    let to_delete = {
        let tokens = TOKENS.lock().unwrap();
        let Some(entry) = tokens.get(&token) else {
            return StatusCode::FORBIDDEN.into_response();
        };
        if entry.handle != user.handle {
            return StatusCode::FORBIDDEN.into_response();
        }
        let mut paths = Vec::new();
        for hash in &hashes {
            if let Some(file_entry) = entry.paths.iter().find(|e| &e.hash == hash) {
                if !is_path_under_parent(&user_dirs.root, &file_entry.path) {
                    tracing::warn!(
                        "Attempted deletion of a file outside of the user directory: {:?}",
                        file_entry.path
                    );
                    continue;
                }
                paths.push(file_entry.path.clone());
            }
        }
        paths
    };

    // Delete files.
    for path in &to_delete {
        if path.exists() {
            if let Err(e) = fs::remove_file(path).await {
                tracing::error!("Error deleting file {:?}: {}", path, e);
            }
        }
    }

    StatusCode::NO_CONTENT.into_response()
}
