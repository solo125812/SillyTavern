//! Quick Replies endpoints — Phase 3.
//!
//! Mirrors Node's [`src/endpoints/quick-replies.js`](../../../src/endpoints/quick-replies.js).
//!
//! ## Endpoints
//! - `POST /api/quick-replies/save` — save a quick reply set.
//! - `POST /api/quick-replies/delete` — delete a quick reply set.

use std::fs;
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Request body for `POST /api/quick-replies/delete`.
#[derive(Debug, Deserialize)]
pub struct DeleteQuickReplyRequest {
    /// Name of the quick reply set to delete.
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/quick-replies/save` — save a quick reply set.
///
/// Mirrors Node's `router.post('/save')` in `quick-replies.js:10-18`:
/// - Expects JSON body with `name` field.
/// - Writes the entire body as pretty-printed JSON to `QuickReplies/{name}.json`.
pub async fn save_quick_reply(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return (StatusCode::BAD_REQUEST, "Bad Request").into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let sanitized_name = sanitize_filename_strip(&format!("{}.json", name));
    let filename = dirs.quick_replies.join(&sanitized_name);

    // Ensure QuickReplies directory exists
    if let Err(e) = fs::create_dir_all(&dirs.quick_replies) {
        tracing::error!("Failed to create QuickReplies directory: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let content = match to_pretty_json_4(&body) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to serialize quick reply data: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if let Err(e) = fs::write(&filename, content) {
        tracing::error!("Failed to write quick reply file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    (StatusCode::OK, "OK").into_response()
}

/// `POST /api/quick-replies/delete` — delete a quick reply set.
///
/// Mirrors Node's `router.post('/delete')` in `quick-replies.js:21-32`:
/// - Expects `{name}` in JSON body.
/// - Deletes `QuickReplies/{name}.json` if it exists.
/// - Always returns 200 (even if file doesn't exist, matching Node behavior).
pub async fn delete_quick_reply(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<DeleteQuickReplyRequest>,
) -> impl IntoResponse {
    let name = match &body.name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return (StatusCode::BAD_REQUEST, "Bad Request").into_response(),
    };

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let sanitized_name = sanitize_filename_strip(&format!("{}.json", name));
    let filename = dirs.quick_replies.join(&sanitized_name);

    // Delete if exists (Node always returns 200 regardless)
    if filename.exists() {
        if let Err(e) = fs::remove_file(&filename) {
            tracing::error!("Failed to delete quick reply file: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    (StatusCode::OK, "OK").into_response()
}

fn to_pretty_json_4(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut serializer)?;
    Ok(String::from_utf8(buf).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn save_quick_reply_writes_file() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&dirs.quick_replies).unwrap();

        let body = serde_json::json!({"name": "greetings", "replies": ["Hi", "Hello"]});
        let sanitized = sanitize_filename_strip(&format!("{}.json", "greetings"));
        let filename = dirs.quick_replies.join(&sanitized);

        let content = serde_json::to_string_pretty(&body).unwrap();
        fs::write(&filename, &content).unwrap();

        assert!(filename.exists());
        let read_back: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&filename).unwrap()).unwrap();
        assert_eq!(read_back["name"], "greetings");
    }

    #[test]
    fn delete_quick_reply_removes_file() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&dirs.quick_replies).unwrap();

        let filename = dirs.quick_replies.join("test.json");
        fs::write(&filename, "{}").unwrap();
        assert!(filename.exists());

        fs::remove_file(&filename).unwrap();
        assert!(!filename.exists());
    }

    #[test]
    fn delete_nonexistent_quick_reply_is_ok() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&dirs.quick_replies).unwrap();

        let filename = dirs.quick_replies.join("nonexistent.json");
        // Should not panic — Node returns 200 even if file doesn't exist
        assert!(!filename.exists());
    }
}
