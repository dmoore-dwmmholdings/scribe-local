//! `scribe-api` — the always-on storage-node HTTP API (design §6, §9–§12).
//!
//! This crate is the `serve` half of the single `scribe` binary: an Axum 0.8
//! server that accepts segmented audio uploads, stores blobs on disk, enqueues
//! the processing pipeline, and answers transcript/search/RAG queries. TLS is
//! terminated in front by `tailscale serve` (design §5), so we speak plain HTTP.
//!
//! Public surface (called by the CLI and tests):
//!
//! * [`serve`] — build state, construct the router, bind, and serve forever.
//! * [`router`] — build the router from an [`AppState`] (no bind; for tests).
//! * [`build_state`] — construct the [`AppState`] from a [`Config`].
//! * [`AppState`] — the shared, cheaply-cloneable handler state.
//!
//! ## Routes
//!
//! ```text
//! GET    /health                                       (unauthenticated)
//! POST   /recordings
//! GET    /recordings
//! GET    /recordings/{id}
//! POST   /recordings/{id}/complete
//! PUT    /recordings/{id}/segments/{seq}               (body limit disabled, streamed)
//! GET    /recordings/{id}/segments/{seq}               (range support)
//! GET    /recordings/{id}/audio                         (range support)
//! POST   /recordings/{id}/speakers/{local_idx}/name
//! GET    /search
//! POST   /ask
//! ```
//!
//! Every route except `GET /health` passes through the device-token auth layer
//! (a no-op unless `cfg.auth.require_device_token`).

mod auth;
mod error;
mod handlers;
mod range;
mod state;

use std::net::SocketAddr;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post, put};
use axum::Router;

use scribe_core::config::Config;
use scribe_core::{Error, Result};

pub use error::{ApiError, ApiResult};
pub use state::{AppState, AuthState};

/// Build [`AppState`] from a [`Config`]: connect the DB, build the embedder,
/// point the Ollama client, and load device keys. No socket is bound.
pub async fn build_state(cfg: Config) -> Result<AppState> {
    AppState::build(cfg).await
}

/// Construct the application router for a given [`AppState`].
///
/// `/health` is mounted *outside* the auth middleware; everything else is behind
/// it. The segment-upload route gets its body limit disabled so large chunks
/// stream to disk rather than tripping Axum's `DefaultBodyLimit` (design §10 /
/// the Axum 0.8 note about raising the 2 MB default).
pub fn router(state: AppState) -> Router {
    // Authenticated routes.
    let authed = Router::new()
        .route(
            "/recordings",
            post(handlers::recordings::create_recording).get(handlers::recordings::list_recordings),
        )
        .route("/recordings/{id}", get(handlers::recordings::get_recording))
        .route(
            "/recordings/{id}/complete",
            post(handlers::recordings::complete_recording),
        )
        // Upload streams to disk; disable the body limit on just this route.
        .route(
            "/recordings/{id}/segments/{seq}",
            put(handlers::segments::put_segment)
                .get(handlers::segments::get_segment)
                .layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/recordings/{id}/audio",
            get(handlers::audio::get_audio),
        )
        .route(
            "/recordings/{id}/speakers/{local_idx}/name",
            post(handlers::speakers::name_speaker),
        )
        .route("/search", get(handlers::search::search))
        .route("/ask", post(handlers::search::ask))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    // Self-update routes sit behind the *update token* (not device auth) and
    // only respond when [update].enabled. The package upload streams to disk,
    // so its body limit is disabled too.
    let admin = Router::new()
        .route(
            "/admin/update",
            post(handlers::admin::update).layer(DefaultBodyLimit::disable()),
        )
        .route("/admin/update/rollback", post(handlers::admin::rollback_handler))
        .route("/admin/info", get(handlers::admin::info))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_update_auth,
        ));

    // Health is unauthenticated and outside the auth layer.
    Router::new()
        .route("/health", get(handlers::health::health))
        .merge(admin)
        .merge(authed)
        .with_state(state)
}

/// Build state, construct the router, bind `cfg.api.bind`, and serve forever.
///
/// Plain HTTP — `tailscale serve` terminates TLS in front (design §5).
pub async fn serve(cfg: Config) -> Result<()> {
    let bind = cfg.api.bind.clone();
    let state = build_state(cfg).await?;
    let app = router(state);

    let addr: SocketAddr = bind
        .parse()
        .map_err(|e| Error::Config(format!("invalid api.bind `{bind}`: {e}")))?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| Error::Internal(format!("binding {addr}: {e}")))?;
    let local = listener
        .local_addr()
        .map_err(|e| Error::Internal(format!("reading local addr: {e}")))?;
    tracing::info!(%local, "scribe-api listening");

    axum::serve(listener, app)
        .await
        .map_err(|e| Error::Internal(format!("server error: {e}")))?;
    Ok(())
}
