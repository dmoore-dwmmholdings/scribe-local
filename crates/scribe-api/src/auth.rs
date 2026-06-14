//! Device-token auth (design §12) — defense in depth behind the tailnet.
//!
//! A tower middleware checks the `Authorization: Bearer <device_key>` header
//! against the keys loaded from `cfg.auth.device_keys`. It is wired with
//! [`axum::middleware::from_fn_with_state`] over every route *except*
//! `GET /health`, which is mounted outside the auth layer.
//!
//! When `cfg.auth.require_device_token` is false (the dev default), the
//! middleware is a pass-through so local development needs no token.

use axum::extract::State;
use axum::http::{header, Request};
use axum::middleware::Next;
use axum::response::Response;

use scribe_core::Error;

use crate::error::ApiError;
use crate::state::AppState;

/// Reject requests lacking a valid bearer token when tokens are required.
pub async fn require_auth(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    if !state.auth.require_device_token {
        // Dev mode: allow all.
        return Ok(next.run(req).await);
    }

    let token = bearer_token(&req);
    match token {
        Some(t) if state.auth.is_valid(t) => Ok(next.run(req).await),
        Some(_) => Err(ApiError(Error::Unauthorized(
            "invalid device token".to_string(),
        ))),
        None => Err(ApiError(Error::Unauthorized(
            "missing Authorization: Bearer <device_token>".to_string(),
        ))),
    }
}

/// Extract the bearer token from the `Authorization` header, if present and
/// well-formed (`Bearer <token>`, case-insensitive scheme).
fn bearer_token<B>(req: &Request<B>) -> Option<&str> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        let token = token.trim();
        if token.is_empty() {
            None
        } else {
            Some(token)
        }
    } else {
        None
    }
}
