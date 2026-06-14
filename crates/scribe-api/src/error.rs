//! API error handling: a thin wrapper over [`scribe_core::Error`] that knows how
//! to turn itself into an HTTP response.
//!
//! Handlers return `Result<T, ApiError>`; any `scribe_core::Error` (or anything
//! convertible into one) bubbles up via `?` and is mapped to the right status by
//! [`scribe_core::Error::http_status`]. The body is a stable
//! `{"error":{"code","message"}}` shape the mobile app can branch on.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use scribe_core::Error;

/// Newtype wrapper so we can implement `IntoResponse` for `scribe_core::Error`
/// without orphan-rule trouble.
#[derive(Debug)]
pub struct ApiError(pub Error);

impl ApiError {
    /// The underlying domain error.
    pub fn inner(&self) -> &Error {
        &self.0
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError(e)
    }
}

// Let handlers use `?` on the common leaf error types directly.
impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError(Error::Io(e))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError(Error::Serde(e))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let err = self.0;
        let status =
            StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        // 5xx errors carry internal detail we don't want to leak verbatim, but
        // for a single-user self-hosted system the operator *is* the user, so we
        // surface the message — it makes debugging the mobile app far easier.
        let body = Json(json!({
            "error": {
                "code": err.code(),
                "message": err.to_string(),
            }
        }));
        if status.is_server_error() {
            tracing::error!(code = err.code(), error = %err, "request failed");
        } else {
            tracing::debug!(code = err.code(), error = %err, "request rejected");
        }
        (status, body).into_response()
    }
}

/// Convenience alias for handler return types.
pub type ApiResult<T> = Result<T, ApiError>;
