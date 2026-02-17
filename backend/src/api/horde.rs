//! AI Horde endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/horde.js`](../../../src/endpoints/horde.js).
//!
//! ## Endpoints
//! - `POST /api/horde/text-workers`   — Get text workers list.
//! - `POST /api/horde/text-models`    — Get text models list.
//! - `POST /api/horde/status`         — Heartbeat check.
//! - `POST /api/horde/cancel-task`    — Cancel a generation task.
//! - `POST /api/horde/task-status`    — Check task status.
//! - `POST /api/horde/generate-text`  — Generate text asynchronously.
//! - `POST /api/horde/sd-samplers`    — Get SD sampler list.
//! - `POST /api/horde/sd-models`      — Get SD model list.
//! - `POST /api/horde/caption-image`  — Caption an image via interrogation.
//! - `POST /api/horde/user-info`      — Get user/key info.
//! - `POST /api/horde/generate-image` — Generate image asynchronously.

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

const SECRET_HORDE: &str = "api_key_horde";
const HORDE_API_BASE: &str = "https://aihorde.net/api/v2";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CancelTaskRequest {
    #[serde(rename = "taskId")]
    pub task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TaskStatusRequest {
    #[serde(rename = "taskId")]
    pub task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CaptionImageRequest {
    pub image: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UserInfoRequest {
    pub api_key_horde: Option<String>,
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

/// Sanitize prompt for Horde image generation.
fn sanitize_horde_image_prompt(prompt: &str) -> String {
    // Remove disallowed patterns
    let re_csam = regex::Regex::new(r"(?i)(child|underage|minor|loli|shota)").unwrap_or_else(|_| regex::Regex::new(".^").unwrap());
    re_csam.replace_all(prompt, "").to_string()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/horde/text-workers` — Get text worker list.
///
/// Mirrors Node's `router.post('/text-workers')` in `horde.js:62-77`.
pub async fn text_workers(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    let url = format!("{}/workers?type=text", HORDE_API_BASE);
    let client = http_client();

    match client
        .get(&url)
        .header("Client-Agent", "SillyTavern:UNKNOWN:1")
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
            tracing::error!("Horde text-workers error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/horde/text-models` — Get text model list.
///
/// Mirrors Node's `router.post('/text-models')` in `horde.js:79-127`.
pub async fn text_models(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    let url = format!("{}/status/models?type=text", HORDE_API_BASE);
    let client = http_client();

    match client
        .get(&url)
        .header("Client-Agent", "SillyTavern:UNKNOWN:1")
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
            tracing::error!("Horde text-models error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/horde/status` — Heartbeat check.
///
/// Mirrors Node's `router.post('/status')` in `horde.js:129-143`.
pub async fn horde_status(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    let url = format!("{}/status/heartbeat", HORDE_API_BASE);
    let client = http_client();

    match client
        .get(&url)
        .header("Client-Agent", "SillyTavern:UNKNOWN:1")
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            (
                StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                resp.text().await.unwrap_or_default(),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Horde status error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/horde/cancel-task` — Cancel a generation task.
///
/// Mirrors Node's `router.post('/cancel-task')` in `horde.js:145-167`.
pub async fn cancel_task(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<CancelTaskRequest>,
) -> Response {
    let task_id = match &body.task_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No taskId"}))).into_response();
        }
    };

    let url = format!(
        "{}/generate/text/status/{}",
        HORDE_API_BASE, task_id
    );
    let client = http_client();

    match client
        .delete(&url)
        .header("Client-Agent", "SillyTavern:UNKNOWN:1")
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
            tracing::error!("Horde cancel-task error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/horde/task-status` — Check task status.
///
/// Mirrors Node's `router.post('/task-status')` in `horde.js:169-186`.
pub async fn task_status(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<TaskStatusRequest>,
) -> Response {
    let task_id = match &body.task_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No taskId"}))).into_response();
        }
    };

    let url = format!(
        "{}/generate/text/status/{}",
        HORDE_API_BASE, task_id
    );
    let client = http_client();

    match client
        .get(&url)
        .header("Client-Agent", "SillyTavern:UNKNOWN:1")
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
            tracing::error!("Horde task-status error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/horde/generate-text` — Generate text asynchronously.
///
/// Mirrors Node's `router.post('/generate-text')` in `horde.js:188-237`.
pub async fn generate_text(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_HORDE)
        .unwrap_or_else(|| "0000000000".to_string());

    let url = format!("{}/generate/text/async", HORDE_API_BASE);
    let client = http_client();

    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("apikey", &api_key)
        .header("Client-Agent", "SillyTavern:UNKNOWN:1")
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Horde generate-text error ({}): {}", status, error_text);
                return (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    error_text,
                )
                    .into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("Horde generate-text parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Horde generate-text request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/horde/sd-samplers` — Get SD sampler list.
///
/// Mirrors Node's `router.post('/sd-samplers')` in `horde.js:239-241`.
pub async fn sd_samplers(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    // Return the known sampler list
    Json(json!([
        "k_lms",
        "k_heun",
        "k_euler",
        "k_euler_a",
        "k_dpm_2",
        "k_dpm_2_a",
        "k_dpm_fast",
        "k_dpm_adaptive",
        "k_dpmpp_2s_a",
        "k_dpmpp_2m",
        "k_dpmpp_sde",
        "DDIM",
        "dpmsolver",
        "lcm"
    ]))
    .into_response()
}

/// `POST /api/horde/sd-models` — Get SD model list.
///
/// Mirrors Node's `router.post('/sd-models')` in `horde.js:243-260`.
pub async fn sd_models(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    let url = format!("{}/status/models?type=image", HORDE_API_BASE);
    let client = http_client();

    match client
        .get(&url)
        .header("Client-Agent", "SillyTavern:UNKNOWN:1")
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
            tracing::error!("Horde sd-models error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/horde/caption-image` — Caption an image via interrogation.
///
/// Mirrors Node's `router.post('/caption-image')` in `horde.js:262-307`.
pub async fn caption_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<CaptionImageRequest>,
) -> Response {
    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_HORDE)
        .unwrap_or_else(|| "0000000000".to_string());

    let image = match &body.image {
        Some(img) if !img.is_empty() => img.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "No image"}))).into_response();
        }
    };

    let client = http_client();

    // Submit interrogation request
    let interrogate_body = json!({
        "source_image": image,
        "forms": [{"name": "caption"}],
    });

    let submit_url = format!("{}/interrogate/async", HORDE_API_BASE);

    let submit_resp = match client
        .post(&submit_url)
        .header("Content-Type", "application/json")
        .header("apikey", &api_key)
        .header("Client-Agent", "SillyTavern:UNKNOWN:1")
        .json(&interrogate_body)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Horde caption submit error: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if !submit_resp.status().is_success() {
        let error_text = submit_resp.text().await.unwrap_or_default();
        tracing::error!("Horde caption submit error: {}", error_text);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let submit_data: Value = match submit_resp.json().await {
        Ok(d) => d,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let task_id = match submit_data.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // Poll for result
    let status_url = format!("{}/interrogate/status/{}", HORDE_API_BASE, task_id);

    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        match client
            .get(&status_url)
            .header("Client-Agent", "SillyTavern:UNKNOWN:1")
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(data) = resp.json::<Value>().await {
                    let state_str = data.get("state").and_then(|v| v.as_str()).unwrap_or("");
                    if state_str == "done" {
                        // Extract caption from forms
                        if let Some(forms) = data.get("forms").and_then(|f| f.as_array()) {
                            for form in forms {
                                if form.get("form").and_then(|f| f.as_str()) == Some("caption") {
                                    if let Some(result) = form.get("result") {
                                        if let Some(caption) = result.get("caption").and_then(|c| c.as_str()) {
                                            return Json(json!({"caption": caption})).into_response();
                                        }
                                    }
                                }
                            }
                        }
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    } else if state_str == "faulted" || state_str == "cancelled" {
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                    // Still processing, continue polling
                }
            }
            Err(e) => {
                tracing::error!("Horde caption poll error: {}", e);
            }
        }
    }

    // Timeout
    (StatusCode::REQUEST_TIMEOUT, "Caption timed out").into_response()
}

/// `POST /api/horde/user-info` — Get user info.
///
/// Mirrors Node's `router.post('/user-info')` in `horde.js:309-340`.
pub async fn user_info(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<UserInfoRequest>,
) -> Response {
    let api_key = body
        .api_key_horde
        .clone()
        .or_else(|| read_secret(&state.config.data_root, &user.handle, SECRET_HORDE))
        .unwrap_or_else(|| "0000000000".to_string());

    let client = http_client();

    // Try to get user info with the key
    let url = format!("{}/find_user", HORDE_API_BASE);

    match client
        .get(&url)
        .header("apikey", &api_key)
        .header("Client-Agent", "SillyTavern:UNKNOWN:1")
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
            tracing::error!("Horde user-info error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/horde/generate-image` — Generate image asynchronously.
///
/// Mirrors Node's `router.post('/generate-image')` in `horde.js:342-411`.
pub async fn generate_image(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let api_key = read_secret(&state.config.data_root, &user.handle, SECRET_HORDE)
        .unwrap_or_else(|| "0000000000".to_string());

    // Sanitize prompt if present
    let mut request_body = body.clone();
    if let Some(prompt) = request_body.get("prompt").and_then(|p| p.as_str()) {
        let sanitized = sanitize_horde_image_prompt(prompt);
        request_body["prompt"] = json!(sanitized);
    }

    let url = format!("{}/generate/async", HORDE_API_BASE);
    let client = http_client();

    // Submit generation request
    let submit_resp = match client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("apikey", &api_key)
        .header("Client-Agent", "SillyTavern:UNKNOWN:1")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Horde image submit error: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if !submit_resp.status().is_success() {
        let status = submit_resp.status().as_u16();
        let error_text = submit_resp.text().await.unwrap_or_default();
        tracing::error!("Horde image submit error ({}): {}", status, error_text);
        return (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            error_text,
        )
            .into_response();
    }

    let submit_data: Value = match submit_resp.json().await {
        Ok(d) => d,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let task_id = match submit_data.get("id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return Json(submit_data).into_response(),
    };

    // Poll for result
    let status_url = format!(
        "{}/generate/status/{}",
        HORDE_API_BASE, task_id
    );

    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        match client
            .get(&status_url)
            .header("Client-Agent", "SillyTavern:UNKNOWN:1")
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(data) = resp.json::<Value>().await {
                    let done = data.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
                    let faulted = data.get("faulted").and_then(|v| v.as_bool()).unwrap_or(false);

                    if faulted {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": "Generation faulted"})),
                        )
                            .into_response();
                    }

                    if done {
                        return Json(data).into_response();
                    }
                    // Still processing, continue polling
                }
            }
            Err(e) => {
                tracing::error!("Horde image poll error: {}", e);
            }
        }
    }

    // Timeout
    (StatusCode::REQUEST_TIMEOUT, Json(json!({"error": "Image generation timed out"}))).into_response()
}
