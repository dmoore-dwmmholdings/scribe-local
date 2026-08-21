//! `scribe-db` — the persistence layer for Scribe.
//!
//! This crate owns every SQL statement in the system. It maps Postgres rows
//! (the schema in `migrations/`, design §10) onto the storage-agnostic domain
//! types in [`scribe_core::types`], and implements the job queue (design §7)
//! and hybrid search (design §9).
//!
//! The entry point is [`Db`], a thin handle around a [`sqlx::PgPool`]. All
//! operations are exposed as methods on it, grouped into modules by table:
//!
//! * [`recordings`] — meeting metadata + lifecycle.
//! * [`segments`] — chunked-upload pieces.
//! * [`queue`] — the `FOR UPDATE SKIP LOCKED` work queue.
//! * [`speakers`] / [`recording_speakers`] — diarization identity.
//! * [`transcript`] — utterances / words.
//! * [`chunks`] — retrieval chunks + embeddings.
//! * [`summaries`] — LLM-generated metadata.
//! * [`search`] — hybrid keyword + vector search with RRF fusion.
//! * [`settings`] — server-side settings documents (the processing schedule).
//!
//! ## sqlx style
//!
//! Queries use the **runtime** functions (`sqlx::query`, `query_as`,
//! `query_scalar`) rather than the compile-time-checked macros, so the crate
//! builds without a live `DATABASE_URL`. Rows are mapped manually via
//! `Row::try_get` / `FromRow` into the core structs.

use scribe_core::config::DatabaseConfig;
use scribe_core::Result;
use sqlx::postgres::{PgListener, PgPoolOptions};
use sqlx::PgPool;

pub mod chunks;
pub mod queue;
pub mod recording_speakers;
pub mod recordings;
pub mod search;
pub mod segments;
pub mod settings;
pub mod speakers;
pub mod summaries;
pub mod transcript;

mod error;
mod row;

pub use error::db_err;

/// The NOTIFY channel workers `LISTEN` on for instant wakeups (design §6/§7).
/// Matches the trigger payload in `migrations/0001_init.sql`.
pub const JOBS_CHANNEL: &str = "scribe_jobs";

/// Safety guard for **destructive integration tests**.
///
/// The smoke / api / pipeline integration tests `DROP SCHEMA public CASCADE` and
/// insert fixtures against whatever `DATABASE_URL` points at. If that is the live
/// dev database the app uses (`scribe` on :5433), running `cargo test` silently
/// wipes real recordings. Call this before any destructive test setup: it refuses
/// to proceed unless the target database is clearly disposable — its name ends in
/// `_test`/`-test` (e.g. `scribe_test`) — or the operator explicitly opts in with
/// `SCRIBE_ALLOW_DESTRUCTIVE_TESTS=1`.
pub fn assert_disposable_test_db(url: &str) {
    // Database name = the last '/'-segment, minus any `?query`/`#fragment`.
    let name = url
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let disposable = name.ends_with("_test") || name.ends_with("-test");
    let overridden = std::env::var("SCRIBE_ALLOW_DESTRUCTIVE_TESTS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    assert!(
        disposable || overridden,
        "refusing to run a destructive schema reset against database `{name}`: the \
         integration tests DROP SCHEMA and would wipe it. Point DATABASE_URL at a \
         throwaway database whose name ends in `_test` (e.g. \
         postgres://scribe:scribe@localhost:5433/scribe_test), or set \
         SCRIBE_ALLOW_DESTRUCTIVE_TESTS=1 to override."
    );
}

/// Handle to the Postgres-backed store. Cheap to clone (the pool is `Arc`-shared).
#[derive(Debug, Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Connect to Postgres using the pool settings in [`DatabaseConfig`].
    pub async fn connect(cfg: &DatabaseConfig) -> Result<Db> {
        let pool = PgPoolOptions::new()
            .max_connections(cfg.max_connections)
            .connect(&cfg.url)
            .await
            .map_err(db_err)?;
        Ok(Db { pool })
    }

    /// Build a [`Db`] from an already-constructed pool (useful in tests).
    pub fn from_pool(pool: PgPool) -> Db {
        Db { pool }
    }

    /// Borrow the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Apply all migrations from the workspace `migrations/` directory.
    ///
    /// The path is resolved at compile time relative to this crate's manifest
    /// dir, so `sqlx::migrate!` embeds the SQL into the binary.
    pub async fn run_migrations(&self) -> Result<()> {
        sqlx::migrate!("../../migrations")
            .run(&self.pool)
            .await
            .map_err(|e| scribe_core::Error::Database(e.to_string()))?;
        Ok(())
    }

    /// A [`PgListener`] subscribed to the `scribe_jobs` NOTIFY channel. The
    /// worker awaits notifications on this to wake the instant a job is enqueued
    /// (with a poll backstop in case a NOTIFY is missed — design §6).
    pub async fn job_listener(&self, cfg: &DatabaseConfig) -> Result<PgListener> {
        let mut listener = PgListener::connect(&cfg.url).await.map_err(db_err)?;
        listener.listen(JOBS_CHANNEL).await.map_err(db_err)?;
        Ok(listener)
    }
}
