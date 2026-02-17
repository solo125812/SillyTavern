//! OpenRouter endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/openrouter.js`](../../../src/endpoints/openrouter.js).
//!
//! ## Endpoints
//! - `POST /api/openrouter/models/providers` — Get model endpoint providers.
//! - `POST /api/openrouter/models/multimodal` — Get image→text model IDs.
//! - `POST /api/openrouter/models/embedding` — Get embedding models.
//! - `POST /api/openrouter/models/image` — Get text→image models.
//! - `POST /api/openrouter/image/generate` — Generate image via chat completions.

use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SECRET_OPENROUTER: &str = "api_key_openrouter";
const API_OPENROUTER: &str = "https://openrouter.ai/api/v1";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ModelProvidersRequest {
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ImageGenerateRequest {
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub negative_prompt: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub num_inference_steps: Option<u32>,
    pub guidance_scale: Option<f64>,
    pub seed: Option<i64>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_secret(data_root: &std::path::Path, handle: &str, key: &str) -> Option<String> {
    let dirs = UserDirectories::new(data_root, handle);
    let secrets_path = dirs.root.join("secrets.json");
    let content = std::fs::read_to_string(&secrets_path).ok()?;
    let secrets: serde_json::Map<String, Value> = serde_json::from_str(&content).ok()?;
    match secrets.get(key)? {
        Value::Array(arr) => {
            for item in arr {
                if item.get("active").and_then(|v| v.as_bool()).unwrap_or(false) {
                    return item.get("value").and_then(|v| v.as_str()).map(|s| s.to_string());
                }
            }
            arr.first()
                .and_then(|item| item.get("value").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
        }
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_default()
}

/// Fetch models filtered by modality.
async fn fetch_models_by_modality(
    api_key: &str,
    endpoint: &str,
    input_modality: &str,
    output_modality: &str,
    ids_only: bool,
) -> Response {
    let url = format!("{}{}", API_OPENROUTER, endpoint);
    let client = http_client();

    let mut req_builder = client.get(&url);
    if !api_key.is_empty() {
        req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
    }

    match req_builder.send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let models = data.get("data").and_then(|d| d.as_array());
                    let filtered: Vec<Value> = match models {
                        Some(models) => models
                            .iter()
                            .filter(|m| {
                                let input_ok = input_modality.is_empty()
                                    || m.get("supported_parameters")
                                        .and_then(|sp| sp.get("supported_input_modalities"))
                                        .and_then(|v| v.as_array())
                                        .map(|arr| arr.iter().any(|v| v.as_str() == Some(input_modality)))
                                        .unwrap_or(false)
                                    || m.get("architecture")
                                        .and_then(|a| a.get("input_modalities"))
                                        .and_then(|v| v.as_array())
                                        .map(|arr| arr.iter().any(|v| v.as_str() == Some(input_modality)))
                                        .unwrap_or(false);

                                let output_ok = output_modality.is_empty()
                                    || m.get("supported_parameters")
                                        .and_then(|sp| sp.get("supported_output_modalities"))
                                        .and_then(|v| v.as_array())
                                        .map(|arr| arr.iter().any(|v| v.as_str() == Some(output_modality)))
                                        .unwrap_or(false)
                                    || m.get("architecture")
                                        .and_then(|a| a.get("output_modalities"))
                                        .and_then(|v| v.as_array())
                                        .map(|arr| arr.iter().any(|v| v.as_str() == Some(output_modality)))
                                        .unwrap_or(false);

                                input_ok && output_ok
                            })
                            .cloned()
                            .collect(),
                        None => Vec::new(),
                    };

                    if ids_only {
                        let ids: Vec<Value> = filtered
                            .iter()
                            .filter_map(|m| m.get("id").cloned())
                            .collect();
                        Json(ids).into_response()
                    } else {
                        Json(filtered).into_response()
                    }
                }
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/openrouter/models/providers` — Get endpoint providers for a model.
///
/// Mirrors Node's `router.post('/models/providers')` in `openrouter.js:8-36`.
pub async fn models_providers(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ModelProvidersRequest>,
) -> Response {
    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_OPENROUTER)
        .unwrap_or_default();

    let model = match &body.model {
        Some(m) if !m.is_empty() => m.clone(),
        _ => {
            return Json(json!([])).into_response();
        }
    };

    let url = format!("{}/models/{}/endpoints", API_OPENROUTER, model);
    let client = http_client();

    match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let endpoints = data.get("data").and_then(|d| d.get("endpoints"));
                    match endpoints {
                        Some(ep) => Json(ep.clone()).into_response(),
                        None => Json(json!([])).into_response(),
                    }
                }
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}

/// `POST /api/openrouter/models/multimodal` — Get image→text model IDs.
///
/// Mirrors Node's `router.post('/models/multimodal')` in `openrouter.js:38-74`.
pub async fn models_multimodal(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_OPENROUTER)
        .unwrap_or_default();

    fetch_models_by_modality(&api_key, "/models", "image", "text", true).await
}

/// `POST /api/openrouter/models/embedding` — Get embedding models.
///
/// Mirrors Node's `router.post('/models/embedding')` in `openrouter.js:76-96`.
pub async fn models_embedding(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_OPENROUTER)
        .unwrap_or_default();

    fetch_models_by_modality(&api_key, "/models", "text", "embedding", false).await
}

/// `POST /api/openrouter/models/image` — Get text→image models.
///
/// Mirrors Node's `router.post('/models/image')` in `openrouter.js:98-115`.
pub async fn models_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_OPENROUTER)
        .unwrap_or_default();

    fetch_models_by_modality(&api_key, "/models", "text", "image", true).await
}

/// `POST /api/openrouter/image/generate` — Generate an image via chat completions.
///
/// Mirrors Node's `router.post('/image/generate')` in `openrouter.js:117-172`.
pub async fn image_generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ImageGenerateRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_OPENROUTER) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("OpenRouter API key is not configured.");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let model = match &body.model {
        Some(m) if !m.is_empty() => m.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No model specified"}))).into_response();
        }
    };

    let prompt = body.prompt.clone().unwrap_or_default();
    let negative_prompt = body.negative_prompt.clone().unwrap_or_default();

    // Build the prompt text
    let mut full_prompt = prompt;
    if !negative_prompt.is_empty() {
        full_prompt = format!("{}\nNegative prompt: {}", full_prompt, negative_prompt);
    }

    let mut generation_params = json!({});
    if let Some(w) = body.width {
        generation_params["width"] = json!(w);
    }
    if let Some(h) = body.height {
        generation_params["height"] = json!(h);
    }
    if let Some(steps) = body.num_inference_steps {
        generation_params["num_inference_steps"] = json!(steps);
    }
    if let Some(gs) = body.guidance_scale {
        generation_params["guidance_scale"] = json!(gs);
    }
    if let Some(seed) = body.seed {
        generation_params["seed"] = json!(seed);
    }

    let request_body = json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": full_prompt,
                    }
                ],
            }
        ],
        "provider": {
            "sort": "throughput",
        },
        "generation_params": generation_params,
    });

    let url = format!("{}/chat/completions", API_OPENROUTER);
    let client = http_client();

    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("HTTP-Referer", "https://sillytavern.app")
        .header("X-Title", "SillyTavern")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("OpenRouter image generation error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(json_val) => {
                    // Extract image URL from response
                    let image_url = json_val
                        .get("choices")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|choice| choice.get("message"))
                        .and_then(|msg| msg.get("content"))
                        .and_then(|content| {
                            // Content could be a string with markdown image or array with image_url
                            if let Some(s) = content.as_str() {
                                // Extract URL from markdown: ![...](url)
                                if let Some(start) = s.find("](") {
                                    let rest = &s[start + 2..];
                                    if let Some(end) = rest.find(')') {
                                        return Some(rest[..end].to_string());
                                    }
                                }
                                None
                            } else if let Some(arr) = content.as_array() {
                                arr.iter()
                                    .find(|item| item.get("type").and_then(|t| t.as_str()) == Some("image_url"))
                                    .and_then(|item| item.get("image_url"))
                                    .and_then(|iu| iu.get("url"))
                                    .and_then(|u| u.as_str())
                                    .map(|s| s.to_string())
                            } else {
                                None
                            }
                        });

                    let result = json!({
                        "created": json_val.get("created"),
                        "data": [{
                            "url": image_url,
                        }],
                    });

                    Json(result).into_response()
                }
                Err(e) => {
                    tracing::error!("OpenRouter image parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("OpenRouter image request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
