//! Caption endpoints — Phase 9.
//!
//! Mirrors Node's [`src/endpoints/caption.js`](../../../src/endpoints/caption.js).
//!
//! ## Endpoints
//! - `POST /api/extra/caption` — Generate a text caption from an image.
//!
//! Note: The actual captioning uses transformers.js `image-to-text` pipeline
//! in the Node server. In the Rust sidecar, this endpoint returns 501 Not
//! Implemented until native transformers bindings or a Python bridge is added.
//! The route structure and request/response shapes are preserved for parity.

use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CaptionRequest {
    /// Base64-encoded image data or URL.
    pub image: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/extra/caption` — Generate a text caption from an image.
///
/// Mirrors Node's `router.post('/')` in `caption.js:8-29`.
///
/// Note: Returns 501 until native transformers pipeline (`image-to-text`) is
/// available. When implemented, should return `{ caption: string }`.
pub async fn caption_image(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<CaptionRequest>,
) -> impl IntoResponse {
    let _image = match &body.image {
        Some(img) if !img.is_empty() => img.clone(),
        _ => {
            return (StatusCode::BAD_REQUEST, "Bad Request").into_response()
        }
    };

    // The Node implementation uses getPipeline('image-to-text')
    // and calls pipe(rawImage), returning result[0].generated_text.
    // Without native transformers support, return 501.
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "Image captioning pipeline not available in Rust sidecar. Use Node server for this feature."
        })),
    )
        .into_response()
}
