//! The shared, load-once compute context for the pipeline.
//!
//! The worker loads the speech engine, the text embedder, and the Ollama client
//! exactly once and shares them across every job via an `Arc<Engines>` (design
//! §7). `process_recording_inline` builds the same struct on demand so the
//! inline path (tests, `ingest --inline`) runs through identical stage code.

use std::sync::Arc;

use scribe_asr::SpeechEngine;
use scribe_core::config::Config;
use scribe_core::{Error, Result};
use scribe_llm::{build_embedder, Embedder, OllamaClient};
use tokio::sync::Mutex;

/// Everything a stage might need to run, loaded once.
pub struct Engines {
    /// ASR + diarization + speaker embedding (real sherpa-onnx or the stub).
    pub speech: SpeechEngine,
    /// Text embedder for retrieval chunks (fastembed or the hash stub).
    pub embedder: LazyEmbedder,
    /// Client for the local Ollama server (summaries).
    pub ollama: OllamaClient,
}

/// The text embedder, built on first use rather than at worker startup.
///
/// fastembed downloads its model from Hugging Face the first time it runs. When
/// that download fails — an offline boot, a blip, a rate limit — building it
/// eagerly took the whole worker down, and a worker that exits stops
/// transcribing too: every recording sits at `queued` with nothing in the log
/// but one line, forever. Nothing else in the pipeline behaves that way. A
/// missing ASR model falls back to the stub engine, and an unreachable LLM
/// leaves the summary empty.
///
/// So the embedder is built on demand and cached once it succeeds. A failure
/// fails only the `embed` job, which the queue then retries with backoff — by
/// which time the download usually works.
///
/// It deliberately does NOT fall back to the hash stub. That would write
/// plausible-looking vectors into `chunks.embedding` that no search could ever
/// match, and nothing downstream could tell them from real ones.
pub struct LazyEmbedder {
    cfg: scribe_core::config::LlmConfig,
    cached: Mutex<Option<Arc<dyn Embedder>>>,
}

impl LazyEmbedder {
    fn new(cfg: &Config) -> Self {
        Self { cfg: cfg.llm.clone(), cached: Mutex::new(None) }
    }

    /// The embedder, building it if this is the first call to succeed.
    pub async fn get(&self) -> Result<Arc<dyn Embedder>> {
        let mut slot = self.cached.lock().await;
        if let Some(e) = slot.as_ref() {
            return Ok(Arc::clone(e));
        }
        let built = build_embedder(&self.cfg).map_err(|e| {
            Error::Model(format!(
                "the text embedder is not ready ({e}). fastembed downloads `{}` \
from Hugging Face on first use, so this job will retry.",
                self.cfg.embed_model
            ))
        })?;
        *slot = Some(Arc::clone(&built));
        Ok(built)
    }
}

impl Engines {
    /// Load all three engines from config. Cheap when the stubs are selected
    /// (`--no-default-features`); on a real build it loads ONNX models from
    /// `cfg.worker.models_dir` (falling back to the stub if absent).
    pub fn load(cfg: &Config) -> Result<Arc<Engines>> {
        let speech = SpeechEngine::load(&cfg.worker.models_dir, &cfg.asr)?;
        let embedder = LazyEmbedder::new(cfg);
        let ollama = OllamaClient::from_config(&cfg.llm);
        tracing::info!(
            backend = speech.backend().as_str(),
            // The configured width, not the model's: the model is not loaded
            // yet, and this value is the one the schema commits to anyway.
            embed_dim = cfg.llm.embed_dim,
            "loaded pipeline engines"
        );
        Ok(Arc::new(Engines {
            speech,
            embedder,
            ollama,
        }))
    }
}
