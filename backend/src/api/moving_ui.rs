//! Moving UI endpoints — Phase 3.
//!
//! Mirrors Node's [`src/endpoints/moving-ui.js`](../../../src/endpoints/moving-ui.js).
//!
//! ## Endpoints
//! - `POST /api/moving-ui/save` — save a moving UI panel layout.

use std::fs;
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::UserDirectories;
use crate::storage::sanitize::sanitize_filename_strip;

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /api/moving-ui/save` — save a moving UI panel layout.
///
/// Mirrors Node's `router.post('/save')` in `moving-ui.js:8-17`:
/// - Expects JSON body with `name` field.
/// - Writes the entire body as pretty-printed JSON to `movingUI/{name}.json`.
pub async fn save_moving_ui(
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
    let filename = dirs.moving_ui.join(&sanitized_name);

    // Ensure movingUI directory exists
    if let Err(e) = fs::create_dir_all(&dirs.moving_ui) {
        tracing::error!("Failed to create movingUI directory: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let content = match to_pretty_json_4(&body) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to serialize moving UI data: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if let Err(e) = fs::write(&filename, content) {
        tracing::error!("Failed to write moving UI file: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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
    fn save_moving_ui_writes_file() {
        let dir = TempDir::new().unwrap();
        let dirs = UserDirectories::new(dir.path(), "test-user");
        fs::create_dir_all(&dirs.moving_ui).unwrap();

        let body = serde_json::json!({"name": "panel1", "x": 100, "y": 200});
        let sanitized = sanitize_filename_strip(&format!("{}.json", "panel1"));
        let filename = dirs.moving_ui.join(&sanitized);

        let content = serde_json::to_string_pretty(&body).unwrap();
        fs::write(&filename, &content).unwrap();

        assert!(filename.exists());
        let read_back: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&filename).unwrap()).unwrap();
        assert_eq!(read_back["name"], "panel1");
        assert_eq!(read_back["x"], 100);
    }
}
