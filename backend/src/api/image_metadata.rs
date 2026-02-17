//! Image metadata endpoints — Phase 9.
//!
//! Mirrors Node's [`src/endpoints/image-metadata.js`](../../../src/endpoints/image-metadata.js).
//!
//! ## Endpoints
//! - `POST /api/image-metadata`          — Get metadata for image(s) by path.
//! - `POST /api/image-metadata/all`      — Get all metadata from the index.
//! - `POST /api/image-metadata/cleanup`  — Clean up orphaned metadata entries.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::is_path_under_parent;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const METADATA_FILE: &str = "image-metadata.json";

// ---------------------------------------------------------------------------
// Metadata types
// ---------------------------------------------------------------------------

/// Image metadata entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageMetadata {
    /// SHA-256 hash of the image file.
    pub hash: String,
    /// Aspect ratio (width / height).
    pub aspect_ratio: f64,
    /// Whether the image is animated.
    pub is_animated: bool,
    /// Dominant color in hex format (e.g., '#RRGGBB').
    pub dominant_color: String,
    /// Virtual folder IDs the image belongs to.
    #[serde(default)]
    pub folder_ids: Vec<String>,
    /// Timestamp when the image was added.
    pub added_timestamp: i64,
    /// Thumbnail resolution (width * height) for cache invalidation.
    pub thumbnail_resolution: u32,
    /// File modification time for cache invalidation (internal use).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<f64>,
}

/// Centralized metadata index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataIndex {
    pub version: u32,
    pub images: HashMap<String, ImageMetadata>,
    #[serde(default)]
    pub folders: Vec<MetadataFolder>,
}

/// Virtual folder entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataFolder {
    pub id: String,
    pub name: String,
    pub thumbnail_file: String,
}

impl Default for MetadataIndex {
    fn default() -> Self {
        Self {
            version: 1,
            images: HashMap::new(),
            folders: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GetMetadataRequest {
    /// Single path (relative to user data root).
    pub path: Option<String>,
    /// Multiple paths (relative to user data root).
    pub paths: Option<Vec<String>>,
    /// Thumbnail type for resolution calculation.
    #[serde(rename = "type")]
    pub thumbnail_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GetAllMetadataRequest {
    /// Optional path prefix to filter results.
    #[serde(default)]
    pub prefix: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read the centralized metadata index from the user data root.
fn read_metadata_index(user_data_root: &Path) -> MetadataIndex {
    let index_path = user_data_root.join(METADATA_FILE);
    match fs::read_to_string(&index_path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => MetadataIndex::default(),
    }
}

/// Write the centralized metadata index to the user data root.
fn write_metadata_index(user_data_root: &Path, metadata: &MetadataIndex) {
    let index_path = user_data_root.join(METADATA_FILE);
    if let Ok(json_string) = serde_json::to_string_pretty(metadata) {
        let _ = fs::write(&index_path, json_string);
    }
}

/// Get configured thumbnail resolution for a given type.
fn get_thumbnail_resolution(config: &crate::config::AppConfig, thumb_type: &str) -> u32 {
    let (w, h) = match thumb_type {
        "bg" => config.thumbnail_dimensions.bg,
        "avatar" => config.thumbnail_dimensions.avatar,
        "persona" => config.thumbnail_dimensions.persona,
        _ => (0, 0),
    };
    w * h
}

/// Check if a buffer is an animated APNG by looking for the 'acTL' chunk.
fn is_animated_apng(buffer: &[u8]) -> bool {
    let check_len = std::cmp::min(200, buffer.len());
    buffer[..check_len]
        .windows(4)
        .any(|w| w == b"acTL")
}

/// Check if a WebP buffer is animated by looking for 'ANIM' or 'ANMF' chunks.
fn is_animated_webp(buffer: &[u8]) -> bool {
    let check_len = std::cmp::min(200, buffer.len());
    let header = &buffer[..check_len];
    header.windows(4).any(|w| w == b"ANIM" || w == b"ANMF")
}

/// Get the average/dominant color of an image. For animated images, returns gray.
fn get_dominant_color(buffer: &[u8], is_animated: bool) -> String {
    if is_animated {
        return "#808080".to_string();
    }

    // Try to decode the image and resize to 1x1 to get average color
    match image::load_from_memory(buffer) {
        Ok(img) => {
            let tiny = img.resize_exact(1, 1, image::imageops::FilterType::Lanczos3);
            let rgb = tiny.to_rgb8();
            let pixel = rgb.get_pixel(0, 0);
            format!("#{:02x}{:02x}{:02x}", pixel[0], pixel[1], pixel[2])
        }
        Err(_) => "#808080".to_string(),
    }
}

/// Generate metadata for a single image file.
fn generate_image_metadata(
    file_path: &Path,
    config: &crate::config::AppConfig,
    thumb_type: &str,
) -> Result<ImageMetadata, String> {
    let buffer = fs::read(file_path).map_err(|e| format!("Failed to read file: {e}"))?;

    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(&buffer);
        format!("{:x}", hasher.finalize())
    };

    // Try to get dimensions using the image crate
    let (width, height, image_format) = match image::guess_format(&buffer) {
        Ok(fmt) => {
            match image::load_from_memory(&buffer) {
                Ok(img) => (img.width() as f64, img.height() as f64, Some(fmt)),
                Err(_) => return Err("Could not determine image dimensions.".to_string()),
            }
        }
        Err(_) => return Err("Could not determine image dimensions.".to_string()),
    };

    if width == 0.0 || height == 0.0 {
        return Err("Could not determine image dimensions.".to_string());
    }

    let aspect_ratio = width / height;
    let aspect_ratio = (aspect_ratio * 10000.0).round() / 10000.0;

    let is_animated = match image_format {
        Some(image::ImageFormat::Gif) => true,
        Some(image::ImageFormat::Png) => is_animated_apng(&buffer),
        Some(image::ImageFormat::WebP) => is_animated_webp(&buffer),
        _ => false,
    };

    let dominant_color = get_dominant_color(&buffer, is_animated);

    let added_timestamp = fs::metadata(file_path)
        .and_then(|m| {
            m.created()
                .or_else(|_| m.modified())
                .map(|t| {
                    t.duration_since(SystemTime::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis())
                })
        })
        .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis());

    Ok(ImageMetadata {
        hash,
        aspect_ratio,
        is_animated,
        dominant_color,
        folder_ids: Vec::new(),
        added_timestamp,
        thumbnail_resolution: get_thumbnail_resolution(config, thumb_type),
        mtime: None,
    })
}

/// Normalize path separators to forward slashes for consistent keys.
fn to_posix_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// Get metadata for multiple images, generating on-demand as needed.
fn get_or_generate_metadata_batch(
    user_data_root: &Path,
    relative_paths: &[String],
    config: &crate::config::AppConfig,
    thumb_type: &str,
) -> (HashMap<String, Value>, usize) {
    let mut results: HashMap<String, Value> = HashMap::new();
    let mut index = read_metadata_index(user_data_root);
    let mut index_modified = false;
    let mut generated_count = 0usize;

    for relative_path in relative_paths {
        let posix_path = to_posix_path(relative_path);
        let full_path = user_data_root.join(relative_path);

        let mtime_ms = match fs::metadata(&full_path) {
            Ok(meta) => meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64() * 1000.0)
                .unwrap_or(0.0),
            Err(_) => continue, // File doesn't exist, skip
        };

        // Check if cached and not modified
        if let Some(cached) = index.images.get(&posix_path) {
            if cached.mtime == Some(mtime_ms) {
                if let Ok(val) = serde_json::to_value(cached) {
                    results.insert(relative_path.clone(), val);
                }
                continue;
            }
        }

        // Generate new metadata
        match generate_image_metadata(&full_path, config, thumb_type) {
            Ok(mut metadata) => {
                metadata.mtime = Some(mtime_ms);

                // Preserve folderIds if they existed
                if let Some(cached) = index.images.get(&posix_path) {
                    if !cached.folder_ids.is_empty() {
                        metadata.folder_ids = cached.folder_ids.clone();
                    }
                }

                if let Ok(val) = serde_json::to_value(&metadata) {
                    results.insert(relative_path.clone(), val);
                }
                index.images.insert(posix_path, metadata);
                index_modified = true;
                generated_count += 1;
            }
            Err(err) => {
                tracing::warn!(
                    "[ImageMetadata] Failed to generate metadata for {}: {}",
                    relative_path,
                    err
                );
            }
        }
    }

    if index_modified {
        write_metadata_index(user_data_root, &index);
    }

    (results, generated_count)
}

/// Clean up orphaned entries from the metadata index.
fn cleanup_orphaned_metadata(user_data_root: &Path) -> Vec<String> {
    let mut index = read_metadata_index(user_data_root);
    let mut orphaned_paths = Vec::new();

    let keys: Vec<String> = index.images.keys().cloned().collect();
    for relative_path in keys {
        let full_path = user_data_root.join(&relative_path);

        // Check path is under user root
        if !is_path_under_parent(user_data_root, &full_path) {
            orphaned_paths.push(relative_path.clone());
            index.images.remove(&relative_path);
            continue;
        }

        // Check file exists
        if !full_path.exists() {
            orphaned_paths.push(relative_path.clone());
            index.images.remove(&relative_path);
        }
    }

    if !orphaned_paths.is_empty() {
        write_metadata_index(user_data_root, &index);
        tracing::info!(
            "[ImageMetadata] Cleaned up {} orphaned metadata entries",
            orphaned_paths.len()
        );
    }

    orphaned_paths
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/image-metadata` — Get metadata for image(s) by path.
///
/// Mirrors Node's `router.post('/')` in `image-metadata.js:374-449`.
pub async fn get_image_metadata(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GetMetadataRequest>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let user_data_root = &dirs.root;
    let thumb_type = body.thumbnail_type.as_deref().unwrap_or("bg");

    // Validate that a path is under user data directory
    let validate_path = |relative_path: &str| -> Result<String, String> {
        let full_path = user_data_root.join(relative_path);
        if !is_path_under_parent(user_data_root, &full_path) {
            return Err(format!(
                "Path \"{}\" is outside the user data directory.",
                relative_path
            ));
        }
        Ok(relative_path.to_string())
    };

    if body.path.is_none() && body.paths.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Either \"path\" or \"paths\" is required."})),
        )
            .into_response();
    }

    // Handle single path
    if let Some(ref single_path) = body.path {
        if body.paths.is_none() {
            let relative_path = match validate_path(single_path) {
                Ok(p) => p,
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response()
                }
            };

            let full_path = user_data_root.join(&relative_path);
            if !full_path.exists() {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "File not found."})),
                )
                    .into_response();
            }

            let (metadata_results, _) = get_or_generate_metadata_batch(
                user_data_root,
                &[relative_path.clone()],
                &state.config,
                thumb_type,
            );

            match metadata_results.get(&relative_path) {
                Some(metadata) => Json(metadata.clone()).into_response(),
                None => (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "Could not generate metadata for file."})),
                )
                    .into_response(),
            }
        } else {
            // Both path and paths are set — process as multi
            handle_multi_paths(user_data_root, &body, &state, thumb_type, &validate_path)
        }
    } else if let Some(ref paths) = body.paths {
        if paths.is_empty() || !paths.iter().all(|_| true) {
            // Process paths array
        }
        handle_multi_paths(user_data_root, &body, &state, thumb_type, &validate_path)
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid request format."})),
        )
            .into_response()
    }
}

fn handle_multi_paths(
    user_data_root: &Path,
    body: &GetMetadataRequest,
    state: &Arc<AppState>,
    thumb_type: &str,
    validate_path: &dyn Fn(&str) -> Result<String, String>,
) -> axum::response::Response {
    let paths = match &body.paths {
        Some(p) => p.clone(),
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "Invalid request format."}))).into_response(),
    };

    let mut results: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut valid_paths = Vec::new();

    // Validate all paths first
    for relative_path in &paths {
        match validate_path(relative_path) {
            Ok(_) => valid_paths.push(relative_path.clone()),
            Err(err) => {
                results.insert(relative_path.clone(), json!({"error": err}));
            }
        }
    }

    // Process all valid paths in a single batch
    let (batch_metadata, _) =
        get_or_generate_metadata_batch(user_data_root, &valid_paths, &state.config, thumb_type);

    for relative_path in &valid_paths {
        if let Some(metadata) = batch_metadata.get(relative_path) {
            results.insert(relative_path.clone(), metadata.clone());
        } else {
            results.insert(
                relative_path.clone(),
                json!({"error": "File not found or could not process."}),
            );
        }
    }

    Json(Value::Object(results)).into_response()
}

/// `POST /api/image-metadata/all` — Get all metadata from the index.
///
/// Mirrors Node's `router.post('/all')` in `image-metadata.js:457-479`.
pub async fn get_all_image_metadata(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GetAllMetadataRequest>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let index = read_metadata_index(&dirs.root);

    let prefix = body.prefix.unwrap_or_default();

    if !prefix.is_empty() {
        // Filter to only matching paths
        let filtered_images: HashMap<String, ImageMetadata> = index
            .images
            .into_iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .collect();
        Json(json!({
            "version": index.version,
            "images": filtered_images
        }))
        .into_response()
    } else {
        Json(serde_json::to_value(&index).unwrap_or(json!({}))).into_response()
    }
}

/// `POST /api/image-metadata/cleanup` — Clean up orphaned metadata entries.
///
/// Mirrors Node's `router.post('/cleanup')` in `image-metadata.js:485-494`.
pub async fn cleanup_image_metadata(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let removed = cleanup_orphaned_metadata(&dirs.root);
    Json(json!({
        "removed": removed,
        "count": removed.len()
    }))
}
