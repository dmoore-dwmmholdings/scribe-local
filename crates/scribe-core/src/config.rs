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
    pub update: UpdateConfig,
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
    /// Announce this server over mDNS as `_scribe._tcp`, so the app can find it
    /// on the local network instead of being told the URL by hand.
    ///
    /// What is advertised is `public_base_url` — the tailnet address — not this
    /// machine's LAN address, so `bind` can stay loopback-only. mDNS is
    /// link-local multicast: a container on Docker's default bridge network
    /// cannot send it, so this needs host networking to reach the LAN.
    pub advertise_lan: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Path to a TOML file of `device_id = "api_key"` pairs (defense in depth).
    pub device_keys: Option<PathBuf>,
    /// If true, reject any request without a recognised device token.
    pub require_device_token: bool,
    /// Accept the identity headers `tailscale serve` injects (`Tailscale-User-Login`)
    /// in place of a device token, so a phone on the tailnet needs no key at all.
    ///
    /// Off by default, and deliberately so: headers are forgeable, and this is
    /// only sound when nothing but the local `tailscale serve` process can reach
    /// `api.bind`. The middleware additionally refuses the header on any
    /// connection that did not arrive from loopback, but that check cannot save
    /// a deployment that publishes the port on `0.0.0.0`.
    pub trust_tailscale_identity: bool,
    /// Tailnet logins allowed in when `trust_tailscale_identity` is on. Empty
    /// means any user the tailnet vouches for — right for a personal tailnet,
    /// wrong for a shared one.
    pub tailnet_users: Vec<String>,
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
    /// Strip non-lexical filler words (uh, um, …) from the transcript.
    pub remove_fillers: bool,
    /// The filler words removed when `remove_fillers` is on (matched
    /// case-insensitively, surrounding punctuation ignored).
    pub filler_words: Vec<String>,
    /// Optional hotwords file (one phrase per line) that biases recognition
    /// toward known names / domain terms — the targeted fix for the ASR
    /// mis-hearing proper nouns. When set, decoding switches to
    /// `modified_beam_search` (required for hotwords to take effect on the
    /// transducer). Leave unset to keep the default greedy decode unchanged.
    /// Transducer (Parakeet) models only — ignored by Whisper.
    pub hotwords_file: Option<String>,
    /// Boost applied to hotwords (typical range 1.5–3.0; higher = stronger bias
    /// but more risk of false positives). Only used when `hotwords_file` is set.
    pub hotwords_score: f32,
}

/// Default non-lexical fillers stripped from transcripts. Deliberately
/// conservative — only clear hesitation sounds, not contentious words like
/// "like" or "you know" (add those to `[asr].filler_words` if you want them).
pub fn default_filler_words() -> Vec<String> {
    [
        "uh", "uhh", "um", "umm", "uhm", "er", "err", "erm", "ah", "ahh", "hmm", "hm", "mm",
        "mmm", "mhm", "eh", "uhhuh",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
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

/// Which local LLM server serves chat (summaries / RAG answers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    /// Ollama's native API (`/api/chat`, `/api/tags`), default `:11434`.
    Ollama,
    /// Any OpenAI-compatible server (`/v1/chat/completions`, `/v1/models`):
    /// **LM Studio**, llama.cpp server, vLLM, etc.
    #[serde(alias = "openai-compatible", alias = "lmstudio", alias = "lm-studio")]
    Openai,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// Which server type to talk to: `ollama` (default) or `openai`
    /// (OpenAI-compatible, e.g. LM Studio).
    pub provider: LlmProvider,
    /// Base URL of the chat server. For Ollama: `http://127.0.0.1:11434`.
    /// For LM Studio / OpenAI-compatible: `http://127.0.0.1:1234/v1`
    /// (the `/v1` is added automatically if omitted).
    pub base_url: String,
    /// Bearer token for OpenAI-compatible servers that require one. LM Studio
    /// ignores it; leave unset (`None`) unless your server enforces a key.
    pub api_key: Option<String>,
    /// Model used for summaries / Q&A. For LM Studio this is the loaded model's
    /// identifier (as shown in LM Studio / `GET /v1/models`).
    pub summarize_model: String,
    /// Summary template id used by the automatic pipeline run (`general`,
    /// `standup`, `interview`, `one_on_one`, `lecture`, `sales`). A recording can
    /// be re-summarized with a different template via the API. Unknown/empty
    /// falls back to `general`.
    pub summary_template: String,
    /// Embedding model id (dimension is a schema commitment — design §9).
    pub embed_model: String,
    /// Embedding dimension; must match the `chunks.embedding` column width.
    pub embed_dim: usize,
    /// Run a best-effort LLM pass over the merged transcript to fix
    /// misrecognised names / proper nouns. Degrades gracefully (keeps the raw
    /// text) if the LLM is unreachable or returns anything unexpected.
    pub correct_transcript: bool,
    /// Largest transcript, in characters, that may go into a single prompt.
    ///
    /// A long meeting does not fit in a local model's context: a 50-minute
    /// recording is ~30 000 characters, and an 8k-token model rejects the
    /// request outright (`exceed_context_size_error`), which used to leave the
    /// recording with a silently empty summary. Above this budget the summarize
    /// stage switches to map-reduce — summarize each part, then summarize those
    /// notes. Roughly 4 characters per token, so the default leaves ample room
    /// for the instructions and the reply inside an 8k context.
    pub max_prompt_chars: usize,
}

/// How `serve` restarts itself into a freshly-installed binary (design: §5
/// self-update). `SelfExec` replaces the process image (`execv`) and needs no
/// service manager; `Supervisor` exits cleanly and lets systemd/launchd relaunch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartMode {
    SelfExec,
    Supervisor,
}

/// Backend self-update over the API (`POST /admin/update`). Disabled by default
/// — enabling it lets a holder of the update token install a new binary, so it
/// requires an ed25519 public key and a token to turn on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// Master switch. When false, the update endpoints return 404.
    pub enabled: bool,
    /// Bearer token authorizing update calls (distinct from device keys).
    pub token: Option<String>,
    /// ed25519 public key (hex, 32 bytes) that must have signed the package
    /// manifest. Required when `enabled`.
    pub public_key: Option<String>,
    /// How to restart after a successful install.
    pub restart: RestartMode,
    /// Directory where uploaded packages are staged/unpacked.
    pub staging_dir: PathBuf,
    /// Path to the running binary to replace. `None` → `std::env::current_exe()`.
    pub binary_path: Option<PathBuf>,
    /// Allow installing a package whose version is <= the running version.
    pub allow_downgrade: bool,
    /// Allow installing a package whose `target` triple differs from this host.
    pub allow_target_mismatch: bool,
    /// Delay (ms) between answering the update request and restarting, so the
    /// HTTP response flushes to the client first.
    pub restart_delay_ms: u64,
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
            update: UpdateConfig::default(),
        }
    }
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token: None,
            public_key: None,
            restart: RestartMode::SelfExec,
            staging_dir: PathBuf::from("/var/lib/scribe/updates"),
            binary_path: None,
            allow_downgrade: false,
            allow_target_mismatch: false,
            restart_delay_ms: 750,
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
            advertise_lan: false,
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            device_keys: None,
            require_device_token: false,
            trust_tailscale_identity: false,
            tailnet_users: Vec::new(),
        }
    }
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            model: "parakeet-tdt-0.6b-v3".to_string(),
            diarization: true,
            device: "cpu".to_string(),
            remove_fillers: true,
            filler_words: default_filler_words(),
            hotwords_file: None,
            hotwords_score: 2.0,
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
            provider: LlmProvider::Ollama,
            base_url: "http://127.0.0.1:11434".to_string(),
            api_key: None,
            summarize_model: "gemma3:27b".to_string(),
            summary_template: "general".to_string(),
            embed_model: "nomic-embed-text".to_string(),
            embed_dim: 768,
            correct_transcript: true,
            max_prompt_chars: 18_000,
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
        let mut cfg: Config = fig
            .extract()
            .map_err(|e| Error::Config(format!("invalid configuration: {e}")))?;
        cfg.normalize_paths();
        Ok(cfg)
    }

    /// Resolve filesystem paths to absolute form. Relative paths in the config
    /// are otherwise interpreted against each process's working directory, which
    /// (a) makes `serve` and `worker` disagree on `blobs` if launched from
    /// different dirs, and (b) breaks ffmpeg's concat demuxer, which resolves
    /// list entries relative to the list file. Absolutizing once at load avoids
    /// both. Absolute paths (the production norm) pass through unchanged.
    pub fn normalize_paths(&mut self) {
        fn abs(p: &Path) -> PathBuf {
            std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
        }
        self.storage.blobs = abs(&self.storage.blobs);
        self.worker.models_dir = abs(&self.worker.models_dir);
        self.update.staging_dir = abs(&self.update.staging_dir);
        if let Some(bp) = &self.update.binary_path {
            self.update.binary_path = Some(abs(bp));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use figment::{providers::Env, Figment};

    /// `deploy/docker-compose.lan.yml` turns discovery on with
    /// `SCRIBE_API__ADVERTISE_LAN=true`, which only works if figment coerces the
    /// string to a bool. A silent failure here would leave the overlay looking
    /// applied while the server never advertises.
    #[test]
    fn bool_config_can_be_set_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("SCRIBE_API__ADVERTISE_LAN", "true");
            let cfg: Config = Figment::from(Serialized::defaults(Config::default()))
                .merge(Env::prefixed("SCRIBE_").split("__"))
                .extract()
                .expect("extract");
            assert!(cfg.api.advertise_lan);
            Ok(())
        });
    }

    #[test]
    fn advertise_lan_is_off_without_the_environment() {
        assert!(!Config::default().api.advertise_lan);
    }
}
