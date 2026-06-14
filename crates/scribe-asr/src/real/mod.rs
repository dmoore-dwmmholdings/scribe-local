//! Real speech engine built on the official `sherpa-onnx` crate (v1.13.x).
//!
//! Compiled only when the `onnx` feature is on. Wraps:
//! * [`OfflineRecognizer`](sherpa_onnx::OfflineRecognizer) for ASR with
//!   word-level timestamps (Parakeet transducer / Whisper),
//! * [`OfflineSpeakerDiarization`](sherpa_onnx::OfflineSpeakerDiarization) for
//!   the Silero-VAD → pyannote-segmentation → speaker-embedding → FastClustering
//!   pipeline (design §8),
//! * [`SpeakerEmbeddingExtractor`](sherpa_onnx::SpeakerEmbeddingExtractor) for
//!   `scribe enroll`.
//!
//! Model file paths are resolved from the worker's `models_dir`; see
//! [`crate::models`] for the expected filenames and presence checks.
//!
//! NOTE: the sherpa-onnx 1.13 Rust API surface was cross-checked against
//! docs.rs while writing this. Where a precise detail could not be confirmed
//! from the docs, the most plausible call is used and flagged with
//! `// TODO(sherpa-api): verify against docs.rs`.

mod asr;
mod diarize;
mod embed;

pub use asr::SherpaTranscriber;
pub use diarize::SherpaDiarizer;
pub use embed::SherpaEmbedder;
