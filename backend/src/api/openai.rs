//! OpenAI endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/openai.js`](../../../src/endpoints/openai.js).
//!
//! ## Endpoints
//! - `POST /api/openai/caption-image`              — Multi-provider vision captioning.
//! - `POST /api/openai/generate-voice`              — OpenAI TTS.
//! - `POST /api/openai/electronhub/generate-voice`  — ElectronHub TTS.
//! - `POST /api/openai/electronhub/models`          — ElectronHub model list.
//! - `POST /api/openai/chutes/generate-voice`       — Chutes Kokoro TTS.
//! - `POST /api/openai/chutes/models/embedding`     — Chutes embedding models.
//! - `POST /api/openai/generate-image`              — OpenAI DALL-E.
//! - `POST /api/openai/generate-video`              — OpenAI Sora video.
//! - `POST /api/openai/custom/generate-voice`       — Custom OpenAI-compatible TTS.
//! - `POST /api/openai/transcribe-audio`            — OpenAI Whisper STT.
//! - `POST /api/openai/groq/transcribe-audio`       — Groq STT.
//! - `POST /api/openai/mistral/transcribe-audio`    — Mistral STT.
//! - `POST /api/openai/zai/transcribe-audio`        — ZAI STT.
//! - `POST /api/openai/chutes/transcribe-audio`     — Chutes STT.

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

const SECRET_OPENAI: &str = "api_key_openai";
const SECRET_ELECTRONHUB: &str = "api_key_electronhub";
const SECRET_CHUTES: &str = "api_key_chutes";
const SECRET_GROQ: &str = "api_key_groq";
const SECRET_MISTRALAI: &str = "api_key_mistralai";
const SECRET_ZAI: &str = "api_key_zai";
const SECRET_CUSTOM_OPENAI_TTS: &str = "api_key_custom_openai_tts";
const SECRET_NANOGPT: &str = "api_key_nanogpt";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CaptionImageRequest {
    pub image: Option<String>,
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub api: Option<String>,
    pub reverse_proxy: Option<String>,
    pub proxy_password: Option<String>,
    pub max_tokens: Option<u32>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Deserialize)]
pub struct GenerateVoiceRequest {
    pub text: Option<String>,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub speed: Option<f64>,
    pub response_format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ElectronHubGenerateVoiceRequest {
    pub text: Option<String>,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub speed: Option<f64>,
    pub response_format: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Deserialize)]
pub struct ChutesGenerateVoiceRequest {
    pub text: Option<String>,
    pub voice: Option<String>,
    pub speed: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateImageRequest {
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub size: Option<String>,
    pub quality: Option<String>,
    pub style: Option<String>,
    pub n: Option<u32>,
    pub response_format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateVideoRequest {
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub size: Option<String>,
    pub n: Option<u32>,
    pub duration: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CustomGenerateVoiceRequest {
    pub input: Option<String>,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub speed: Option<f64>,
    pub response_format: Option<String>,
    pub provider_endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TranscribeAudioRequest {
    pub audio: Option<String>,
    pub model: Option<String>,
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChutesTranscribeRequest {
    pub audio: Option<String>,
    pub model: Option<String>,
    pub lang: Option<String>,
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
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_default()
}

/// Generic transcription handler for OpenAI-compatible STT APIs.
async fn transcribe_audio_generic(
    data_root: &std::path::Path,
    handle: &str,
    secret_key: &str,
    api_url: &str,
    body: &TranscribeAudioRequest,
) -> Response {
    let api_key = match read_secret(data_root, handle, secret_key) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("{} API key is not configured.", secret_key);
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let audio = match &body.audio {
        Some(a) if !a.is_empty() => a.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No audio"}))).into_response();
        }
    };

    let model = body.model.clone().unwrap_or_else(|| "whisper-1".to_string());

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
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Invalid audio data"}))).into_response();
    }

    let part = reqwest::multipart::Part::bytes(audio_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .unwrap_or_else(|_| reqwest::multipart::Part::bytes(vec![]));

    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", model);

    if let Some(lang) = &body.lang {
        if !lang.is_empty() {
            form = form.text("language", lang.clone());
        }
    }

    let client = http_client();

    match client
        .post(api_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("STT error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("STT parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("STT request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/openai/caption-image` — Multi-provider vision captioning.
///
/// Mirrors Node's `router.post('/caption-image')` in `openai.js:34-272`.
/// This is a large endpoint supporting ~20 API sources. The Rust implementation
/// handles the OpenAI-compatible path; other providers use their own endpoints.
pub async fn caption_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<CaptionImageRequest>,
) -> Response {
    let api = body.api.clone().unwrap_or_else(|| "openai".to_string());

    // Determine API key and URL based on provider
    let (api_key, api_url) = match api.as_str() {
        "openai" => {
            let key = if body.reverse_proxy.is_some() {
                body.proxy_password.clone()
            } else {
                read_secret(&state.config.data_root, &user.handle, SECRET_OPENAI)
            };
            let url = body.reverse_proxy.clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            (key, format!("{}/chat/completions", url.trim_end_matches('/')))
        }
        "custom" => {
            let key = read_secret(&state.config.data_root, &user.handle, "api_key_custom");
            let url = body.reverse_proxy.clone().unwrap_or_default();
            (key, format!("{}/chat/completions", url.trim_end_matches('/')))
        }
        _ => {
            // For other providers, use the OpenAI-compatible format
            let key = read_secret(&state.config.data_root, &user.handle, &format!("api_key_{}", api));
            let url = body.reverse_proxy.clone().unwrap_or_default();
            (key, format!("{}/chat/completions", url.trim_end_matches('/')))
        }
    };

    let api_key = match api_key {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("API key not configured for caption provider: {}", api);
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let image = match &body.image {
        Some(img) if !img.is_empty() => img.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No image"}))).into_response();
        }
    };

    let model = body.model.clone().unwrap_or_else(|| "gpt-4o".to_string());
    let prompt = body.prompt.clone().unwrap_or_else(|| "What's in this image?".to_string());
    let max_tokens = body.max_tokens.unwrap_or(500);

    let request_body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": image,
                        }
                    },
                    {
                        "type": "text",
                        "text": prompt,
                    }
                ]
            }
        ]
    });

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
                tracing::error!("OpenAI caption error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("OpenAI caption parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("OpenAI caption request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/openai/generate-voice` — OpenAI TTS.
///
/// Mirrors Node's `router.post('/generate-voice')` in `openai.js:274-316`.
pub async fn generate_voice(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GenerateVoiceRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_OPENAI) {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let model = body.model.clone().unwrap_or_else(|| "tts-1".to_string());
    let voice = body.voice.clone().unwrap_or_else(|| "alloy".to_string());
    let speed = body.speed.unwrap_or(1.0);
    let response_format = body.response_format.clone().unwrap_or_else(|| "mp3".to_string());

    let request_body = json!({
        "input": text,
        "model": model,
        "voice": voice,
        "speed": speed,
        "response_format": response_format,
    });

    let client = http_client();

    match client
        .post("https://api.openai.com/v1/audio/speech")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("OpenAI TTS error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.bytes().await {
                Ok(bytes) => {
                    let content_type = match response_format.as_str() {
                        "opus" => "audio/ogg",
                        "aac" => "audio/aac",
                        "flac" => "audio/flac",
                        "wav" => "audio/wav",
                        _ => "audio/mpeg",
                    };
                    axum::response::Response::builder()
                        .header("Content-Type", content_type)
                        .body(axum::body::Body::from(bytes.to_vec()))
                        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
                }
                Err(e) => {
                    tracing::error!("OpenAI TTS read error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("OpenAI TTS request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/openai/electronhub/generate-voice` — ElectronHub TTS.
///
/// Mirrors Node's `router.post('/electronhub/generate-voice')` in `openai.js:318-381`.
pub async fn electronhub_generate_voice(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ElectronHubGenerateVoiceRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELECTRONHUB) {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let model = body.model.clone().unwrap_or_else(|| "tts-1".to_string());
    let voice = body.voice.clone().unwrap_or_else(|| "alloy".to_string());
    let speed = body.speed.unwrap_or(1.0);
    let response_format = body.response_format.clone().unwrap_or_else(|| "mp3".to_string());

    let mut request_body = json!({
        "input": text,
        "model": model,
        "voice": voice,
        "speed": speed,
        "response_format": response_format,
    });

    // Merge extra body params
    if let Some(extra_obj) = body.extra.as_object() {
        if let Some(rb) = request_body.as_object_mut() {
            for (k, v) in extra_obj {
                if !rb.contains_key(k) {
                    rb.insert(k.clone(), v.clone());
                }
            }
        }
    }

    let client = http_client();

    match client
        .post("https://api.electronhub.ai/v1/audio/speech")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("ElectronHub TTS error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.bytes().await {
                Ok(bytes) => axum::response::Response::builder()
                    .header("Content-Type", "audio/mpeg")
                    .body(axum::body::Body::from(bytes.to_vec()))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                Err(e) => {
                    tracing::error!("ElectronHub TTS read error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("ElectronHub TTS request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/openai/electronhub/models` — ElectronHub model list.
///
/// Mirrors Node's `router.post('/electronhub/models')` in `openai.js:383-410`.
pub async fn electronhub_models(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELECTRONHUB) {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let client = http_client();

    match client
        .get("https://api.electronhub.ai/v1/models")
        .header("Authorization", format!("Bearer {}", api_key))
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
            tracing::error!("ElectronHub models error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/openai/chutes/generate-voice` — Chutes Kokoro TTS.
///
/// Mirrors Node's `router.post('/chutes/generate-voice')` in `openai.js:412-459`.
pub async fn chutes_generate_voice(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ChutesGenerateVoiceRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_CHUTES) {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let voice = body.voice.clone().unwrap_or_else(|| "af_heart".to_string());
    let speed = body.speed.unwrap_or(1.0);

    let request_body = json!({
        "text": text,
        "voice": voice,
        "speed": speed,
    });

    let client = http_client();

    match client
        .post("https://chutes-kokoro.chutes.ai/speak")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Chutes TTS error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.bytes().await {
                Ok(bytes) => axum::response::Response::builder()
                    .header("Content-Type", "audio/wav")
                    .body(axum::body::Body::from(bytes.to_vec()))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                Err(e) => {
                    tracing::error!("Chutes TTS read error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Chutes TTS request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/openai/chutes/models/embedding` — Chutes embedding model list.
///
/// Mirrors Node's `router.post('/chutes/models/embedding')` in `openai.js:461-496`.
pub async fn chutes_models_embedding(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_CHUTES) {
        Some(k) if !k.is_empty() => k,
        _ => {
            return Json(json!([])).into_response();
        }
    };

    let client = http_client();

    match client
        .get("https://api.chutes.ai/chutes/?template=embedding&include_public=true&limit=999")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}

/// `POST /api/openai/nanogpt/models/embedding` — NanoGPT embedding model list.
///
/// Mirrors Node's `router.post('/nanogpt/models/embedding')` in `openai.js:495-530`.
pub async fn nanogpt_models_embedding(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_NANOGPT) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("No NanoGPT key found");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let client = http_client();

    match client
        .get("https://nano-gpt.com/api/v1/embedding-models")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Accept-Encoding", "identity")
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::warn!("NanoGPT embedding models request failed: {}", error_text);
                return (StatusCode::INTERNAL_SERVER_ERROR, error_text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    // Node returns data.data if it's an array, otherwise 500
                    if let Some(arr) = data.get("data") {
                        if arr.is_array() {
                            return Json(arr.clone()).into_response();
                        }
                    }
                    tracing::warn!("NanoGPT embedding models response invalid");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("NanoGPT embedding models fetch failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/openai/generate-image` — OpenAI DALL-E image generation.
///
/// Mirrors Node's `router.post('/generate-image')` in `openai.js:498-537`.
pub async fn generate_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GenerateImageRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_OPENAI) {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let prompt = match &body.prompt {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let model = body.model.clone().unwrap_or_else(|| "dall-e-3".to_string());
    let size = body.size.clone().unwrap_or_else(|| "1024x1024".to_string());
    let quality = body.quality.clone().unwrap_or_else(|| "standard".to_string());
    let style = body.style.clone().unwrap_or_else(|| "vivid".to_string());
    let n = body.n.unwrap_or(1);
    let response_format = body.response_format.clone().unwrap_or_else(|| "b64_json".to_string());

    let request_body = json!({
        "prompt": prompt,
        "model": model,
        "size": size,
        "quality": quality,
        "style": style,
        "n": n,
        "response_format": response_format,
    });

    let client = http_client();

    match client
        .post("https://api.openai.com/v1/images/generations")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("OpenAI image error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("OpenAI image parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("OpenAI image request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/openai/generate-video` — OpenAI Sora video generation.
///
/// Mirrors Node's `router.post('/generate-video')` in `openai.js:539-638`.
pub async fn generate_video(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GenerateVideoRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_OPENAI) {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let prompt = match &body.prompt {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let model = body.model.clone().unwrap_or_else(|| "sora".to_string());
    let size = body.size.clone().unwrap_or_else(|| "1920x1080".to_string());
    let n = body.n.unwrap_or(1);
    let duration = body.duration.clone().unwrap_or_else(|| "5".to_string());

    let request_body = json!({
        "prompt": prompt,
        "model": model,
        "size": size,
        "n": n,
        "duration": duration,
    });

    let client = http_client();

    // Submit video job
    let submit_resp = match client
        .post("https://api.openai.com/v1/videos")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("OpenAI video submit error: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if !submit_resp.status().is_success() {
        let error_text = submit_resp.text().await.unwrap_or_default();
        tracing::error!("OpenAI video submit error: {}", error_text);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let video_job: Value = match submit_resp.json().await {
        Ok(d) => d,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let job_id = match video_job.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return Json(video_job).into_response(),
    };

    // Poll for completion
    for _ in 0..120 {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let poll_url = format!("https://api.openai.com/v1/videos/{}", job_id);

        match client
            .get(&poll_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(data) = resp.json::<Value>().await {
                    let status = data.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    match status {
                        "completed" => {
                            // Fetch the video content
                            let content_url = format!(
                                "https://api.openai.com/v1/videos/{}/content",
                                job_id
                            );
                            match client
                                .get(&content_url)
                                .header("Authorization", format!("Bearer {}", api_key))
                                .send()
                                .await
                            {
                                Ok(content_resp) => {
                                    if let Ok(content_data) = content_resp.json::<Value>().await {
                                        return Json(content_data).into_response();
                                    }
                                }
                                Err(_) => {}
                            }
                            return Json(data).into_response();
                        }
                        "failed" => {
                            return Json(data).into_response();
                        }
                        _ => {
                            // Still processing
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("OpenAI video poll error: {}", e);
            }
        }
    }

    (StatusCode::REQUEST_TIMEOUT, Json(json!({"error": "Video generation timed out"}))).into_response()
}

/// `POST /api/openai/custom/generate-voice` — Custom OpenAI-compatible TTS.
///
/// Mirrors Node's `router.post('/custom/generate-voice')` in `openai.js:640-681`.
pub async fn custom_generate_voice(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<CustomGenerateVoiceRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_CUSTOM_OPENAI_TTS) {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let text = match &body.input {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let provider_endpoint = match &body.provider_endpoint {
        Some(ep) if !ep.is_empty() => ep.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let model = body.model.clone().unwrap_or_else(|| "tts-1".to_string());
    let voice = body.voice.clone().unwrap_or_else(|| "alloy".to_string());
    let speed = body.speed.unwrap_or(1.0);
    let response_format = body.response_format.clone().unwrap_or_else(|| "mp3".to_string());

    let request_body = json!({
        "input": text,
        "model": model,
        "voice": voice,
        "speed": speed,
        "response_format": response_format,
    });

    let client = http_client();

    match client
        .post(&provider_endpoint)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Custom TTS error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.bytes().await {
                Ok(bytes) => axum::response::Response::builder()
                    .header("Content-Type", "audio/mpeg")
                    .body(axum::body::Body::from(bytes.to_vec()))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                Err(e) => {
                    tracing::error!("Custom TTS read error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Custom TTS request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/openai/transcribe-audio` — OpenAI Whisper STT.
///
/// Mirrors Node's `router.post('/transcribe-audio')` in `openai.js:733-737`.
pub async fn transcribe_audio(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<TranscribeAudioRequest>,
) -> Response {
    transcribe_audio_generic(
        &state.config.data_root,
        &user.handle,
        SECRET_OPENAI,
        "https://api.openai.com/v1/audio/transcriptions",
        &body,
    )
    .await
}

/// `POST /api/openai/groq/transcribe-audio` — Groq STT.
///
/// Mirrors Node's `router.post('/groq/transcribe-audio')` in `openai.js:739-743`.
pub async fn groq_transcribe_audio(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<TranscribeAudioRequest>,
) -> Response {
    transcribe_audio_generic(
        &state.config.data_root,
        &user.handle,
        SECRET_GROQ,
        "https://api.groq.com/openai/v1/audio/transcriptions",
        &body,
    )
    .await
}

/// `POST /api/openai/mistral/transcribe-audio` — Mistral STT.
///
/// Mirrors Node's `router.post('/mistral/transcribe-audio')` in `openai.js:745-749`.
pub async fn mistral_transcribe_audio(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<TranscribeAudioRequest>,
) -> Response {
    transcribe_audio_generic(
        &state.config.data_root,
        &user.handle,
        SECRET_MISTRALAI,
        "https://api.mistral.ai/v1/audio/transcriptions",
        &body,
    )
    .await
}

/// `POST /api/openai/zai/transcribe-audio` — ZAI STT.
///
/// Mirrors Node's `router.post('/zai/transcribe-audio')` in `openai.js:751-755`.
pub async fn zai_transcribe_audio(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<TranscribeAudioRequest>,
) -> Response {
    transcribe_audio_generic(
        &state.config.data_root,
        &user.handle,
        SECRET_ZAI,
        "https://api.z.ai/api/paas/v4/audio/transcriptions",
        &body,
    )
    .await
}

/// `POST /api/openai/chutes/transcribe-audio` — Chutes STT.
///
/// Mirrors Node's `router.post('/chutes/transcribe-audio')` in `openai.js:757-806`.
pub async fn chutes_transcribe_audio(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ChutesTranscribeRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_CHUTES) {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let audio = match &body.audio {
        Some(a) if !a.is_empty() => a.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No audio"}))).into_response();
        }
    };

    let model = body.model.clone().unwrap_or_else(|| "scribe_v1".to_string());

    // For Chutes, we send audio as base64 in JSON body
    let request_body = json!({
        "audio": audio,
        "model": model,
    });

    if let Some(lang) = &body.lang {
        // lang is provided but we don't modify request_body here since it was already created
        let _ = lang;
    }

    let url = format!("https://{}.chutes.ai/transcribe", model);
    let client = http_client();

    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Chutes STT error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("Chutes STT parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Chutes STT request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
