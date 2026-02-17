//! Azure TTS endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/azure.js`](../../../src/endpoints/azure.js).
//!
//! ## Endpoints
//! - `POST /api/azure/list`     — List available Azure TTS voices.
//! - `POST /api/azure/generate` — Generate speech using Azure TTS.

use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::Value;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SECRET_AZURE_TTS: &str = "api_key_azure_tts";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AzureListRequest {
    pub region: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AzureGenerateRequest {
    pub text: Option<String>,
    pub region: Option<String>,
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
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/azure/list` — List available Azure TTS voices for a region.
///
/// Mirrors Node's `router.post('/list')` in `azure.js:12-40`.
pub async fn list_voices(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<AzureListRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_AZURE_TTS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("Azure TTS API key is not configured.");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    let region = match &body.region {
        Some(r) if !r.is_empty() => r.clone(),
        _ => {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let url = format!(
        "https://{}.tts.speech.microsoft.com/cognitiveservices/voices/list",
        region
    );

    let client = http_client();

    match client
        .get(&url)
        .header("Ocp-Apim-Subscription-Key", &api_key)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Azure TTS list error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(json_val) => Json(json_val).into_response(),
                Err(e) => {
                    tracing::error!("Azure TTS list parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Azure TTS list request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/azure/generate` — Generate speech using Azure TTS (SSML).
///
/// Mirrors Node's `router.post('/generate')` in `azure.js:42-87`.
pub async fn generate_voice(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<AzureGenerateRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_AZURE_TTS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("Azure TTS API key is not configured.");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    let region = match &body.region {
        Some(r) if !r.is_empty() => r.clone(),
        _ => {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let url = format!(
        "https://{}.tts.speech.microsoft.com/cognitiveservices/v1",
        region
    );

    let client = http_client();

    match client
        .post(&url)
        .header("Ocp-Apim-Subscription-Key", &api_key)
        .header("Content-Type", "application/ssml+xml")
        .header("X-Microsoft-OutputFormat", "audio-16khz-128kbitrate-mono-mp3")
        .body(text)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Azure TTS generate error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.bytes().await {
                Ok(bytes) => {
                    let mut response = bytes.to_vec().into_response();
                    response.headers_mut().insert(
                        "Content-Type",
                        "audio/mpeg".parse().unwrap(),
                    );
                    response
                }
                Err(e) => {
                    tracing::error!("Azure TTS generate read error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Azure TTS generate request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
