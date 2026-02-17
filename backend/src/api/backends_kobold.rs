//! KoboldAI backend endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/backends/kobold.js`](../../../../src/endpoints/backends/kobold.js).
//!
//! ## Endpoints
//! - `POST /api/backends/kobold/generate`         — Generate text with KoboldAI.
//! - `POST /api/backends/kobold/status`            — Get KoboldAI model status.
//! - `POST /api/backends/kobold/transcribe-audio`  — KoboldCpp STT.
//! - `POST /api/backends/kobold/embed`             — KoboldCpp embeddings.

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

const SECRET_KOBOLDCPP: &str = "api_key_koboldcpp";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct KoboldGenerateRequest {
    pub api_server: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Deserialize)]
pub struct KoboldStatusRequest {
    pub api_server: Option<String>,
    pub legacy_api: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct KoboldTranscribeRequest {
    pub api_server: Option<String>,
    pub audio: Option<String>,
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct KoboldEmbedRequest {
    pub api_server: Option<String>,
    pub text: Option<Value>,
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
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/backends/kobold/generate` — Generate text with KoboldAI.
///
/// Mirrors Node's `router.post('/generate')` in `kobold.js:28-131`.
pub async fn generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_server = body.get("api_server").and_then(|v| v.as_str()).unwrap_or("");
    if api_server.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "No api_server"}))).into_response();
    }

    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_KOBOLDCPP)
        .unwrap_or_default();

    let url = format!("{}/api/v1/generate", api_server.trim_end_matches('/'));
    let client = http_client();

    // Build the generation request body (pass through most fields)
    let mut request_body = body.clone();
    // Remove api_server from the body sent to KoboldAI
    if let Some(obj) = request_body.as_object_mut() {
        obj.remove("api_server");
    }

    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request_body);

    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    // Retry logic: try up to 2 times
    for attempt in 0..2 {
        let req_clone = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request_body);

        let req_with_auth = if !api_key.is_empty() {
            req_clone.header("Authorization", format!("Bearer {}", api_key))
        } else {
            req_clone
        };

        match req_with_auth.send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<Value>().await {
                        Ok(data) => return Json(data).into_response(),
                        Err(e) => {
                            tracing::error!("Kobold generate parse error: {}", e);
                            if attempt == 1 {
                                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                            }
                        }
                    }
                } else {
                    let status = resp.status().as_u16();
                    let error_text = resp.text().await.unwrap_or_default();
                    tracing::error!("Kobold generate error ({}): {}", status, error_text);
                    if attempt == 1 {
                        return (
                            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                            error_text,
                        )
                            .into_response();
                    }
                }
            }
            Err(e) => {
                tracing::error!("Kobold generate request error (attempt {}): {}", attempt + 1, e);
                if attempt == 1 {
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }

        // Wait before retry
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    let _ = req;
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// `POST /api/backends/kobold/status` — Get KoboldAI model status.
///
/// Mirrors Node's `router.post('/status')` in `kobold.js:133-210`.
pub async fn status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<KoboldStatusRequest>,
) -> Response {
    let api_server = match &body.api_server {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No api_server"}))).into_response();
        }
    };

    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_KOBOLDCPP)
        .unwrap_or_default();

    let client = http_client();

    // Try multiple endpoints to get model info
    let base_url = api_server.trim_end_matches('/');

    // Try KoboldCpp united endpoint
    let united_url = format!("{}/api/extra/true_max_context_length", base_url);
    let extra_url = format!("{}/api/extra/version", base_url);
    let model_url = format!("{}/api/v1/model", base_url);

    let mut result = json!({});

    // Fetch model info
    let mut req = client.get(&model_url);
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    if let Ok(resp) = req.send().await {
        if resp.status().is_success() {
            if let Ok(data) = resp.json::<Value>().await {
                result["model"] = data;
            }
        }
    }

    // Fetch extra version info
    let mut req = client.get(&extra_url);
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    if let Ok(resp) = req.send().await {
        if resp.status().is_success() {
            if let Ok(data) = resp.json::<Value>().await {
                result["extra"] = data;
            }
        }
    }

    // Fetch true max context length
    let mut req = client.get(&united_url);
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    if let Ok(resp) = req.send().await {
        if resp.status().is_success() {
            if let Ok(data) = resp.json::<Value>().await {
                result["united"] = data;
            }
        }
    }

    Json(result).into_response()
}

/// `POST /api/backends/kobold/transcribe-audio` — KoboldCpp STT.
///
/// Mirrors Node's `router.post('/transcribe-audio')` in `kobold.js:212-243`.
pub async fn transcribe_audio(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<KoboldTranscribeRequest>,
) -> Response {
    let api_server = match &body.api_server {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_KOBOLDCPP)
        .unwrap_or_default();

    let audio = match &body.audio {
        Some(a) if !a.is_empty() => a.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let url = format!("{}/api/extra/transcribe", api_server.trim_end_matches('/'));

    let request_body = json!({
        "prompt": audio,
    });

    let client = http_client();

    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request_body);

    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    match req.send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("Kobold transcribe error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/backends/kobold/embed` — KoboldCpp embeddings.
///
/// Mirrors Node's `router.post('/embed')` in `kobold.js:245-280`.
pub async fn embed(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<KoboldEmbedRequest>,
) -> Response {
    let api_server = match &body.api_server {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_KOBOLDCPP)
        .unwrap_or_default();

    let text = match &body.text {
        Some(t) => t.clone(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let url = format!("{}/api/extra/generate/check/embeddings", api_server.trim_end_matches('/'));

    let request_body = json!({
        "prompt": text,
    });

    let client = http_client();

    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request_body);

    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    match req.send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("Kobold embed error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
