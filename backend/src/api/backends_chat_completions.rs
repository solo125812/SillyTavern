//! Chat completions backend endpoints — Phase 11.
//!
//! Mirrors Node's [`src/endpoints/backends/chat-completions.js`](../../../../src/endpoints/backends/chat-completions.js).
//!
//! ## Endpoints
//! - `POST /api/backends/chat-completions/status`   — Get model list for a source.
//! - `POST /api/backends/chat-completions/bias`      — Calculate logit bias.
//! - `POST /api/backends/chat-completions/generate`  — Generate chat completion.
//! - `POST /api/backends/chat-completions/process`   — Process/post-process prompt.
//!
//! ### Multimodal model sub-routes
//! - `POST /api/backends/chat-completions/multimodal-models/pollinations`
//! - `POST /api/backends/chat-completions/multimodal-models/aimlapi`
//! - `POST /api/backends/chat-completions/multimodal-models/nanogpt`
//! - `POST /api/backends/chat-completions/multimodal-models/electronhub`
//! - `POST /api/backends/chat-completions/multimodal-models/chutes`
//! - `POST /api/backends/chat-completions/multimodal-models/mistral`
//! - `POST /api/backends/chat-completions/multimodal-models/xai`
//! - `POST /api/backends/chat-completions/multimodal-models/moonshot`

use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use reqwest::header::HeaderValue;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SECRET_OPENAI: &str = "api_key_openai";
const SECRET_CLAUDE: &str = "api_key_claude";
const SECRET_MAKERSUITE: &str = "api_key_makersuite";
const SECRET_AI21: &str = "api_key_ai21";
const SECRET_MISTRALAI: &str = "api_key_mistralai";
const SECRET_COHERE: &str = "api_key_cohere";
const SECRET_DEEPSEEK: &str = "api_key_deepseek";
const SECRET_OPENROUTER: &str = "api_key_openrouter";
const SECRET_CUSTOM: &str = "api_key_custom";
const SECRET_PERPLEXITY: &str = "api_key_perplexity";
const SECRET_GROQ: &str = "api_key_groq";
const SECRET_FIREWORKS: &str = "api_key_fireworks";
const SECRET_NANOGPT: &str = "api_key_nanogpt";
const SECRET_POLLINATIONS: &str = "api_key_pollinations";
const SECRET_XAI: &str = "api_key_xai";
const SECRET_CHUTES: &str = "api_key_chutes";
const SECRET_ELECTRONHUB: &str = "api_key_electronhub";
const SECRET_AIMLAPI: &str = "api_key_aimlapi";
const SECRET_MOONSHOT: &str = "api_key_moonshot";
const SECRET_AZURE_OPENAI: &str = "api_key_azure_openai";
const SECRET_ZAI: &str = "api_key_zai";
const SECRET_SILICONFLOW: &str = "api_key_siliconflow";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StatusRequest {
    pub chat_completion_source: Option<String>,
    pub api_server: Option<String>,
    pub reverse_proxy: Option<String>,
    pub proxy_password: Option<String>,
    pub model: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Deserialize)]
pub struct BiasRequest {
    pub text: Option<String>,
    pub bias_preset: Option<Value>,
    pub model: Option<String>,
    pub chat_completion_source: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProcessRequest {
    pub messages: Option<Value>,
    #[serde(rename = "type")]
    pub processing_type: Option<String>,
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

/// Get the secret key for a chat completion source.
fn get_secret_for_source(source: &str) -> &str {
    match source {
        "openai" => SECRET_OPENAI,
        "claude" => SECRET_CLAUDE,
        "makersuite" | "google" | "vertexai" => SECRET_MAKERSUITE,
        "ai21" => SECRET_AI21,
        "mistralai" => SECRET_MISTRALAI,
        "cohere" => SECRET_COHERE,
        "deepseek" => SECRET_DEEPSEEK,
        "openrouter" => SECRET_OPENROUTER,
        "custom" => SECRET_CUSTOM,
        "perplexity" => SECRET_PERPLEXITY,
        "groq" => SECRET_GROQ,
        "fireworks" => SECRET_FIREWORKS,
        "nanogpt" => SECRET_NANOGPT,
        "pollinations" => SECRET_POLLINATIONS,
        "xai" => SECRET_XAI,
        "chutes" => SECRET_CHUTES,
        "electronhub" => SECRET_ELECTRONHUB,
        "aimlapi" => SECRET_AIMLAPI,
        "moonshot" => SECRET_MOONSHOT,
        "azure_openai" => SECRET_AZURE_OPENAI,
        "zai" => SECRET_ZAI,
        "siliconflow" => SECRET_SILICONFLOW,
        _ => SECRET_CUSTOM,
    }
}

/// Get the API URL for a chat completion source.
fn get_api_url_for_source(source: &str) -> &str {
    match source {
        "openai" => "https://api.openai.com/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "mistralai" => "https://api.mistral.ai/v1",
        "cohere" => "https://api.cohere.com/v2",
        "deepseek" => "https://api.deepseek.com/v1",
        "perplexity" => "https://api.perplexity.ai",
        "groq" => "https://api.groq.com/openai/v1",
        "fireworks" => "https://api.fireworks.ai/inference/v1",
        "nanogpt" => "https://nano-gpt.com/api/v1",
        "pollinations" => "https://gen.pollinations.ai/v1",
        "xai" => "https://api.x.ai/v1",
        "chutes" => "https://llm.chutes.ai/v1",
        "electronhub" => "https://api.electronhub.ai/v1",
        "aimlapi" => "https://api.aimlapi.com/v1",
        "moonshot" => "https://api.moonshot.ai/v1",
        "siliconflow" => "https://api.siliconflow.cn/v1",
        "ai21" => "https://api.ai21.com/studio/v1",
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/backends/chat-completions/status` — Get model list.
///
/// Mirrors Node's `router.post('/status')` in `chat-completions.js:1639-1915`.
/// This endpoint handles ~30 different sources for model listing.
pub async fn cc_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<StatusRequest>,
) -> Response {
    let source = body.chat_completion_source.clone().unwrap_or_else(|| "openai".to_string());
    let secret_name = get_secret_for_source(&source);

    let api_key = if body.reverse_proxy.is_some() {
        body.proxy_password.clone()
    } else {
        read_secret(&state.config.data_root, &user.handle, secret_name)
    };

    let api_key = api_key.unwrap_or_default();

    // Special cases
    match source.as_str() {
        "claude" => {
            if api_key.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(json!({"error": "No API key"}))).into_response();
            }
            let url = body.reverse_proxy.clone()
                .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string());
            let models_url = format!("{}/models?limit=1000", url.trim_end_matches('/'));
            let client = http_client();

            match client
                .get(&models_url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_success() {
                        if let Ok(data) = resp.json::<Value>().await {
                            return Json(data).into_response();
                        }
                    }
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        "makersuite" | "google" => {
            if api_key.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(json!({"error": "No API key"}))).into_response();
            }
            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models?key={}",
                api_key
            );
            let client = http_client();

            match client.get(&url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        if let Ok(data) = resp.json::<Value>().await {
                            return Json(data).into_response();
                        }
                    }
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        "azure_openai" => {
            // Azure OpenAI requires special handling with deployment URLs
            let url = body.reverse_proxy.clone().unwrap_or_default();
            if url.is_empty() || api_key.is_empty() {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "Missing Azure config"}))).into_response();
            }
            let models_url = format!("{}/models?api-version=2023-03-15-preview", url.trim_end_matches('/'));
            let client = http_client();

            match client
                .get(&models_url)
                .header("api-key", &api_key)
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_success() {
                        if let Ok(data) = resp.json::<Value>().await {
                            return Json(data).into_response();
                        }
                    }
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        _ => {
            // Generic OpenAI-compatible model listing
            let base_url = body.reverse_proxy.clone()
                .unwrap_or_else(|| get_api_url_for_source(&source).to_string());

            if base_url.is_empty() {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "No API URL"}))).into_response();
            }

            let models_url = format!("{}/models", base_url.trim_end_matches('/'));
            let client = http_client();

            let mut req = client.get(&models_url);
            if !api_key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", api_key));
            }

            // Add OpenRouter-specific headers
            if source == "openrouter" {
                req = req
                    .header("HTTP-Referer", "https://sillytavern.app")
                    .header("X-Title", "SillyTavern");
            }

            match req.send().await {
                Ok(resp) => {
                    let status_code = resp.status();
                    if status_code.is_success() {
                        if let Ok(data) = resp.json::<Value>().await {
                            return Json(data).into_response();
                        }
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                    let status = status_code.as_u16();
                    let error_text = resp.text().await.unwrap_or_default();
                    return (
                        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                        error_text,
                    )
                        .into_response();
                }
                Err(e) => {
                    tracing::error!("Chat completions status error: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
    }
}

/// `POST /api/backends/chat-completions/bias` — Calculate logit bias.
///
/// Mirrors Node's `router.post('/bias')` in `chat-completions.js:1917-2010`.
/// Note: Tokenizer-based logit bias calculation requires tiktoken or similar.
/// This returns an empty object as the bias should be computed client-side.
pub async fn cc_bias(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(_body): Json<BiasRequest>,
) -> Response {
    // The Node implementation uses tokenizer to convert text → token IDs for logit bias.
    // Without a tokenizer in Rust, return an empty object so the client can handle it.
    Json(json!({})).into_response()
}

/// `POST /api/backends/chat-completions/generate` — Generate chat completion.
///
/// Mirrors Node's `router.post('/generate')` in `chat-completions.js:2012-2441`.
/// This is the central endpoint that routes to ~20 different providers.
pub async fn cc_generate(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<Value>,
) -> Response {
    let source = body.get("chat_completion_source").and_then(|v| v.as_str()).unwrap_or("openai");
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);

    // For Claude, Google, AI21, Mistral, Cohere, DeepSeek, xAI, AIMLAPI,
    // ElectronHub, Chutes, and Azure OpenAI, the Node code has dedicated
    // send*Request functions. We handle all via the OpenAI-compatible path
    // with provider-specific adjustments.

    let secret_name = get_secret_for_source(source);

    let api_key = if body.get("reverse_proxy").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).is_some() {
        body.get("proxy_password").and_then(|v| v.as_str()).map(|s| s.to_string())
    } else {
        read_secret(&state.config.data_root, &user.handle, secret_name)
    };

    let api_key = match &api_key {
        Some(k) if !k.is_empty() => k.clone(),
        _ => {
            // Some sources (custom) may work without key
            if source != "custom" && source != "pollinations" {
                tracing::warn!("API key missing for source: {}", source);
                return (StatusCode::BAD_REQUEST, Json(json!({"error": true}))).into_response();
            }
            String::new()
        }
    };

    // Determine the API URL
    let api_url = if let Some(rp) = body.get("reverse_proxy").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        rp.to_string()
    } else if source == "custom" {
        body.get("custom_url").and_then(|v| v.as_str()).unwrap_or("").to_string()
    } else {
        get_api_url_for_source(source).to_string()
    };

    if api_url.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "No API URL"}))).into_response();
    }

    // Special handling for Claude
    if source == "claude" {
        return send_claude_request(&state, &user, &api_key, &api_url, &body, stream).await;
    }

    // Special handling for Google/MakerSuite
    if source == "makersuite" || source == "google" || source == "vertexai" {
        return send_google_request(&state, &user, &api_key, &api_url, &body).await;
    }

    // Build the generic OpenAI-compatible request
    let endpoint_url = format!("{}/chat/completions", api_url.trim_end_matches('/'));

    // Build request body - keep most fields, strip internal ones
    let mut request_body = json!({});
    if let Some(obj) = body.as_object() {
        for (key, val) in obj {
            match key.as_str() {
                "chat_completion_source" | "api_server" | "reverse_proxy" | "proxy_password"
                | "custom_url" | "custom_include_body" | "custom_include_headers"
                | "custom_exclude_body" | "custom_prompt_post_processing" | "json_schema"
                | "api_type" => {}
                _ => {
                    request_body[key] = val.clone();
                }
            }
        }
    }

    let client = http_client();

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    if !api_key.is_empty() {
        let auth_value = match HeaderValue::from_str(&format!("Bearer {}", api_key)) {
            Ok(value) => value,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": {"message": "Invalid API key header value"}})),
                )
                    .into_response();
            }
        };
        headers.insert("Authorization", auth_value);
    }

    // Source-specific headers
    if source == "openrouter" {
        headers.insert("HTTP-Referer", HeaderValue::from_static("https://sillytavern.app"));
        headers.insert("X-Title", HeaderValue::from_static("SillyTavern"));
    }

    if source == "azure_openai" {
        if let Ok(val) = api_key.parse() {
            headers.insert("api-key", val);
        }
        headers.remove("Authorization");
    }

    match client
        .post(&endpoint_url)
        .headers(headers)
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if stream && resp.status().is_success() {
                let content_type = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("text/event-stream")
                    .to_string();

                let stream = resp.bytes_stream();
                return axum::response::Response::builder()
                    .header("Content-Type", content_type)
                    .header("Transfer-Encoding", "chunked")
                    .body(axum::body::Body::from_stream(stream))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }

            if resp.status().is_success() {
                match resp.json::<Value>().await {
                    Ok(data) => Json(data).into_response(),
                    Err(e) => {
                        tracing::error!("Chat completion parse error: {}", e);
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                }
            } else {
                let status = resp.status().as_u16();
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Chat completion error ({}): {}", status, error_text);

                let quota_error = status == 429;
                Json(json!({
                    "error": {"message": error_text},
                    "quota_error": quota_error,
                }))
                .into_response()
            }
        }
        Err(e) => {
            tracing::error!("Chat completion request error: {}", e);
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

/// Send a Claude/Anthropic request.
async fn send_claude_request(
    _state: &AppState,
    _user: &UserContext,
    api_key: &str,
    api_url: &str,
    body: &Value,
    stream: bool,
) -> Response {
    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("claude-3-5-sonnet-latest");
    let messages = body.get("messages").cloned().unwrap_or(json!([]));
    let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(4096);
    let temperature = body.get("temperature").and_then(|v| v.as_f64()).unwrap_or(1.0);

    let mut request_body = json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "stream": stream,
    });

    // Add optional params
    if let Some(top_p) = body.get("top_p") {
        request_body["top_p"] = top_p.clone();
    }
    if let Some(top_k) = body.get("top_k") {
        request_body["top_k"] = top_k.clone();
    }
    if let Some(stop) = body.get("stop") {
        request_body["stop_sequences"] = stop.clone();
    }

    // Add system prompt if first message has role "system"
    if let Some(msgs) = messages.as_array() {
        if let Some(first) = msgs.first() {
            if first.get("role").and_then(|r| r.as_str()) == Some("system") {
                request_body["system"] = first.get("content").cloned().unwrap_or(json!(""));
                // Remove system message from messages array
                let non_system: Vec<Value> = msgs.iter().skip(1).cloned().collect();
                request_body["messages"] = json!(non_system);
            }
        }
    }

    let url = format!("{}/messages", api_url.trim_end_matches('/'));
    let client = http_client();

    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if stream && resp.status().is_success() {
                let stream = resp.bytes_stream();
                return axum::response::Response::builder()
                    .header("Content-Type", "text/event-stream")
                    .body(axum::body::Body::from_stream(stream))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }

            if resp.status().is_success() {
                match resp.json::<Value>().await {
                    Ok(data) => Json(data).into_response(),
                    Err(e) => {
                        tracing::error!("Claude parse error: {}", e);
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                }
            } else {
                let status = resp.status().as_u16();
                let error_text = resp.text().await.unwrap_or_default();
                (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    error_text,
                )
                    .into_response()
            }
        }
        Err(e) => {
            tracing::error!("Claude request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Send a Google/MakerSuite request.
async fn send_google_request(
    _state: &AppState,
    _user: &UserContext,
    api_key: &str,
    _api_url: &str,
    body: &Value,
) -> Response {
    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("gemini-1.5-flash-latest");
    let messages = body.get("messages").cloned().unwrap_or(json!([]));

    // Convert OpenAI-style messages to Gemini format
    let mut contents: Vec<Value> = Vec::new();

    if let Some(msgs) = messages.as_array() {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = msg.get("content").cloned().unwrap_or(json!(""));

            let gemini_role = match role {
                "assistant" => "model",
                "system" => "user", // Gemini doesn't have system role
                _ => "user",
            };

            let parts = if let Some(s) = content.as_str() {
                json!([{"text": s}])
            } else {
                content
            };

            contents.push(json!({
                "role": gemini_role,
                "parts": parts,
            }));
        }
    }

    let temperature = body.get("temperature").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(4096);

    let request_body = json!({
        "contents": contents,
        "generationConfig": {
            "temperature": temperature,
            "maxOutputTokens": max_tokens,
        },
    });

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );
    let client = http_client();

    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<Value>().await {
                    Ok(data) => Json(data).into_response(),
                    Err(e) => {
                        tracing::error!("Google parse error: {}", e);
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                }
            } else {
                let error_text = resp.text().await.unwrap_or_default();
                tracing::error!("Google generate error: {}", error_text);
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
        Err(e) => {
            tracing::error!("Google request error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/backends/chat-completions/process` — Process prompt.
///
/// Mirrors Node's `router.post('/process')` in `chat-completions.js:2658-2674`.
pub async fn cc_process(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<ProcessRequest>,
) -> Response {
    let messages = match &body.messages {
        Some(msgs) if msgs.is_array() => msgs.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "Invalid messages format"}))).into_response();
        }
    };

    // The Node implementation uses postProcessPrompt to handle things like
    // merging consecutive messages, etc. For now, pass through unchanged.
    Json(json!({"messages": messages})).into_response()
}

// ---------------------------------------------------------------------------
// Multimodal model handlers
// ---------------------------------------------------------------------------

/// `POST /api/backends/chat-completions/multimodal-models/pollinations`
pub async fn mm_pollinations(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    let client = http_client();

    match client.get("https://gen.pollinations.ai/models").send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    if let Some(arr) = data.as_array() {
                        let models: Vec<String> = arr
                            .iter()
                            .filter(|m| {
                                m.get("input_modalities")
                                    .and_then(|im| im.as_array())
                                    .map(|arr| arr.iter().any(|v| v.as_str() == Some("image")))
                                    .unwrap_or(false)
                            })
                            .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                            .collect();
                        Json(json!(models)).into_response()
                    } else {
                        Json(json!([])).into_response()
                    }
                }
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}

/// `POST /api/backends/chat-completions/multimodal-models/aimlapi`
pub async fn mm_aimlapi(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    let client = http_client();

    match client.get("https://api.aimlapi.com/v1/models").send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let models: Vec<String> = data
                        .get("data")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter(|m| {
                                    m.get("features")
                                        .and_then(|f| f.as_array())
                                        .map(|arr| arr.iter().any(|v| v.as_str() == Some("openai/chat-completion.vision")))
                                        .unwrap_or(false)
                                })
                                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    Json(json!(models)).into_response()
                }
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}

/// `POST /api/backends/chat-completions/multimodal-models/nanogpt`
pub async fn mm_nanogpt(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    let client = http_client();

    match client.get("https://nano-gpt.com/api/v1/models?detailed=true").send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let models: Vec<String> = data
                        .get("data")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter(|m| {
                                    m.get("capabilities")
                                        .and_then(|c| c.get("vision"))
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false)
                                })
                                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    Json(json!(models)).into_response()
                }
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}

/// `POST /api/backends/chat-completions/multimodal-models/electronhub`
pub async fn mm_electronhub(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    let client = http_client();

    match client.get("https://api.electronhub.ai/v1/models").send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let models: Vec<String> = data
                        .get("data")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter(|m| {
                                    m.get("metadata")
                                        .and_then(|md| md.get("vision"))
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false)
                                })
                                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    Json(json!(models)).into_response()
                }
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}

/// `POST /api/backends/chat-completions/multimodal-models/chutes`
pub async fn mm_chutes(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_CHUTES) {
        Some(k) if !k.is_empty() => k,
        _ => return Json(json!([])).into_response(),
    };

    let client = http_client();

    match client
        .get("https://llm.chutes.ai/v1/models")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let models: Vec<String> = data
                        .get("data")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter(|m| {
                                    m.get("input_modalities")
                                        .and_then(|im| im.as_array())
                                        .map(|arr| arr.iter().any(|v| v.as_str() == Some("image")))
                                        .unwrap_or(false)
                                })
                                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    Json(json!(models)).into_response()
                }
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}

/// `POST /api/backends/chat-completions/multimodal-models/mistral`
pub async fn mm_mistral(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_MISTRALAI) {
        Some(k) if !k.is_empty() => k,
        _ => return Json(json!([])).into_response(),
    };

    let client = http_client();

    match client
        .get("https://api.mistral.ai/v1/models")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let models: Vec<String> = data
                        .get("data")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter(|m| {
                                    m.get("capabilities")
                                        .and_then(|c| c.get("vision"))
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false)
                                })
                                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    Json(json!(models)).into_response()
                }
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}

/// `POST /api/backends/chat-completions/multimodal-models/xai`
pub async fn mm_xai(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_XAI) {
        Some(k) if !k.is_empty() => k,
        _ => return Json(json!([])).into_response(),
    };

    let client = http_client();

    match client
        .get("https://api.x.ai/v1/language-models")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let mut models: Vec<String> = data
                        .get("models")
                        .and_then(|m| m.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter(|m| {
                                    m.get("input_modalities")
                                        .and_then(|im| im.as_array())
                                        .map(|arr| arr.iter().any(|v| v.as_str() == Some("image")))
                                        .unwrap_or(false)
                                })
                                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();

                    // The Node code hardcodes grok-4-0709 as supporting images
                    if !models.contains(&"grok-4-0709".to_string()) {
                        models.push("grok-4-0709".to_string());
                    }

                    Json(json!(models)).into_response()
                }
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}

/// `POST /api/backends/chat-completions/multimodal-models/moonshot`
pub async fn mm_moonshot(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_MOONSHOT) {
        Some(k) if !k.is_empty() => k,
        _ => return Json(json!([])).into_response(),
    };

    let client = http_client();

    match client
        .get("https://api.moonshot.ai/v1/models")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Json(json!([])).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let models: Vec<String> = data
                        .get("data")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter(|m| {
                                    m.get("supports_image_in")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false)
                                })
                                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    Json(json!(models)).into_response()
                }
                Err(_) => Json(json!([])).into_response(),
            }
        }
        Err(_) => Json(json!([])).into_response(),
    }
}
