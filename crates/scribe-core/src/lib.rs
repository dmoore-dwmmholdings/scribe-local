//! `scribe-core` — the shared contract for the Scribe system.
//!
//! Every other crate depends on this one and nothing else internal. It holds:
//!
//! * [`config`] — the role-shared TOML/env configuration (see design §17 appendix B).
//! * [`error`] — the crate-wide error type and `Result` alias.
//! * [`types`] — the domain model: recordings, segments, jobs, speakers,
//!   utterances, chunks, summaries. These mirror the Postgres schema in
//!   `migrations/` (design §10) but are storage-agnostic.
//!
//! The rule for keeping a multi-crate Rust workspace coherent: domain types and
//! interfaces live here, implementations live in the leaf crates. If a leaf
//! crate needs a new shared type, it is added here, not invented locally.

pub mod config;
pub mod error;
pub mod types;

pub use error::{Error, Result};

/// Initialise process-wide tracing from the `RUST_LOG` env var (defaulting to
/// `info`). Safe to call once at the start of any subcommand.
pub fn init_tracing(json: bool) {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,hyper=warn"));

    let builder = fmt().with_env_filter(filter).with_target(true);
    if json {
        // `try_init` so repeated calls in tests don't panic.
        let _ = builder.json().try_init();
    } else {
        let _ = builder.try_init();
    }
}

/// Crate version string, surfaced by `scribe --version` and `/health`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
