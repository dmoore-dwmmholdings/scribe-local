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
//! GET    /recordings                                    (optional ?tag= filter)
//! GET    /recordings/{id}                               (detail: summaries[] per template)
//! DELETE /recordings/{id}                               (recording + derived rows + blobs)
//! POST   /recordings/{id}/complete
//! POST   /recordings/{id}/reprocess                     (re-run the whole pipeline)
//! POST   /recordings/{id}/summarize                     (re-summarize w/ template, adds a view)
//! POST   /recordings/{id}/translate                      (translate the summary via the LLM)
//! PUT    /recordings/{id}/tags                           (replace org tags)
//! PUT    /recordings/{id}/participants                   (state the speaker count)
//! PATCH  /recordings/{id}/utterances/{utterance_id}      (edit transcript text)
//! PUT    /recordings/{id}/segments/{seq}               (body limit disabled, streamed)
//! GET    /recordings/{id}/segments/{seq}               (range support)
//! GET    /recordings/{id}/audio                         (range support)
//! POST   /recordings/{id}/speakers/{local_idx}/name      (tag by name or speaker_id)
//! DELETE /recordings/{id}/speakers/{local_idx}/name      (untag, back to "Speaker N")
//! GET    /speakers                                       (enrolled speaker library)
//! PATCH  /speakers/{id}                                  (rename everywhere)
//! DELETE /speakers/{id}                                  (forget a speaker)
//! GET    /tags                                           (distinct tags in use)
//! GET    /search
//! POST   /ask
//! GET    /summary-templates
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
use axum::routing::{get, patch, post, put};
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
        .route(
            "/recordings/{id}",
            get(handlers::recordings::get_recording)
                .delete(handlers::recordings::delete_recording),
        )
        .route(
            "/recordings/{id}/complete",
            post(handlers::recordings::complete_recording),
        )
        .route(
            "/recordings/{id}/reprocess",
            post(handlers::recordings::reprocess_recording),
        )
        .route(
            "/recordings/{id}/summarize",
            post(handlers::recordings::summarize_recording),
        )
        .route(
            "/recordings/{id}/translate",
            post(handlers::recordings::translate_summary),
        )
        .route(
            "/recordings/{id}/tags",
            put(handlers::recordings::set_recording_tags),
        )
        .route(
            "/recordings/{id}/participants",
            put(handlers::recordings::set_participants),
        )
        .route(
            "/recordings/{id}/utterances/{utterance_id}",
            patch(handlers::recordings::edit_utterance),
        )
        .route("/tags", get(handlers::recordings::list_tags))
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
            post(handlers::speakers::name_speaker).delete(handlers::speakers::unname_speaker),
        )
        .route("/speakers", get(handlers::speakers::list_speakers))
        .route(
            "/speakers/{id}",
            patch(handlers::speakers::rename_speaker)
                .delete(handlers::speakers::delete_speaker),
        )
        .route("/search", get(handlers::search::search))
        .route("/ask", post(handlers::search::ask))
        .route(
            "/summary-templates",
            get(handlers::recordings::list_summary_templates),
        )
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
