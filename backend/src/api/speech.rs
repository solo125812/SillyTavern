//! Speech endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/speech.js`](../../../src/endpoints/speech.js).
//!
//! ## Endpoints
//! ### Browser (transformers-based — returns 501)
//! - `POST /api/speech/recognize`  — Speech recognition (browser/transformers).
//! - `POST /api/speech/synthesize` — Speech synthesis (browser/transformers).
//!
//! ### Pollinations
//! - `POST /api/speech/pollinations/voices`   — List Pollinations voices.
//! - `POST /api/speech/pollinations/generate`  — Generate speech via Pollinations.
//!
//! ### ElevenLabs
//! - `POST /api/speech/elevenlabs/voices`        — List ElevenLabs voices.
//! - `POST /api/speech/elevenlabs/voice-settings` — Get default voice settings.
//! - `POST /api/speech/elevenlabs/synthesize`     — Synthesize speech.
//! - `POST /api/speech/elevenlabs/history`        — Get generation history.
//! - `POST /api/speech/elevenlabs/history-audio`  — Get audio from history.
//! - `POST /api/speech/elevenlabs/voices/add`     — Upload a custom voice.
//! - `POST /api/speech/elevenlabs/recognize`      — Speech-to-text.

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

const SECRET_ELEVENLABS: &str = "api_key_elevenlabs";
const SECRET_POLLINATIONS: &str = "api_key_pollinations";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RecognizeRequest {
    pub model: Option<String>,
    pub audio: Option<String>,
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SynthesizeRequest {
    pub text: Option<String>,
    pub model: Option<String>,
    pub speaker: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PollinationsVoicesRequest {
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PollinationsGenerateRequest {
    pub text: Option<String>,
    pub model: Option<String>,
    pub voice: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ElevenLabsSynthesizeRequest {
    #[serde(rename = "voiceId")]
    pub voice_id: Option<String>,
    pub request: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ElevenLabsHistoryAudioRequest {
    #[serde(rename = "historyItemId")]
    pub history_item_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ElevenLabsVoiceAddRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub labels: Option<String>,
    pub files: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ElevenLabsRecognizeRequest {
    pub model: Option<String>,
    pub audio: Option<String>,
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
// Handlers — Browser/Transformers (not supported in Rust sidecar)
// ---------------------------------------------------------------------------

/// `POST /api/speech/recognize` — Speech recognition (transformers).
///
/// Returns 501 — transformers pipeline not available in Rust sidecar.
pub async fn recognize(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(_body): Json<RecognizeRequest>,
) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "Speech recognition pipeline not available in Rust sidecar."})),
    )
        .into_response()
}

/// `POST /api/speech/synthesize` — Speech synthesis (transformers).
///
/// Returns 501 — transformers pipeline not available in Rust sidecar.
pub async fn synthesize(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(_body): Json<SynthesizeRequest>,
) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "Speech synthesis pipeline not available in Rust sidecar."})),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Handlers — Pollinations
// ---------------------------------------------------------------------------

/// `POST /api/speech/pollinations/voices` — List Pollinations TTS voices.
///
/// Mirrors Node's `pollinations.post('/voices')` in `speech.js:88-115`.
pub async fn pollinations_voices(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<PollinationsVoicesRequest>,
) -> Response {
    let model = body.model.clone().unwrap_or_else(|| "openai-audio".to_string());
    let client = http_client();

    match client.get("https://gen.pollinations.ai/text/models").send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    if let Some(arr) = data.as_array() {
                        let audio_model = arr.iter().find(|m| {
                            m.get("name").and_then(|n| n.as_str()) == Some(&model)
                        });
                        if let Some(model_data) = audio_model {
                            if let Some(voices) = model_data.get("voices") {
                                return Json(voices.clone()).into_response();
                            }
                        }
                    }
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("Pollinations voices error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/speech/pollinations/generate` — Generate speech via Pollinations.
///
/// Mirrors Node's `pollinations.post('/generate')` in `speech.js:117-173`.
pub async fn pollinations_generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<PollinationsGenerateRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_POLLINATIONS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("No API key saved for Pollinations TTS.");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let model = body.model.clone().unwrap_or_else(|| "openai-audio".to_string());
    let voice = body.voice.clone().unwrap_or_else(|| "alloy".to_string());

    tracing::debug!("Pollinations TTS request: text={}, model={}, voice={}", text, model, voice);

    // Generate a random seed
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u32;

    let request_body = json!({
        "model": model,
        "stream": false,
        "modalities": ["text", "audio"],
        "seed": seed,
        "audio": {
            "format": "mp3",
            "voice": voice,
        },
        "messages": [{
            "role": "user",
            "content": text,
        }],
    });

    let client = http_client();

    match client
        .post("https://gen.pollinations.ai/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Pollinations TTS error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let audio_data = data
                        .get("choices")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|choice| choice.get("message"))
                        .and_then(|msg| msg.get("audio"))
                        .and_then(|audio| audio.get("data"))
                        .and_then(|d| d.as_str());

                    match audio_data {
                        Some(b64) => {
                            match base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                b64,
                            ) {
                                Ok(bytes) => axum::response::Response::builder()
                                    .header("Content-Type", "audio/mpeg")
                                    .body(axum::body::Body::from(bytes))
                                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                                Err(e) => {
                                    tracing::error!("Pollinations audio decode error: {}", e);
                                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                                }
                            }
                        }
                        None => {
                            tracing::warn!("Pollinations TTS audio data is missing from the response");
                            StatusCode::INTERNAL_SERVER_ERROR.into_response()
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Pollinations TTS parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Pollinations TTS request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers — ElevenLabs
// ---------------------------------------------------------------------------

/// `POST /api/speech/elevenlabs/voices` — List ElevenLabs voices.
///
/// Mirrors Node's `elevenlabs.post('/voices')` in `speech.js:179-205`.
pub async fn elevenlabs_voices(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELEVENLABS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("ElevenLabs API key not found");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let client = http_client();

    match client
        .get("https://api.elevenlabs.io/v1/voices")
        .header("xi-api-key", &api_key)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::warn!("ElevenLabs voices fetch failed: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("ElevenLabs voices error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/speech/elevenlabs/voice-settings` — Get default voice settings.
///
/// Mirrors Node's `elevenlabs.post('/voice-settings')` in `speech.js:207-232`.
pub async fn elevenlabs_voice_settings(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELEVENLABS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("ElevenLabs API key not found");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let client = http_client();

    match client
        .get("https://api.elevenlabs.io/v1/voices/settings/default")
        .header("xi-api-key", &api_key)
        .send()
        .await
    {
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
            tracing::error!("ElevenLabs voice-settings error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/speech/elevenlabs/synthesize` — Synthesize speech.
///
/// Mirrors Node's `elevenlabs.post('/synthesize')` in `speech.js:234-272`.
pub async fn elevenlabs_synthesize(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ElevenLabsSynthesizeRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELEVENLABS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("ElevenLabs API key not found");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let voice_id = match &body.voice_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            tracing::warn!("ElevenLabs synthesis missing voiceId");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let request_body = match &body.request {
        Some(req) => req.clone(),
        None => {
            tracing::warn!("ElevenLabs synthesis missing request body");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    tracing::debug!("ElevenLabs TTS request for voice: {}", voice_id);

    let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{}", voice_id);
    let client = http_client();

    match client
        .post(&url)
        .header("xi-api-key", &api_key)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::warn!("ElevenLabs synthesis failed: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            // Stream the response back
            match resp.bytes().await {
                Ok(bytes) => axum::response::Response::builder()
                    .header("Content-Type", "audio/mpeg")
                    .body(axum::body::Body::from(bytes.to_vec()))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                Err(e) => {
                    tracing::error!("ElevenLabs synthesize read error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("ElevenLabs synthesize request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/speech/elevenlabs/history` — Get generation history.
///
/// Mirrors Node's `elevenlabs.post('/history')` in `speech.js:274-300`.
pub async fn elevenlabs_history(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELEVENLABS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("ElevenLabs API key not found");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let client = http_client();

    match client
        .get("https://api.elevenlabs.io/v1/history")
        .header("xi-api-key", &api_key)
        .send()
        .await
    {
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
            tracing::error!("ElevenLabs history error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/speech/elevenlabs/history-audio` — Get audio from history.
///
/// Mirrors Node's `elevenlabs.post('/history-audio')` in `speech.js:302-336`.
pub async fn elevenlabs_history_audio(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ElevenLabsHistoryAudioRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELEVENLABS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("ElevenLabs API key not found");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let history_item_id = match &body.history_item_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let url = format!(
        "https://api.elevenlabs.io/v1/history/{}/audio",
        history_item_id
    );
    let client = http_client();

    match client
        .get(&url)
        .header("xi-api-key", &api_key)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.bytes().await {
                Ok(bytes) => axum::response::Response::builder()
                    .header("Content-Type", "audio/mpeg")
                    .body(axum::body::Body::from(bytes.to_vec()))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                Err(e) => {
                    tracing::error!("ElevenLabs history-audio read error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("ElevenLabs history-audio request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/speech/elevenlabs/voices/add` — Upload a custom voice.
///
/// Mirrors Node's `elevenlabs.post('/voices/add')` in `speech.js:338-388`.
pub async fn elevenlabs_voices_add(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ElevenLabsVoiceAddRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELEVENLABS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("ElevenLabs API key not found");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let name = body.name.clone().unwrap_or_else(|| "Custom Voice".to_string());
    let description = body.description.clone().unwrap_or_else(|| "Uploaded via SillyTavern".to_string());
    let labels = body.labels.clone().unwrap_or_default();

    // Build multipart form
    let mut form = reqwest::multipart::Form::new()
        .text("name", name)
        .text("description", description)
        .text("labels", labels);

    if let Some(files) = &body.files {
        for (idx, file_data) in files.iter().enumerate() {
            // Parse data URI: data:mime;base64,data
            let re = regex::Regex::new(r"^data:(.+);base64,(.+)$").unwrap();
            if let Some(caps) = re.captures(file_data) {
                let mime_type = caps.get(1).map(|m| m.as_str()).unwrap_or("audio/wav");
                let b64_data = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                if let Ok(decoded) = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    b64_data,
                ) {
                    let ext = match mime_type {
                        "audio/wav" | "audio/wave" => "wav",
                        "audio/mpeg" | "audio/mp3" => "mp3",
                        "audio/ogg" => "ogg",
                        _ => "wav",
                    };
                    let filename = format!("audio_{}.{}", idx, ext);
                    let part = reqwest::multipart::Part::bytes(decoded)
                        .file_name(filename)
                        .mime_str(mime_type)
                        .unwrap_or_else(|_| reqwest::multipart::Part::bytes(vec![]));
                    form = form.part("files", part);
                }
            }
        }
    }

    let client = http_client();

    match client
        .post("https://api.elevenlabs.io/v1/voices/add")
        .header("xi-api-key", &api_key)
        .multipart(form)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::warn!("ElevenLabs voice upload failed: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("ElevenLabs voice upload error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/speech/elevenlabs/recognize` — Speech-to-text.
///
/// Mirrors Node's `elevenlabs.post('/recognize')` in `speech.js:390-430`.
pub async fn elevenlabs_recognize(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ElevenLabsRecognizeRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELEVENLABS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("ElevenLabs API key not found");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let audio = match &body.audio {
        Some(a) if !a.is_empty() => a.clone(),
        _ => {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let model_id = body.model.clone().unwrap_or_else(|| "scribe_v1".to_string());

    // Decode audio from base64 data URI
    let audio_bytes = if let Some(rest) = audio.strip_prefix("data:") {
        if let Some((_mime, data)) = rest.split_once(";base64,") {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
                .unwrap_or_default()
        } else {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &audio)
                .unwrap_or_default()
        }
    } else {
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &audio)
            .unwrap_or_default()
    };

    if audio_bytes.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let part = reqwest::multipart::Part::bytes(audio_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .unwrap_or_else(|_| reqwest::multipart::Part::bytes(vec![]));

    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model_id", model_id);

    let client = http_client();

    match client
        .post("https://api.elevenlabs.io/v1/speech-to-text")
        .header("xi-api-key", &api_key)
        .multipart(form)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::warn!("ElevenLabs STT failed: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("ElevenLabs STT error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
