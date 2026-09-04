//! Device auth (design §12) — defense in depth behind the tailnet.
//!
//! A tower middleware authenticates every route *except* `GET /health`, which
//! is mounted outside the auth layer. It is wired with
//! [`axum::middleware::from_fn_with_state`].
//!
//! Two credentials are accepted:
//!
//! 1. **Tailnet identity** — the `Tailscale-User-Login` header that
//!    `tailscale serve` injects, when `cfg.auth.trust_tailscale_identity` is on.
//!    The tailnet has already authenticated the peer, so re-authenticating it
//!    with a secret the user must copy by hand buys nothing; this path exists so
//!    that a phone on the tailnet needs no key at all.
//! 2. **Device token** — `Authorization: Bearer <device_key>` checked against
//!    the keys loaded from `cfg.auth.device_keys`. Unchanged, and still the only
//!    way in from off the tailnet.
//!
//! When `cfg.auth.require_device_token` is false (the dev default), the
//! middleware is a pass-through so local development needs no token.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, Request};
use axum::middleware::Next;
use axum::response::Response;

use scribe_core::Error;

use crate::error::ApiError;
use crate::state::{AppState, AuthState};

/// The header `tailscale serve` injects on every proxied request, carrying the
/// tailnet login of the authenticated peer (e.g. `dawson@example.com`).
const TAILSCALE_USER_LOGIN: &str = "tailscale-user-login";

/// Reject requests lacking a valid bearer token when tokens are required.
///
/// Two credentials are accepted. A tailnet identity vouched for by
/// `tailscale serve` is tried first, because it is the one a user never has to
/// type; a device bearer token remains the fallback and the only option off the
/// tailnet.
pub async fn require_auth(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    if !state.auth.require_device_token {
        // Dev mode: allow all.
        return Ok(next.run(req).await);
    }

    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| *addr);
    if let Some(login) = tailnet_login(&state.auth, req.headers(), peer) {
        tracing::debug!(%login, "authenticated via tailnet identity");
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

/// Resolve the tailnet login this request may be authenticated as, or `None`.
///
/// Returning `None` never rejects on its own — the caller falls through to the
/// token check — so every branch here is free to be strict.
///
/// The header is only meaningful because `tailscale serve` sets it *and strips
/// any client-supplied copy* before proxying. That guarantee holds only for
/// requests that actually came through it, so we require the connection to
/// originate from loopback: `api.bind` is loopback-only by default, which
/// leaves the local `tailscale serve` process as the sole party able to reach
/// it. A deployment that rebinds the API to a routable address forfeits this,
/// which is why the whole path is opt-in.
///
/// `peer` is the address the connection came from, or `None` when the server was
/// not wired with `ConnectInfo` — treated as untrusted, so a wiring mistake
/// fails closed rather than accepting forged headers.
fn tailnet_login(
    auth: &AuthState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> Option<String> {
    if !auth.trust_tailscale_identity {
        return None;
    }

    // Read the header first: on a correctly bound server this is absent on
    // almost every request, and there is nothing to warn about when no one is
    // presenting the credential at all.
    let login = headers.get(TAILSCALE_USER_LOGIN)?.to_str().ok()?.trim();
    if login.is_empty() {
        return None;
    }

    if !peer.is_some_and(|addr| addr.ip().is_loopback()) {
        tracing::warn!(
            ?peer,
            "ignoring {TAILSCALE_USER_LOGIN} from a non-loopback peer; \
             is the API bound to a routable address?"
        );
        return None;
    }

    if !auth.tailnet_user_allowed(login) {
        tracing::warn!(%login, "tailnet user not in auth.tailnet_users; refusing");
        return None;
    }

    Some(login.to_string())
}

/// Reject update calls unless the update feature is enabled AND a valid update
/// token is presented. Distinct from device auth: the update token gates code
/// installation (design: §5 self-update). When disabled, returns 404 so the
/// endpoint is invisible.
pub async fn require_update_auth(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let upd = &state.cfg.update;
    if !upd.enabled {
        return Err(ApiError(Error::NotFound("update endpoint disabled".to_string())));
    }
    let Some(expected) = upd.token.as_deref().filter(|t| !t.is_empty()) else {
        return Err(ApiError(Error::Config(
            "update enabled but [update].token is unset".to_string(),
        )));
    };
    match bearer_token(&req) {
        Some(t) if ct_eq(t, expected) => Ok(next.run(req).await),
        Some(_) => Err(ApiError(Error::Unauthorized("invalid update token".to_string()))),
        None => Err(ApiError(Error::Unauthorized(
            "missing Authorization: Bearer <update_token>".to_string(),
        ))),
    }
}

/// Constant-time string comparison for secrets.
fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && a.as_bytes().ct_eq(b.as_bytes()).unwrap_u8() == 1
}

/// Extract the bearer token from the `Authorization` header, if present and
/// well-formed (`Bearer <token>`, case-insensitive scheme).
pub(crate) fn bearer_token<B>(req: &Request<B>) -> Option<&str> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(trust: bool, users: &[&str]) -> AuthState {
        AuthState {
            require_device_token: true,
            keys: Default::default(),
            trust_tailscale_identity: trust,
            tailnet_users: users.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn headers(login: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(l) = login {
            h.insert(TAILSCALE_USER_LOGIN, l.parse().unwrap());
        }
        h
    }

    fn loopback() -> Option<SocketAddr> {
        Some("127.0.0.1:54321".parse().unwrap())
    }

    fn lan() -> Option<SocketAddr> {
        Some("192.168.1.50:54321".parse().unwrap())
    }

    #[test]
    fn accepts_loopback_header_when_trust_is_on() {
        let got = tailnet_login(&auth(true, &[]), &headers(Some("dawson@example.com")), loopback());
        assert_eq!(got.as_deref(), Some("dawson@example.com"));
    }

    #[test]
    fn refuses_when_trust_is_off() {
        // The default posture: the header is inert until explicitly enabled.
        let got = tailnet_login(&auth(false, &[]), &headers(Some("dawson@example.com")), loopback());
        assert_eq!(got, None);
    }

    #[test]
    fn refuses_forged_header_from_a_routable_peer() {
        // The whole threat model: someone who can reach a wrongly-bound API
        // sets the header themselves.
        let got = tailnet_login(&auth(true, &[]), &headers(Some("attacker@evil.example")), lan());
        assert_eq!(got, None);
    }

    #[test]
    fn refuses_when_connect_info_is_missing() {
        // Fails closed — an unknown peer is not a loopback peer.
        let got = tailnet_login(&auth(true, &[]), &headers(Some("dawson@example.com")), None);
        assert_eq!(got, None);
    }

    #[test]
    fn refuses_a_login_outside_the_allowlist() {
        let a = auth(true, &["dawson@example.com"]);
        assert_eq!(tailnet_login(&a, &headers(Some("guest@example.com")), loopback()), None);
        assert!(tailnet_login(&a, &headers(Some("dawson@example.com")), loopback()).is_some());
    }

    #[test]
    fn allowlist_matching_ignores_case() {
        let a = auth(true, &["Dawson@Example.com"]);
        assert!(tailnet_login(&a, &headers(Some("dawson@example.com")), loopback()).is_some());
    }

    #[test]
    fn refuses_absent_or_blank_login() {
        assert_eq!(tailnet_login(&auth(true, &[]), &headers(None), loopback()), None);
        assert_eq!(tailnet_login(&auth(true, &[]), &headers(Some("   ")), loopback()), None);
    }
}
