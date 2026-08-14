//! Real ASR via `sherpa_onnx::OfflineRecognizer`.

use std::path::Path;

use scribe_core::{Error, Result};
use sherpa_onnx::{
    OfflineModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig,
    OfflineWhisperModelConfig,
};

use crate::models::AsrModelPaths;
use crate::types::{AsrWord, Transcriber, Transcript};
use crate::wav;

/// Whisper's offline encoder processes a fixed 30-second window — feeding it a
/// longer clip silently transcribes only the first 30s (and misaligns the
/// token/timestamp arrays, which zeroes word timings). We transcribe long audio
/// in windows safely under that limit and stitch the results together. 28s
/// leaves headroom so the encoder never truncates a window.
const WHISPER_WINDOW_MS: i64 = 28_000;

/// Wraps an `OfflineRecognizer` configured for the discovered ASR model.
pub struct SherpaTranscriber {
    recognizer: OfflineRecognizer,
    /// Transcribe long audio in <30s chunks (true for Whisper, whose offline
    /// encoder is limited to a 30s window). Transducer models decode arbitrary
    /// length in one pass, so this stays false for them.
    chunk_long_audio: bool,
}

impl SherpaTranscriber {
    /// Build a recognizer from the model files under `models_dir`.
    ///
    /// `device` is the `[asr].device` hint (`cpu`/`cuda`) → sherpa `provider`.
    pub fn load(
        paths: AsrModelPaths,
        device: &str,
        num_threads: i32,
        hotwords_file: Option<&str>,
        hotwords_score: f32,
    ) -> Result<Self> {
        let provider = provider_for(device);

        let mut model_config = OfflineModelConfig {
            num_threads,
            provider: Some(provider),
            debug: false,
            ..Default::default()
        };

        // Whisper needs windowed transcription for clips > 30s; transducers don't.
        let mut chunk_long_audio = false;

        match paths {
            AsrModelPaths::Transducer {
                encoder,
                decoder,
                joiner,
                tokens,
            } => {
                model_config.tokens = Some(path_str(&tokens)?);
                model_config.transducer = OfflineTransducerModelConfig {
                    encoder: Some(path_str(&encoder)?),
                    decoder: Some(path_str(&decoder)?),
                    joiner: Some(path_str(&joiner)?),
                };
            }
            AsrModelPaths::Whisper {
                encoder,
                decoder,
                tokens,
            } => {
                model_config.tokens = Some(path_str(&tokens)?);
                model_config.whisper = OfflineWhisperModelConfig {
                    encoder: Some(path_str(&encoder)?),
                    decoder: Some(path_str(&decoder)?),
                    // Word/token-level timing for the WhisperX-style merge (§8).
                    enable_token_timestamps: true,
                    ..Default::default()
                };
                chunk_long_audio = true;
            }
        }

        let mut config = OfflineRecognizerConfig {
            model_config,
            ..Default::default()
        };

        // Opt-in hotword biasing: only engage when a hotwords file is configured,
        // so the default decode path is untouched. Hotwords require
        // modified_beam_search on the transducer (greedy ignores them).
        if let Some(hw) = hotwords_file.filter(|p| !p.is_empty()) {
            config.decoding_method = Some("modified_beam_search".to_string());
            if config.max_active_paths <= 0 {
                config.max_active_paths = 4;
            }
            config.hotwords_file = Some(hw.to_string());
            config.hotwords_score = if hotwords_score > 0.0 { hotwords_score } else { 2.0 };
            tracing::info!(file = hw, score = config.hotwords_score, "ASR hotword biasing enabled");
        }

        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| Error::Model("failed to create OfflineRecognizer".into()))?;

        Ok(SherpaTranscriber {
            recognizer,
            chunk_long_audio,
        })
    }

    /// Decode one clip in a single pass, offsetting word timings by `offset_ms`
    /// so chunked transcription lands on the global recording timeline.
    fn decode_clip(&self, sample_rate: u32, samples: &[f32], offset_ms: i64) -> Result<Transcript> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(sample_rate as i32, samples);
        self.recognizer.decode(&stream);

        let result = stream
            .get_result()
            .ok_or_else(|| Error::Model("OfflineRecognizer produced no result".into()))?;

        let mut words = build_words(
            &result.text,
            &result.tokens,
            &result.timestamps,
            &result.durations,
        );
        if offset_ms != 0 {
            for w in &mut words {
                w.start_ms += offset_ms;
                w.end_ms += offset_ms;
            }
        }

        Ok(Transcript {
            text: result.text,
            words,
        })
    }
}

impl Transcriber for SherpaTranscriber {
    fn transcribe(&self, wav_path: &Path) -> Result<Transcript> {
        let audio = wav::read_wav(wav_path)?;
        let sr = audio.sample_rate;

        // Single-pass for transducers and for clips that fit one Whisper window.
        let window = ((WHISPER_WINDOW_MS * sr as i64) / 1000) as usize;
        if !self.chunk_long_audio || window == 0 || audio.samples.len() <= window {
            return self.decode_clip(sr, &audio.samples, 0);
        }

        // Whisper + long audio: transcribe in <30s windows, offsetting each
        // window's word timings onto the global timeline, then concatenate.
        // Without this, Whisper transcribes only the first 30s of the recording.
        let mut text = String::new();
        let mut words: Vec<AsrWord> = Vec::new();
        let mut start = 0usize;
        while start < audio.samples.len() {
            let end = (start + window).min(audio.samples.len());
            let offset_ms = (start as i64 * 1000) / sr as i64;
            let part = self.decode_clip(sr, &audio.samples[start..end], offset_ms)?;
            let piece = part.text.trim();
            if !piece.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(piece);
            }
            words.extend(part.words);
            start = end;
        }

        Ok(Transcript { text, words })
    }
}

/// Map the `[asr].device` hint to a sherpa-onnx execution provider.
fn provider_for(device: &str) -> String {
    match device.trim().to_ascii_lowercase().as_str() {
        "cuda" | "gpu" => "cuda".to_string(),
        "coreml" => "coreml".to_string(),
        _ => "cpu".to_string(),
    }
}

fn path_str(p: &Path) -> Result<String> {
    p.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::Model(format!("non-UTF-8 model path: {}", p.display())))
}

/// Reconstruct word-level [`AsrWord`]s from sherpa's per-token output.
///
/// sherpa emits `tokens` (subword/BPE pieces) with parallel `timestamps`
/// (seconds, start of each token) and optional `durations`. Transducer models
/// mark word boundaries by a leading space (`▁` / a real space) on the token
/// that starts a new word; we group tokens into words on that boundary. When
/// token-level timing is unavailable we fall back to whitespace-splitting the
/// flat text and spreading timings evenly.
fn build_words(
    text: &str,
    tokens: &[String],
    timestamps: &Option<Vec<f32>>,
    durations: &Option<Vec<f32>>,
) -> Vec<AsrWord> {
    // Fast path: no token timing → split text, no timings.
    let Some(ts) = timestamps.as_ref() else {
        return text
            .split_whitespace()
            .map(|w| AsrWord {
                text: w.to_string(),
                start_ms: 0,
                end_ms: 0,
                conf: 1.0,
            })
            .collect();
    };
    if tokens.is_empty() || tokens.len() != ts.len() {
        // Misaligned token/timestamp arrays — don't trust them.
        return text
            .split_whitespace()
            .map(|w| AsrWord {
                text: w.to_string(),
                start_ms: 0,
                end_ms: 0,
                conf: 1.0,
            })
            .collect();
    }

    let sec_to_ms = |s: f32| (s as f64 * 1000.0).round() as i64;

    let mut words: Vec<AsrWord> = Vec::new();
    let mut cur_text = String::new();
    let mut cur_start: Option<i64> = None;
    let mut cur_end: i64 = 0;

    let flush = |words: &mut Vec<AsrWord>, t: &mut String, start: &mut Option<i64>, end: i64| {
        let piece = t.trim();
        if !piece.is_empty() {
            words.push(AsrWord {
                text: piece.to_string(),
                start_ms: start.unwrap_or(0),
                end_ms: end.max(start.unwrap_or(0)),
                conf: 1.0,
            });
        }
        t.clear();
        *start = None;
    };

    for (i, tok) in tokens.iter().enumerate() {
        // A leading space marker denotes the start of a new word.
        let starts_word = tok.starts_with('▁') || tok.starts_with(' ');
        let clean = tok.trim_start_matches('▁').trim_start();

        let start_ms = sec_to_ms(ts[i]);
        let dur_ms = durations
            .as_ref()
            .and_then(|d| d.get(i))
            .map(|&d| sec_to_ms(d))
            .unwrap_or(0);
        let end_ms = start_ms + dur_ms;

        if starts_word && cur_start.is_some() {
            flush(&mut words, &mut cur_text, &mut cur_start, cur_end);
        }

        if cur_start.is_none() {
            cur_start = Some(start_ms);
        }
        cur_text.push_str(clean);
        cur_end = end_ms.max(cur_end);
    }
    flush(&mut words, &mut cur_text, &mut cur_start, cur_end);

    // If the boundary heuristic collapsed everything into one blob (e.g. a model
    // that doesn't use ▁ markers), fall back to whitespace splitting of the text
    // while keeping the overall start/end span.
    if words.len() <= 1 && text.split_whitespace().count() > 1 {
        let span_start = words.first().map(|w| w.start_ms).unwrap_or(0);
        let span_end = words.first().map(|w| w.end_ms).unwrap_or(0).max(span_start);
        let parts: Vec<&str> = text.split_whitespace().collect();
        let n = parts.len() as i64;
        let step = ((span_end - span_start).max(0)) / n.max(1);
        return parts
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let s = span_start + step * i as i64;
                AsrWord {
                    text: (*w).to_string(),
                    start_ms: s,
                    end_ms: s + step,
                    conf: 1.0,
                }
            })
            .collect();
    }

    words
}
