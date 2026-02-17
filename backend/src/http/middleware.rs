//! Request context extraction middleware.
//!
//! The Node proxy forwards user identity via custom headers:
//! - `x-st-user-handle`: User's unique handle/slug
//! - `x-st-user-name`: User's display name
//! - `x-st-user-admin`: Whether the user is an admin ("true"/"false")
//!
//! This middleware extracts these headers into a [`UserContext`] struct
//! and injects it as a request extension. Handlers can then extract it
//! via `Extension<UserContext>`.
//!
//! Requests without a valid user context are rejected with 401 Unauthorized.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Extracted user context from Node proxy headers.
#[derive(Debug, Clone)]
pub struct UserContext {
    /// Unique user handle/slug (e.g. "default-user").
    pub handle: String,
    /// User display name.
    pub name: String,
    /// Whether this user has admin privileges.
    pub is_admin: bool,
}

/// Header names used by the Node proxy to forward user context.
mod headers {
    pub const USER_HANDLE: &str = "x-st-user-handle";
    pub const USER_NAME: &str = "x-st-user-name";
    pub const USER_ADMIN: &str = "x-st-user-admin";
}

/// Middleware that extracts user context from Node proxy headers.
///
/// Rejects requests with missing or malformed user context with 401.
///
/// # Usage in router
/// ```rust,ignore
/// use axum::middleware;
/// let app = Router::new()
///     .route("/api/example", post(handler))
///     .layer(middleware::from_fn(require_user_context));
/// ```
pub async fn require_user_context(mut req: Request, next: Next) -> Response {
    // Extract user handle — this is required
    let handle = match req.headers().get(headers::USER_HANDLE) {
        Some(value) => match value.to_str() {
            Ok(s) if !s.is_empty() => s.to_string(),
            _ => {
                tracing::warn!("Missing or malformed {} header", headers::USER_HANDLE);
                return (
                    StatusCode::UNAUTHORIZED,
                    "Missing or malformed user context: handle",
                )
                    .into_response();
            }
        },
        None => {
            tracing::warn!("Missing {} header", headers::USER_HANDLE);
            return (
                StatusCode::UNAUTHORIZED,
                "Missing user context: no user handle header",
            )
                .into_response();
        }
    };

    // Extract user name (optional, defaults to handle)
    let name = req
        .headers()
        .get(headers::USER_NAME)
        .and_then(|v| v.to_str().ok())
        .unwrap_or(&handle)
        .to_string();

    // Extract admin flag (optional, defaults to false)
    let is_admin = req
        .headers()
        .get(headers::USER_ADMIN)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let user_ctx = UserContext {
        handle,
        name,
        is_admin,
    };

    tracing::debug!(
        user_handle = %user_ctx.handle,
        user_admin = user_ctx.is_admin,
        "User context extracted"
    );

    // Insert into request extensions so handlers can access it
    req.extensions_mut().insert(user_ctx);

    next.run(req).await
}

/// Middleware that extracts user context but does NOT reject on missing context.
///
/// Useful for routes that can optionally use user context (e.g. public endpoints
/// that behave differently for authenticated users).
pub async fn optional_user_context(mut req: Request, next: Next) -> Response {
    if let Some(handle_header) = req.headers().get(headers::USER_HANDLE) {
        if let Ok(handle) = handle_header.to_str() {
            if !handle.is_empty() {
                let name = req
                    .headers()
                    .get(headers::USER_NAME)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(handle)
                    .to_string();

                let is_admin = req
                    .headers()
                    .get(headers::USER_ADMIN)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);

                let user_ctx = UserContext {
                    handle: handle.to_string(),
                    name,
                    is_admin,
                };

                req.extensions_mut().insert(user_ctx);
            }
        }
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_names() {
        assert_eq!(headers::USER_HANDLE, "x-st-user-handle");
        assert_eq!(headers::USER_NAME, "x-st-user-name");
        assert_eq!(headers::USER_ADMIN, "x-st-user-admin");
    }
}
