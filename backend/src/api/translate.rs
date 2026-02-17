//! Translate endpoints — Phase 9.
//!
//! Mirrors Node's [`src/endpoints/translate.js`](../../../src/endpoints/translate.js).
//!
//! ## Endpoints
//! - `POST /api/translate/libre`   — Translate via LibreTranslate.
//! - `POST /api/translate/google`  — Translate via Google Translate.
//! - `POST /api/translate/yandex`  — Translate via Yandex Translate.
//! - `POST /api/translate/lingva`  — Translate via Lingva.
//! - `POST /api/translate/deepl`   — Translate via DeepL.
//! - `POST /api/translate/onering` — Translate via OneRing.
//! - `POST /api/translate/deeplx`  — Translate via DeepLX.
//! - `POST /api/translate/bing`    — Translate via Bing Translate.
//!
//! All translation endpoints act as proxy pass-throughs to external services,
//! reading API keys/URLs from the user's secrets store.

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

const DEEPLX_URL_DEFAULT: &str = "http://127.0.0.1:1188/translate";
const ONERING_URL_DEFAULT: &str = "http://127.0.0.1:4990/translate";
const LINGVA_DEFAULT: &str = "https://lingva.ml/api/v1";

// Secret keys (mirrors Node's SECRET_KEYS)
const SECRET_LIBRE: &str = "api_key_libre";
const SECRET_LIBRE_URL: &str = "api_key_libre_url";
const SECRET_DEEPL: &str = "api_key_deepl";
const SECRET_LINGVA_URL: &str = "api_key_lingva";
const SECRET_ONERING_URL: &str = "api_key_onering_url";
const SECRET_DEEPLX_URL: &str = "api_key_deeplx";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct LibreTranslateRequest {
    pub text: Option<String>,
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoogleTranslateRequest {
    pub text: Option<String>,
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct YandexTranslateRequest {
    pub chunks: Option<Vec<String>>,
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LingvaTranslateRequest {
    pub text: Option<String>,
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeeplTranslateRequest {
    pub text: Option<String>,
    pub lang: Option<String>,
    pub endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OneringTranslateRequest {
    pub text: Option<String>,
    pub from_lang: Option<String>,
    pub to_lang: Option<String>,
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeeplxTranslateRequest {
    pub text: Option<String>,
    pub lang: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BingTranslateRequest {
    pub text: Option<String>,
    pub lang: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a secret value for a given key from the user's secrets file.
fn read_secret(data_root: &std::path::Path, handle: &str, key: &str) -> Option<String> {
    let dirs = UserDirectories::new(data_root, handle);
    let secrets_path = dirs.root.join("secrets.json");

    let content = std::fs::read_to_string(&secrets_path).ok()?;
    let secrets: serde_json::Map<String, Value> = serde_json::from_str(&content).ok()?;

    match secrets.get(key)? {
        Value::Array(arr) => {
            // Find active secret
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

/// Build an HTTP client with reasonable defaults.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default()
}

fn bad_request_plain() -> Response {
    (StatusCode::BAD_REQUEST, "Bad Request").into_response()
}

/// Apply language normalization for Chinese variants.
fn normalize_lang_zh(lang: &str) -> String {
    match lang {
        "zh-CN" | "zh-TW" => "zh".to_string(),
        _ => lang.to_string(),
    }
}

/// Apply language normalization for Portuguese variants.
fn normalize_lang_pt(lang: &str) -> String {
    match lang {
        "pt-BR" | "pt-PT" => "pt".to_string(),
        _ => lang.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/translate/libre` — Translate via LibreTranslate.
///
/// Mirrors Node's `router.post('/libre')` in `translate.js:16-74`.
pub async fn translate_libre(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<LibreTranslateRequest>,
) -> impl IntoResponse {
    let key = read_secret(&state.config.data_root, &user.handle, SECRET_LIBRE);
    let url = match read_secret(&state.config.data_root, &user.handle, SECRET_LIBRE_URL) {
        Some(u) if !u.is_empty() => u,
        _ => {
            tracing::warn!("LibreTranslate URL is not configured.");
            return bad_request_plain();
        }
    };

    let mut lang = body.lang.clone().unwrap_or_default();
    // Normalize language codes
    match lang.as_str() {
        "zh-CN" => lang = "zh".to_string(),
        "zh-TW" => lang = "zt".to_string(),
        "pt-BR" | "pt-PT" => lang = "pt".to_string(),
        _ => {}
    }

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return bad_request_plain(),
    };

    if lang.is_empty() {
        return bad_request_plain();
    }

    tracing::debug!("Input text: {}", text);

    let client = http_client();
    let request_body = json!({
        "q": text,
        "source": "auto",
        "target": lang,
        "format": "text",
        "api_key": key.unwrap_or_default(),
    });

    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error = resp.text().await.unwrap_or_default();
                tracing::warn!("LibreTranslate error: {}", error);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(json_val) => {
                    let translated = json_val
                        .get("translatedText")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    tracing::debug!("Translated text: {}", translated);
                    translated.to_string().into_response()
                }
                Err(e) => {
                    tracing::error!("Translation error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Translation error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/translate/google` — Translate via Google Translate.
///
/// Mirrors Node's `router.post('/google')` in `translate.js:76-101`.
/// Note: Uses Google Translate API via a simple HTTP approach.
/// Full `google-translate-api-x` integration is deferred.
pub async fn translate_google(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<GoogleTranslateRequest>,
) -> impl IntoResponse {
    let mut lang = body.lang.clone().unwrap_or_default();
    if lang == "pt-BR" {
        lang = "pt".to_string();
    }

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return bad_request_plain(),
    };

    if lang.is_empty() {
        return bad_request_plain();
    }

    tracing::debug!("Input text: {}", text);

    // Use Google Translate's free endpoint
    let client = http_client();
    let url = format!(
        "https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={}&dt=t&q={}",
        urlencoding::encode(&lang),
        urlencoding::encode(&text)
    );

    match client.get(&url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                tracing::error!("Translation error");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    // Google returns [[["translated","original",...],...],...] format
                    let translated = data
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|v| v.as_array())
                        .map(|sentences| {
                            sentences
                                .iter()
                                .filter_map(|s| s.as_array().and_then(|a| a.first()).and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("")
                        })
                        .unwrap_or_default();

                    tracing::debug!("Translated text: {}", translated);

                    axum::response::Response::builder()
                        .header("Content-Type", "text/plain; charset=utf-8")
                        .body(axum::body::Body::from(translated))
                        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
                }
                Err(e) => {
                    tracing::error!("Translation error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Translation error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/translate/yandex` — Translate via Yandex Translate.
///
/// Mirrors Node's `router.post('/yandex')` in `translate.js:103-157`.
pub async fn translate_yandex(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<YandexTranslateRequest>,
) -> impl IntoResponse {
    let mut lang = body.lang.clone().unwrap_or_default();
    match lang.as_str() {
        "pt-PT" => lang = "pt".to_string(),
        "zh-CN" | "zh-TW" => lang = "zh".to_string(),
        _ => {}
    }

    let chunks = match &body.chunks {
        Some(c) if !c.is_empty() => c.clone(),
        _ => return bad_request_plain(),
    };

    if lang.is_empty() {
        return bad_request_plain();
    }

    let input_text: String = chunks.join("");
    tracing::debug!("Input text: {}", input_text);

    let ucid = uuid::Uuid::new_v4().to_string().replace('-', "");

    // Build form body
    let mut form_parts = Vec::new();
    for chunk in &chunks {
        form_parts.push(format!("text={}", urlencoding::encode(chunk)));
    }
    form_parts.push(format!("lang={}", urlencoding::encode(&lang)));
    let form_body = form_parts.join("&");

    let url = format!(
        "https://translate.yandex.net/api/v1/tr.json/translate?ucid={}&srv=android&format=text",
        ucid
    );

    let client = http_client();
    match client
        .post(&url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error = resp.text().await.unwrap_or_default();
                tracing::warn!("Yandex error: {}", error);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(json_val) => {
                    let translated = json_val
                        .get("text")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join(",")
                        })
                        .unwrap_or_default();
                    tracing::debug!("Translated text: {}", translated);
                    translated.into_response()
                }
                Err(e) => {
                    tracing::error!("Translation error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Translation error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/translate/lingva` — Translate via Lingva.
///
/// Mirrors Node's `router.post('/lingva')` in `translate.js:159-201`.
pub async fn translate_lingva(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<LingvaTranslateRequest>,
) -> impl IntoResponse {
    let secret_url = read_secret(&state.config.data_root, &user.handle, SECRET_LINGVA_URL);
    let base_url = secret_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(LINGVA_DEFAULT);

    let mut lang = body.lang.clone().unwrap_or_default();
    lang = normalize_lang_zh(&lang);
    lang = normalize_lang_pt(&lang);

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return bad_request_plain(),
    };

    if lang.is_empty() {
        return bad_request_plain();
    }

    tracing::debug!("Input text: {}", text);

    let url = format!(
        "{}/auto/{}/{}",
        base_url.trim_end_matches('/'),
        lang,
        urlencoding::encode(&text)
    );

    let client = http_client();
    match client.get(&url).send().await {
        Ok(resp) => {
            match resp.json::<Value>().await {
                Ok(data) => {
                    let translation = data
                        .get("translation")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    tracing::debug!("Translated text: {}", translation);
                    translation.to_string().into_response()
                }
                Err(e) => {
                    tracing::error!("Translation error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Translation error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/translate/deepl` — Translate via DeepL.
///
/// Mirrors Node's `router.post('/deepl')` in `translate.js:203-263`.
pub async fn translate_deepl(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeeplTranslateRequest>,
) -> impl IntoResponse {
    let key = match read_secret(&state.config.data_root, &user.handle, SECRET_DEEPL) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::warn!("DeepL key is not configured.");
            return bad_request_plain();
        }
    };

    let mut lang = body.lang.clone().unwrap_or_default();
    if lang == "zh-CN" || lang == "zh-TW" {
        lang = "ZH".to_string();
    }

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return bad_request_plain(),
    };

    if lang.is_empty() {
        return bad_request_plain();
    }

    tracing::debug!("Input text: {}", text);

    // Build form body
    let mut form_parts = vec![
        format!("text={}", urlencoding::encode(&text)),
        format!("target_lang={}", urlencoding::encode(&lang)),
    ];

    // Add formality for supported languages
    let formality_langs = ["de", "fr", "it", "es", "nl", "ja", "ru", "pt-BR", "pt-PT"];
    if formality_langs.contains(&lang.as_str()) {
        form_parts.push("formality=default".to_string());
    }

    let endpoint = match body.endpoint.as_deref() {
        Some("pro") => "https://api.deepl.com/v2/translate",
        _ => "https://api-free.deepl.com/v2/translate",
    };

    let client = http_client();
    match client
        .post(endpoint)
        .header("Accept", "application/json")
        .header("Authorization", format!("DeepL-Auth-Key {}", key))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_parts.join("&"))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error = resp.text().await.unwrap_or_default();
                tracing::warn!("DeepL error: {}", error);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(json_val) => {
                    let translated = json_val
                        .pointer("/translations/0/text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    tracing::debug!("Translated text: {}", translated);
                    translated.to_string().into_response()
                }
                Err(e) => {
                    tracing::error!("Translation error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Translation error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/translate/onering` — Translate via OneRing.
///
/// Mirrors Node's `router.post('/onering')` in `translate.js:265-320`.
pub async fn translate_onering(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<OneringTranslateRequest>,
) -> impl IntoResponse {
    let secret_url = read_secret(&state.config.data_root, &user.handle, SECRET_ONERING_URL);
    let url = secret_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(ONERING_URL_DEFAULT);

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return bad_request_plain(),
    };

    let from_lang = match &body.from_lang {
        Some(l) if !l.is_empty() => l.clone(),
        _ => return bad_request_plain(),
    };

    let to_lang = match &body.to_lang {
        Some(l) if !l.is_empty() => l.clone(),
        _ => return bad_request_plain(),
    };

    tracing::debug!("Input text: {}", text);

    let query_url = format!(
        "{}?text={}&from_lang={}&to_lang={}",
        url,
        urlencoding::encode(&text),
        urlencoding::encode(&from_lang),
        urlencoding::encode(&to_lang)
    );

    let client = http_client();
    match client.get(&query_url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error = resp.text().await.unwrap_or_default();
                tracing::warn!("OneRing error: {}", error);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let result = data
                        .get("result")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    tracing::debug!("Translated text: {}", result);
                    result.to_string().into_response()
                }
                Err(e) => {
                    tracing::error!("Translation error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Translation error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/translate/deeplx` — Translate via DeepLX.
///
/// Mirrors Node's `router.post('/deeplx')` in `translate.js:322-376`.
pub async fn translate_deeplx(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeeplxTranslateRequest>,
) -> impl IntoResponse {
    let secret_url = read_secret(&state.config.data_root, &user.handle, SECRET_DEEPLX_URL);
    let url = secret_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEEPLX_URL_DEFAULT);

    let mut lang = body.lang.clone().unwrap_or_default();
    if lang == "zh-CN" || lang == "zh-TW" {
        lang = "ZH".to_string();
    }

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return bad_request_plain(),
    };

    if lang.is_empty() {
        return bad_request_plain();
    }

    tracing::debug!("Input text: {}", text);

    let request_body = json!({
        "text": text,
        "source_lang": "auto",
        "target_lang": lang,
    });

    let client = http_client();
    match client
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let error = resp.text().await.unwrap_or_default();
                tracing::warn!("DeepLX error: {}", error);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(json_val) => {
                    let translated = json_val
                        .get("data")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    tracing::debug!("Translated text: {}", translated);
                    translated.to_string().into_response()
                }
                Err(e) => {
                    tracing::error!("DeepLX translation error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("DeepLX translation error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/translate/bing` — Translate via Bing Translate.
///
/// Mirrors Node's `router.post('/bing')` in `translate.js:378-409`.
/// Note: Uses Bing's public translate API endpoint directly.
/// The npm `bing-translate-api` package is not available in Rust.
pub async fn translate_bing(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<BingTranslateRequest>,
) -> impl IntoResponse {
    let mut lang = body.lang.clone().unwrap_or_default();
    match lang.as_str() {
        "zh-CN" => lang = "zh-Hans".to_string(),
        "zh-TW" => lang = "zh-Hant".to_string(),
        "pt-BR" => lang = "pt".to_string(),
        _ => {}
    }

    let text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return bad_request_plain(),
    };

    if lang.is_empty() {
        return bad_request_plain();
    }

    tracing::debug!("Input text: {}", text);

    // Use Bing's translator API
    let client = http_client();
    let url = format!(
        "https://api.cognitive.microsofttranslator.com/translate?api-version=3.0&to={}",
        urlencoding::encode(&lang)
    );

    let request_body = json!([{"Text": text}]);

    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                // Fallback: try a simpler approach
                tracing::warn!("Bing Translate API returned non-success status");
                // Return the text as-is if translation fails
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => {
                    let translated = data
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|v| v.get("translations"))
                        .and_then(|v| v.as_array())
                        .and_then(|a| a.first())
                        .and_then(|v| v.get("text"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    tracing::debug!("Translated text: {}", translated);
                    translated.to_string().into_response()
                }
                Err(e) => {
                    tracing::error!("Translation error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Translation error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
