//! MiniMax TTS endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/minimax.js`](../../../src/endpoints/minimax.js).
//!
//! ## Endpoints
//! - `POST /api/minimax/generate-voice` — Generate speech using MiniMax TTS.

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

const SECRET_MINIMAX: &str = "api_key_minimax";
const SECRET_MINIMAX_GROUP_ID: &str = "minimax_group_id";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MinimaxGenerateVoiceRequest {
    pub text: Option<String>,
    pub voice_id: Option<String>,
    pub speed: Option<f64>,
    pub vol: Option<f64>,
    pub pitch: Option<f64>,
    pub audio_sample_rate: Option<u32>,
    pub bitrate: Option<u32>,
    pub model: Option<String>,
    pub format: Option<String>,
    pub language_boost: Option<String>,
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

/// Get MIME type for audio format.
fn get_audio_mime_type(format: &str) -> &str {
    match format {
        "wav" => "audio/wav",
        "pcm" => "audio/x-pcm",
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "hex" => "audio/mpeg", // hex returns mp3 data encoded as hex
        _ => "audio/mpeg",
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/minimax/generate-voice` — Generate speech using MiniMax TTS.
///
/// Mirrors Node's `router.post('/generate-voice')` in `minimax.js:25-228`.
pub async fn generate_voice(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<MinimaxGenerateVoiceRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_MINIMAX) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("MiniMax API key is not configured.");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let group_id = read_secret(&state.config.data_root, &user.handle, SECRET_MINIMAX_GROUP_ID)
        .unwrap_or_default();

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No text"}))).into_response();
        }
    };

    let voice_id = body.voice_id.clone().unwrap_or_else(|| "male-qn-qingse".to_string());
    let speed = body.speed.unwrap_or(1.0);
    let vol = body.vol.unwrap_or(1.0);
    let pitch = body.pitch.unwrap_or(0.0);
    let audio_sample_rate = body.audio_sample_rate.unwrap_or(32000);
    let bitrate = body.bitrate.unwrap_or(128000);
    let model = body.model.clone().unwrap_or_else(|| "speech-01-turbo".to_string());
    let format = body.format.clone().unwrap_or_else(|| "mp3".to_string());

    let api_url = format!(
        "https://api.minimax.chat/v1/t2a_v2?GroupId={}",
        group_id
    );

    let mut request_body = json!({
        "model": model,
        "text": text,
        "timber_weights": [
            {
                "voice_id": voice_id,
                "weight": 1,
            }
        ],
        "voice_setting": {
            "voice_id": voice_id,
            "speed": speed,
            "vol": vol,
            "pitch": pitch,
        },
        "audio_setting": {
            "sample_rate": audio_sample_rate,
            "bitrate": bitrate,
            "format": format,
        },
    });

    // Add language_boost if provided
    if let Some(lang) = &body.language_boost {
        if !lang.is_empty() {
            request_body["language_boost"] = json!(lang);
        }
    }

    tracing::debug!("MiniMax TTS Request: voice_id={}, model={}, format={}", voice_id, model, format);

    let client = http_client();

    match client
        .post(&api_url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("MiniMax TTS error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            // The response can contain audio data in different ways:
            // 1. For "hex" format: JSON with data.audio.data field (hex-encoded audio)
            // 2. For "url" format: JSON with data.audio.audio_url field
            // 3. For other formats: JSON with base_resp and data
            match resp.json::<Value>().await {
                Ok(json_val) => {
                    // Check for errors in response
                    if let Some(base_resp) = json_val.get("base_resp") {
                        let status_code = base_resp.get("status_code").and_then(|v| v.as_i64()).unwrap_or(0);
                        if status_code != 0 {
                            let msg = base_resp.get("status_msg").and_then(|v| v.as_str()).unwrap_or("Unknown error");
                            tracing::error!("MiniMax TTS API error: {}", msg);
                            return (StatusCode::INTERNAL_SERVER_ERROR, msg.to_string()).into_response();
                        }
                    }

                    // Try to get audio data from hex encoding
                    if let Some(audio_hex) = json_val
                        .get("data")
                        .and_then(|d| d.get("audio"))
                        .and_then(|a| a.get("data"))
                        .and_then(|v| v.as_str())
                    {
                        // Decode hex to bytes
                        let bytes: Result<Vec<u8>, _> = (0..audio_hex.len())
                            .step_by(2)
                            .map(|i| u8::from_str_radix(&audio_hex[i..i + 2], 16))
                            .collect();

                        match bytes {
                            Ok(audio_data) => {
                                let content_type = get_audio_mime_type(&format);
                                return axum::response::Response::builder()
                                    .header("Content-Type", content_type)
                                    .body(axum::body::Body::from(audio_data))
                                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
                            }
                            Err(e) => {
                                tracing::error!("MiniMax hex decode error: {}", e);
                                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                            }
                        }
                    }

                    // Try to get audio URL
                    if let Some(audio_url) = json_val
                        .get("data")
                        .and_then(|d| d.get("audio"))
                        .and_then(|a| a.get("audio_url"))
                        .and_then(|v| v.as_str())
                    {
                        // Fetch audio from URL
                        match client.get(audio_url).send().await {
                            Ok(audio_resp) => {
                                if audio_resp.status().is_success() {
                                    match audio_resp.bytes().await {
                                        Ok(bytes) => {
                                            let content_type = get_audio_mime_type(&format);
                                            return axum::response::Response::builder()
                                                .header("Content-Type", content_type)
                                                .body(axum::body::Body::from(bytes.to_vec()))
                                                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
                                        }
                                        Err(e) => {
                                            tracing::error!("MiniMax audio download error: {}", e);
                                            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("MiniMax audio URL fetch error: {}", e);
                                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                            }
                        }
                    }

                    // Return the JSON response if no audio data found
                    Json(json_val).into_response()
                }
                Err(e) => {
                    tracing::error!("MiniMax TTS parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("MiniMax TTS request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
