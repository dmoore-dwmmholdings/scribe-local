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

/// Longest clip handed to a transducer in one `accept_waveform` call.
///
/// This was 150 s, chosen as the largest value that stays clear of the hard
/// limit below. That is a bound on what the model *tolerates*, and it turns out
/// to be nowhere near what it is *good at*. On eleven minutes of degraded
/// four-voice conversation the word error rate is 19.1% at 150 s and 5.1% at
/// 30 s; on the same audio clean, both are under 1%. Long-form transducer
/// decoding does not fail loudly when it goes wrong — it silently omits a
/// stretch and carries on, and on one recording it dropped fifty-four seconds
/// and ten consecutive turns out of the middle of a transcript while still
/// producing words either side of the gap.
///
/// 30 s sits in the middle of the range that measured well (20–45 s). Shorter
/// windows also decode slightly *faster*, since the encoder's self-attention
/// grows with the square of the sequence length.
///
/// ## The hard limit this still has to respect
///
/// Transducers decode arbitrary length *algorithmically*, but the exported ONNX
/// graph does not: Parakeet TDT 0.6b v3 carries a fixed-size self-attention mask
/// of 2500 encoder frames. At 8× subsampling of 10 ms frames (80 ms per encoder
/// frame) that is exactly 200 seconds of audio. Exceed it and the encoder aborts
/// the process mid-run — ONNX Runtime raises a C++ exception that unwinds into
/// Rust, which cannot catch foreign exceptions:
///
/// ```text
/// '/layers.0/self_attn/Add_2': right operand cannot broadcast on dim 3
/// LeftShape: {1,8,7500,7500}, RightShape: {1,8,7500,2500}
/// fatal runtime error: Rust cannot catch foreign exceptions, aborting
/// ```
const TRANSDUCER_WINDOW_MS: i64 = 30_000;

/// A decode is trusted when its word timings reach this fraction of the clip.
/// Real speech ends before the audio does — a pause, a closing silence — so this
/// is deliberately forgiving; it is aimed at a decode that stopped less than
/// halfway, not at the last second of a recording.
const MIN_COVERAGE: f64 = 0.75;

/// Never re-decode a remainder shorter than this. Below a few seconds the tail
/// is almost certainly trailing silence, and another pass buys nothing.
const MIN_RECOVER_MS: i64 = 4_000;

/// Bound on extra passes, so a model that emits one word per attempt cannot
/// turn one recording into an unbounded decode loop.
const MAX_RECOVERY_PASSES: usize = 6;

/// A stretch with no words this long is decoded again on its own. Against a
/// professional transcript of a real meeting, on the full-precision Parakeet:
/// word error rate 16.5% without, 15.3% at 1.0 s, 15.1% at 1.5 s, 15.2% at
/// 2.5 s — a plateau, and this sits in it. Deletions fell from 1,536 to 750.
const GAP_RECOVERY_MS: i64 = 1_500;

/// How far back from a window's nominal end to look for a quiet moment to cut at.
///
/// A quarter of the window: far enough to reach a pause in ordinary speech,
/// near enough that windows stay close to the size asked for.
const CUT_SEARCH_FRACTION: usize = 4;

/// Frame size for the quiet-point search.
const CUT_FRAME: usize = 320; // 20 ms at 16 kHz

/// Choose where to end a decode window: the quietest moment near its nominal end.
///
/// Windows used to end at a fixed offset, which lands mid-word as often as not —
/// and a word split across two decodes is mangled in both. That shows up as
/// word error rate bouncing around with the window size for no principled
/// reason: 15 s beat 30 s on one recording and lost to it on another, which is
/// not a fact about window sizes but about where their boundaries happened to
/// fall.
///
/// Cutting at the quietest point in the last stretch of the window puts the
/// boundary in a pause instead. There is no threshold to tune and no failure
/// case: some moment is always the quietest, and in speech the quietest moment
/// of a several-second span is a gap between words.
fn choose_window_end(samples: &[f32], start: usize, nominal_end: usize) -> usize {
    if nominal_end >= samples.len() {
        return samples.len();
    }
    let span = nominal_end - start;
    let band = span / CUT_SEARCH_FRACTION;
    if band < CUT_FRAME * 2 {
        return nominal_end;
    }

    let from = nominal_end - band;
    let mut best = nominal_end;
    let mut best_energy = f32::MAX;
    let mut at = from;
    while at + CUT_FRAME <= nominal_end {
        let energy: f32 = samples[at..at + CUT_FRAME].iter().map(|x| x * x).sum();
        if energy < best_energy {
            best_energy = energy;
            best = at + CUT_FRAME / 2;
        }
        at += CUT_FRAME;
    }
    best
}

fn samples_to_ms(samples: usize, sample_rate: u32) -> i64 {
    if sample_rate == 0 {
        return 0;
    }
    (samples as i64 * 1000) / sample_rate as i64
}

fn ms_to_samples(ms: i64, sample_rate: u32) -> usize {
    if ms <= 0 {
        return 0;
    }
    ((ms * sample_rate as i64) / 1000) as usize
}

/// Whether a decode that consumed up to `next` of `total` samples left enough
/// audio uncovered to be worth decoding again.
///
/// Split out from the decode loop so the rule can be tested without a model.
fn needs_recovery(cursor: usize, next: usize, total: usize, sample_rate: u32) -> bool {
    // The cursor must advance, or the next pass would decode the same samples.
    if next <= cursor || next >= total {
        return false;
    }
    let span = total - cursor;
    let remaining = total - next;
    if samples_to_ms(remaining, sample_rate) < MIN_RECOVER_MS {
        return false;
    }
    let covered = (next - cursor) as f64 / span as f64;
    covered < MIN_COVERAGE
}

/// Wraps an `OfflineRecognizer` configured for the discovered ASR model.
pub struct SherpaTranscriber {
    recognizer: OfflineRecognizer,
    /// Longest clip fed to sherpa in one call. Whisper's is a hard model
    /// constraint (its encoder only sees 30s); a transducer's is a much larger
    /// guard against sherpa's native buffering blowing up on multi-hour audio.
    max_clip_ms: i64,
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
        bpe_vocab: Option<&Path>,
    ) -> Result<Self> {
        let provider = provider_for(device);

        let mut model_config = OfflineModelConfig {
            num_threads,
            provider: Some(provider),
            debug: false,
            ..Default::default()
        };

        // Every layout is windowed; only the window size differs.
        let mut max_clip_ms = TRANSDUCER_WINDOW_MS;

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
                max_clip_ms = WHISPER_WINDOW_MS;
            }
        }

        let mut config = OfflineRecognizerConfig {
            model_config,
            ..Default::default()
        };

        // SCRIBE_ASR_DECODING=modified_beam_search (with SCRIBE_ASR_BEAM paths,
        // default 4) instead of greedy search. Experiment hook.
        if let Ok(method) = std::env::var("SCRIBE_ASR_DECODING") {
            config.decoding_method = Some(method);
            config.max_active_paths = std::env::var("SCRIBE_ASR_BEAM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4);
        }

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

            // Hotwords are matched against the tokens the model predicts, and on
            // a sub-word model that means they have to be tokenized the same
            // way it was trained. Without the vocabulary to do that they are
            // accepted and silently ignored — measured, biasing toward a dozen
            // names in a recording full of them changed the word error rate by
            // nothing, at any boost, while the log said biasing was enabled.
            match bpe_vocab {
                Some(vocab) => {
                    config.model_config.modeling_unit = Some("bpe".to_string());
                    config.model_config.bpe_vocab = Some(path_str(vocab)?);
                    tracing::info!(
                        file = hw,
                        score = config.hotwords_score,
                        vocab = %vocab.display(),
                        "ASR hotword biasing enabled"
                    );
                }
                None => tracing::warn!(
                    file = hw,
                    "hotwords are configured but will have NO effect: [asr].bpe_vocab is unset, \
                     so they cannot be turned into model tokens. The checkpoint `scribe models \
                     pull` installs does not publish that file."
                ),
            }
        }

        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| Error::Model("failed to create OfflineRecognizer".into()))?;

        Ok(SherpaTranscriber {
            recognizer,
            max_clip_ms,
        })
    }

    /// Decode a span, and decode again from where the words stopped if the
    /// result does not reach the end of the audio.
    ///
    /// A transducer is supposed to decode a whole clip in one pass, and usually
    /// does. But on some recordings sherpa stops emitting partway and returns a
    /// short result with no error: a 65-second recording came back with words
    /// only up to 29.2 s, while the same audio from 29 s onward transcribed
    /// fine on its own. Nothing downstream could tell that from a person who
    /// simply stopped talking, so the rest of the meeting was silently missing
    /// from the transcript.
    ///
    /// So: measure how far the emitted word timings actually reach, and if they
    /// fall well short, decode the uncovered remainder and append it. Healthy
    /// audio covers its clip and returns after the first pass, keeping the
    /// full-file context that makes the text and punctuation better.
    fn decode_span(&self, sample_rate: u32, samples: &[f32], offset_ms: i64) -> Result<Transcript> {
        let mut text = String::new();
        let mut words: Vec<AsrWord> = Vec::new();
        let mut cursor = 0usize; // samples consumed within this span

        for _ in 0..=MAX_RECOVERY_PASSES {
            let part = self.decode_clip(
                sample_rate,
                &samples[cursor..],
                offset_ms + samples_to_ms(cursor, sample_rate),
            )?;
            let piece = part.text.trim();
            if !piece.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(piece);
            }
            let last_end_ms = part.words.last().map(|w| w.end_ms);
            words.extend(part.words);

            // Nothing came back: there is no more speech to recover, and
            // re-decoding the same samples would not terminate.
            let Some(last_end_ms) = last_end_ms else { break };

            let next = ms_to_samples(last_end_ms - offset_ms, sample_rate);
            if !needs_recovery(cursor, next, samples.len(), sample_rate) {
                break;
            }
            tracing::warn!(
                covered_ms = last_end_ms - offset_ms,
                span_ms = samples_to_ms(samples.len(), sample_rate),
                "ASR stopped short of the clip; decoding the remainder"
            );
            cursor = next;
        }

        Ok(Transcript { text, words })
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

        // Clip length, so the no-timestamp fallback can spread words across it.
        let clip_ms = if sample_rate > 0 {
            (samples.len() as i64 * 1000) / sample_rate as i64
        } else {
            0
        };
        let mut words = build_words(
            &result.text,
            &result.tokens,
            &result.timestamps,
            &result.durations,
            clip_ms,
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

        // Single pass whenever the clip already fits the model's window.
        // SCRIBE_ASR_WINDOW_MS overrides the window, which is how its value was
        // chosen; see docs/measuring-diarization.md.
        let max_clip_ms = std::env::var("SCRIBE_ASR_WINDOW_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(self.max_clip_ms);
        let window = ((max_clip_ms * sr as i64) / 1000) as usize;
        if window == 0 || audio.samples.len() <= window {
            let first = self.decode_span(sr, &audio.samples, 0)?;
            return self.recover_gaps(sr, &audio.samples, first);
        }

        // Long audio: decode a window at a time, offsetting each window's word
        // timings onto the global timeline, then concatenate. Whisper needs this
        // for correctness (it would otherwise transcribe only the first 30s);
        // transducers need it so sherpa doesn't abort on a multi-hour buffer.
        // A window boundary can split a word, which is the accepted cost.
        let mut text = String::new();
        let mut words: Vec<AsrWord> = Vec::new();
        let mut start = 0usize;
        while start < audio.samples.len() {
            let nominal = (start + window).min(audio.samples.len());
            let end = choose_window_end(&audio.samples, start, nominal);
            let offset_ms = (start as i64 * 1000) / sr as i64;
            let part = self.decode_span(sr, &audio.samples[start..end], offset_ms)?;
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

        self.recover_gaps(sr, &audio.samples, Transcript { text, words })
    }
}

impl SherpaTranscriber {
    /// Decode, on its own, every stretch where the transcript has no words for
    /// longer than `GAP_RECOVERY_MS` (`SCRIBE_ASR_GAP_MS` overrides; 0 is off).
    ///
    /// The recovery in `decode_span` only notices a decode that stops early. A
    /// transducer can also go quiet in the middle of a window and pick up again
    /// later, and then the words either side of the hole look healthy. Against
    /// a professional transcript of a real meeting, a hundred such holes of ten
    /// to twenty seconds held over two thousand words. A gap that is really
    /// silence decodes to nothing, so this costs little when nothing is missing.
    fn recover_gaps(&self, sr: u32, samples: &[f32], first: Transcript) -> Result<Transcript> {
        let gap_ms: i64 = std::env::var("SCRIBE_ASR_GAP_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(GAP_RECOVERY_MS);
        if gap_ms <= 0 {
            return Ok(first);
        }
        let total_ms = samples_to_ms(samples.len(), sr);
        let mut bounds: Vec<(i64, i64)> = Vec::new();
        let mut prev_end = 0i64;
        for w in &first.words {
            if w.start_ms - prev_end >= gap_ms {
                bounds.push((prev_end, w.start_ms));
            }
            prev_end = prev_end.max(w.end_ms);
        }
        if total_ms - prev_end >= gap_ms {
            bounds.push((prev_end, total_ms));
        }

        let mut words = first.words;
        let mut recovered = 0usize;
        for (from_ms, to_ms) in bounds {
            let a = ms_to_samples(from_ms, sr).min(samples.len());
            let b = ms_to_samples(to_ms, sr).min(samples.len());
            if b <= a {
                continue;
            }
            let part = self.decode_span(sr, &samples[a..b], from_ms)?;
            recovered += part.words.len();
            words.extend(part.words);
        }
        if recovered > 0 {
            words.sort_by_key(|w| (w.start_ms, w.end_ms));
            tracing::info!(recovered, "ASR: recovered words from gaps in the transcript");
        }
        let text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
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
/// flat text and spreading timings evenly over `clip_ms`.
fn build_words(
    text: &str,
    tokens: &[String],
    timestamps: &Option<Vec<f32>>,
    durations: &Option<Vec<f32>>,
    clip_ms: i64,
) -> Vec<AsrWord> {
    let trace = std::env::var("SCRIBE_ASR_TIMING_PATH").is_ok();
    // Fast path: no token timing → split text and spread evenly.
    let Some(ts) = timestamps.as_ref() else {
        if trace {
            eprintln!("   build_words: NO timestamps -> spread evenly ({} tokens)", tokens.len());
        }
        return spread_evenly(text, clip_ms);
    };
    if tokens.is_empty() || tokens.len() != ts.len() {
        if trace {
            eprintln!(
                "   build_words: {} tokens vs {} timestamps -> spread evenly",
                tokens.len(),
                ts.len()
            );
        }
        return spread_evenly(text, clip_ms);
    }
    if trace {
        eprintln!("   build_words: real token timings ({} tokens)", tokens.len());
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

/// Whitespace-split `text` and spread the words evenly over `clip_ms`.
///
/// Used when the model gives no usable token timing — notably sherpa's Whisper
/// models, which return tokens without parallel timestamps. Zeroing the timings
/// instead would strand every word at `[0, 0]`, which makes the merge stage find
/// no overlap with the diarization turns (so no utterance ever gets a speaker)
/// and collapses the whole recording into a single unseekable block. Evenly
/// spread timings are approximate but keep speaker assignment and playback
/// seeking working.
fn spread_evenly(text: &str, clip_ms: i64) -> Vec<AsrWord> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let n = parts.len() as i64;
    let step = if n > 0 { clip_ms.max(0) / n } else { 0 };
    parts
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let s = step * i as i64;
            AsrWord {
                text: (*w).to_string(),
                start_ms: s,
                // Last word absorbs any rounding remainder up to the clip end.
                end_ms: if i as i64 == n - 1 { clip_ms.max(s) } else { s + step },
                conf: 1.0,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// sherpa's Whisper models return tokens with no parallel timestamps. The
    /// fallback must still produce advancing word timings spanning the clip:
    /// zeroed timings strand every word at [0, 0], which leaves the merge stage
    /// with no overlap against the diarization turns and collapses the whole
    /// recording into one unseekable block.
    #[test]
    fn missing_timestamps_spread_words_across_the_clip() {
        let words = build_words("one two three four", &[], &None, &None, 4_000);

        assert_eq!(words.len(), 4);
        assert_eq!(words[0].start_ms, 0);
        // Strictly advancing — never all-zero.
        for pair in words.windows(2) {
            assert!(
                pair[1].start_ms > pair[0].start_ms,
                "word timings must advance: {:?}",
                words.iter().map(|w| w.start_ms).collect::<Vec<_>>()
            );
        }
        // The last word runs to the end of the clip.
        assert_eq!(words.last().unwrap().end_ms, 4_000);
    }

    /// Token/timestamp arrays of differing lengths are untrustworthy, so that
    /// path takes the same evenly-spread fallback rather than trusting them.
    #[test]
    fn misaligned_timestamps_fall_back_to_spreading() {
        let tokens = vec!["▁one".to_string(), "▁two".to_string()];
        let ts = Some(vec![0.0f32]); // deliberately shorter than `tokens`
        let words = build_words("one two", &tokens, &ts, &None, 2_000);

        assert_eq!(words.len(), 2);
        assert_eq!(words[0].start_ms, 0);
        assert_eq!(words[1].start_ms, 1_000);
        assert_eq!(words[1].end_ms, 2_000);
    }

    /// A zero-length clip must not panic or produce negative timings.
    #[test]
    fn a_window_ends_at_the_quiet_moment_near_its_target() {
        // Two seconds of tone, 300 ms of silence, two more seconds of tone.
        let sr = 16_000usize;
        let mut samples: Vec<f32> = (0..sr * 2).map(|i| (i as f32 / 40.0).sin() * 0.3).collect();
        let gap = samples.len();
        samples.extend(std::iter::repeat(0.0).take(sr * 300 / 1000));
        let gap_end = samples.len();
        samples.extend((0..sr * 2).map(|i| (i as f32 / 40.0).sin() * 0.3));

        // A nominal end just past the silence: the cut should fall inside it,
        // not at the nominal point in the middle of the second tone.
        let nominal = gap_end + sr / 2;
        let end = choose_window_end(&samples, 0, nominal);
        assert!(
            (gap..=gap_end).contains(&end),
            "cut at {end}, silence is {gap}..{gap_end}"
        );
    }

    #[test]
    fn the_last_window_ends_at_the_end_of_the_audio() {
        let samples = vec![0.1f32; 16_000];
        assert_eq!(choose_window_end(&samples, 0, 32_000), samples.len());
    }

    #[test]
    fn a_window_too_short_to_search_keeps_its_nominal_end() {
        let samples = vec![0.1f32; 16_000];
        // A band of a quarter of 400 samples is under two frames.
        assert_eq!(choose_window_end(&samples, 0, 400), 400);
    }

    #[test]
    fn zero_length_clip_is_safe() {
        let words = build_words("hello world", &[], &None, &None, 0);
        assert_eq!(words.len(), 2);
        assert!(words.iter().all(|w| w.start_ms == 0 && w.end_ms == 0));
    }

    // ---- coverage recovery ------------------------------------------------
    // 16 kHz throughout: 16 samples per millisecond.
    const SR: u32 = 16_000;
    fn secs(n: i64) -> usize {
        ms_to_samples(n * 1000, SR)
    }

    #[test]
    fn recovers_when_the_decode_stops_less_than_halfway() {
        // The reported case: 65 s of audio, words only to 29.2 s.
        assert!(needs_recovery(0, ms_to_samples(29_200, SR), secs(65), SR));
    }

    #[test]
    fn leaves_a_healthy_decode_alone() {
        // Words to 65.36 s of a 65.4 s clip: a normal closing silence.
        assert!(!needs_recovery(0, ms_to_samples(65_360, SR), secs(65) + 6400, SR));
    }

    #[test]
    fn ignores_a_short_tail() {
        // 3 s uncovered is below MIN_RECOVER_MS even though coverage is low.
        assert!(!needs_recovery(0, secs(4), secs(7), SR));
    }

    #[test]
    fn refuses_to_loop_when_the_cursor_cannot_advance() {
        assert!(!needs_recovery(secs(10), secs(10), secs(65), SR));
        assert!(!needs_recovery(secs(10), secs(5), secs(65), SR));
    }

    #[test]
    fn a_second_pass_is_measured_against_the_remaining_span() {
        // Already at 30 s of 65 s; the next pass reached 40 s, so 25 s of the
        // 35 s remainder is still uncovered and another pass is warranted.
        assert!(needs_recovery(secs(30), secs(40), secs(65), SR));
        // Reaching 62 s of that same remainder is fine.
        assert!(!needs_recovery(secs(30), secs(62), secs(65), SR));
    }

    #[test]
    fn sample_and_ms_conversions_round_trip() {
        assert_eq!(samples_to_ms(ms_to_samples(29_200, SR), SR), 29_200);
        assert_eq!(ms_to_samples(-5, SR), 0);
        assert_eq!(samples_to_ms(1000, 0), 0);
    }
}
