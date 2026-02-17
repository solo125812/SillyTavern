//! Content manager endpoints — Phase 8.
//!
//! Mirrors Node's [`src/endpoints/content-manager.js`](../../../src/endpoints/content-manager.js).
//!
//! ## Endpoints
//! - `POST /api/content/importURL`  — Import a character/lorebook from a URL
//! - `POST /api/content/importUUID` — Import a character/lorebook by UUID
//!
//! ## Supported Sources
//! - Chub.ai / CharacterHub (characters + lorebooks)
//! - Pygmalion.chat
//! - JanitorAI
//! - AICharacterCards.com (AICC)
//! - RisuAI Realm
//! - Perchance.org
//! - Generic whitelisted PNG download sources

use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::Value;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const USER_AGENT: &str = "SillyTavern";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ImportUrlRequest {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ImportUuidRequest {
    url: Option<String>,
}

// ---------------------------------------------------------------------------
// URL parsing helpers
// ---------------------------------------------------------------------------

/// Extract UUID from a URL (standard UUID format).
fn get_uuid_from_url(url: &str) -> Option<String> {
    let re = regex::Regex::new(
        r"[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}",
    )
    .ok()?;
    re.find(url).map(|m| m.as_str().to_string())
}

/// Get hostname from a URL.
fn get_host_from_url(url: &str) -> String {
    url::Url::parse(url)
        .map(|u| u.host_str().unwrap_or("").to_string())
        .unwrap_or_default()
}

/// Parse a Chub URL to extract the type and id.
fn parse_chub_url(url: &str) -> Option<(String, String)> {
    let split: Vec<&str> = url.split('/').collect();
    if split.len() < 2 {
        return None;
    }

    // Find domain index
    let domain_index = split.iter().position(|part| {
        *part == "www.chub.ai"
            || *part == "chub.ai"
            || *part == "www.characterhub.org"
            || *part == "characterhub.org"
    });

    let last_parts: Vec<&str> = match domain_index {
        Some(idx) => split[idx + 1..].to_vec(),
        None => split.clone(),
    };

    if last_parts.is_empty() {
        return None;
    }

    let first_part = last_parts[0].to_lowercase();

    if first_part == "characters" || first_part == "lorebooks" {
        let content_type = if first_part == "characters" {
            "character"
        } else {
            "lorebook"
        };
        let id = if content_type == "character" {
            last_parts[1..].join("/")
        } else {
            last_parts.join("/")
        };
        Some((content_type.to_string(), id))
    } else if split.len() == 2 {
        Some(("character".to_string(), last_parts.join("/")))
    } else {
        None
    }
}

/// Parse an AICC URL.
fn parse_aicc(url: &str) -> Option<String> {
    let re = regex::Regex::new(
        r"^https?://aicharactercards\.com/character-cards/([^/]+)/([^/]+)/?$|([^/]+)/([^/]+)$",
    )
    .ok()?;
    let caps = re.captures(url)?;

    if let (Some(a), Some(b)) = (caps.get(1), caps.get(2)) {
        Some(format!("{}/{}", a.as_str(), b.as_str()))
    } else if let (Some(a), Some(b)) = (caps.get(3), caps.get(4)) {
        Some(format!("{}/{}", a.as_str(), b.as_str()))
    } else {
        None
    }
}

/// Parse a Risu Realm URL to extract UUID.
fn parse_risu_url(url: &str) -> Option<String> {
    let re = regex::Regex::new(
        r"(?i)^https?://realm\.risuai\.net/character/([a-f0-9-]+)/?$",
    )
    .ok()?;
    re.captures(url)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Check if a string is a valid Perchance UUID.
fn is_perchance_uuid(uuid: &str) -> bool {
    if uuid.is_empty() {
        return false;
    }
    let re = regex::Regex::new(r"^\w+~[a-f0-9]{32}\.gz$").unwrap();
    re.is_match(uuid)
}

/// Parse Perchance slug from URL.
fn parse_perchance_slug(url: &str) -> String {
    url.split('~').nth(1).unwrap_or("").to_string()
}

// ---------------------------------------------------------------------------
// Download helpers
// ---------------------------------------------------------------------------

/// Result of a content download.
struct DownloadResult {
    buffer: Vec<u8>,
    file_name: String,
    file_type: Option<String>,
}

/// Build a reqwest client with the SillyTavern user agent.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Download a Chub lorebook.
async fn download_chub_lorebook(id: &str) -> Result<DownloadResult, String> {
    let parts: Vec<&str> = id.split('/').collect();
    if parts.len() < 3 {
        return Err("Invalid lorebook ID format".to_string());
    }

    let client = http_client();
    let api_url = format!(
        "https://api.chub.ai/api/{}/{}/{}",
        parts[0], parts[1], parts[2]
    );

    let result = client
        .get(&api_url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch lorebook metadata: {e}"))?;

    if !result.status().is_success() {
        let text = result.text().await.unwrap_or_default();
        return Err(format!("Chub returned error: {text}"));
    }

    let metadata: Value = result
        .json()
        .await
        .map_err(|e| format!("Failed to parse metadata: {e}"))?;

    let project_id = metadata
        .pointer("/node/id")
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .ok_or("Project ID not found in lorebook metadata")?;

    let download_url = format!(
        "https://api.chub.ai/api/v4/projects/{}/repository/files/raw%252Fsillytavern_raw.json/raw",
        project_id
    );

    let download_result = client
        .get(&download_url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Failed to download lorebook: {e}"))?;

    if !download_result.status().is_success() {
        let text = download_result.text().await.unwrap_or_default();
        return Err(format!("Chub returned error: {text}"));
    }

    let content_type = download_result
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let buffer = download_result
        .bytes()
        .await
        .map_err(|e| format!("Failed to read lorebook bytes: {e}"))?
        .to_vec();

    let name = parts.last().unwrap_or(&"lorebook");
    let file_name = format!("{}.json", sanitize_filename::sanitize(name));

    Ok(DownloadResult {
        buffer,
        file_name,
        file_type: content_type,
    })
}

/// Download a Chub character.
async fn download_chub_character(id: &str) -> Result<DownloadResult, String> {
    let parts: Vec<&str> = id.split('/').collect();
    if parts.len() < 2 {
        return Err("Invalid character ID format".to_string());
    }

    let client = http_client();
    let api_url = format!(
        "https://api.chub.ai/api/characters/{}/{}?full=true",
        parts[0], parts[1]
    );

    let result = client
        .get(&api_url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch character metadata: {e}"))?;

    if !result.status().is_success() {
        let text = result.text().await.unwrap_or_default();
        return Err(format!("Chub returned error: {text}"));
    }

    let metadata: Value = result
        .json()
        .await
        .map_err(|e| format!("Failed to parse metadata: {e}"))?;

    // Build character card JSON
    let definition = &metadata["node"]["definition"];
    let topics = &metadata["node"]["topics"];

    let character_card = serde_json::json!({
        "data": {
            "name": definition["name"],
            "description": definition["personality"],
            "personality": definition["tavern_personality"],
            "scenario": definition["scenario"],
            "first_mes": definition["first_message"],
            "mes_example": definition["example_dialogs"],
            "creator_notes": definition["description"],
            "system_prompt": definition["system_prompt"],
            "post_history_instructions": definition["post_history_instructions"],
            "alternate_greetings": definition["alternate_greetings"],
            "tags": topics,
            "creator": parts[0],
            "character_version": "",
            "character_book": definition["embedded_lorebook"],
            "extensions": definition["extensions"],
        },
        "spec": "chara_card_v2",
        "spec_version": "2.0",
    });

    let char_json = serde_json::to_string(&character_card)
        .map_err(|e| format!("Failed to serialize character: {e}"))?;

    // For now, return JSON instead of PNG with embedded data
    // (PNG embedding requires the character-card-parser which is JS-specific)
    let char_name = definition["name"]
        .as_str()
        .unwrap_or("Unknown");
    let file_name = format!("{}.json", sanitize_filename::sanitize(char_name));

    Ok(DownloadResult {
        buffer: char_json.into_bytes(),
        file_name,
        file_type: Some("application/json".to_string()),
    })
}

/// Download a Pygmalion character.
async fn download_pygmalion_character(id: &str) -> Result<DownloadResult, String> {
    let client = http_client();
    let url = format!(
        "https://server.pygmalion.chat/api/export/character/{}/v2",
        id
    );

    let result = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Failed to download character: {e}"))?;

    if !result.status().is_success() {
        let text = result.text().await.unwrap_or_default();
        return Err(format!("Pygsite returned error: {text}"));
    }

    let json_data: Value = result
        .json()
        .await
        .map_err(|e| format!("Failed to parse character data: {e}"))?;

    let _character_data = json_data
        .get("character")
        .ok_or("Failed to download character: invalid data")?;

    // Return as JSON (PNG embedding requires JS character-card-parser)
    let buffer = serde_json::to_vec(&json_data)
        .map_err(|e| format!("Failed to serialize: {e}"))?;
    let file_name = format!("{}.json", sanitize_filename::sanitize(id));

    Ok(DownloadResult {
        buffer,
        file_name,
        file_type: Some("application/json".to_string()),
    })
}

/// Download a Janny character.
async fn download_janny_character(uuid: &str) -> Result<DownloadResult, String> {
    let client = http_client();

    let body = serde_json::json!({ "characterId": uuid });
    let result = client
        .post("https://api.jannyai.com/api/v1/download")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Failed to download character: {e}"))?;

    if !result.status().is_success() {
        let text = result.text().await.unwrap_or_default();
        return Err(format!("Janny returned error: {text}"));
    }

    let download_result: Value = result
        .json()
        .await
        .map_err(|e| format!("Failed to parse Janny response: {e}"))?;

    if download_result.get("status").and_then(|s| s.as_str()) != Some("ok") {
        return Err(format!("Janny failed to download: {download_result}"));
    }

    let download_url = download_result
        .get("downloadUrl")
        .and_then(|u| u.as_str())
        .ok_or("Missing downloadUrl in Janny response")?;

    let image_result = client
        .get(download_url)
        .send()
        .await
        .map_err(|e| format!("Failed to download image: {e}"))?;

    let content_type = image_result
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let buffer = image_result
        .bytes()
        .await
        .map_err(|e| format!("Failed to read image bytes: {e}"))?
        .to_vec();

    let file_name = format!("{}.png", sanitize_filename::sanitize(uuid));

    Ok(DownloadResult {
        buffer,
        file_name,
        file_type: content_type,
    })
}

/// Download an AICC character.
async fn download_aicc_character(id: &str) -> Result<DownloadResult, String> {
    let client = http_client();
    let api_url = format!(
        "https://aicharactercards.com/wp-json/pngapi/v1/image/{}",
        id
    );

    let result = client
        .get(&api_url)
        .send()
        .await
        .map_err(|e| format!("Failed to download character: {e}"))?;

    if !result.status().is_success() {
        return Err(format!(
            "Failed to download character: {}",
            result.status()
        ));
    }

    let content_type = result
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/png")
        .to_string();

    let buffer = result
        .bytes()
        .await
        .map_err(|e| format!("Failed to read bytes: {e}"))?
        .to_vec();

    let file_name = format!("{}.png", sanitize_filename::sanitize(id));

    Ok(DownloadResult {
        buffer,
        file_name,
        file_type: Some(content_type),
    })
}

/// Download a Risu character.
async fn download_risu_character(uuid: &str) -> Result<DownloadResult, String> {
    let client = http_client();
    let url = format!(
        "https://realm.risuai.net/api/v1/download/png-v3/{}?non_commercial=true",
        uuid
    );

    let result = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Failed to download character: {e}"))?;

    if !result.status().is_success() {
        let text = result.text().await.unwrap_or_default();
        return Err(format!("RisuAI returned error: {text}"));
    }

    let buffer = result
        .bytes()
        .await
        .map_err(|e| format!("Failed to read bytes: {e}"))?
        .to_vec();

    let file_name = format!("{}.png", sanitize_filename::sanitize(uuid));

    Ok(DownloadResult {
        buffer,
        file_name,
        file_type: Some("image/png".to_string()),
    })
}

/// Download a Perchance character.
async fn download_perchance_character(slug: &str) -> Result<DownloadResult, String> {
    if slug.is_empty() {
        return Err("Empty slug".to_string());
    }

    let client = http_client();
    let char_url = format!("https://user.uploads.dev/file/{}", slug);

    let result = client
        .get(&char_url)
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| format!("Failed to download character: {e}"))?;

    if !result.status().is_success() {
        return Err("Failed to download Perchance character".to_string());
    }

    // Decompress gzipped content
    let compressed = result
        .bytes()
        .await
        .map_err(|e| format!("Failed to read bytes: {e}"))?;

    let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
    let mut decompressed = String::new();
    std::io::Read::read_to_string(&mut decoder, &mut decompressed)
        .map_err(|e| format!("Failed to decompress: {e}"))?;

    let perchance_data: Value = serde_json::from_str(&decompressed)
        .map_err(|e| format!("Failed to parse Perchance data: {e}"))?;

    let add_character = perchance_data
        .get("addCharacter")
        .ok_or("Invalid Perchance character data: missing addCharacter field")?;

    let char_name = add_character
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("Unnamed Perchance Character");

    let char_data = serde_json::json!({
        "name": char_name,
        "first_mes": "",
        "tags": [],
        "description": add_character.get("roleInstruction").and_then(|v| v.as_str()).unwrap_or(""),
        "creator": add_character.get("metaTitle").and_then(|v| v.as_str()).unwrap_or(""),
        "creator_notes": add_character.get("metaDescription").and_then(|v| v.as_str()).unwrap_or(""),
        "alternate_greetings": [],
        "character_version": "",
        "mes_example": "",
        "post_history_instructions": "",
        "system_prompt": "",
        "scenario": "",
        "personality": add_character.get("reminderMessage").and_then(|v| v.as_str()).unwrap_or(""),
        "extensions": {
            "perchance_data": {
                "slug": slug,
                "char_url": char_url,
                "uuid": add_character.get("uuid"),
            }
        },
    });

    let card = serde_json::json!({
        "spec": "chara_card_v2",
        "spec_version": "2.0",
        "data": char_data,
    });

    let buffer = serde_json::to_vec(&card)
        .map_err(|e| format!("Failed to serialize: {e}"))?;
    let file_name = format!("{}.json", char_name);

    Ok(DownloadResult {
        buffer,
        file_name,
        file_type: Some("application/json".to_string()),
    })
}

/// Download from a generic whitelisted PNG source.
async fn download_generic_png(url: &str) -> Result<DownloadResult, String> {
    let client = http_client();

    let result = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Failed to download: {e}"))?;

    if !result.status().is_success() {
        return Err(format!("Download failed: {}", result.status()));
    }

    let content_type = result
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/png")
        .to_string();

    let final_url = result.url().to_string();

    let buffer = result
        .bytes()
        .await
        .map_err(|e| format!("Failed to read bytes: {e}"))?
        .to_vec();

    let mut file_name = sanitize_filename::sanitize(
        final_url
            .split('?')
            .next()
            .unwrap_or(&final_url)
            .rsplit('/')
            .next()
            .unwrap_or("download"),
    );

    // Append .png if content type is image/png and no extension present
    if content_type == "image/png" {
        let has_ext = regex::Regex::new(r"\.(\w+)$")
            .map(|re| re.is_match(&file_name))
            .unwrap_or(false);
        if !has_ext {
            file_name.push_str(".png");
        }
    }

    Ok(DownloadResult {
        buffer,
        file_name,
        file_type: Some(content_type),
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/content/importURL` — Import content from a URL.
pub async fn import_url(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<ImportUrlRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let host = get_host_from_url(&url);

    let is_chub =
        host.contains("chub.ai") || host.contains("characterhub.org");
    let is_janny = host.contains("janitorai");
    let is_pygmalion = host.contains("pygmalion.chat");
    let is_aicc = host.contains("aicharactercards.com");
    let is_risu = host.contains("realm.risuai.net");
    let is_perchance = host.contains("perchance.org");

    // Check whitelist for generic downloads
    let whitelist: Vec<String> = state
        .config
        .whitelist_import_domains
        .clone();
    let is_generic = whitelist.iter().any(|d| d == &host);

    let import_result: Result<(DownloadResult, String), Response> = if is_pygmalion {
        let uuid = match get_uuid_from_url(&url) {
            Some(u) => u,
            None => return StatusCode::NOT_FOUND.into_response(),
        };
        download_pygmalion_character(&uuid)
            .await
            .map(|r| (r, "character".to_string()))
            .map_err(|e| {
                tracing::error!("Importing custom content failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            })
    } else if is_janny {
        let uuid = match get_uuid_from_url(&url) {
            Some(u) => u,
            None => return StatusCode::NOT_FOUND.into_response(),
        };
        download_janny_character(&uuid)
            .await
            .map(|r| (r, "character".to_string()))
            .map_err(|e| {
                tracing::error!("Importing custom content failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            })
    } else if is_aicc {
        let aicc_id = match parse_aicc(&url) {
            Some(id) => id,
            None => return StatusCode::NOT_FOUND.into_response(),
        };
        download_aicc_character(&aicc_id)
            .await
            .map(|r| (r, "character".to_string()))
            .map_err(|e| {
                tracing::error!("Importing custom content failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            })
    } else if is_chub {
        let (content_type, id) = match parse_chub_url(&url) {
            Some(parsed) => parsed,
            None => return StatusCode::NOT_FOUND.into_response(),
        };
        if content_type == "character" {
            tracing::info!("Downloading chub character: {id}");
            download_chub_character(&id)
                .await
                .map(|r| (r, content_type))
                .map_err(|e| {
                    tracing::error!("Importing custom content failed: {e}");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                })
        } else if content_type == "lorebook" {
            tracing::info!("Downloading chub lorebook: {id}");
            download_chub_lorebook(&id)
                .await
                .map(|r| (r, content_type))
                .map_err(|e| {
                    tracing::error!("Importing custom content failed: {e}");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                })
        } else {
            return StatusCode::NOT_FOUND.into_response();
        }
    } else if is_risu {
        let uuid = match parse_risu_url(&url) {
            Some(u) => u,
            None => return StatusCode::NOT_FOUND.into_response(),
        };
        download_risu_character(&uuid)
            .await
            .map(|r| (r, "character".to_string()))
            .map_err(|e| {
                tracing::error!("Importing custom content failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            })
    } else if is_perchance {
        let slug = parse_perchance_slug(&url);
        if slug.is_empty() {
            return StatusCode::NOT_FOUND.into_response();
        }
        download_perchance_character(&slug)
            .await
            .map(|r| (r, "character".to_string()))
            .map_err(|e| {
                tracing::error!("Importing custom content failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            })
    } else if is_generic {
        tracing::info!("Downloading from generic url: {url}");
        download_generic_png(&url)
            .await
            .map(|r| (r, "character".to_string()))
            .map_err(|e| {
                tracing::error!("Importing custom content failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            })
    } else {
        tracing::error!(
            "Received an import for \"{}\", but site is not whitelisted.",
            host
        );
        return StatusCode::NOT_FOUND.into_response();
    };

    match import_result {
        Ok((result, content_type)) => build_download_response(result, &content_type, true),
        Err(err_response) => err_response,
    }
}

/// `POST /api/content/importUUID` — Import content by UUID.
pub async fn import_uuid(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<ImportUuidRequest>,
) -> Response {
    let uuid = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let is_janny = uuid.contains("_character");
    let is_pygmalion = !is_janny && uuid.len() == 36;
    let is_aicc = uuid.starts_with("AICC/");
    let is_perchance = is_perchance_uuid(&uuid);
    let uuid_type = if uuid.contains("lorebook") {
        "lorebook"
    } else {
        "character"
    };

    let download_result: Result<DownloadResult, String> = if is_pygmalion {
        tracing::info!("Downloading Pygmalion character: {uuid}");
        download_pygmalion_character(&uuid).await
    } else if is_janny {
        let janny_uuid = uuid.split('_').next().unwrap_or(&uuid);
        tracing::info!("Downloading Janitor character: {janny_uuid}");
        download_janny_character(janny_uuid).await
    } else if is_aicc {
        let parts: Vec<&str> = uuid.splitn(3, '/').collect();
        if parts.len() >= 3 {
            let aicc_id = format!("{}/{}", parts[1], parts[2]);
            tracing::info!("Downloading AICC character: {aicc_id}");
            download_aicc_character(&aicc_id).await
        } else {
            Err("Invalid AICC UUID format".to_string())
        }
    } else if is_perchance {
        tracing::info!("Downloading Perchance character: {uuid}");
        let parsed_slug = parse_perchance_slug(&uuid);
        download_perchance_character(&parsed_slug).await
    } else if uuid_type == "character" {
        tracing::info!("Downloading chub character: {uuid}");
        download_chub_character(&uuid).await
    } else if uuid_type == "lorebook" {
        tracing::info!("Downloading chub lorebook: {uuid}");
        download_chub_lorebook(&uuid).await
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match download_result {
        Ok(result) => build_download_response(result, uuid_type, false),
        Err(e) => {
            tracing::error!("Importing custom content failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Build the HTTP response for a downloaded file.
fn build_download_response(
    result: DownloadResult,
    content_type: &str,
    encode_filename: bool,
) -> Response {
    let mut builder = Response::builder().status(StatusCode::OK);

    if let Some(ft) = &result.file_type {
        builder = builder.header(header::CONTENT_TYPE, ft.as_str());
    }

    let filename = if encode_filename {
        urlencoding::encode(&result.file_name).to_string()
    } else {
        result.file_name.clone()
    };

    builder = builder.header(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", filename),
    );

    builder = builder.header("X-Custom-Content-Type", content_type);

    builder
        .body(axum::body::Body::from(result.buffer))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
