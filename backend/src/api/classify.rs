//! Classify endpoints — Phase 9.
//!
//! Mirrors Node's [`src/endpoints/classify.js`](../../../src/endpoints/classify.js).
//!
//! ## Endpoints
//! - `POST /api/extra/classify/labels` — Get classification labels.
//! - `POST /api/extra/classify`        — Classify text.
//!
//! Note: The actual classification uses transformers.js pipelines in the Node
//! server. In the Rust sidecar, these endpoints return 501 Not Implemented
//! until native transformers bindings or a Python bridge is added.
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
pub struct ClassifyRequest {
    pub text: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/extra/classify/labels` — Get classification labels.
///
/// Mirrors Node's `router.post('/labels')` in `classify.js:14-23`.
///
/// Note: Returns 501 until native transformers pipeline is available.
/// When implemented, should return `{ labels: string[] }` from the model config.
pub async fn classify_labels(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> impl IntoResponse {
    // The Node implementation uses getPipeline('text-classification')
    // and returns Object.keys(pipe.model.config.label2id).
    // Without native transformers support, return a 501 with informative message.
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "Text classification pipeline not available in Rust sidecar. Use Node server for this feature."
        })),
    )
        .into_response()
}

/// `POST /api/extra/classify` — Classify text.
///
/// Mirrors Node's `router.post('/')` in `classify.js:25-55`.
///
/// Note: Returns 501 until native transformers pipeline is available.
/// When implemented, should return `{ classification: [{label, score}, ...] }`.
pub async fn classify_text(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
    Json(body): Json<ClassifyRequest>,
) -> impl IntoResponse {
    let _text = match &body.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Text is required"})),
            )
                .into_response()
        }
    };

    // The Node implementation uses getPipeline('text-classification')
    // and calls pipe(text, { topk: 5 }), then sorts by score descending.
    // Without native transformers support, return 501.
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "Text classification pipeline not available in Rust sidecar. Use Node server for this feature."
        })),
    )
        .into_response()
}
