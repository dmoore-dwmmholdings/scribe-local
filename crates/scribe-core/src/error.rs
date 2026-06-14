//! The crate-wide error type. Leaf crates convert their library errors
//! (sqlx, reqwest, io, …) into this so handlers and the CLI deal with one
//! `Result`. Errors that don't fit a variant become [`Error::Internal`] via
//! [`Error::internal`].

/// Convenience alias used throughout the workspace.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every fallible operation in Scribe surfaces one of these.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Configuration could not be loaded or was invalid.
    #[error("configuration error: {0}")]
    Config(String),

    /// A requested entity does not exist (maps to HTTP 404).
    #[error("not found: {0}")]
    NotFound(String),

    /// The caller supplied something invalid (maps to HTTP 400).
    #[error("invalid request: {0}")]
    BadRequest(String),

    /// Authentication / authorization failed (maps to HTTP 401/403).
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// A state-machine violation, e.g. completing a recording twice (HTTP 409).
    #[error("conflict: {0}")]
    Conflict(String),

    /// Database / persistence failure.
    #[error("database error: {0}")]
    Database(String),

    /// Object/blob storage failure.
    #[error("storage error: {0}")]
    Storage(String),

    /// A pipeline stage failed (transcode/diarize/transcribe/merge/embed/summarize).
    #[error("pipeline stage `{stage}` failed: {message}")]
    Pipeline { stage: &'static str, message: String },

    /// An ML model failed to load or run.
    #[error("model error: {0}")]
    Model(String),

    /// An outbound HTTP call (Ollama, audio pull, …) failed.
    #[error("http error: {0}")]
    Http(String),

    /// Local I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization failure.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// Anything not otherwise classified.
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Construct a [`Error::Pipeline`] from any stage error.
    pub fn pipeline<E: std::fmt::Display>(stage: &'static str, source: E) -> Self {
        Error::Pipeline {
            stage,
            message: source.to_string(),
        }
    }

    /// Wrap an arbitrary error as [`Error::Internal`].
    pub fn internal<E: std::fmt::Display>(e: E) -> Self {
        Error::Internal(e.to_string())
    }

    /// Stable short code, useful for logs/metrics and the API error body.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Config(_) => "config",
            Error::NotFound(_) => "not_found",
            Error::BadRequest(_) => "bad_request",
            Error::Unauthorized(_) => "unauthorized",
            Error::Conflict(_) => "conflict",
            Error::Database(_) => "database",
            Error::Storage(_) => "storage",
            Error::Pipeline { .. } => "pipeline",
            Error::Model(_) => "model",
            Error::Http(_) => "http",
            Error::Io(_) => "io",
            Error::Serde(_) => "serde",
            Error::Internal(_) => "internal",
        }
    }

    /// The HTTP status code the API should return for this error.
    pub fn http_status(&self) -> u16 {
        match self {
            Error::NotFound(_) => 404,
            Error::BadRequest(_) => 400,
            Error::Unauthorized(_) => 401,
            Error::Conflict(_) => 409,
            _ => 500,
        }
    }
}
