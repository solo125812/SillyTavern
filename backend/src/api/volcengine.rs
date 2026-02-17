//! Volcengine TTS endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/volcengine.js`](../../../src/endpoints/volcengine.js).
//!
//! ## Endpoints
//! - `POST /api/volcengine/generate-voice` — Generate speech using Volcengine TTS.

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

const SECRET_VOLCENGINE_APP_ID: &str = "volcengine_app_id";
const SECRET_VOLCENGINE_ACCESS_KEY: &str = "volcengine_access_key";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct VolcengineGenerateVoiceRequest {
    pub text: Option<String>,
    pub provider_endpoint: Option<String>,
    pub speaker: Option<String>,
    pub speed: Option<f64>,
    pub volume: Option<f64>,
    pub pitch: Option<f64>,
    pub audio_type: Option<String>,
    pub language: Option<String>,
    pub emotion: Option<String>,
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

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/volcengine/generate-voice` — Generate speech using Volcengine TTS.
///
/// Mirrors Node's `router.post('/generate-voice')` in `volcengine.js:14-133`.
///
/// The Volcengine TTS API returns NDJSON with base64 audio chunks.
/// We collect all chunks and return the concatenated audio.
pub async fn generate_voice(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<VolcengineGenerateVoiceRequest>,
) -> Response {
    let app_id = match read_secret(&state.config.data_root, &user.handle, SECRET_VOLCENGINE_APP_ID) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("Volcengine App ID is not configured.");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let access_key = match read_secret(&state.config.data_root, &user.handle, SECRET_VOLCENGINE_ACCESS_KEY) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("Volcengine Access Key is not configured.");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let provider_endpoint = match &body.provider_endpoint {
        Some(ep) if !ep.is_empty() => ep.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No provider endpoint"}))).into_response();
        }
    };

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No text"}))).into_response();
        }
    };

    let speaker = body.speaker.clone().unwrap_or_default();
    let speed_ratio = body.speed.unwrap_or(1.0);
    let volume_ratio = body.volume.unwrap_or(1.0);
    let pitch_ratio = body.pitch.unwrap_or(1.0);
    let audio_type = body.audio_type.clone().unwrap_or_else(|| "mp3".to_string());
    let language = body.language.clone().unwrap_or_default();
    let emotion = body.emotion.clone().unwrap_or_default();

    let mut additions = json!({
        "frontend_type": "unitTson",
    });

    if !language.is_empty() {
        additions["language"] = json!({
            "language_type": language,
        });
    }

    if !emotion.is_empty() {
        additions["emotion"] = json!(emotion);
    }

    // Add cache config
    additions["cache_config"] = json!({
        "cache_switch": "off",
    });

    let request_body = json!({
        "req_params": {
            "text": text,
            "speaker": speaker,
            "audio_params": {
                "format": audio_type,
                "speech_rate": speed_ratio,
                "volume": volume_ratio,
                "pitch": pitch_ratio,
            },
            "additions": serde_json::to_string(&additions).unwrap_or_default(),
        },
    });

    let client = http_client();

    match client
        .post(&provider_endpoint)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer;{}", access_key))
        .header("Resource-Id", &app_id)
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Volcengine TTS error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            // Response is NDJSON — each line is a JSON object with `audio` field (base64)
            let response_text = match resp.text().await {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("Volcengine TTS read error: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };

            let mut audio_chunks: Vec<Vec<u8>> = Vec::new();

            for line in response_text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(json_val) = serde_json::from_str::<Value>(line) {
                    if let Some(audio_b64) = json_val.get("audio").and_then(|v| v.as_str()) {
                        if !audio_b64.is_empty() {
                            if let Ok(decoded) = base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                audio_b64,
                            ) {
                                audio_chunks.push(decoded);
                            }
                        }
                    }
                }
            }

            if audio_chunks.is_empty() {
                tracing::warn!("Volcengine TTS returned no audio data");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            let total_len: usize = audio_chunks.iter().map(|c| c.len()).sum();
            let mut audio_data = Vec::with_capacity(total_len);
            for chunk in audio_chunks {
                audio_data.extend_from_slice(&chunk);
            }

            let content_type = match audio_type.as_str() {
                "wav" => "audio/wav",
                "ogg" | "ogg_opus" => "audio/ogg",
                _ => "audio/mpeg",
            };

            axum::response::Response::builder()
                .header("Content-Type", content_type)
                .body(axum::body::Body::from(audio_data))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => {
            tracing::error!("Volcengine TTS request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
