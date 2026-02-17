//! Google endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/google.js`](../../../src/endpoints/google.js).
//!
//! ## Endpoints
//! - `POST /api/google/caption-image`       — Caption image with Gemini vision.
//! - `POST /api/google/list-voices`         — List Google Translate TTS languages.
//! - `POST /api/google/generate-voice`      — Generate voice via Google Translate TTS.
//! - `POST /api/google/list-native-voices`  — List native Gemini TTS voices.
//! - `POST /api/google/generate-native-tts` — Generate speech with Gemini native TTS.
//! - `POST /api/google/generate-image`      — Generate image with Imagen.
//! - `POST /api/google/generate-video`      — Generate video with Veo.

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

const SECRET_MAKERSUITE: &str = "api_key_makersuite";
#[allow(dead_code)]
const SECRET_VERTEXAI: &str = "api_key_vertexai";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CaptionImageRequest {
    pub image: Option<String>,
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub reverse_proxy: Option<String>,
    pub proxy_password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateVoiceRequest {
    pub text: Option<String>,
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateNativeTtsRequest {
    pub text: Option<String>,
    pub model: Option<String>,
    pub voice_name: Option<String>,
    pub sample_rate: Option<u32>,
    pub reverse_proxy: Option<String>,
    pub proxy_password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateImageRequest {
    pub prompt: Option<String>,
    pub negative_prompt: Option<String>,
    pub model: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub count: Option<u32>,
    pub aspect_ratio: Option<String>,
    pub reverse_proxy: Option<String>,
    pub proxy_password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateVideoRequest {
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub image: Option<String>,
    pub aspect_ratio: Option<String>,
    pub reverse_proxy: Option<String>,
    pub proxy_password: Option<String>,
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

/// Create a WAV header for PCM audio data.
fn create_wav_header(data_size: usize, sample_rate: u32, num_channels: u16, bits_per_sample: u16) -> Vec<u8> {
    let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
    let block_align = num_channels * bits_per_sample / 8;
    let chunk_size = 36 + data_size as u32;

    let mut header = Vec::with_capacity(44);
    // RIFF header
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&chunk_size.to_le_bytes());
    header.extend_from_slice(b"WAVE");
    // fmt sub-chunk
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes()); // SubChunk1Size (PCM)
    header.extend_from_slice(&1u16.to_le_bytes()); // AudioFormat (PCM)
    header.extend_from_slice(&num_channels.to_le_bytes());
    header.extend_from_slice(&sample_rate.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&block_align.to_le_bytes());
    header.extend_from_slice(&bits_per_sample.to_le_bytes());
    // data sub-chunk
    header.extend_from_slice(b"data");
    header.extend_from_slice(&(data_size as u32).to_le_bytes());
    header
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/google/caption-image` — Caption image with Gemini vision.
///
/// Mirrors Node's `router.post('/caption-image')` in `google.js:232-290`.
pub async fn caption_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<CaptionImageRequest>,
) -> Response {
    let api_key = if body.reverse_proxy.is_some() {
        body.proxy_password.clone()
    } else {
        read_secret(&state.config.data_root, &user.handle, SECRET_MAKERSUITE)
    };

    let api_key = match api_key {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("Google API key is not configured.");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let image = match &body.image {
        Some(img) if !img.is_empty() => img.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No image"}))).into_response();
        }
    };

    let model = body.model.clone().unwrap_or_else(|| "gemini-1.5-flash-latest".to_string());
    let prompt = body.prompt.clone().unwrap_or_else(|| "What's in this image?".to_string());

    // Parse base64 image data URI
    let (mime_type, base64_data) = if let Some(rest) = image.strip_prefix("data:") {
        if let Some((mime, data)) = rest.split_once(";base64,") {
            (mime.to_string(), data.to_string())
        } else {
            ("image/png".to_string(), image.clone())
        }
    } else {
        ("image/png".to_string(), image.clone())
    };

    let api_url = if let Some(rp) = &body.reverse_proxy {
        format!("{}/v1beta/models/{}:generateContent", rp.trim_end_matches('/'), model)
    } else {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model, api_key
        )
    };

    let request_body = json!({
        "contents": [{
            "parts": [
                {
                    "text": prompt,
                },
                {
                    "inlineData": {
                        "mimeType": mime_type,
                        "data": base64_data,
                    }
                }
            ]
        }]
    });

    let client = http_client();

    match client
        .post(&api_url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Google caption error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("Google caption parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Google caption request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/google/list-voices` — List Google Translate TTS languages.
///
/// Mirrors Node's `router.post('/list-voices')` in `google.js:292-315`.
/// Returns a hardcoded list of language codes supported by Google Translate TTS.
pub async fn list_voices(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    // Return the well-known Google Translate TTS language list
    Json(json!([
        "af", "am", "ar", "bg", "bn", "bs", "ca", "cs", "cy", "da", "de", "el",
        "en", "eo", "es", "et", "eu", "fi", "fr", "gl", "gu", "ha", "hi", "hr",
        "hu", "hy", "id", "is", "it", "iw", "ja", "jw", "ka", "kk", "km", "kn",
        "ko", "ku", "la", "lt", "lv", "mg", "mi", "mk", "ml", "mn", "mr", "ms",
        "my", "ne", "nl", "no", "ny", "pa", "pl", "ps", "pt", "ro", "ru", "rw",
        "sd", "si", "sk", "sl", "sm", "sn", "so", "sq", "sr", "st", "su", "sv",
        "sw", "ta", "te", "tg", "th", "tl", "tr", "ts", "tt", "ug", "uk", "ur",
        "uz", "vi", "xh", "yi", "yo", "zh-CN", "zh-TW", "zu"
    ]))
    .into_response()
}

/// `POST /api/google/generate-voice` — Generate voice via Google Translate TTS.
///
/// Mirrors Node's `router.post('/generate-voice')` in `google.js:347-360`.
/// Note: Uses Google Translate TTS URL directly. The full `google-translate-api-x`
/// integration with cookie/token handling is deferred.
pub async fn generate_voice(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<GenerateVoiceRequest>,
) -> Response {
    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let lang = body.lang.clone().unwrap_or_else(|| "en".to_string());

    let encoded_text = urlencoding::encode(&text);
    let url = format!(
        "https://translate.google.com/translate_tts?ie=UTF-8&q={}&tl={}&client=tw-ob",
        encoded_text, lang
    );

    let client = http_client();

    match client.get(&url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.bytes().await {
                Ok(bytes) => axum::response::Response::builder()
                    .header("Content-Type", "audio/mpeg")
                    .body(axum::body::Body::from(bytes.to_vec()))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("Google TTS error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/google/list-native-voices` — List native Gemini TTS voices.
///
/// Mirrors Node's `router.post('/list-native-voices')` in `google.js:317-345`.
/// Returns the hardcoded list of 30 Gemini native TTS voices.
pub async fn list_native_voices(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    Json(json!([
        {"voice_id": "Zephyr", "name": "Zephyr", "lang": "en"},
        {"voice_id": "Puck", "name": "Puck", "lang": "en"},
        {"voice_id": "Charon", "name": "Charon", "lang": "en"},
        {"voice_id": "Kore", "name": "Kore", "lang": "en"},
        {"voice_id": "Fenrir", "name": "Fenrir", "lang": "en"},
        {"voice_id": "Leda", "name": "Leda", "lang": "en"},
        {"voice_id": "Orus", "name": "Orus", "lang": "en"},
        {"voice_id": "Aoede", "name": "Aoede", "lang": "en"},
        {"voice_id": "Callirrhoe", "name": "Callirrhoe", "lang": "en"},
        {"voice_id": "Autonoe", "name": "Autonoe", "lang": "en"},
        {"voice_id": "Enceladus", "name": "Enceladus", "lang": "en"},
        {"voice_id": "Iapetus", "name": "Iapetus", "lang": "en"},
        {"voice_id": "Umbriel", "name": "Umbriel", "lang": "en"},
        {"voice_id": "Algieba", "name": "Algieba", "lang": "en"},
        {"voice_id": "Despina", "name": "Despina", "lang": "en"},
        {"voice_id": "Erinome", "name": "Erinome", "lang": "en"},
        {"voice_id": "Gacrux", "name": "Gacrux", "lang": "en"},
        {"voice_id": "Linus", "name": "Linus", "lang": "en"},
        {"voice_id": "Pegasi", "name": "Pegasi", "lang": "en"},
        {"voice_id": "Schedar", "name": "Schedar", "lang": "en"},
        {"voice_id": "Sulafat", "name": "Sulafat", "lang": "en"},
        {"voice_id": "Vindemiatrix", "name": "Vindemiatrix", "lang": "en"},
        {"voice_id": "Achernar", "name": "Achernar", "lang": "en"},
        {"voice_id": "Hamal", "name": "Hamal", "lang": "en"},
        {"voice_id": "Zubenelgenubi", "name": "Zubenelgenubi", "lang": "en"},
        {"voice_id": "Sadachbia", "name": "Sadachbia", "lang": "en"},
        {"voice_id": "Sadaltager", "name": "Sadaltager", "lang": "en"},
        {"voice_id": "Diphda", "name": "Diphda", "lang": "en"},
        {"voice_id": "Ginan", "name": "Ginan", "lang": "en"},
        {"voice_id": "Muscida", "name": "Muscida", "lang": "en"}
    ]))
    .into_response()
}

/// `POST /api/google/generate-native-tts` — Generate speech with Gemini native TTS.
///
/// Mirrors Node's `router.post('/generate-native-tts')` in `google.js:362-424`.
pub async fn generate_native_tts(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GenerateNativeTtsRequest>,
) -> Response {
    let api_key = if body.reverse_proxy.is_some() {
        body.proxy_password.clone()
    } else {
        read_secret(&state.config.data_root, &user.handle, SECRET_MAKERSUITE)
    };

    let api_key = match api_key {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No API key"}))).into_response();
        }
    };

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let model = body.model.clone().unwrap_or_else(|| "gemini-2.0-flash".to_string());
    let voice_name = body.voice_name.clone().unwrap_or_else(|| "Kore".to_string());
    let sample_rate = body.sample_rate.unwrap_or(24000);

    let api_url = if let Some(rp) = &body.reverse_proxy {
        format!("{}/v1beta/models/{}:generateContent", rp.trim_end_matches('/'), model)
    } else {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model, api_key
        )
    };

    let request_body = json!({
        "contents": [{
            "parts": [{"text": text}],
        }],
        "generationConfig": {
            "response_modalities": ["AUDIO"],
            "speechConfig": {
                "voiceConfig": {
                    "prebuiltVoiceConfig": {
                        "voiceName": voice_name,
                    }
                }
            }
        }
    });

    let client = http_client();

    match client
        .post(&api_url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Google native TTS error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    // Extract audio data from response
                    let audio_b64 = data
                        .get("candidates")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|c| c.get("content"))
                        .and_then(|content| content.get("parts"))
                        .and_then(|parts| parts.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|part| part.get("inlineData"))
                        .and_then(|id| id.get("data"))
                        .and_then(|d| d.as_str());

                    match audio_b64 {
                        Some(b64) => {
                            match base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                b64,
                            ) {
                                Ok(pcm_data) => {
                                    // Create WAV file with header
                                    let header = create_wav_header(pcm_data.len(), sample_rate, 1, 16);
                                    let mut wav_data = Vec::with_capacity(header.len() + pcm_data.len());
                                    wav_data.extend_from_slice(&header);
                                    wav_data.extend_from_slice(&pcm_data);

                                    axum::response::Response::builder()
                                        .header("Content-Type", "audio/wav")
                                        .body(axum::body::Body::from(wav_data))
                                        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
                                }
                                Err(e) => {
                                    tracing::error!("Google native TTS decode error: {}", e);
                                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                                }
                            }
                        }
                        None => {
                            tracing::warn!("Google native TTS: no audio data in response");
                            StatusCode::INTERNAL_SERVER_ERROR.into_response()
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Google native TTS parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Google native TTS request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/google/generate-image` — Generate image with Imagen.
///
/// Mirrors Node's `router.post('/generate-image')` in `google.js:426-502`.
pub async fn generate_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GenerateImageRequest>,
) -> Response {
    let api_key = if body.reverse_proxy.is_some() {
        body.proxy_password.clone()
    } else {
        read_secret(&state.config.data_root, &user.handle, SECRET_MAKERSUITE)
    };

    let api_key = match api_key {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No API key"}))).into_response();
        }
    };

    let prompt = match &body.prompt {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let model = body.model.clone().unwrap_or_else(|| "imagen-3.0-generate-001".to_string());
    let count = body.count.unwrap_or(1);
    let aspect_ratio = body.aspect_ratio.clone().unwrap_or_else(|| "1:1".to_string());

    let api_url = if let Some(rp) = &body.reverse_proxy {
        format!(
            "{}/v1beta/models/{}:predict",
            rp.trim_end_matches('/'),
            model
        )
    } else {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:predict?key={}",
            model, api_key
        )
    };

    let mut parameters = json!({
        "sampleCount": count,
        "aspectRatio": aspect_ratio,
    });

    if let Some(neg) = &body.negative_prompt {
        if !neg.is_empty() {
            parameters["negativePrompt"] = json!(neg);
        }
    }

    // Add output options
    parameters["outputOptions"] = json!({
        "mimeType": "image/png",
    });

    let request_body = json!({
        "instances": [{"prompt": prompt}],
        "parameters": parameters,
    });

    let client = http_client();

    match client
        .post(&api_url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Google Imagen error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("Google Imagen parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Google Imagen request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/google/generate-video` — Generate video with Veo.
///
/// Mirrors Node's `router.post('/generate-video')` in `google.js:504-641`.
pub async fn generate_video(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<GenerateVideoRequest>,
) -> Response {
    let api_key = if body.reverse_proxy.is_some() {
        body.proxy_password.clone()
    } else {
        read_secret(&state.config.data_root, &user.handle, SECRET_MAKERSUITE)
    };

    let api_key = match api_key {
        Some(k) if !k.is_empty() => k,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No API key"}))).into_response();
        }
    };

    let prompt = match &body.prompt {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let model = body.model.clone().unwrap_or_else(|| "veo-2.0-generate-001".to_string());
    let aspect_ratio = body.aspect_ratio.clone().unwrap_or_else(|| "16:9".to_string());

    let api_url = if let Some(rp) = &body.reverse_proxy {
        format!(
            "{}/v1beta/models/{}:predictLongRunning",
            rp.trim_end_matches('/'),
            model
        )
    } else {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:predictLongRunning?key={}",
            model, api_key
        )
    };

    let mut instances = json!([{"prompt": prompt}]);
    if let Some(img) = &body.image {
        if !img.is_empty() {
            instances[0]["image"] = json!({"bytesBase64Encoded": img});
        }
    }

    let request_body = json!({
        "instances": instances,
        "parameters": {
            "aspectRatio": aspect_ratio,
        },
    });

    let client = http_client();

    // Submit video generation job
    let submit_resp = match client
        .post(&api_url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Google Veo submit error: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if !submit_resp.status().is_success() {
        let error_text = submit_resp.text().await.unwrap_or_default();
        tracing::error!("Google Veo submit error: {}", error_text);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let submit_data: Value = match submit_resp.json().await {
        Ok(d) => d,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // Get operation name for polling
    let operation_name = match submit_data.get("name").and_then(|v| v.as_str()) {
        Some(name) => name.to_string(),
        None => return Json(submit_data).into_response(),
    };

    // Poll for result
    let base_url = if let Some(rp) = &body.reverse_proxy {
        rp.trim_end_matches('/').to_string()
    } else {
        "https://generativelanguage.googleapis.com".to_string()
    };

    for _ in 0..120 {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let poll_url = if body.reverse_proxy.is_some() {
            format!("{}/v1beta/{}", base_url, operation_name)
        } else {
            format!(
                "{}/v1beta/{}?key={}",
                base_url, operation_name, api_key
            )
        };

        match client.get(&poll_url).send().await {
            Ok(resp) => {
                if let Ok(data) = resp.json::<Value>().await {
                    let done = data.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
                    if done {
                        return Json(data).into_response();
                    }
                    // Check for error
                    if data.get("error").is_some() {
                        return Json(data).into_response();
                    }
                }
            }
            Err(e) => {
                tracing::error!("Google Veo poll error: {}", e);
            }
        }
    }

    (StatusCode::REQUEST_TIMEOUT, Json(json!({"error": "Video generation timed out"}))).into_response()
}
