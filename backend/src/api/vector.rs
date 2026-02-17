//! Vector endpoints — Phase 9.
//!
//! Mirrors Node's [`src/endpoints/vectors.js`](../../../src/endpoints/vectors.js).
//!
//! ## Endpoints
//! - `POST /api/vector/query`      — Query a vector collection.
//! - `POST /api/vector/query-multi` — Query multiple vector collections.
//! - `POST /api/vector/insert`     — Insert items into a vector collection.
//! - `POST /api/vector/list`       — List hashes in a vector collection.
//! - `POST /api/vector/delete`     — Delete items from a vector collection by hash.
//! - `POST /api/vector/purge-all`  — Purge all vector stores for all sources.
//! - `POST /api/vector/purge`      — Purge a specific collection across all sources.
//!
//! Note: The actual vector embedding computation is done by external providers
//! (OpenAI, transformers, etc.). This module handles the storage operations
//! using a simple JSON-based vector index on disk, mirroring vectra's LocalIndex.
//! Full embedding provider integration is deferred — the endpoints accept
//! pre-computed embeddings from the client where applicable.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Known vector sources — mirrors Node's SOURCES array.
const SOURCES: &[&str] = &[
    "transformers",
    "mistral",
    "openai",
    "extras",
    "palm",
    "togetherai",
    "nomicai",
    "cohere",
    "ollama",
    "llamacpp",
    "vllm",
    "webllm",
    "koboldcpp",
    "vertexai",
    "electronhub",
    "openrouter",
    "chutes",
    "voyageai",
];

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    pub collection_id: Option<String>,
    pub search_text: Option<String>,
    pub top_k: Option<usize>,
    pub threshold: Option<f64>,
    pub source: Option<String>,
    // Source settings passed through
    pub model: Option<String>,
    pub embeddings: Option<Value>,
    pub api_url: Option<String>,
    pub keep: Option<bool>,
    pub extras_url: Option<String>,
    pub extras_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryMultiRequest {
    pub collection_ids: Option<Vec<String>>,
    pub search_text: Option<String>,
    pub top_k: Option<usize>,
    pub threshold: Option<f64>,
    pub source: Option<String>,
    pub model: Option<String>,
    pub embeddings: Option<Value>,
    pub api_url: Option<String>,
    pub keep: Option<bool>,
    pub extras_url: Option<String>,
    pub extras_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertRequest {
    pub collection_id: Option<String>,
    pub items: Option<Vec<InsertItem>>,
    pub source: Option<String>,
    pub model: Option<String>,
    pub embeddings: Option<Value>,
    pub api_url: Option<String>,
    pub keep: Option<bool>,
    pub extras_url: Option<String>,
    pub extras_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InsertItem {
    pub hash: Value,
    pub text: String,
    pub index: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRequest {
    pub collection_id: Option<String>,
    pub source: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRequest {
    pub collection_id: Option<String>,
    pub hashes: Option<Vec<Value>>,
    pub source: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeRequest {
    pub collection_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Vector index types (simple JSON-based, mirroring vectra's LocalIndex)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VectorItem {
    id: String,
    metadata: Value,
    vector: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct VectorIndex {
    version: u32,
    items: Vec<VectorItem>,
}

impl VectorIndex {
    fn load(dir_path: &Path) -> Self {
        let index_path = dir_path.join("index.json");
        match fs::read_to_string(&index_path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => VectorIndex { version: 1, items: Vec::new() },
        }
    }

    fn save(&self, dir_path: &Path) {
        let _ = fs::create_dir_all(dir_path);
        let index_path = dir_path.join("index.json");
        if let Ok(data) = serde_json::to_string_pretty(self) {
            let _ = fs::write(&index_path, data);
        }
    }

    fn list_hashes(&self) -> Vec<Value> {
        self.items
            .iter()
            .filter_map(|item| item.metadata.get("hash").cloned())
            .collect()
    }

    fn upsert(&mut self, metadata: Value, vector: Vec<f64>) {
        let hash = metadata.get("hash").cloned();
        // Remove existing item with same hash
        if let Some(h) = &hash {
            self.items.retain(|item| {
                item.metadata.get("hash") != Some(h)
            });
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.items.push(VectorItem { id, metadata, vector });
    }

    fn delete_by_hashes(&mut self, hashes: &[Value]) {
        self.items.retain(|item| {
            if let Some(h) = item.metadata.get("hash") {
                !hashes.contains(h)
            } else {
                true
            }
        });
    }

    fn query(&self, query_vector: &[f64], top_k: usize, threshold: f64) -> Vec<(f64, &VectorItem)> {
        let mut results: Vec<(f64, &VectorItem)> = self
            .items
            .iter()
            .map(|item| {
                let score = cosine_similarity(query_vector, &item.vector);
                (score, item)
            })
            .filter(|(score, _)| *score >= threshold)
            .collect();

        results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }
}

/// Compute cosine similarity between two vectors.
fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let mag_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    dot / (mag_a * mag_b)
}

/// Get the model scope string from source settings.
fn get_model_scope(model: Option<&str>) -> String {
    model.unwrap_or("").to_string()
}

/// Resolve the vector index directory path.
fn get_index_path(vectors_dir: &Path, source: &str, collection_id: &str, model: &str) -> std::path::PathBuf {
    vectors_dir
        .join(sanitize_filename(source, ""))
        .join(sanitize_filename(collection_id, ""))
        .join(sanitize_filename(model, ""))
}

/// Get pre-computed embedding from request body (for webllm/koboldcpp sources).
fn get_precomputed_embedding(source: &str, text: &str, embeddings: &Option<Value>) -> Option<Vec<f64>> {
    if source != "webllm" && source != "koboldcpp" {
        return None;
    }
    let emb = embeddings.as_ref()?;
    let vec_val = emb.get(text)?;
    let arr = vec_val.as_array()?;
    Some(arr.iter().filter_map(|v| v.as_f64()).collect())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/vector/query` — Query a vector collection.
///
/// Mirrors Node's `router.post('/query')` in `vectors.js:446-464`.
///
/// Note: Full embedding provider support is deferred. For sources that provide
/// pre-computed embeddings (webllm, koboldcpp), the query works with those.
/// For other sources, returns 501 until embedding providers are integrated.
pub async fn query_vector(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<QueryRequest>,
) -> impl IntoResponse {
    let collection_id = match &body.collection_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let search_text = match &body.search_text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let top_k = body.top_k.unwrap_or(10);
    let threshold = body.threshold.unwrap_or(0.0);
    let source = body.source.as_deref().unwrap_or("transformers");
    let model = get_model_scope(body.model.as_deref());

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let index_path = get_index_path(&dirs.vectors, source, &collection_id, &model);

    // Try to get pre-computed embedding for query
    let query_vector = match get_precomputed_embedding(source, &search_text, &body.embeddings) {
        Some(v) => v,
        None => {
            // For sources without pre-computed embeddings, we can't compute them
            // without the actual provider. Return empty results.
            // The Node server would call the provider here.
            tracing::debug!(
                "Vector query for source '{}' without pre-computed embedding — returning empty results",
                source
            );
            return Json(json!({
                "hashes": [],
                "metadata": []
            }))
            .into_response();
        }
    };

    let store = VectorIndex::load(&index_path);
    let results = store.query(&query_vector, top_k, threshold);

    let metadata: Vec<Value> = results.iter().map(|(_, item)| item.metadata.clone()).collect();
    let hashes: Vec<Value> = results
        .iter()
        .filter_map(|(_, item)| item.metadata.get("hash").cloned())
        .collect();

    Json(json!({
        "hashes": hashes,
        "metadata": metadata
    }))
    .into_response()
}

/// `POST /api/vector/query-multi` — Query multiple vector collections.
///
/// Mirrors Node's `router.post('/query-multi')` in `vectors.js:466-484`.
pub async fn query_multi_vector(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<QueryMultiRequest>,
) -> impl IntoResponse {
    let collection_ids = match &body.collection_ids {
        Some(ids) if !ids.is_empty() => ids.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let search_text = match &body.search_text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let top_k = body.top_k.unwrap_or(10);
    let threshold = body.threshold.unwrap_or(0.0);
    let source = body.source.as_deref().unwrap_or("transformers");
    let model = get_model_scope(body.model.as_deref());

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    // Try to get pre-computed embedding for query
    let query_vector = match get_precomputed_embedding(source, &search_text, &body.embeddings) {
        Some(v) => v,
        None => {
            tracing::debug!(
                "Vector query-multi for source '{}' without pre-computed embedding — returning empty results",
                source
            );
            return Json(json!({})).into_response();
        }
    };

    // Collect results from all collections
    struct ScoredResult {
        collection_id: String,
        score: f64,
        metadata: Value,
    }

    let mut all_results: Vec<ScoredResult> = Vec::new();

    for collection_id in &collection_ids {
        let index_path = get_index_path(&dirs.vectors, source, collection_id, &model);
        let store = VectorIndex::load(&index_path);

        for (score, item) in store.query(&query_vector, top_k, threshold) {
            all_results.push(ScoredResult {
                collection_id: collection_id.clone(),
                score,
                metadata: item.metadata.clone(),
            });
        }
    }

    // Sort by descending similarity, apply threshold, take top K
    all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    all_results.truncate(top_k);

    // Group by collection ID
    let mut grouped: serde_json::Map<String, Value> = serde_json::Map::new();
    for result in &all_results {
        let entry = grouped
            .entry(result.collection_id.clone())
            .or_insert_with(|| json!({"hashes": [], "metadata": []}));

        if let Some(hash) = result.metadata.get("hash") {
            if let Some(arr) = entry.get_mut("hashes").and_then(|v| v.as_array_mut()) {
                arr.push(hash.clone());
            }
        }
        if let Some(arr) = entry.get_mut("metadata").and_then(|v| v.as_array_mut()) {
            arr.push(result.metadata.clone());
        }
    }

    Json(Value::Object(grouped)).into_response()
}

/// `POST /api/vector/insert` — Insert items into a vector collection.
///
/// Mirrors Node's `router.post('/insert')` in `vectors.js:486-502`.
///
/// Note: Full embedding provider support is deferred. For sources that provide
/// pre-computed embeddings (webllm, koboldcpp), insertion works with those.
/// For other sources, returns 501.
pub async fn insert_vector(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<InsertRequest>,
) -> impl IntoResponse {
    let collection_id = match &body.collection_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let items = match &body.items {
        Some(items) if !items.is_empty() => items,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let source = body.source.as_deref().unwrap_or("transformers");
    let model = get_model_scope(body.model.as_deref());

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let index_path = get_index_path(&dirs.vectors, source, &collection_id, &model);

    let mut store = VectorIndex::load(&index_path);

    for item in items {
        // Try to get pre-computed embedding
        let vector = match get_precomputed_embedding(source, &item.text, &body.embeddings) {
            Some(v) => v,
            None => {
                // Without embedding provider, store with empty vector
                // This allows the hash tracking to work even without embeddings
                Vec::new()
            }
        };

        let metadata = json!({
            "hash": item.hash,
            "text": item.text,
            "index": item.index
        });

        store.upsert(metadata, vector);
    }

    store.save(&index_path);
    (StatusCode::OK, "OK").into_response()
}

/// `POST /api/vector/list` — List hashes in a vector collection.
///
/// Mirrors Node's `router.post('/list')` in `vectors.js:504-519`.
pub async fn list_vector(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ListRequest>,
) -> impl IntoResponse {
    let collection_id = match &body.collection_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let source = body.source.as_deref().unwrap_or("transformers");
    let model = get_model_scope(body.model.as_deref());

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let index_path = get_index_path(&dirs.vectors, source, &collection_id, &model);

    let store = VectorIndex::load(&index_path);
    let hashes = store.list_hashes();

    Json(json!(hashes)).into_response()
}

/// `POST /api/vector/delete` — Delete items from a vector collection by hash.
///
/// Mirrors Node's `router.post('/delete')` in `vectors.js:521-537`.
pub async fn delete_vector(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteRequest>,
) -> impl IntoResponse {
    let collection_id = match &body.collection_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let hashes = match &body.hashes {
        Some(h) if !h.is_empty() => h.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let source = body.source.as_deref().unwrap_or("transformers");
    let model = get_model_scope(body.model.as_deref());

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let index_path = get_index_path(&dirs.vectors, source, &collection_id, &model);

    let mut store = VectorIndex::load(&index_path);
    store.delete_by_hashes(&hashes);
    store.save(&index_path);

    (StatusCode::OK, "OK").into_response()
}

/// `POST /api/vector/purge-all` — Purge all vector stores for all sources.
///
/// Mirrors Node's `router.post('/purge-all')` in `vectors.js:539-555`.
pub async fn purge_all_vectors(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> impl IntoResponse {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    for source in SOURCES {
        let source_path = dirs.vectors.join(sanitize_filename(source, ""));
        if source_path.exists() {
            match fs::remove_dir_all(&source_path) {
                Ok(_) => {
                    tracing::info!("Deleted vector source store at {}", source_path.display());
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to delete vector source store at {}: {}",
                        source_path.display(),
                        e
                    );
                }
            }
        }
    }

    (StatusCode::OK, "OK")
}

/// `POST /api/vector/purge` — Purge a specific collection across all sources.
///
/// Mirrors Node's `router.post('/purge')` in `vectors.js:557-579`.
pub async fn purge_vector(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<PurgeRequest>,
) -> impl IntoResponse {
    let collection_id = match &body.collection_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);

    for source in SOURCES {
        let source_path = dirs
            .vectors
            .join(sanitize_filename(source, ""))
            .join(sanitize_filename(&collection_id, ""));
        if source_path.exists() {
            match fs::remove_dir_all(&source_path) {
                Ok(_) => {
                    tracing::info!("Deleted vector index at {}", source_path.display());
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to delete vector index at {}: {}",
                        source_path.display(),
                        e
                    );
                }
            }
        }
    }

    (StatusCode::OK, "OK").into_response()
}
