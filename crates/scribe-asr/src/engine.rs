//! The aggregate [`SpeechEngine`]: loads and owns the three speech models.
//!
//! Selection logic (design §8, and the crate's stub-fallback requirement):
//! * With the `onnx` feature **on** *and* the required model files present in
//!   `models_dir`, load the real sherpa-onnx engine.
//! * Otherwise (feature off, or models missing), log a warning and use the
//!   deterministic [`StubEngine`] so the workspace still builds and the pipeline
//!   runs end-to-end without models.

use std::path::Path;
use std::sync::Arc;

use scribe_core::config::AsrConfig;
use scribe_core::Result;

use crate::stub::StubEngine;
use crate::types::{Diarizer, SpeakerEmbedder, Transcriber};

/// Default thread count for the ONNX runtime when running on CPU.
#[cfg(feature = "onnx")]
const DEFAULT_NUM_THREADS: i32 = 2;

/// Holds the three loaded speech models behind trait objects.
///
/// The concrete types differ between the real and stub builds, but the public
/// accessors only ever hand out `&dyn Trait`, so the pipeline is agnostic.
pub struct SpeechEngine {
    transcriber: Arc<dyn Transcriber>,
    diarizer: Arc<dyn Diarizer>,
    embedder: Arc<dyn SpeakerEmbedder>,
    /// Which engine actually backs this instance (for logging/diagnostics).
    backend: Backend,
}

/// Which implementation backs a [`SpeechEngine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Real sherpa-onnx models.
    Onnx,
    /// Deterministic stub (no models / no native deps).
    Stub,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Onnx => "onnx",
            Backend::Stub => "stub",
        }
    }
}

impl SpeechEngine {
    /// Load the speech engine from `models_dir` honouring `cfg`.
    ///
    /// Never fails just because models are missing: it falls back to the stub.
    /// It only returns `Err` if real models are present but fail to initialise.
    pub fn load(models_dir: &Path, cfg: &AsrConfig) -> Result<SpeechEngine> {
        #[cfg(feature = "onnx")]
        {
            if crate::models::real_models_present(models_dir, &cfg.model, cfg.diarization) {
                match Self::load_real(models_dir, cfg) {
                    Ok(engine) => {
                        tracing::info!(
                            backend = "onnx",
                            models_dir = %models_dir.display(),
                            model = %cfg.model,
                            "loaded real speech engine"
                        );
                        return Ok(engine);
                    }
                    Err(e) => {
                        // Real models were present but failed to load. Surface the
                        // error rather than silently degrading — the operator
                        // intended to use real models.
                        tracing::error!(error = %e, "failed to load real speech engine");
                        return Err(e);
                    }
                }
            }
            tracing::warn!(
                models_dir = %models_dir.display(),
                "ONNX feature enabled but required model files are missing; using stub speech engine"
            );
        }

        #[cfg(not(feature = "onnx"))]
        {
            let _ = (models_dir, cfg);
            tracing::warn!("built without the `onnx` feature; using stub speech engine");
        }

        Ok(Self::load_stub())
    }

    /// Construct the deterministic stub-backed engine.
    pub fn load_stub() -> SpeechEngine {
        let stub = Arc::new(StubEngine::new());
        SpeechEngine {
            transcriber: stub.clone(),
            diarizer: stub.clone(),
            embedder: stub,
            backend: Backend::Stub,
        }
    }

    #[cfg(feature = "onnx")]
    fn load_real(models_dir: &Path, cfg: &AsrConfig) -> Result<SpeechEngine> {
        use crate::models::{AsrModelPaths, DiarizationModelPaths};
        use crate::real::{SherpaDiarizer, SherpaEmbedder, SherpaTranscriber};
        use scribe_core::Error;

        // `[asr].model` names a subdirectory of `{models_dir}/asr` when one
        // exists, so Whisper and Parakeet can be installed side by side; the
        // layout itself is detected from the files, not from the name.
        let asr_paths = AsrModelPaths::discover_for(models_dir, &cfg.model).ok_or_else(|| {
            Error::Model(format!(
                "ASR model files for `{}` not found under {}/asr",
                cfg.model,
                models_dir.display()
            ))
        })?;
        tracing::info!(
            model = %cfg.model,
            layout = asr_paths.layout_name(),
            dir = %asr_paths.dir().display(),
            "resolved ASR model"
        );
        let transcriber: Arc<dyn Transcriber> = Arc::new(SherpaTranscriber::load(
            asr_paths,
            &cfg.device,
            DEFAULT_NUM_THREADS,
            cfg.hotwords_file.as_deref(),
            cfg.hotwords_score,
        )?);

        let diar_paths = DiarizationModelPaths::discover(models_dir).ok_or_else(|| {
            Error::Model("diarization model files not found in models_dir".into())
        })?;
        let diarizer: Arc<dyn Diarizer> = Arc::new(SherpaDiarizer::load(
            diar_paths.clone(),
            &cfg.device,
            DEFAULT_NUM_THREADS,
        )?);

        let embedder: Arc<dyn SpeakerEmbedder> = Arc::new(SherpaEmbedder::load(
            &diar_paths.embedding,
            &cfg.device,
            DEFAULT_NUM_THREADS,
        )?);

        Ok(SpeechEngine {
            transcriber,
            diarizer,
            embedder,
            backend: Backend::Onnx,
        })
    }

    /// Which backend is in use (`onnx` or `stub`).
    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn transcriber(&self) -> &dyn Transcriber {
        self.transcriber.as_ref()
    }

    pub fn diarizer(&self) -> &dyn Diarizer {
        self.diarizer.as_ref()
    }

    pub fn speaker_embedder(&self) -> &dyn SpeakerEmbedder {
        self.embedder.as_ref()
    }

    /// An owned handle to the transcriber, for running it on a blocking thread.
    ///
    /// Transcription and diarization are long CPU-bound native calls. Run on a
    /// runtime worker thread they hold it for minutes, and tokio's timers stop
    /// firing — which silently killed the job heartbeat, so the reaper treated
    /// a working stage as an abandoned one and took its job back mid-run. The
    /// callers hand these to `spawn_blocking`, which needs an owned value.
    pub fn transcriber_handle(&self) -> Arc<dyn Transcriber> {
        self.transcriber.clone()
    }

    /// An owned handle to the diarizer. See [`Self::transcriber_handle`].
    pub fn diarizer_handle(&self) -> Arc<dyn Diarizer> {
        self.diarizer.clone()
    }
}
