//! Model-asset layout under the worker's `models_dir`.
//!
//! `scribe models pull` (design §4) populates this directory with the ONNX
//! assets the real engine needs. This module is the single source of truth for
//! the expected filenames and the presence check the [`SpeechEngine`] loader
//! uses to decide between the real and stub engines.
//!
//! Layout (relative to `models_dir`):
//! ```text
//! models/
//!   asr/                         # ASR model (Parakeet TDT transducer by default)
//!     encoder.onnx
//!     decoder.onnx
//!     joiner.onnx
//!     tokens.txt
//!   diarization/
//!     segmentation.onnx          # pyannote-segmentation-3.0
//!     embedding.onnx             # 3D-Speaker / WeSpeaker / NeMo (192-dim)
//! ```
//!
//! Whisper-style models (a single `*-encoder.onnx` / `*-decoder.onnx` pair plus
//! `tokens.txt`) are also supported by [`AsrModelPaths::discover`].

use std::path::{Path, PathBuf};

/// Resolved paths for the ASR model.
#[derive(Debug, Clone)]
pub enum AsrModelPaths {
    /// A transducer model (Parakeet, Zipformer): encoder + decoder + joiner.
    Transducer {
        encoder: PathBuf,
        decoder: PathBuf,
        joiner: PathBuf,
        tokens: PathBuf,
    },
    /// A Whisper model: encoder + decoder + tokens.
    Whisper {
        encoder: PathBuf,
        decoder: PathBuf,
        tokens: PathBuf,
    },
}

/// Resolved paths for the diarization models.
#[derive(Debug, Clone)]
pub struct DiarizationModelPaths {
    /// pyannote segmentation ONNX.
    pub segmentation: PathBuf,
    /// Speaker-embedding extractor ONNX (also reused for `embed_speaker`).
    pub embedding: PathBuf,
}

/// First existing path among `candidates`, else `None`.
fn first_existing(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|p| p.exists())
}

impl AsrModelPaths {
    /// Locate the ASR model files under `{models_dir}/asr`, preferring a
    /// transducer layout (the Parakeet default) and falling back to Whisper.
    pub fn discover(models_dir: &Path) -> Option<AsrModelPaths> {
        let dir = models_dir.join("asr");
        let tokens = dir.join("tokens.txt");
        if !tokens.exists() {
            return None;
        }

        // Transducer (Parakeet / Zipformer): encoder+decoder+joiner.
        let encoder_t = first_existing([dir.join("encoder.onnx"), dir.join("encoder.int8.onnx")]);
        let decoder_t = first_existing([dir.join("decoder.onnx"), dir.join("decoder.int8.onnx")]);
        let joiner_t = first_existing([dir.join("joiner.onnx"), dir.join("joiner.int8.onnx")]);
        if let (Some(encoder), Some(decoder), Some(joiner)) = (encoder_t, decoder_t, joiner_t) {
            return Some(AsrModelPaths::Transducer {
                encoder,
                decoder,
                joiner,
                tokens,
            });
        }

        // Whisper: *-encoder.onnx + *-decoder.onnx (single encoder/decoder pair).
        let encoder_w =
            first_existing([dir.join("encoder.onnx"), dir.join("whisper-encoder.onnx")]);
        let decoder_w =
            first_existing([dir.join("decoder.onnx"), dir.join("whisper-decoder.onnx")]);
        if let (Some(encoder), Some(decoder)) = (encoder_w, decoder_w) {
            return Some(AsrModelPaths::Whisper {
                encoder,
                decoder,
                tokens,
            });
        }

        None
    }
}

impl DiarizationModelPaths {
    /// Locate the diarization model files under `{models_dir}/diarization`.
    pub fn discover(models_dir: &Path) -> Option<DiarizationModelPaths> {
        let dir = models_dir.join("diarization");
        let segmentation = first_existing([
            dir.join("segmentation.onnx"),
            dir.join("segmentation.int8.onnx"),
        ])?;
        let embedding =
            first_existing([dir.join("embedding.onnx"), dir.join("embedding.int8.onnx")])?;
        Some(DiarizationModelPaths {
            segmentation,
            embedding,
        })
    }
}

/// The speaker-embedding model path (reused by both diarization and enrollment).
pub fn embedding_model_path(models_dir: &Path) -> Option<PathBuf> {
    DiarizationModelPaths::discover(models_dir).map(|d| d.embedding)
}

/// True when *all* assets the real engine needs are present: the ASR model and
/// (when diarization is enabled) the diarization models. The loader uses this to
/// decide whether to instantiate the real engine or fall back to the stub.
pub fn real_models_present(models_dir: &Path, diarization: bool) -> bool {
    if AsrModelPaths::discover(models_dir).is_none() {
        return false;
    }
    if diarization && DiarizationModelPaths::discover(models_dir).is_none() {
        return false;
    }
    true
}
