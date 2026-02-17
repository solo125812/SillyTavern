//! Text completions backend endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/backends/text-completions.js`](../../../../src/endpoints/backends/text-completions.js).
//!
//! ## Endpoints
//! - `POST /api/backends/text-completions/status`   — Get model status.
//! - `POST /api/backends/text-completions/props`     — Get server properties.
//! - `POST /api/backends/text-completions/generate`  — Generate text completions.

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

const SECRET_MANCER: &str = "api_key_mancer";
const SECRET_APHRODITE: &str = "api_key_aphrodite";
const SECRET_TABBY: &str = "api_key_tabby";
const SECRET_VLLM: &str = "api_key_vllm";
const SECRET_INFERMATICAI: &str = "api_key_infermaticai";
const SECRET_DREAMGEN: &str = "api_key_dreamgen";
const SECRET_OOBA: &str = "api_key_ooba";
const SECRET_FEATHERLESS: &str = "api_key_featherless";
const SECRET_HUGGINGFACE: &str = "api_key_huggingface";
const SECRET_LLAMACPP: &str = "api_key_llamacpp";
const SECRET_KOBOLDCPP: &str = "api_key_koboldcpp";
const SECRET_OPENROUTER: &str = "api_key_openrouter";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StatusRequest {
    pub api_server: Option<String>,
    #[serde(rename = "api_type")]
    pub api_type: Option<String>,
    pub legacy_api: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct PropsRequest {
    pub api_server: Option<String>,
    #[serde(rename = "api_type")]
    pub api_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateRequest {
    pub api_server: Option<String>,
    #[serde(rename = "api_type")]
    pub api_type: Option<String>,
    pub model: Option<String>,
    pub stream: Option<bool>,
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

/// Get the secret key for an API type.
fn get_secret_key_for_api_type(api_type: &str) -> &str {
    match api_type {
        "mancer" => SECRET_MANCER,
        "aphrodite" => SECRET_APHRODITE,
        "tabby" => SECRET_TABBY,
        "vllm" => SECRET_VLLM,
        "infermaticai" => SECRET_INFERMATICAI,
        "dreamgen" => SECRET_DREAMGEN,
        "ooba" => SECRET_OOBA,
        "featherless" => SECRET_FEATHERLESS,
        "huggingface" => SECRET_HUGGINGFACE,
        "llamacpp" => SECRET_LLAMACPP,
        "koboldcpp" => SECRET_KOBOLDCPP,
        "openrouter" => SECRET_OPENROUTER,
        _ => "api_key_custom",
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/backends/text-completions/status` — Get model status.
///
/// Mirrors Node's `router.post('/status')` in `text-completions.js:103-307`.
pub async fn tc_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<StatusRequest>,
) -> Response {
    let api_server = match &body.api_server {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No api_server"}))).into_response();
        }
    };

    let api_type = body.api_type.clone().unwrap_or_else(|| "koboldcpp".to_string());
    let secret_key = get_secret_key_for_api_type(&api_type);
    let api_key = read_secret(&state.config.data_root, &user.handle, secret_key)
        .unwrap_or_default();

    let base_url = api_server.trim_end_matches('/');
    let client = http_client();

    // Different API types have different model endpoints
    let model_url = match api_type.as_str() {
        "mancer" => format!("{}/oai/v1/models", base_url),
        "aphrodite" | "vllm" => format!("{}/v1/models", base_url),
        "tabby" => format!("{}/v1/model/list", base_url),
        "ooba" => format!("{}/v1/internal/model/info", base_url),
        "llamacpp" | "koboldcpp" => format!("{}/v1/models", base_url),
        "infermaticai" | "featherless" | "huggingface" => format!("{}/v1/models", base_url),
        "openrouter" => "https://openrouter.ai/api/v1/models".to_string(),
        _ => format!("{}/v1/models", base_url),
    };

    let mut req = client.get(&model_url);
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    match req.send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Text completions status error: {}", error_text);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("Text completions status parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Text completions status request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/backends/text-completions/props` — Get server properties.
///
/// Mirrors Node's `router.post('/props')` in `text-completions.js:309-350`.
pub async fn tc_props(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<PropsRequest>,
) -> Response {
    let api_server = match &body.api_server {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No api_server"}))).into_response();
        }
    };

    let api_type = body.api_type.clone().unwrap_or_else(|| "koboldcpp".to_string());
    let secret_key = get_secret_key_for_api_type(&api_type);
    let api_key = read_secret(&state.config.data_root, &user.handle, secret_key)
        .unwrap_or_default();

    let base_url = api_server.trim_end_matches('/');
    let props_url = format!("{}/props", base_url);

    let client = http_client();

    let mut req = client.get(&props_url);
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    match req.send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!({})).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(_) => Json(json!({})).into_response(),
            }
        }
        Err(_) => Json(json!({})).into_response(),
    }
}

/// `POST /api/backends/text-completions/generate` — Generate text completions.
///
/// Mirrors Node's `router.post('/generate')` in `text-completions.js:352-648`.
/// This is a complex endpoint supporting many backend types (KoboldCpp, ooba,
/// llamacpp, mancer, aphrodite, tabby, vllm, etc.)
pub async fn tc_generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_server = body.get("api_server").and_then(|v| v.as_str()).unwrap_or("");
    if api_server.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "No api_server"}))).into_response();
    }

    let api_type = body.get("api_type").and_then(|v| v.as_str()).unwrap_or("koboldcpp");
    let secret_key = get_secret_key_for_api_type(api_type);
    let api_key = read_secret(&state.config.data_root, &user.handle, secret_key)
        .unwrap_or_default();

    let base_url = api_server.trim_end_matches('/');
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);

    // Build endpoint URL based on API type
    let generate_url = match api_type {
        "mancer" => format!("{}/oai/v1/completions", base_url),
        "aphrodite" | "vllm" | "tabby" | "infermaticai" | "featherless" | "huggingface" => {
            format!("{}/v1/completions", base_url)
        }
        "koboldcpp" => format!("{}/api/v1/generate", base_url),
        "ooba" => format!("{}/v1/completions", base_url),
        "llamacpp" => format!("{}/completion", base_url),
        "openrouter" => "https://openrouter.ai/api/v1/completions".to_string(),
        _ => format!("{}/v1/completions", base_url),
    };

    // Build request body - strip internal fields
    let mut request_body = body.clone();
    if let Some(obj) = request_body.as_object_mut() {
        obj.remove("api_server");
        obj.remove("api_type");
    }

    let client = http_client();

    let mut req = client
        .post(&generate_url)
        .header("Content-Type", "application/json")
        .json(&request_body);

    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    // Add OpenRouter-specific headers
    if api_type == "openrouter" {
        req = req
            .header("HTTP-Referer", "https://sillytavern.app")
            .header("X-Title", "SillyTavern");
    }

    match req.send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Text completion generate error ({}): {}", status, error_text);
                return (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    error_text,
                )
                    .into_response();
            }

            if stream {
                // For streaming, forward the response body
                let content_type = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("text/event-stream")
                    .to_string();

                let stream = resp.bytes_stream();
                axum::response::Response::builder()
                    .header("Content-Type", content_type)
                    .header("Transfer-Encoding", "chunked")
                    .body(axum::body::Body::from_stream(stream))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
            } else {
                match resp.json::<Value>().await {
                    Ok(data) => Json(data).into_response(),
                    Err(e) => {
                        tracing::error!("Text completion generate parse error: {}", e);
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!("Text completion generate request error: {}", e);
            let message = if e.is_connect() {
                format!("Connection refused: {}", e)
            } else {
                e.to_string()
            };
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"message": message}})),
            )
                .into_response()
        }
    }
}
