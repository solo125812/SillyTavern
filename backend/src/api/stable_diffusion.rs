//! Stable Diffusion endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/stable-diffusion.js`](../../../src/endpoints/stable-diffusion.js).
//!
//! ## Core Endpoints
//! - `POST /api/sd/ping`           — Ping SD API.
//! - `POST /api/sd/upscalers`      — List upscaler models.
//! - `POST /api/sd/vaes`           — List VAE models.
//! - `POST /api/sd/samplers`       — List samplers.
//! - `POST /api/sd/schedulers`     — List schedulers.
//! - `POST /api/sd/models`         — List checkpoints.
//! - `POST /api/sd/get-model`      — Get current model.
//! - `POST /api/sd/set-model`      — Set current model.
//! - `POST /api/sd/generate`       — Generate image (txt2img).
//! - `POST /api/sd/sd-next/upscalers` — SDNext upscalers.
//!
//! ## External Provider Sub-endpoints (also proxied here)
//! - `POST /api/sd/comfy/*`        — ComfyUI endpoints.
//! - `POST /api/sd/together/*`     — Together AI endpoints.
//! - `POST /api/sd/stability/*`    — Stability AI endpoints.
//! - `POST /api/sd/pollinations/*` — Pollinations endpoints.
//! - `POST /api/sd/huggingface/*`  — HuggingFace endpoints.
//! - `POST /api/sd/electronhub/*`  — ElectronHub endpoints.
//! - `POST /api/sd/chutes/*`       — Chutes endpoints.
//! - `POST /api/sd/nanogpt/*`      — NanoGPT endpoints.
//! - `POST /api/sd/bfl/*`          — BFL endpoints.
//! - `POST /api/sd/falai/*`        — fal.ai endpoints.
//! - `POST /api/sd/xai/*`          — xAI endpoints.
//! - `POST /api/sd/aimlapi/*`      — AIMLAPI endpoints.
//! - `POST /api/sd/zai/*`          — ZAI endpoints.

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

const SECRET_STABILITY: &str = "api_key_stability";
const SECRET_TOGETHERAI: &str = "api_key_togetherai";
const SECRET_HUGGINGFACE: &str = "api_key_huggingface";
const SECRET_ELECTRONHUB: &str = "api_key_electronhub";
const SECRET_CHUTES: &str = "api_key_chutes";
const SECRET_NANOGPT: &str = "api_key_nanogpt";
const SECRET_BFL: &str = "api_key_bfl";
const SECRET_FALAI: &str = "api_key_falai";
const SECRET_XAI: &str = "api_key_xai";
const SECRET_AIMLAPI: &str = "api_key_aimlapi";
const SECRET_ZAI: &str = "api_key_zai";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SdApiRequest {
    pub url: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Deserialize)]
pub struct SdGenerateRequest {
    pub url: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Deserialize)]
pub struct SdSetModelRequest {
    pub url: Option<String>,
    pub model: Option<Value>,
    #[serde(flatten)]
    pub extra: Value,
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
// Core SD API Handlers (A1111/Forge/SDNext compatible)
// ---------------------------------------------------------------------------

/// `POST /api/sd/ping` — Ping SD API to check if it's available.
///
/// Mirrors Node's `router.post('/ping')` in `stable-diffusion.js:31-53`.
pub async fn ping(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SdApiRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let base_url = url.trim_end_matches('/');
    let client = http_client();

    // Try internal/ping first, then fall back to /app_id
    let ping_url = format!("{}/internal/ping", base_url);

    match client.get(&ping_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => Json(json!({"status": "ok"})).into_response(),
            }
        }
        _ => {
            // Fall back to /app_id
            let fallback_url = format!("{}/app_id", base_url);
            match client.get(&fallback_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<Value>().await {
                        Ok(data) => Json(data).into_response(),
                        Err(_) => Json(json!({"status": "ok"})).into_response(),
                    }
                }
                _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    }
}

/// `POST /api/sd/upscalers` — List upscaler models.
///
/// Mirrors Node's `router.post('/upscalers')` in `stable-diffusion.js:55-104`.
pub async fn upscalers(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SdApiRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let base_url = url.trim_end_matches('/');
    let client = http_client();

    let upscaler_url = format!("{}/sdapi/v1/upscalers", base_url);

    match client.get(&upscaler_url).send().await {
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

/// `POST /api/sd/vaes` — List VAE models.
///
/// Mirrors Node's `router.post('/vaes')` in `stable-diffusion.js:106-133`.
pub async fn vaes(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SdApiRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let base_url = url.trim_end_matches('/');
    let client = http_client();
    let vae_url = format!("{}/sdapi/v1/sd-vae", base_url);

    match client.get(&vae_url).send().await {
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

/// `POST /api/sd/samplers` — List samplers.
///
/// Mirrors Node's `router.post('/samplers')` in `stable-diffusion.js:135-165`.
pub async fn samplers(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SdApiRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let base_url = url.trim_end_matches('/');
    let client = http_client();
    let sampler_url = format!("{}/sdapi/v1/samplers", base_url);

    match client.get(&sampler_url).send().await {
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

/// `POST /api/sd/schedulers` — List schedulers.
///
/// Mirrors Node's `router.post('/schedulers')` in `stable-diffusion.js:167-197`.
pub async fn schedulers(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SdApiRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let base_url = url.trim_end_matches('/');
    let client = http_client();
    let scheduler_url = format!("{}/sdapi/v1/schedulers", base_url);

    match client.get(&scheduler_url).send().await {
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

/// `POST /api/sd/models` — List checkpoint models.
///
/// Mirrors Node's `router.post('/models')` in `stable-diffusion.js:199-225`.
pub async fn models(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SdApiRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let base_url = url.trim_end_matches('/');
    let client = http_client();
    let models_url = format!("{}/sdapi/v1/sd-models", base_url);

    match client.get(&models_url).send().await {
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

/// `POST /api/sd/get-model` — Get current model.
///
/// Mirrors Node's `router.post('/get-model')` in `stable-diffusion.js:227-239`.
pub async fn get_model(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SdApiRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let base_url = url.trim_end_matches('/');
    let client = http_client();
    let options_url = format!("{}/sdapi/v1/options", base_url);

    match client.get(&options_url).send().await {
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
            tracing::error!("SD get-model error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sd/set-model` — Set current model.
///
/// Mirrors Node's `router.post('/set-model')` in `stable-diffusion.js:241-293`.
pub async fn set_model(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SdSetModelRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let model = match &body.model {
        Some(m) => m.clone(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let base_url = url.trim_end_matches('/');
    let client = http_client();
    let options_url = format!("{}/sdapi/v1/options", base_url);

    let request_body = json!({
        "sd_model_checkpoint": model,
    });

    match client
        .post(&options_url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("SD set-model error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            Json(json!({"status": "ok"})).into_response()
        }
        Err(e) => {
            tracing::error!("SD set-model request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sd/generate` — Generate image (txt2img/img2img).
///
/// Mirrors Node's `router.post('/generate')` in `stable-diffusion.js:295-377`.
pub async fn generate(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let url = body.get("url").and_then(|v| v.as_str()).unwrap_or("");
    if url.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let base_url = url.trim_end_matches('/');

    // Determine if this is img2img (has init_images) or txt2img
    let is_img2img = body.get("init_images").is_some();
    let endpoint = if is_img2img { "img2img" } else { "txt2img" };
    let gen_url = format!("{}/sdapi/v1/{}", base_url, endpoint);

    // Build request body - strip internal fields
    let mut request_body = body.clone();
    if let Some(obj) = request_body.as_object_mut() {
        obj.remove("url");
    }

    let client = http_client();

    match client
        .post(&gen_url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("SD generate error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("SD generate parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("SD generate request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sd/sd-next/upscalers` — SDNext upscalers.
///
/// Mirrors Node's `router.post('/sd-next/upscalers')` in `stable-diffusion.js:379-405`.
pub async fn sdnext_upscalers(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SdApiRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let base_url = url.trim_end_matches('/');
    let client = http_client();

    // SDNext has latent upscalers at a different endpoint
    let upscaler_url = format!("{}/sdapi/v1/latent-upscale-modes", base_url);

    match client.get(&upscaler_url).send().await {
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

// ---------------------------------------------------------------------------
// External Provider Handlers
// ---------------------------------------------------------------------------

/// `POST /api/sd/together/models` — Together AI image models.
pub async fn together_models(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_TOGETHERAI) {
        Some(k) if !k.is_empty() => k,
        _ => return Json(json!([])).into_response(),
    };

    let client = http_client();

    match client
        .get("https://api.together.xyz/api/models")
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

/// `POST /api/sd/together/generate` — Together AI image generation.
pub async fn together_generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_TOGETHERAI) {
        Some(k) if !k.is_empty() => k,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response(),
    };

    let client = http_client();

    match client
        .post("https://api.together.xyz/v1/images/generations")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                return (StatusCode::INTERNAL_SERVER_ERROR, error_text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("Together image error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sd/stability/generate` — Stability AI image generation.
pub async fn stability_generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_STABILITY) {
        Some(k) if !k.is_empty() => k,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response(),
    };

    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("stable-diffusion-xl-1024-v1-0");
    let url = format!(
        "https://api.stability.ai/v1/generation/{}/text-to-image",
        model
    );

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
                let error_text = resp.text().await.unwrap_or_default();
                return (StatusCode::INTERNAL_SERVER_ERROR, error_text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("Stability error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sd/pollinations/generate` — Pollinations image generation.
pub async fn pollinations_generate(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let prompt = body.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
    if prompt.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("flux");
    let width = body.get("width").and_then(|v| v.as_u64()).unwrap_or(1024);
    let height = body.get("height").and_then(|v| v.as_u64()).unwrap_or(1024);
    let seed = body.get("seed").and_then(|v| v.as_i64()).unwrap_or(42);

    let params = format!(
        "width={}&height={}&seed={}&model={}&nologo=true&enhance=false",
        width, height, seed, urlencoding::encode(model)
    );

    let url = format!(
        "https://image.pollinations.ai/prompt/{}?{}",
        urlencoding::encode(prompt),
        params
    );

    let client = http_client();

    match client.get(&url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.bytes().await {
                Ok(bytes) => {
                    let b64 = base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &bytes,
                    );
                    Json(json!({"images": [b64]})).into_response()
                }
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("Pollinations image error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sd/huggingface/generate` — HuggingFace image generation.
pub async fn huggingface_generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_HUGGINGFACE) {
        Some(k) if !k.is_empty() => k,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response(),
    };

    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("stabilityai/stable-diffusion-2-1");

    let url = format!("https://api-inference.huggingface.co/models/{}", model);
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
                let error_text = resp.text().await.unwrap_or_default();
                return (StatusCode::INTERNAL_SERVER_ERROR, error_text).into_response();
            }
            match resp.bytes().await {
                Ok(bytes) => {
                    let b64 = base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &bytes,
                    );
                    Json(json!({"images": [b64]})).into_response()
                }
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("HuggingFace image error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sd/electronhub/models` — ElectronHub image models.
pub async fn electronhub_models(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELECTRONHUB) {
        Some(k) if !k.is_empty() => k,
        _ => return Json(json!([])).into_response(),
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

/// `POST /api/sd/electronhub/generate` — ElectronHub image generation.
pub async fn electronhub_generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_ELECTRONHUB) {
        Some(k) if !k.is_empty() => k,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response(),
    };

    let client = http_client();

    match client
        .post("https://api.electronhub.ai/v1/images/generations")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                return (StatusCode::INTERNAL_SERVER_ERROR, error_text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("ElectronHub image error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sd/chutes/models` — Chutes diffusion models.
pub async fn chutes_models(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_CHUTES) {
        Some(k) if !k.is_empty() => k,
        _ => return Json(json!([])).into_response(),
    };

    let client = http_client();

    match client
        .get("https://api.chutes.ai/chutes/?template=diffusion&include_public=true&limit=999")
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

/// `POST /api/sd/chutes/generate` — Chutes image generation.
pub async fn chutes_generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_CHUTES) {
        Some(k) if !k.is_empty() => k,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response(),
    };

    let client = http_client();

    match client
        .post("https://image.chutes.ai/generate")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                return (StatusCode::INTERNAL_SERVER_ERROR, error_text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("Chutes image error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sd/comfy/ping` — Ping ComfyUI.
pub async fn comfy_ping(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SdApiRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let client = http_client();
    let ping_url = format!("{}/system_stats", url.trim_end_matches('/'));

    match client.get(&ping_url).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<Value>().await {
                    Ok(data) => Json(data).into_response(),
                    Err(_) => StatusCode::OK.into_response(),
                }
            } else {
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Generic proxy for external image API providers (nanogpt, bfl, falai, xai, aimlapi, zai).
/// This is a pass-through handler.
pub async fn generic_proxy_generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let provider = body.get("provider").and_then(|v| v.as_str()).unwrap_or("");
    let secret_key = match provider {
        "nanogpt" => SECRET_NANOGPT,
        "bfl" => SECRET_BFL,
        "falai" => SECRET_FALAI,
        "xai" => SECRET_XAI,
        "aimlapi" => SECRET_AIMLAPI,
        "zai" => SECRET_ZAI,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": "Unknown provider"}))).into_response(),
    };

    let api_key = match read_secret(&state.config.data_root, &user.handle, secret_key) {
        Some(k) if !k.is_empty() => k,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response(),
    };

    let api_url = body.get("api_url").and_then(|v| v.as_str()).unwrap_or("");
    if api_url.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "No api_url"}))).into_response();
    }

    // Strip internal fields from request body
    let mut request_body = body.clone();
    if let Some(obj) = request_body.as_object_mut() {
        obj.remove("provider");
        obj.remove("api_url");
    }

    let client = http_client();

    match client
        .post(api_url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                return (StatusCode::INTERNAL_SERVER_ERROR, error_text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(e) => {
            tracing::error!("{} image error: {}", provider, e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
