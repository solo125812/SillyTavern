//! Anthropic endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/anthropic.js`](../../../src/endpoints/anthropic.js).
//!
//! ## Endpoints
//! - `POST /api/anthropic/caption-image` — Caption an image using Claude vision.

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

const SECRET_CLAUDE: &str = "api_key_claude";
const API_CLAUDE: &str = "https://api.anthropic.com/v1";

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

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/anthropic/caption-image` — Caption an image using Claude vision.
///
/// Mirrors Node's `router.post('/caption-image')` in `anthropic.js:6-62`.
pub async fn caption_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<CaptionImageRequest>,
) -> Response {
    let api_key = if body.reverse_proxy.is_some() {
        body.proxy_password.clone()
    } else {
        read_secret(&state.config.data_root, &user.handle, SECRET_CLAUDE)
    };

    let api_key = match api_key {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("Claude API key is not configured.");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
        }
    };

    let image = match &body.image {
        Some(img) if !img.is_empty() => img.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No image provided"}))).into_response();
        }
    };

    let model = body.model.clone().unwrap_or_else(|| "claude-3-5-sonnet-latest".to_string());
    let prompt = body.prompt.clone().unwrap_or_else(|| "What's in this image?".to_string());

    // Parse base64 image data URI
    let (media_type, base64_data) = if let Some(rest) = image.strip_prefix("data:") {
        if let Some((mime, data)) = rest.split_once(";base64,") {
            (mime.to_string(), data.to_string())
        } else {
            ("image/png".to_string(), image.clone())
        }
    } else {
        ("image/png".to_string(), image.clone())
    };

    let api_url = body.reverse_proxy.clone().unwrap_or_else(|| API_CLAUDE.to_string());

    let request_body = json!({
        "model": model,
        "max_tokens": 500,
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": base64_data,
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

    let url = format!("{}/messages", api_url.trim_end_matches('/'));
    let client = http_client();

    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Claude caption error ({}): {}", status, error_text);
                return (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    error_text,
                )
                    .into_response();
            }
            match resp.json::<Value>().await {
                Ok(json_val) => Json(json_val).into_response(),
                Err(e) => {
                    tracing::error!("Claude caption parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Claude caption request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
