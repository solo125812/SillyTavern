//! Search endpoints — Phase 9.
//!
//! Mirrors Node's [`src/endpoints/search.js`](../../../src/endpoints/search.js).
//!
//! ## Endpoints
//! - `POST /api/search/serpapi`   — Search via SerpApi.
//! - `POST /api/search/transcript` — Get YouTube video transcript.
//! - `POST /api/search/searxng`   — Search via SearXNG.
//! - `POST /api/search/tavily`    — Search via Tavily.
//! - `POST /api/search/koboldcpp` — Search via KoboldCpp.
//! - `POST /api/search/serper`    — Search via Serper.
//! - `POST /api/search/zai`       — Search via Z.AI.
//! - `POST /api/search/visit`     — Visit a web URL and return content.
//!
//! All search endpoints act as proxy pass-throughs to external services,
//! reading API keys from the user's secrets store.

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

/// Browser-like User-Agent for web visits.
const VISIT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36";

// ---------------------------------------------------------------------------
// Secret key constants (mirrors Node's SECRET_KEYS used by search)
// ---------------------------------------------------------------------------

const SECRET_SERPAPI: &str = "api_key_serpapi";
const SECRET_TAVILY: &str = "api_key_tavily";
const SECRET_SERPER: &str = "api_key_serper";
const SECRET_ZAI: &str = "api_key_zai";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SerpApiRequest {
    pub query: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TranscriptRequest {
    pub id: Option<String>,
    pub lang: Option<String>,
    pub json: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearxngRequest {
    pub base_url: Option<String>,
    pub query: Option<String>,
    pub preferences: Option<String>,
    pub categories: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TavilyRequest {
    pub query: Option<String>,
    pub include_images: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct KoboldCppSearchRequest {
    pub query: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SerperRequest {
    pub query: Option<String>,
    pub images: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ZaiRequest {
    pub query: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VisitRequest {
    pub url: Option<String>,
    pub html: Option<bool>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a secret value for a given key from the user's secrets file.
/// This is a simplified version that reads the active secret value.
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
            // Fallback to first
            arr.first()
                .and_then(|item| item.get("value").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
        }
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Trim trailing /v1 from a URL (mirrors Node's `trimV1()`).
fn trim_v1(url: &str) -> String {
    url.trim_end_matches("/v1")
        .trim_end_matches("/v1/")
        .to_string()
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

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/search/serpapi` — Search via SerpApi.
///
/// Mirrors Node's `router.post('/serpapi')` in `search.js:93-120`.
pub async fn search_serpapi(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<SerpApiRequest>,
) -> impl IntoResponse {
    let key = match read_secret(&state.config.data_root, &user.handle, SECRET_SERPAPI) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::error!("No SerpApi key found");
            return bad_request_plain();
        }
    };

    let query = match &body.query {
        Some(q) if !q.is_empty() => q.clone(),
        _ => return bad_request_plain(),
    };

    tracing::debug!("SerpApi query: {}", query);

    let client = http_client();
    let url = format!(
        "https://serpapi.com/search.json?q={}&api_key={}",
        urlencoding::encode(&query),
        key
    );

    match client.get(&url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                tracing::error!("SerpApi request failed: {}", text);
                return (StatusCode::INTERNAL_SERVER_ERROR, text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("SerpApi parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("SerpApi error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/search/transcript` — Get YouTube video transcript.
///
/// Mirrors Node's `router.post('/transcript')` in `search.js:126-161`.
pub async fn search_transcript(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<TranscriptRequest>,
) -> impl IntoResponse {
    let id = match &body.id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            tracing::error!("Id is required for /transcript");
            return bad_request_plain();
        }
    };

    let lang = body.lang.clone();
    let return_json = body.json.unwrap_or(false);

    let client = http_client();
    let url = format!("https://www.youtube.com/watch?v={}", id);

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("User-Agent", VISIT_USER_AGENT.parse().unwrap());
    if let Some(ref l) = lang {
        if let Ok(val) = l.parse() {
            headers.insert("Accept-Language", val);
        }
    }

    let video_response = match client.get(&url).headers(headers).send().await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Transcript fetch error: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let video_page_body = match video_response.text().await {
        Ok(body) => body,
        Err(e) => {
            tracing::error!("Transcript body read error: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    match extract_transcript(&video_page_body, lang.as_deref()).await {
        Ok(transcript) => {
            if return_json {
                Json(json!({ "transcript": transcript, "html": video_page_body })).into_response()
            } else {
                transcript.into_response()
            }
        }
        Err(_) => {
            if return_json {
                Json(json!({ "html": video_page_body, "transcript": "" })).into_response()
            } else {
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }
}

/// Extract transcript from YouTube video page HTML.
async fn extract_transcript(video_page_body: &str, lang: Option<&str>) -> Result<String, String> {
    let re_xml = regex::Regex::new(r#"<text start="([^"]*)" dur="([^"]*)">([^<]*)</text>"#)
        .map_err(|e| format!("Regex error: {e}"))?;

    let parts: Vec<&str> = video_page_body.split("\"captions\":").collect();
    if parts.len() <= 1 {
        if video_page_body.contains("class=\"g-recaptcha\"") {
            return Err("Too many requests".to_string());
        }
        if !video_page_body.contains("\"playabilityStatus\":") {
            return Err("Video is not available".to_string());
        }
        return Err("Transcript not available".to_string());
    }

    // Extract captions JSON
    let captions_str = parts[1].split(",\"videoDetails").next().unwrap_or("");
    let captions: Value = serde_json::from_str(captions_str.replace('\n', "").as_str())
        .map_err(|_| "Transcript disabled".to_string())?;

    let caption_tracks = captions
        .pointer("/playerCaptionsTracklistRenderer/captionTracks")
        .and_then(|v| v.as_array())
        .ok_or("Transcript not available")?;

    if caption_tracks.is_empty() {
        return Err("Transcript not available".to_string());
    }

    // Find matching language track
    if let Some(l) = lang {
        if !caption_tracks
            .iter()
            .any(|t| t.get("languageCode").and_then(|v| v.as_str()) == Some(l))
        {
            return Err("Transcript not available in this language".to_string());
        }
    }

    let track = if let Some(l) = lang {
        caption_tracks
            .iter()
            .find(|t| t.get("languageCode").and_then(|v| v.as_str()) == Some(l))
            .ok_or("Transcript not available in this language")?
    } else {
        &caption_tracks[0]
    };

    let transcript_url = track
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .ok_or("Missing transcript URL")?;

    let client = http_client();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("User-Agent", VISIT_USER_AGENT.parse().unwrap());
    if let Some(l) = lang {
        if let Ok(val) = l.parse() {
            headers.insert("Accept-Language", val);
        }
    }

    let transcript_response = client
        .get(transcript_url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("Transcript request failed: {e}"))?;

    if !transcript_response.status().is_success() {
        return Err("Transcript request failed".to_string());
    }

    let transcript_body = transcript_response
        .text()
        .await
        .map_err(|e| format!("Transcript body read failed: {e}"))?;

    let transcript_text: String = re_xml
        .captures_iter(&transcript_body)
        .map(|cap| {
            let text = &cap[3];
            // Simple HTML entity decode
            text.replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&#39;", "'")
                .replace("&apos;", "'")
        })
        .collect::<Vec<_>>()
        .join(" ");

    Ok(transcript_text)
}

/// `POST /api/search/searxng` — Search via SearXNG.
///
/// Mirrors Node's `router.post('/searxng')` in `search.js:163-215`.
pub async fn search_searxng(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<SearxngRequest>,
) -> impl IntoResponse {
    let base_url = match &body.base_url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return bad_request_plain(),
    };

    let query = match &body.query {
        Some(q) if !q.is_empty() => q.clone(),
        _ => return bad_request_plain(),
    };

    tracing::debug!("SearXNG query: {} {}", base_url, query);

    let client = http_client();

    // Visit main page first (for cookies/session)
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Accept", "text/html".parse().unwrap());
    headers.insert("User-Agent", VISIT_USER_AGENT.parse().unwrap());

    let _ = client.get(&base_url).headers(headers.clone()).send().await;

    // Build search URL
    let mut search_url = format!("{}/search", base_url.trim_end_matches('/'));
    let mut params = vec![format!("q={}", urlencoding::encode(&query))];

    if let Some(ref preferences) = body.preferences {
        params.push(format!("preferences={}", urlencoding::encode(preferences)));
    }
    if let Some(ref categories) = body.categories {
        params.push(format!("categories={}", urlencoding::encode(categories)));
    }

    search_url = format!("{}?{}", search_url, params.join("&"));

    match client.get(&search_url).headers(headers).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                tracing::error!("SearXNG request failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            match resp.text().await {
                Ok(data) => data.into_response(),
                Err(e) => {
                    tracing::error!("SearXNG body read error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("SearXNG request failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/search/tavily` — Search via Tavily.
///
/// Mirrors Node's `router.post('/tavily')` in `search.js:217-264`.
pub async fn search_tavily(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<TavilyRequest>,
) -> impl IntoResponse {
    let api_key = match read_secret(&state.config.data_root, &user.handle, SECRET_TAVILY) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::error!("No Tavily key found");
            return bad_request_plain();
        }
    };

    let query = match &body.query {
        Some(q) if !q.is_empty() => q.clone(),
        _ => return bad_request_plain(),
    };

    tracing::debug!("Tavily query: {}", query);

    let request_body = json!({
        "query": query,
        "api_key": api_key,
        "search_depth": "basic",
        "topic": "general",
        "include_answer": true,
        "include_raw_content": false,
        "include_images": body.include_images.unwrap_or(false),
        "include_image_descriptions": false,
        "include_domains": [],
        "max_results": 10,
    });

    let client = http_client();
    match client
        .post("https://api.tavily.com/search")
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                tracing::error!("Tavily request failed: {}", text);
                return (StatusCode::INTERNAL_SERVER_ERROR, text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("Tavily parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Tavily error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/search/koboldcpp` — Search via KoboldCpp.
///
/// Mirrors Node's `router.post('/koboldcpp')` in `search.js:266-300`.
pub async fn search_koboldcpp(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<KoboldCppSearchRequest>,
) -> impl IntoResponse {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => {
            tracing::error!("No URL provided for KoboldCpp search");
            return bad_request_plain();
        }
    };

    let query = body.query.clone().unwrap_or_default();
    tracing::debug!("KoboldCpp search query: {}", query);

    let base_url = trim_v1(&url);
    let search_url = format!("{}/api/extra/websearch", base_url);

    let client = http_client();
    match client
        .post(&search_url)
        .json(&json!({"q": query}))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                tracing::error!("KoboldCpp request failed: {}", text);
                return (StatusCode::INTERNAL_SERVER_ERROR, text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("KoboldCpp parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("KoboldCpp error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/search/serper` — Search via Serper.
///
/// Mirrors Node's `router.post('/serper')` in `search.js:302-342`.
pub async fn search_serper(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<SerperRequest>,
) -> impl IntoResponse {
    let key = match read_secret(&state.config.data_root, &user.handle, SECRET_SERPER) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::error!("No Serper key found");
            return bad_request_plain();
        }
    };

    let query = match &body.query {
        Some(q) if !q.is_empty() => q.clone(),
        _ => return bad_request_plain(),
    };

    tracing::debug!("Serper query: {}", query);

    let url = if body.images.unwrap_or(false) {
        "https://google.serper.dev/images"
    } else {
        "https://google.serper.dev/search"
    };

    let client = http_client();
    match client
        .post(url)
        .header("X-API-KEY", &key)
        .header("Content-Type", "application/json")
        .json(&json!({"q": query}))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                tracing::warn!("Serper request failed: {}", text);
                return (StatusCode::INTERNAL_SERVER_ERROR, text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("Serper parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Serper error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/search/zai` — Search via Z.AI.
///
/// Mirrors Node's `router.post('/zai')` in `search.js:344-388`.
pub async fn search_zai(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ZaiRequest>,
) -> impl IntoResponse {
    let key = match read_secret(&state.config.data_root, &user.handle, SECRET_ZAI) {
        Some(k) if !k.is_empty() => k,
        _ => {
            tracing::error!("No Z.AI key found");
            return bad_request_plain();
        }
    };

    let query = match &body.query {
        Some(q) if !q.is_empty() => q.clone(),
        _ => {
            tracing::error!("No query provided for /zai");
            return bad_request_plain();
        }
    };

    tracing::debug!("Z.AI web search query: {}", query);

    let client = http_client();
    match client
        .post("https://api.z.ai/api/paas/v4/web_search")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", key))
        .json(&json!({
            "search_engine": "search-prime",
            "search_query": query
        }))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                tracing::error!("Z.AI request failed: {}", text);
                return (StatusCode::INTERNAL_SERVER_ERROR, text).into_response();
            }
            match resp.json::<Value>().await {
                Ok(data) => Json(data).into_response(),
                Err(e) => {
                    tracing::error!("Z.AI parse error: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Z.AI error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/search/visit` — Visit a web URL and return content.
///
/// Mirrors Node's `router.post('/visit')` in `search.js:390-455`.
pub async fn search_visit(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<VisitRequest>,
) -> impl IntoResponse {
    let url_str = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => {
            tracing::error!("No url provided for /visit");
            return bad_request_plain();
        }
    };

    let html = body.html.unwrap_or(true);

    // Validate URL
    let parsed = match url::Url::parse(&url_str) {
        Ok(u) => u,
        Err(_) => {
            tracing::error!("Invalid url provided for /visit: {}", url_str);
            return bad_request_plain();
        }
    };

    // Reject non-HTTP protocols
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        tracing::error!("Invalid protocol for /visit: {}", parsed.scheme());
        return bad_request_plain();
    }

    // Reject non-standard ports
    if parsed.port().is_some() {
        tracing::error!("Invalid port for /visit");
        return bad_request_plain();
    }

    // Reject IP addresses
    if let Some(host) = parsed.host_str() {
        let ip_regex = regex::Regex::new(r"^\d+\.\d+\.\d+\.\d+$").unwrap();
        if ip_regex.is_match(host) {
            tracing::error!("Invalid hostname (IP address) for /visit");
            return bad_request_plain();
        }
    } else {
        return bad_request_plain();
    }

    tracing::info!("Visiting web URL: {}", url_str);

    let client = http_client();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Accept", "text/html".parse().unwrap());
    headers.insert("User-Agent", VISIT_USER_AGENT.parse().unwrap());
    headers.insert("Accept-Language", "en-US,en;q=0.5".parse().unwrap());
    headers.insert("Connection", "keep-alive".parse().unwrap());
    headers.insert("Cache-Control", "no-cache".parse().unwrap());
    headers.insert("DNT", "1".parse().unwrap());

    match client.get(&url_str).headers(headers).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                tracing::error!("Visit failed: {} {}", resp.status(), resp.status());
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }

            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();

            if html {
                if !content_type.contains("text/html") {
                    tracing::error!(
                        "Visit failed, content-type is {}, expected text/html",
                        content_type
                    );
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }

                match resp.text().await {
                    Ok(text) => text.into_response(),
                    Err(e) => {
                        tracing::error!("Visit body read error: {}", e);
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                }
            } else {
                match resp.bytes().await {
                    Ok(bytes) => {
                        axum::response::Response::builder()
                            .header("Content-Type", content_type)
                            .body(axum::body::Body::from(bytes.to_vec()))
                            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
                    }
                    Err(e) => {
                        tracing::error!("Visit body read error: {}", e);
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!("Visit error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
