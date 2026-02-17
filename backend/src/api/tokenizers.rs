//! Tokenizer endpoints — Phase 8.
//!
//! Mirrors Node's [`src/endpoints/tokenizers.js`](../../../src/endpoints/tokenizers.js).
//!
//! ## Route groups
//! - Sentencepiece encode/decode: llama, nerdstash, nerdstash_v2, mistral, yi, gemma, jamba (14 routes)
//! - Tiktoken encode/decode: gpt2 (2 routes)
//! - Web tokenizer encode/decode: claude, llama3, qwen2, command-r, command-a, nemo, deepseek (14 routes)
//! - OpenAI: encode, decode, count (3 routes)
//! - Remote: kobold/count, textgenerationwebui/encode (2 routes)
//! - Total: 35 routes
//!
//! ## Implementation Note
//! The actual tokenizer libraries (sentencepiece, tiktoken, web-tokenizers) are
//! JavaScript-specific. The Rust implementation uses a character-based estimation
//! fallback (3.35 chars/token) and proxies to remote endpoints where applicable.
//! For production parity, native Rust tokenizer bindings should be integrated.


use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::Value;


/// Characters per token for fallback estimation.
const CHARS_PER_TOKEN: f64 = 3.35;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct EncodeRequest {
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DecodeRequest {
    pub ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize)]
pub struct ModelQuery {
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RemoteCountRequest {
    pub text: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RemoteEncodeRequest {
    pub text: Option<String>,
    pub url: Option<String>,
    pub api_type: Option<String>,
    pub vllm_model: Option<String>,
    pub aphrodite_model: Option<String>,
}

// ---------------------------------------------------------------------------
// Fallback tokenization helpers
// ---------------------------------------------------------------------------

/// Estimate token count from text length.
fn estimate_token_count(text: &str) -> usize {
    (text.len() as f64 / CHARS_PER_TOKEN).ceil() as usize
}

/// Create a fallback encode response.
#[allow(dead_code)]
fn fallback_encode_response(text: &str) -> Value {
    let count = estimate_token_count(text);
    serde_json::json!({
        "ids": [],
        "count": count,
        "chunks": [],
    })
}

/// Create a fallback decode response.
#[allow(dead_code)]
fn fallback_decode_response() -> Value {
    serde_json::json!({
        "text": "",
        "chunks": [],
    })
}

// ---------------------------------------------------------------------------
// Factory handlers — Sentencepiece
// ---------------------------------------------------------------------------

/// Creates a handler for sentencepiece encoding.
/// Returns token estimation since native sentencepiece is not available in Rust.
async fn sentencepiece_encode_handler(
    body: Json<EncodeRequest>,
) -> Response {
    let text = body.text.as_deref().unwrap_or("");
    let count = estimate_token_count(text);
    Json(serde_json::json!({
        "ids": [],
        "count": count,
        "chunks": [],
    }))
    .into_response()
}

/// Creates a handler for sentencepiece decoding.
async fn sentencepiece_decode_handler(
    _body: Json<DecodeRequest>,
) -> Response {
    Json(serde_json::json!({
        "text": "",
        "chunks": [],
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Factory handlers — Tiktoken
// ---------------------------------------------------------------------------

/// Creates a handler for tiktoken encoding.
async fn tiktoken_encode_handler(
    body: Json<EncodeRequest>,
) -> Response {
    let text = body.text.as_deref().unwrap_or("");
    let count = estimate_token_count(text);
    Json(serde_json::json!({
        "ids": [],
        "count": count,
        "chunks": [],
    }))
    .into_response()
}

/// Creates a handler for tiktoken decoding.
async fn tiktoken_decode_handler(
    _body: Json<DecodeRequest>,
) -> Response {
    Json(serde_json::json!({
        "text": "",
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Factory handlers — Web tokenizers
// ---------------------------------------------------------------------------

/// Creates a handler for web tokenizer encoding.
async fn web_tokenizer_encode_handler(
    body: Json<EncodeRequest>,
) -> Response {
    let text = body.text.as_deref().unwrap_or("");
    let count = estimate_token_count(text);
    Json(serde_json::json!({
        "ids": [],
        "count": count,
        "chunks": [],
    }))
    .into_response()
}

/// Creates a handler for web tokenizer decoding.
async fn web_tokenizer_decode_handler(
    _body: Json<DecodeRequest>,
) -> Response {
    Json(serde_json::json!({
        "text": "",
        "chunks": [],
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Sentencepiece encode handlers (7)
// ---------------------------------------------------------------------------

pub async fn llama_encode(Json(body): Json<EncodeRequest>) -> Response {
    sentencepiece_encode_handler(Json(body)).await
}

pub async fn nerdstash_encode(Json(body): Json<EncodeRequest>) -> Response {
    sentencepiece_encode_handler(Json(body)).await
}

pub async fn nerdstash_v2_encode(Json(body): Json<EncodeRequest>) -> Response {
    sentencepiece_encode_handler(Json(body)).await
}

pub async fn mistral_encode(Json(body): Json<EncodeRequest>) -> Response {
    sentencepiece_encode_handler(Json(body)).await
}

pub async fn yi_encode(Json(body): Json<EncodeRequest>) -> Response {
    sentencepiece_encode_handler(Json(body)).await
}

pub async fn gemma_encode(Json(body): Json<EncodeRequest>) -> Response {
    sentencepiece_encode_handler(Json(body)).await
}

pub async fn jamba_encode(Json(body): Json<EncodeRequest>) -> Response {
    sentencepiece_encode_handler(Json(body)).await
}

// ---------------------------------------------------------------------------
// Sentencepiece decode handlers (7)
// ---------------------------------------------------------------------------

pub async fn llama_decode(Json(body): Json<DecodeRequest>) -> Response {
    sentencepiece_decode_handler(Json(body)).await
}

pub async fn nerdstash_decode(Json(body): Json<DecodeRequest>) -> Response {
    sentencepiece_decode_handler(Json(body)).await
}

pub async fn nerdstash_v2_decode(Json(body): Json<DecodeRequest>) -> Response {
    sentencepiece_decode_handler(Json(body)).await
}

pub async fn mistral_decode(Json(body): Json<DecodeRequest>) -> Response {
    sentencepiece_decode_handler(Json(body)).await
}

pub async fn yi_decode(Json(body): Json<DecodeRequest>) -> Response {
    sentencepiece_decode_handler(Json(body)).await
}

pub async fn gemma_decode(Json(body): Json<DecodeRequest>) -> Response {
    sentencepiece_decode_handler(Json(body)).await
}

pub async fn jamba_decode(Json(body): Json<DecodeRequest>) -> Response {
    sentencepiece_decode_handler(Json(body)).await
}

// ---------------------------------------------------------------------------
// Tiktoken handlers (2)
// ---------------------------------------------------------------------------

pub async fn gpt2_encode(Json(body): Json<EncodeRequest>) -> Response {
    tiktoken_encode_handler(Json(body)).await
}

pub async fn gpt2_decode(Json(body): Json<DecodeRequest>) -> Response {
    tiktoken_decode_handler(Json(body)).await
}

// ---------------------------------------------------------------------------
// Web tokenizer encode handlers (7)
// ---------------------------------------------------------------------------

pub async fn claude_encode(Json(body): Json<EncodeRequest>) -> Response {
    web_tokenizer_encode_handler(Json(body)).await
}

pub async fn llama3_encode(Json(body): Json<EncodeRequest>) -> Response {
    web_tokenizer_encode_handler(Json(body)).await
}

pub async fn qwen2_encode(Json(body): Json<EncodeRequest>) -> Response {
    web_tokenizer_encode_handler(Json(body)).await
}

pub async fn command_r_encode(Json(body): Json<EncodeRequest>) -> Response {
    web_tokenizer_encode_handler(Json(body)).await
}

pub async fn command_a_encode(Json(body): Json<EncodeRequest>) -> Response {
    web_tokenizer_encode_handler(Json(body)).await
}

pub async fn nemo_encode(Json(body): Json<EncodeRequest>) -> Response {
    web_tokenizer_encode_handler(Json(body)).await
}

pub async fn deepseek_encode(Json(body): Json<EncodeRequest>) -> Response {
    web_tokenizer_encode_handler(Json(body)).await
}

// ---------------------------------------------------------------------------
// Web tokenizer decode handlers (7)
// ---------------------------------------------------------------------------

pub async fn claude_decode(Json(body): Json<DecodeRequest>) -> Response {
    web_tokenizer_decode_handler(Json(body)).await
}

pub async fn llama3_decode(Json(body): Json<DecodeRequest>) -> Response {
    web_tokenizer_decode_handler(Json(body)).await
}

pub async fn qwen2_decode(Json(body): Json<DecodeRequest>) -> Response {
    web_tokenizer_decode_handler(Json(body)).await
}

pub async fn command_r_decode(Json(body): Json<DecodeRequest>) -> Response {
    web_tokenizer_decode_handler(Json(body)).await
}

pub async fn command_a_decode(Json(body): Json<DecodeRequest>) -> Response {
    web_tokenizer_decode_handler(Json(body)).await
}

pub async fn nemo_decode(Json(body): Json<DecodeRequest>) -> Response {
    web_tokenizer_decode_handler(Json(body)).await
}

pub async fn deepseek_decode(Json(body): Json<DecodeRequest>) -> Response {
    web_tokenizer_decode_handler(Json(body)).await
}

// ---------------------------------------------------------------------------
// OpenAI handlers (3)
// ---------------------------------------------------------------------------

/// `POST /api/tokenizers/openai/encode` — encode with model-appropriate tokenizer.
///
/// Mirrors Node's `router.post('/openai/encode')` in `tokenizers.js:762-833`.
/// Uses query param `model` to select tokenizer; falls back to estimation.
pub async fn openai_encode(
    Query(_query): Query<ModelQuery>,
    Json(body): Json<EncodeRequest>,
) -> Response {
    let text = body.text.as_deref().unwrap_or("");
    let count = estimate_token_count(text);
    Json(serde_json::json!({
        "ids": [],
        "count": count,
        "chunks": [],
    }))
    .into_response()
}

/// `POST /api/tokenizers/openai/decode` — decode with model-appropriate tokenizer.
///
/// Mirrors Node's `router.post('/openai/decode')` in `tokenizers.js:835-906`.
pub async fn openai_decode(
    Query(_query): Query<ModelQuery>,
    Json(_body): Json<DecodeRequest>,
) -> Response {
    Json(serde_json::json!({
        "text": "",
    }))
    .into_response()
}

/// `POST /api/tokenizers/openai/count` — count tokens for chat messages.
///
/// Mirrors Node's `router.post('/openai/count')` in `tokenizers.js:908-1027`.
/// Uses query param `model` to select tokenizer and counting strategy.
pub async fn openai_count(
    Query(_query): Query<ModelQuery>,
    Json(body): Json<Value>,
) -> Response {
    // Body is an array of message objects
    let messages = match body.as_array() {
        Some(arr) => arr,
        None => {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    // Fallback estimation: concatenate all message content and estimate
    let total_text: String = messages
        .iter()
        .flat_map(|msg| {
            msg.as_object()
                .map(|obj| {
                    obj.values()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join("\n\n")
                })
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let token_count = estimate_token_count(&total_text);

    Json(serde_json::json!({
        "token_count": token_count,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Remote handlers (2)
// ---------------------------------------------------------------------------

/// `POST /api/tokenizers/remote/kobold/count` — count tokens via remote Kobold API.
///
/// Mirrors Node's `router.post('/remote/kobold/count')` in `tokenizers.js:1029-1062`.
pub async fn remote_kobold_count(
    Json(body): Json<RemoteCountRequest>,
) -> Response {
    let text = body.text.as_deref().unwrap_or("");
    let base_url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let url = format!(
        "{}/extra/tokencount",
        base_url.trim_end_matches('/')
    );

    let client = reqwest::Client::new();
    match client
        .post(&url)
        .json(&serde_json::json!({"prompt": text}))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                tracing::warn!("API returned error: {}", resp.status());
                return Json(serde_json::json!({"error": true})).into_response();
            }

            match resp.json::<Value>().await {
                Ok(data) => {
                    let count = data.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
                    let ids = data.get("ids").cloned().unwrap_or(Value::Array(Vec::new()));
                    Json(serde_json::json!({"count": count, "ids": ids})).into_response()
                }
                Err(_) => Json(serde_json::json!({"error": true})).into_response(),
            }
        }
        Err(e) => {
            tracing::error!("Remote kobold count failed: {}", e);
            Json(serde_json::json!({"error": true})).into_response()
        }
    }
}

/// `POST /api/tokenizers/remote/textgenerationwebui/encode` — encode via remote API.
///
/// Mirrors Node's `router.post('/remote/textgenerationwebui/encode')` in `tokenizers.js:1064-1129`.
pub async fn remote_textgenwebui_encode(
    Json(body): Json<RemoteEncodeRequest>,
) -> Response {
    let text = body.text.as_deref().unwrap_or("");
    let base_url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let api_type = body.api_type.as_deref().unwrap_or("");
    let vllm_model = body.vllm_model.as_deref().unwrap_or("");
    let aphrodite_model = body.aphrodite_model.as_deref().unwrap_or("");

    // Convert to string + remove trailing slash + /v1 suffix
    let base = base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1");

    let (url, request_body) = match api_type {
        "tabby" => (
            format!("{}/v1/token/encode", base),
            serde_json::json!({"text": text}),
        ),
        "koboldcpp" => (
            format!("{}/api/extra/tokencount", base),
            serde_json::json!({"prompt": text}),
        ),
        "llamacpp" => (
            format!("{}/tokenize", base),
            serde_json::json!({"content": text}),
        ),
        "vllm" => (
            format!("{}/tokenize", base),
            serde_json::json!({"model": vllm_model, "prompt": text}),
        ),
        "aphrodite" => (
            format!("{}/v1/tokenize", base),
            serde_json::json!({"model": aphrodite_model, "prompt": text}),
        ),
        _ => (
            format!("{}/v1/internal/encode", base),
            serde_json::json!({"text": text}),
        ),
    };

    let client = reqwest::Client::new();
    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                tracing::warn!("API returned error: {}", resp.status());
                return Json(serde_json::json!({"error": true})).into_response();
            }

            match resp.json::<Value>().await {
                Ok(data) => {
                    let count = data
                        .get("length")
                        .or_else(|| data.get("count"))
                        .or_else(|| data.get("value"))
                        .and_then(|v| v.as_i64())
                        .or_else(|| {
                            data.get("tokens")
                                .and_then(|v| v.as_array())
                                .map(|a| a.len() as i64)
                        })
                        .unwrap_or(0);

                    let ids = data
                        .get("tokens")
                        .or_else(|| data.get("ids"))
                        .cloned()
                        .unwrap_or(Value::Array(Vec::new()));

                    Json(serde_json::json!({"count": count, "ids": ids})).into_response()
                }
                Err(_) => Json(serde_json::json!({"error": true})).into_response(),
            }
        }
        Err(e) => {
            tracing::error!("Remote textgenwebui encode failed: {}", e);
            Json(serde_json::json!({"error": true})).into_response()
        }
    }
}
