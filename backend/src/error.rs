//! Application error types with HTTP status code mappings.
//!
//! All endpoint handlers return `Result<T, AppError>` so that errors
//! are automatically converted into appropriate HTTP responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Application-level error enum.
///
/// Each variant maps to an HTTP status code and a human-readable message.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// 400 Bad Request — invalid input from the client.
    #[error("Bad request: {0}")]
    BadRequest(String),

    /// 401 Unauthorized — missing or invalid authentication.
    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    /// 403 Forbidden — authenticated but not allowed.
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// 404 Not Found — resource does not exist.
    #[error("Not found: {0}")]
    NotFound(String),

    /// 409 Conflict — resource state conflict (e.g. duplicate filename).
    #[error("Conflict: {0}")]
    Conflict(String),

    /// 422 Unprocessable Entity — semantically invalid request.
    #[error("Unprocessable entity: {0}")]
    UnprocessableEntity(String),

    /// 500 Internal Server Error — unexpected failure.
    #[error("Internal error: {0}")]
    Internal(String),

    /// 500 wrapping an anyhow error for convenience.
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),

    /// 500 wrapping a std::io::Error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// 500 wrapping a serde_json error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl AppError {
    /// Returns the HTTP status code for this error.
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::UnprocessableEntity(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Internal(_) | Self::Anyhow(_) | Self::Io(_) | Self::Json(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

/// Convert `AppError` into an Axum response.
///
/// Returns a JSON body `{ "error": "<message>" }` with the appropriate
/// HTTP status code.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let message = self.to_string();

        // Log server errors at error level, client errors at warn
        if status.is_server_error() {
            tracing::error!(%status, error = %message, "Server error");
        } else {
            tracing::warn!(%status, error = %message, "Client error");
        }

        let body = serde_json::json!({
            "error": message,
        });

        (status, axum::Json(body)).into_response()
    }
}

/// Convenience type alias for handler return types.
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_codes() {
        assert_eq!(
            AppError::BadRequest("test".into()).status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AppError::Unauthorized("test".into()).status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            AppError::NotFound("test".into()).status_code(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AppError::Internal("test".into()).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
