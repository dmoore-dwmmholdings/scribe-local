//! Speech-model data types and engine traits.
//!
//! These are the stable contract the `scribe-pipeline` crate depends on. They
//! deliberately mirror — but do not re-use — `scribe_core::types` so this crate
//! stays a pure speech-model layer with no storage coupling: the pipeline maps
//! these into the core/DB types (`Word`, `Utterance`, `RecordingSpeaker`, …).

use std::collections::HashMap;
use std::path::Path;

use scribe_core::Result;

/// Dimension of a speaker-embedding vector. The design's `speakers.embedding`
/// column is `vector(192)` (3D-Speaker / WeSpeaker / NeMo TitaNet family).
pub const EMBEDDING_DIM: usize = 192;

// --------------------------------------------------------------------------
// Data types
// --------------------------------------------------------------------------

/// A single recognized word with its timing and confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct AsrWord {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub conf: f32,
}

/// The full ASR result for one audio file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    /// Flat transcript text (whole file).
    pub text: String,
    /// Word-level timing, in file order.
    pub words: Vec<AsrWord>,
}

/// One contiguous stretch of speech attributed to a single diarized speaker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeakerTurn {
    /// `Speaker 0`, `Speaker 1`, … within this recording.
    pub local_idx: i32,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// The diarization result for one audio file.
#[derive(Debug, Clone, Default)]
pub struct Diarization {
    /// Speaker turns, sorted by start time.
    pub turns: Vec<SpeakerTurn>,
    /// Per-speaker mean embedding, keyed by `local_idx` (for enrollment matching).
    pub embeddings: HashMap<i32, Vec<f32>>,
    /// Number of distinct speakers found.
    pub num_speakers: usize,
}

// --------------------------------------------------------------------------
// Engine traits (object-safe; the pipeline holds `Box<dyn ...>`)
// --------------------------------------------------------------------------

/// Speech-to-text with word-level timestamps (design §8, WhisperX "approach A":
/// transcribe the whole file for full context, merge with diarization later).
pub trait Transcriber: Send + Sync {
    /// Transcribe a 16 kHz mono PCM WAV file.
    fn transcribe(&self, wav_path: &Path) -> Result<Transcript>;
}

/// Speaker diarization (Silero VAD → segmentation → embeddings → clustering).
pub trait Diarizer: Send + Sync {
    /// `expected_speakers` comes from the recording's `participants_expected`
    /// when known; passing it pins the clustering count for better accuracy.
    fn diarize(&self, wav_path: &Path, expected_speakers: Option<i32>) -> Result<Diarization>;
}

/// Single-sample speaker embedding (for `scribe enroll`).
pub trait SpeakerEmbedder: Send + Sync {
    /// Embed a single voice sample. Returns a [`dim`](Self::dim)-length vector.
    fn embed_speaker(&self, wav_path: &Path) -> Result<Vec<f32>>;
    /// Embedding dimension (192 for the diarization-default extractor).
    fn dim(&self) -> usize;
}
