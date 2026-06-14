//! Role-shared configuration. The same `Config` is loaded by every subcommand;
//! `serve` reads the `[api]`/`[storage]` sections, `worker` reads
//! `[worker]`/`[asr]`/`[llm]`, both read `[database]`. See design §17 appendix B.
//!
//! Loading order (later overrides earlier):
//!   1. built-in [`Default`] values,
//!   2. the TOML file passed via `--config`,
//!   3. environment variables prefixed `SCRIBE_` (double underscore = nesting,
//!      e.g. `SCRIBE_DATABASE__URL`, `SCRIBE_API__BIND`).

use std::path::{Path, PathBuf};

use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Top-level configuration, deserialized from TOML + env.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub database: DatabaseConfig,
    pub storage: StorageConfig,
    pub api: ApiConfig,
    pub auth: AuthConfig,
    pub asr: AsrConfig,
    pub worker: WorkerConfig,
    pub llm: LlmConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// `postgres://user@host/db?sslmode=require`
    pub url: String,
    /// Max connections in the pool.
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Root directory for audio blobs (`{root}/{recording_id}/segments/…`).
    pub blobs: PathBuf,
    /// Secret used to sign short-lived audio-pull URLs handed to the worker.
    pub signing_secret: String,
    /// Lifetime of a signed audio-pull URL.
    pub signed_url_ttl_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Socket the Axum server binds. `tailscale serve` fronts this on the tailnet.
    pub bind: String,
    /// Optional `rustus` tus server upstream the completion hook posts back from.
    pub tus_upstream: Option<String>,
    /// Max upload body size (bytes) for a single segment PATCH/PUT.
    pub max_segment_bytes: usize,
    /// Public base URL the worker uses to pull audio (tailnet MagicDNS name).
    pub public_base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Path to a TOML file of `device_id = "api_key"` pairs (defense in depth).
    pub device_keys: Option<PathBuf>,
    /// If true, reject any request without a recognised device token.
    pub require_device_token: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    /// ASR model id (e.g. `parakeet-tdt-0.6b-v3` or `whisper-large-v3-turbo`).
    pub model: String,
    /// Enable the diarization stage.
    pub diarization: bool,
    /// Compute device hint: `cpu` or `cuda`.
    pub device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerConfig {
    /// Which job kinds this worker handles. `["all"]` for every stage.
    pub stages: Vec<String>,
    /// Parallel jobs (GPU box: usually 1 so the card isn't oversubscribed).
    pub concurrency: usize,
    /// Directory holding ONNX model assets.
    pub models_dir: PathBuf,
    /// Heartbeat interval (secs) for the visibility-timeout reaper (design §7).
    pub heartbeat_secs: u64,
    /// Poll backstop interval (secs) in case a `NOTIFY` is missed.
    pub poll_secs: u64,
    /// Max attempts before a job is parked in `failed`.
    pub max_attempts: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// Ollama OpenAI-compatible base URL.
    pub ollama_url: String,
    /// Model used for summaries / Q&A.
    pub summarize_model: String,
    /// Embedding model id (dimension is a schema commitment — design §9).
    pub embed_model: String,
    /// Embedding dimension; must match the `chunks.embedding` column width.
    pub embed_dim: usize,
}

// --------------------------------------------------------------------------
// Defaults
// --------------------------------------------------------------------------

impl Default for Config {
    fn default() -> Self {
        Self {
            database: DatabaseConfig::default(),
            storage: StorageConfig::default(),
            api: ApiConfig::default(),
            auth: AuthConfig::default(),
            asr: AsrConfig::default(),
            worker: WorkerConfig::default(),
            llm: LlmConfig::default(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://scribe@localhost/scribe".to_string(),
            max_connections: 10,
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            blobs: PathBuf::from("/var/lib/scribe/blobs"),
            signing_secret: "change-me-in-production".to_string(),
            signed_url_ttl_secs: 600,
        }
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8443".to_string(),
            tus_upstream: None,
            max_segment_bytes: 16 * 1024 * 1024,
            public_base_url: "http://127.0.0.1:8443".to_string(),
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            device_keys: None,
            require_device_token: false,
        }
    }
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            model: "parakeet-tdt-0.6b-v3".to_string(),
            diarization: true,
            device: "cpu".to_string(),
        }
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            stages: vec!["all".to_string()],
            concurrency: 1,
            models_dir: PathBuf::from("/var/lib/scribe/models"),
            heartbeat_secs: 15,
            poll_secs: 5,
            max_attempts: 5,
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            ollama_url: "http://127.0.0.1:11434".to_string(),
            summarize_model: "gemma3:27b".to_string(),
            embed_model: "nomic-embed-text".to_string(),
            embed_dim: 768,
        }
    }
}

// --------------------------------------------------------------------------
// Loading
// --------------------------------------------------------------------------

impl Config {
    /// Load configuration from an optional TOML file plus `SCRIBE_*` env vars.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut fig = Figment::from(Serialized::defaults(Config::default()));
        if let Some(p) = path {
            if !p.exists() {
                return Err(Error::Config(format!(
                    "config file does not exist: {}",
                    p.display()
                )));
            }
            fig = fig.merge(Toml::file(p));
        }
        fig = fig.merge(Env::prefixed("SCRIBE_").split("__"));
        fig.extract()
            .map_err(|e| Error::Config(format!("invalid configuration: {e}")))
    }

    /// Resolve the effective stage list (`["all"]` expands to every kind).
    pub fn effective_stages(&self) -> Vec<crate::types::JobKind> {
        use crate::types::JobKind;
        if self
            .worker
            .stages
            .iter()
            .any(|s| s.eq_ignore_ascii_case("all"))
        {
            JobKind::ALL.to_vec()
        } else {
            self.worker
                .stages
                .iter()
                .filter_map(|s| s.parse::<JobKind>().ok())
                .collect()
        }
    }
}
