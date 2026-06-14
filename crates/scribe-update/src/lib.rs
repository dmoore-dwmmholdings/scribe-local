//! `scribe-update` — signed-package self-update for the `scribe` binary.
//!
//! The backend can update itself: the frontend (or `scribe update apply`)
//! hands over a **package** — a `.tar.gz` containing the new binary, a JSON
//! manifest, and an ed25519 signature over that manifest. This crate verifies
//! it, stages it, runs the new binary's migrations, atomically swaps it into
//! place (keeping a `.old` backup for rollback), and restarts.
//!
//! Security model (design choice: *token + signature*):
//! * The HTTP endpoint requires an update **token** (authorization).
//! * The package manifest must carry a valid **ed25519 signature** from the
//!   operator's key (authenticity) — a leaked token alone cannot install code.
//! * The manifest binds the binary by **sha256** (integrity).
//!
//! Everything here is pure Rust (ed25519-dalek, sha2, tar, flate2) — no native
//! deps — so it builds on every platform the backend targets.
//!
//! ## Package layout (`.tar.gz`)
//! ```text
//! manifest.json   # {name, version, target, sha256, created_at, notes}
//! manifest.sig    # hex ed25519 signature over the exact bytes of manifest.json
//! scribe          # the new binary (scribe.exe on Windows)
//! ```

mod apply;
mod keys;
mod manifest;
mod package;
mod restart;
mod verify;

pub use apply::{apply_package, rollback, ApplyOptions, UpdateOutcome};
pub use keys::{generate_keypair, sign_package, Keypair};
pub use manifest::Manifest;
pub use package::{current_target, Package, BINARY_ENTRY, MANIFEST_ENTRY, SIG_ENTRY};
pub use restart::restart;
pub use verify::verify_package;

/// Result alias for the crate.
pub type Result<T> = std::result::Result<T, UpdateError>;

/// Failures specific to the update flow. Converts into [`scribe_core::Error`]
/// (`BadRequest` for anything the caller could fix, `Internal`/`Storage` for
/// host-side failures) via [`UpdateError::into_core`].
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("update is disabled (set [update].enabled = true and configure a key + token)")]
    Disabled,
    #[error("update misconfigured: {0}")]
    Misconfigured(String),
    #[error("invalid package: {0}")]
    InvalidPackage(String),
    #[error("signature verification failed")]
    BadSignature,
    #[error("checksum mismatch: manifest says {expected}, binary is {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("refusing downgrade: package {package} <= running {running} (set allow_downgrade)")]
    Downgrade { package: String, running: String },
    #[error("target mismatch: package is for `{package}`, host is `{host}` (set allow_target_mismatch)")]
    TargetMismatch { package: String, host: String },
    #[error("staged binary failed its sanity check: {0}")]
    SanityCheck(String),
    #[error("database migration with the new binary failed: {0}")]
    Migration(String),
    #[error("no backup binary to roll back to")]
    NoBackup,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

impl UpdateError {
    /// Map onto the workspace error type, choosing a sensible HTTP-ish class.
    pub fn into_core(self) -> scribe_core::Error {
        use scribe_core::Error as E;
        match self {
            UpdateError::Disabled => E::NotFound("update endpoint disabled".into()),
            UpdateError::Misconfigured(m) => E::Config(m),
            UpdateError::BadSignature
            | UpdateError::InvalidPackage(_)
            | UpdateError::ChecksumMismatch { .. }
            | UpdateError::Downgrade { .. }
            | UpdateError::TargetMismatch { .. } => E::BadRequest(self.to_string()),
            UpdateError::NoBackup => E::NotFound(self.to_string()),
            other => E::Internal(other.to_string()),
        }
    }
}

impl From<UpdateError> for scribe_core::Error {
    fn from(e: UpdateError) -> Self {
        e.into_core()
    }
}

/// The version of the currently-running binary.
pub fn running_version() -> &'static str {
    scribe_core::VERSION
}
