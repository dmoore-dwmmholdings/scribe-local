//! Deterministic, dependency-free stub speech engine.
//!
//! This is the engine used when the `onnx` feature is off, and the fallback the
//! [`SpeechEngine`](crate::engine::SpeechEngine) loader picks when the real ONNX
//! models are absent. It reads only the WAV duration (via [`crate::wav`]) and
//! synthesizes plausible, *deterministic* output so the whole pipeline — merge,
//! embed, summarize, enrollment matching — runs end-to-end on a machine with no
//! models and no ONNX/CUDA toolchain.
//!
//! Nothing here is real ASR; outputs are placeholders with sensible timings.

use std::collections::HashMap;
use std::path::Path;

use scribe_core::Result;

use crate::types::{
    AsrWord, Diarization, Diarizer, SpeakerEmbedder, SpeakerTurn, Transcriber, Transcript,
    EMBEDDING_DIM,
};
use crate::wav;

/// Number of placeholder words the stub emits per ~5 seconds of audio.
const WORDS_PER_5S: i64 = 4;
/// Stub diarization alternates speakers in turns of this length.
const TURN_MS: i64 = 10_000;

/// The deterministic stub engine. Implements all three speech traits.
#[derive(Debug, Default, Clone)]
pub struct StubEngine;

impl StubEngine {
    pub fn new() -> Self {
        StubEngine
    }

    /// Duration of `wav_path` in ms; 0 if it can't be read (kept non-fatal so a
    /// malformed fixture still produces *some* output rather than erroring out).
    fn duration_ms(wav_path: &Path) -> i64 {
        match wav::read_wav(wav_path) {
            Ok(w) => w.duration_ms().max(0),
            Err(e) => {
                tracing::warn!(error = %e, path = %wav_path.display(), "stub: could not read WAV; assuming 0ms");
                0
            }
        }
    }
}

impl Transcriber for StubEngine {
    fn transcribe(&self, wav_path: &Path) -> Result<Transcript> {
        let dur = Self::duration_ms(wav_path).max(1_000);

        // Spread a handful of placeholder words evenly across the duration.
        let n = ((dur * WORDS_PER_5S) / 5_000).clamp(3, 64) as usize;
        let step = dur / n as i64;

        let placeholders = ["[stub", "transcription]", "scribe", "asr"];
        let mut words = Vec::with_capacity(n);
        let mut text = String::new();
        for i in 0..n {
            let start = i as i64 * step;
            let end = (start + step.saturating_sub(1)).min(dur);
            let tok = placeholders[i % placeholders.len()];
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(tok);
            words.push(AsrWord {
                text: tok.to_string(),
                start_ms: start,
                end_ms: end.max(start),
                conf: 1.0,
            });
        }

        Ok(Transcript { text, words })
    }
}

impl Diarizer for StubEngine {
    fn diarize(&self, wav_path: &Path, expected_speakers: Option<i32>) -> Result<Diarization> {
        let dur = Self::duration_ms(wav_path).max(1_000);

        // Respect the caller's expected count when given; otherwise default to 2.
        let num_speakers = expected_speakers.unwrap_or(2).max(1) as usize;

        // Alternate speakers in ~TURN_MS turns across the whole duration.
        let mut turns = Vec::new();
        let mut start = 0i64;
        let mut idx = 0i64;
        while start < dur {
            let end = (start + TURN_MS).min(dur);
            turns.push(SpeakerTurn {
                local_idx: (idx % num_speakers as i64) as i32,
                start_ms: start,
                end_ms: end,
            });
            start = end;
            idx += 1;
        }
        if turns.is_empty() {
            turns.push(SpeakerTurn {
                local_idx: 0,
                start_ms: 0,
                end_ms: dur,
            });
        }

        // One distinct deterministic unit vector per speaker.
        let mut embeddings = HashMap::new();
        for s in 0..num_speakers as i32 {
            embeddings.insert(s, deterministic_embedding(&[0x5Eu8, 0xED, s as u8]));
        }

        Ok(Diarization {
            turns,
            embeddings,
            num_speakers,
        })
    }
}

impl SpeakerEmbedder for StubEngine {
    fn embed_speaker(&self, wav_path: &Path) -> Result<Vec<f32>> {
        // Derive a deterministic embedding from the file bytes so the same voice
        // sample always enrolls to the same vector.
        let bytes = std::fs::read(wav_path)?;
        Ok(deterministic_embedding(&bytes))
    }

    fn dim(&self) -> usize {
        EMBEDDING_DIM
    }
}

/// A deterministic, L2-normalized `EMBEDDING_DIM`-vector derived from `seed`.
///
/// Uses a small splitmix64-style PRNG so the output is stable across runs and
/// platforms (no `HashMap`/`RandomState` nondeterminism), and guaranteed
/// non-zero so normalization is well-defined.
fn deterministic_embedding(seed: &[u8]) -> Vec<f32> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for &b in seed {
        state = state.wrapping_add(b as u64).wrapping_mul(0x1000_0000_01B3);
        state ^= state >> 27;
    }
    // Ensure non-trivial state even for an empty seed.
    state ^= 0xD1B5_4A32_D192_ED03;

    let mut v = Vec::with_capacity(EMBEDDING_DIM);
    for _ in 0..EMBEDDING_DIM {
        // splitmix64 step.
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // Map to [-1, 1).
        let f = (z as f64 / u64::MAX as f64) as f32 * 2.0 - 1.0;
        v.push(f);
    }
    l2_normalize(&mut v);
    v
}

/// Normalize a vector to unit L2 length (falls back to a fixed unit axis if the
/// vector is all-zero, which the PRNG above won't produce but is cheap to guard).
fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    } else if let Some(first) = v.first_mut() {
        *first = 1.0;
    }
}
