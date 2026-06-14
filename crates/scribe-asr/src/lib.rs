//! `scribe-asr` — pure speech-model wrappers and data types (design §8).
//!
//! This crate is a thin, storage-free layer over the local speech models. It
//! has **no database and no HTTP**: the `scribe-pipeline` crate orchestrates
//! these wrappers and maps their outputs into `scribe_core` domain types.
//!
//! It provides three model abstractions behind object-safe traits —
//! [`Transcriber`] (ASR with word timestamps), [`Diarizer`] (speaker turns +
//! embeddings), and [`SpeakerEmbedder`] (single-sample enrollment) — plus an
//! aggregate [`SpeechEngine`] that loads all three from a `models_dir`.
//!
//! ## Two build paths
//!
//! * **`onnx` feature (default, on):** the real engine wraps the official
//!   [`sherpa-onnx`](https://docs.rs/sherpa-onnx) crate (Parakeet/Whisper ASR,
//!   Silero-VAD + pyannote-segmentation + speaker-embedding + FastClustering
//!   diarization). It needs the ONNX/CUDA native toolchain and model files.
//! * **`--no-default-features`:** a deterministic [`stub`] engine with no native
//!   dependencies, so the whole workspace builds and the pipeline runs
//!   end-to-end on machines without the ONNX toolchain or any models.
//!
//! Even with the `onnx` feature on, [`SpeechEngine::load`] falls back to the
//! stub when the model files are absent — so a default build with no models
//! still runs.

mod engine;
mod stub;
mod types;
mod wav;

pub mod models;

#[cfg(feature = "onnx")]
mod real;

pub use engine::{Backend, SpeechEngine};
pub use stub::StubEngine;
pub use types::{
    AsrWord, Diarization, Diarizer, SpeakerEmbedder, SpeakerTurn, Transcriber, Transcript,
    EMBEDDING_DIM,
};

// Re-export the WAV reader: the transcode stage and tests find it useful, and it
// is the canonical 16 kHz mono PCM reader these engines assume.
pub use wav::{read_wav, WavData};

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;
    use std::path::PathBuf;

    /// Synthesize a small valid 16 kHz mono PCM WAV (a quiet sine sweep) in the
    /// system temp dir and return its path. Caller is responsible for cleanup.
    fn write_test_wav(name: &str, secs: f32) -> PathBuf {
        let sample_rate = 16_000u32;
        let n = (sample_rate as f32 * secs) as usize;
        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / sample_rate as f32;
            // A gentle 220 Hz tone at low amplitude — valid audio, deterministic.
            let v = (2.0 * PI * 220.0 * t).sin() * 0.2;
            samples.push((v * i16::MAX as f32) as i16);
        }
        let path = std::env::temp_dir().join(name);
        wav::write_pcm16_mono(&path, sample_rate, &samples).expect("write test wav");
        path
    }

    #[test]
    fn wav_roundtrip_reports_duration() {
        let path = write_test_wav("scribe_asr_wav_roundtrip.wav", 2.0);
        let w = read_wav(&path).expect("read wav");
        assert_eq!(w.sample_rate, 16_000);
        assert_eq!(w.channels, 1);
        // ~2 seconds; allow a little slack for integer truncation.
        assert!(
            (1_900..=2_100).contains(&w.duration_ms()),
            "dur = {}",
            w.duration_ms()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stub_transcriber_produces_words() {
        let path = write_test_wav("scribe_asr_stub_transcribe.wav", 12.0);
        let engine = SpeechEngine::load_stub();
        assert_eq!(engine.backend(), Backend::Stub);

        let t = engine.transcriber().transcribe(&path).expect("transcribe");
        assert!(!t.text.is_empty(), "transcript text should be non-empty");
        assert!(!t.words.is_empty(), "transcript should have words");
        // Words are ordered and within the audio span.
        for w in &t.words {
            assert!(w.start_ms <= w.end_ms);
            assert_eq!(w.conf, 1.0);
            assert!(!w.text.is_empty());
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stub_diarizer_alternates_speakers() {
        let path = write_test_wav("scribe_asr_stub_diarize.wav", 35.0);
        let engine = SpeechEngine::load_stub();

        // No expected count → defaults to 2 speakers.
        let d = engine.diarizer().diarize(&path, None).expect("diarize");
        assert_eq!(d.num_speakers, 2);
        assert!(!d.turns.is_empty());
        assert_eq!(d.embeddings.len(), 2);
        // Two distinct unit embeddings.
        let e0 = &d.embeddings[&0];
        let e1 = &d.embeddings[&1];
        assert_eq!(e0.len(), EMBEDDING_DIM);
        assert_ne!(e0, e1);
        // Roughly unit length.
        let norm0: f32 = e0.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm0 - 1.0).abs() < 1e-3, "norm = {norm0}");
        // Turns alternate between the two speakers and stay in-bounds.
        for t in &d.turns {
            assert!(t.local_idx == 0 || t.local_idx == 1);
            assert!(t.start_ms <= t.end_ms);
        }

        // Expected count is respected and clamped to >= 1.
        let d3 = engine
            .diarizer()
            .diarize(&path, Some(3))
            .expect("diarize 3");
        assert_eq!(d3.num_speakers, 3);
        let d_clamp = engine
            .diarizer()
            .diarize(&path, Some(0))
            .expect("diarize 0");
        assert_eq!(d_clamp.num_speakers, 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stub_embed_speaker_is_deterministic_unit_vector() {
        let path = write_test_wav("scribe_asr_stub_embed.wav", 3.0);
        let engine = SpeechEngine::load_stub();
        let emb = engine.speaker_embedder();

        let a = emb.embed_speaker(&path).expect("embed");
        let b = emb.embed_speaker(&path).expect("embed again");
        assert_eq!(emb.dim(), EMBEDDING_DIM);
        assert_eq!(a.len(), EMBEDDING_DIM);
        assert_eq!(a, b, "same file must embed deterministically");
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "norm = {norm}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_falls_back_to_stub_without_models() {
        // A models dir that doesn't exist → stub fallback, never an error.
        let cfg = scribe_core::config::AsrConfig::default();
        let engine = SpeechEngine::load(&PathBuf::from("/nonexistent/scribe/models"), &cfg)
            .expect("load should not fail without models");
        assert_eq!(engine.backend(), Backend::Stub);
    }
}
