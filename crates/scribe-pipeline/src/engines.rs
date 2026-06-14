//! The shared, load-once compute context for the pipeline.
//!
//! The worker loads the speech engine, the text embedder, and the Ollama client
//! exactly once and shares them across every job via an `Arc<Engines>` (design
//! §7). `process_recording_inline` builds the same struct on demand so the
//! inline path (tests, `ingest --inline`) runs through identical stage code.

use std::sync::Arc;

use scribe_asr::SpeechEngine;
use scribe_core::config::Config;
use scribe_core::Result;
use scribe_llm::{build_embedder, Embedder, OllamaClient};

/// Everything a stage might need to run, loaded once.
pub struct Engines {
    /// ASR + diarization + speaker embedding (real sherpa-onnx or the stub).
    pub speech: SpeechEngine,
    /// Text embedder for retrieval chunks (fastembed or the hash stub).
    pub embedder: Arc<dyn Embedder>,
    /// Client for the local Ollama server (summaries).
    pub ollama: OllamaClient,
}

impl Engines {
    /// Load all three engines from config. Cheap when the stubs are selected
    /// (`--no-default-features`); on a real build it loads ONNX models from
    /// `cfg.worker.models_dir` (falling back to the stub if absent).
    pub fn load(cfg: &Config) -> Result<Arc<Engines>> {
        let speech = SpeechEngine::load(&cfg.worker.models_dir, &cfg.asr)?;
        let embedder = build_embedder(&cfg.llm)?;
        let ollama = OllamaClient::from_config(&cfg.llm);
        tracing::info!(
            backend = speech.backend().as_str(),
            embed_dim = embedder.dim(),
            "loaded pipeline engines"
        );
        Ok(Arc::new(Engines {
            speech,
            embedder,
            ollama,
        }))
    }
}
