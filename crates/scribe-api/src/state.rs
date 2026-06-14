//! Shared application state, constructed once at startup and cloned into every
//! handler (it is cheap: the DB pool, embedder and Ollama client are all
//! `Arc`-backed).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use scribe_core::config::Config;
use scribe_core::{Error, Result};
use scribe_db::Db;
use scribe_llm::{build_embedder, Embedder, OllamaClient};

/// Everything a handler needs. Cloned per request; all heavy fields are shared
/// behind `Arc`/pool handles.
#[derive(Clone)]
pub struct AppState {
    /// Persistence layer (Postgres pool inside).
    pub db: Db,
    /// Query/RAG embedder — same model as the worker's index embedder.
    pub embedder: Arc<dyn Embedder>,
    /// Ollama client for RAG answers.
    pub ollama: OllamaClient,
    /// Root directory for audio blobs.
    pub blobs: PathBuf,
    /// Loaded device tokens (`device_id` → `key`) and the enforcement flag.
    pub auth: Arc<AuthState>,
    /// The full config, kept for handlers that need ancillary settings.
    pub cfg: Arc<Config>,
}

/// Resolved auth configuration: the set of valid device keys plus whether the
/// token is actually required (dev default: not required).
#[derive(Debug, Default)]
pub struct AuthState {
    /// If true, every non-exempt route demands a recognised bearer token.
    pub require_device_token: bool,
    /// Valid keys, by value, for O(1) lookup. The device id is the value.
    pub keys: HashMap<String, String>,
}

impl AuthState {
    /// Is this bearer token recognised?
    pub fn is_valid(&self, token: &str) -> bool {
        self.keys.contains_key(token)
    }
}

impl AppState {
    /// Build state from a [`Config`]: connect the DB, build the embedder, point
    /// the Ollama client, and load device keys. Does not bind a socket.
    pub async fn build(cfg: Config) -> Result<AppState> {
        let db = Db::connect(&cfg.database).await?;
        let embedder = build_embedder(&cfg.llm)?;
        let ollama = OllamaClient::from_config(&cfg.llm);
        let blobs = cfg.storage.blobs.clone();

        let keys = load_device_keys(cfg.auth.device_keys.as_deref())?;
        let auth = Arc::new(AuthState {
            require_device_token: cfg.auth.require_device_token,
            keys,
        });

        Ok(AppState {
            db,
            embedder,
            ollama,
            blobs,
            auth,
            cfg: Arc::new(cfg),
        })
    }
}

/// Load a TOML file of `device_id = "api_key"` pairs into a `key → device_id`
/// map (we look up by the secret key on each request). A missing file is
/// tolerated → no keys (design §12: "tolerate missing file → no keys").
fn load_device_keys(path: Option<&std::path::Path>) -> Result<HashMap<String, String>> {
    let Some(path) = path else {
        return Ok(HashMap::new());
    };
    if !path.exists() {
        tracing::warn!(path = %path.display(), "device_keys file not found; no device tokens loaded");
        return Ok(HashMap::new());
    }
    let text = std::fs::read_to_string(path).map_err(Error::Io)?;
    // The file is a flat table: device_id = "key".
    let table: HashMap<String, String> = toml::from_str(&text)
        .map_err(|e| Error::Config(format!("invalid device_keys file {}: {e}", path.display())))?;
    // Invert to key → device_id so the bearer token is the lookup key.
    let mut keys = HashMap::with_capacity(table.len());
    for (device_id, key) in table {
        keys.insert(key, device_id);
    }
    tracing::info!(count = keys.len(), "loaded device keys");
    Ok(keys)
}
