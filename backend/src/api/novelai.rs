//! NovelAI endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/novelai.js`](../../../src/endpoints/novelai.js).
//!
//! ## Endpoints
//! - `POST /api/novelai/status`         — Check NovelAI subscription status.
//! - `POST /api/novelai/generate`       — Generate text with NovelAI.
//! - `POST /api/novelai/generate-image` — Generate image with NovelAI.
//! - `POST /api/novelai/generate-voice` — Generate voice with NovelAI TTS.

use std::io::Read;
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

const SECRET_NOVEL: &str = "api_key_novel";
const API_NOVELAI: &str = "https://api.novelai.net";

// Bad words lists for different models
const BAD_WORDS_LIST: &[&[u32]] = &[
    &[3], &[49356], &[1431], &[31715], &[34387], &[20765], &[30702], &[10691],
    &[49333], &[1266], &[19438], &[43145], &[26523], &[41471], &[2936],
    &[85, 85], &[49332], &[7286], &[1115], &[24],
];

const ERATO_BAD_WORDS_LIST: &[&[u32]] = &[
    &[16067], &[933, 11144], &[25106, 11144], &[58, 106901, 16073, 33710, 25, 109933],
    &[933, 58, 11144], &[128030], &[58, 30591, 33503, 17663, 100204, 25, 11144],
];

const HYPE_BOT_BAD_WORDS_LIST: &[&[u32]] = &[
    &[58], &[60], &[90], &[92], &[685], &[1391], &[1782], &[2361], &[3693], &[4083],
    &[4357], &[4895], &[5512], &[5974], &[7131], &[8183], &[8351], &[8762], &[8964],
    &[8973], &[9063], &[11208], &[11709], &[11907], &[11919], &[12878], &[12962],
    &[13018], &[13412], &[14631], &[14692], &[14980], &[15090], &[15437], &[16151],
    &[16410], &[16589], &[17241], &[17414], &[17635], &[17816], &[17912], &[18083],
    &[18161], &[18477], &[19629], &[19779], &[19953], &[20520], &[20598], &[20662],
    &[20740], &[21476], &[21737], &[22133], &[22241], &[22345], &[22935], &[23330],
    &[23785], &[23834], &[23884], &[25295], &[25597], &[25719], &[25787], &[25915],
    &[26076], &[26358], &[26398], &[26894], &[26933], &[27007], &[27422], &[28013],
    &[29164], &[29225], &[29342], &[29565], &[29795], &[30072], &[30109], &[30138],
    &[30866], &[31161], &[31478], &[32092], &[32239], &[32509], &[33116], &[33250],
    &[33761], &[34171], &[34758], &[34949], &[35944], &[36338], &[36463], &[36563],
    &[36786], &[36796], &[36937], &[37250], &[37913], &[37981], &[38165], &[38362],
    &[38381], &[38430], &[38892], &[39850], &[39893], &[41832], &[41888], &[42535],
    &[42669], &[42785], &[42924], &[43839], &[44438], &[44587], &[44926], &[45144],
    &[45297], &[46110], &[46570], &[46581], &[46956], &[47175], &[47182], &[47527],
    &[47715], &[48600], &[48683], &[48688], &[48874], &[48999], &[49074], &[49082],
    &[49146], &[49946], &[10221], &[4841], &[1427], &[2602, 834], &[29343], &[37405],
    &[35780], &[2602], &[50256],
];

// Rep penalty allow list
const REP_PENALTY_ALLOW_LIST: &[u32] = &[
    49256, 49264, 49231, 49230, 49287, 85, 49255, 49399, 49262, 336, 333, 432, 363, 468, 492,
    745, 401, 426, 623, 794, 1096, 2919, 2072, 7379, 1259, 2110, 620, 526, 487, 16562, 603,
    805, 761, 2681, 942, 8917, 653, 3513, 506, 5301, 562, 5010, 614, 10942, 539, 2976, 462,
    5189, 567, 2032, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 588, 803, 1040, 49209,
    4, 5, 6, 7, 8, 9, 10, 11, 12,
];

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct NovelAiGenerateRequest {
    pub model: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Deserialize)]
pub struct NovelAiGenerateImageRequest {
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub negative_prompt: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub scale: Option<f64>,
    pub sampler: Option<String>,
    pub steps: Option<u32>,
    pub seed: Option<i64>,
    pub n_samples: Option<u32>,
    pub upscale: Option<bool>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Deserialize)]
pub struct NovelAiGenerateVoiceRequest {
    pub text: Option<String>,
    pub voice: Option<String>,
    pub seed: Option<String>,
    pub opus: Option<bool>,
    pub version: Option<String>,
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

/// Get the appropriate bad words list for a model.
fn get_bad_words_list(model: &str) -> Vec<Vec<u32>> {
    if model.starts_with("llama-3-erato") || model.starts_with("erato") {
        ERATO_BAD_WORDS_LIST.iter().map(|w| w.to_vec()).collect()
    } else if model == "hypebot" {
        HYPE_BOT_BAD_WORDS_LIST.iter().map(|w| w.to_vec()).collect()
    } else {
        BAD_WORDS_LIST.iter().map(|w| w.to_vec()).collect()
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/novelai/status` — Check NovelAI subscription status.
///
/// Mirrors Node's `router.post('/status')` in `novelai.js:131-162`.
pub async fn status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_NOVEL) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("NovelAI API key is not configured.");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let url = format!("{}/user/subscription", API_NOVELAI);
    let client = http_client();

    match client
        .get(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("NovelAI status error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("NovelAI status parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("NovelAI status request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/novelai/generate` — Generate text with NovelAI.
///
/// Mirrors Node's `router.post('/generate')` in `novelai.js:164-283`.
pub async fn generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_NOVEL) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("NovelAI API key is not configured.");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let model = body.get("model").and_then(|m| m.as_str()).unwrap_or("kayra-v1");

    // Build request with bad words, logit bias, and rep penalty whitelist
    let bad_words_ids = get_bad_words_list(model);

    let mut data = body.clone();
    if let Some(obj) = data.as_object_mut() {
        if let Some(params) = obj.get_mut("parameters").and_then(|p| p.as_object_mut()) {
            params.insert("bad_words_ids".to_string(), json!(bad_words_ids));

            // Add rep penalty whitelist based on model
            if model.starts_with("llama-3-erato") || model.starts_with("erato") {
                // Erato has its own whitelist - use empty for now
            } else {
                params.insert("repetition_penalty_whitelist".to_string(), json!(REP_PENALTY_ALLOW_LIST));
            }
        }
    }

    let url = format!("{}/ai/generate", API_NOVELAI);
    let client = http_client();

    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&data)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("NovelAI generate error ({}): {}", status, error_text);
                return (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    error_text,
                )
                    .into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("NovelAI generate parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("NovelAI generate request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/novelai/generate-image` — Generate image with NovelAI.
///
/// Mirrors Node's `router.post('/generate-image')` in `novelai.js:285-441`.
/// Returns the first image from the ZIP response as base64.
pub async fn generate_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_NOVEL) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("NovelAI API key is not configured.");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let url = format!("{}/ai/generate-image", API_NOVELAI);
    let client = http_client();

    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("NovelAI image error ({}): {}", status, error_text);
                return (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    error_text,
                )
                    .into_response();
            }

            // Response is a ZIP file containing the generated image(s)
            match resp.bytes().await {
                Ok(zip_bytes) => {
                    let cursor = std::io::Cursor::new(zip_bytes.as_ref());
                    match zip::ZipArchive::new(cursor) {
                        Ok(mut archive) => {
                            // Extract first file from ZIP
                            if archive.len() == 0 {
                                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                            }

                            let mut images: Vec<String> = Vec::new();

                            for i in 0..archive.len() {
                                if let Ok(mut file) = archive.by_index(i) {
                                    let mut buf = Vec::new();
                                    if file.read_to_end(&mut buf).is_ok() {
                                        let b64 = base64::Engine::encode(
                                            &base64::engine::general_purpose::STANDARD,
                                            &buf,
                                        );
                                        images.push(b64);
                                    }
                                }
                            }

                            if images.is_empty() {
                                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                            }

                            Json(json!({ "images": images })).into_response()
                        }
                        Err(e) => {
                            tracing::error!("NovelAI image ZIP parse error: {}", e);
                            StatusCode::INTERNAL_SERVER_ERROR.into_response()
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("NovelAI image read error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("NovelAI image request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/novelai/generate-voice` — Generate voice with NovelAI TTS.
///
/// Mirrors Node's `router.post('/generate-voice')` in `novelai.js:443-484`.
pub async fn generate_voice(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<NovelAiGenerateVoiceRequest>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_NOVEL) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("NovelAI API key is not configured.");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No text"}))).into_response();
        }
    };

    let voice = body.voice.clone().unwrap_or_else(|| "Ligeia".to_string());
    let seed = body.seed.clone().unwrap_or_else(|| "Ligeia".to_string());
    let opus = body.opus.unwrap_or(false);
    let version = body.version.clone().unwrap_or_else(|| "v2".to_string());

    let encoded_text = urlencoding::encode(&text);
    let encoded_seed = urlencoding::encode(&seed);
    let encoded_voice = urlencoding::encode(&voice);

    let url = format!(
        "{}/ai/generate-voice?text={}&voice=-1&seed={}&voice2={}&opus={}&version={}",
        API_NOVELAI, encoded_text, encoded_seed, encoded_voice, opus, version
    );

    let client = http_client();

    match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("NovelAI voice error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            let content_type = if opus { "audio/ogg" } else { "audio/mpeg" };

            match resp.bytes().await {
                Ok(bytes) => axum::response::Response::builder()
                    .header("Content-Type", content_type)
                    .body(axum::body::Body::from(bytes.to_vec()))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                Err(e) => {
                    tracing::error!("NovelAI voice read error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("NovelAI voice request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
