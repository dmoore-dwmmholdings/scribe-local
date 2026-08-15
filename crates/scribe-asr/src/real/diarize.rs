//! Real diarization via `sherpa_onnx::OfflineSpeakerDiarization`.
//!
//! Silero VAD + pyannote segmentation + speaker-embedding extraction +
//! FastClustering, all inside one sherpa-onnx object (design §8). When the
//! caller knows the participant count we pin `num_clusters`; otherwise we let
//! clustering discover the count via a cosine threshold.

use std::collections::HashMap;
use std::path::Path;

use scribe_core::{Error, Result};
use sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig,
};

use crate::models::DiarizationModelPaths;
use crate::types::{Diarization, Diarizer, SpeakerTurn};
use crate::wav::{self, WavData};

/// Cosine-similarity threshold used when the speaker count is unknown. Also
/// reused to re-identify a speaker across windows in the chunked path.
const CLUSTER_THRESHOLD: f32 = 0.5;

/// Diarize at most this much audio in one `process` call.
///
/// sherpa's diarization holds the whole clip's segments and embeddings in
/// native memory and overruns its stack on very long input — a 2h49m recording
/// aborted the worker with `0xc0000409` (STATUS_STACK_BUFFER_OVERRUN) after
/// ~40 minutes of work. Ten minutes is comfortably inside the range that has
/// run reliably, and is still long enough for clustering to separate voices
/// within a window; identity is then carried across windows by embedding.
const DIARIZE_WINDOW_MS: i64 = 10 * 60 * 1000;

/// Speech a window speaker must have before it may found a *new* global speaker.
///
/// Matching an existing speaker has no such floor — a recognised voice counts
/// however briefly it speaks. This only stops sub-second noise bursts from
/// inventing participants. The cost is that someone whose entire contribution to
/// a window is under this gets folded into the nearest speaker; that is the
/// better error, since the alternative produced 150 phantom participants.
const MIN_NEW_SPEAKER_MS: i64 = 3_000;

/// Wraps `OfflineSpeakerDiarization` plus a standalone embedding extractor used
/// to compute the per-speaker mean embeddings the pipeline needs for enrollment.
pub struct SherpaDiarizer {
    paths: DiarizationModelPaths,
    device: String,
    num_threads: i32,
    // The diarization object's clustering config depends on `expected_speakers`,
    // which varies per call, so we build the object lazily in `diarize`.
}

impl SherpaDiarizer {
    pub fn load(paths: DiarizationModelPaths, device: &str, num_threads: i32) -> Result<Self> {
        // Validate the extractor can be built up front (fail fast at load time).
        let _ = build_extractor(&paths, device, num_threads)?;
        Ok(SherpaDiarizer {
            paths,
            device: device.to_string(),
            num_threads,
        })
    }

    fn build_diarizer(&self, expected: Option<i32>) -> Result<OfflineSpeakerDiarization> {
        let provider = provider_for(&self.device);

        // `participants_expected` is treated as a soft HINT, not a hard count:
        // always discover the speaker count via the cosine threshold so the
        // system adapts when more people speak (someone joins) or fewer do
        // (someone stays silent) than the user guessed. Pinning `num_clusters`
        // to the hint would force-merge or force-split real voices.
        if let Some(n) = expected {
            tracing::debug!(hint = n, "diarize: speaker-count hint (advisory, threshold discovery)");
        }
        let clustering = FastClusteringConfig {
            num_clusters: -1,
            threshold: CLUSTER_THRESHOLD,
        };

        let config = OfflineSpeakerDiarizationConfig {
            segmentation: OfflineSpeakerSegmentationModelConfig {
                pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                    model: Some(path_str(&self.paths.segmentation)?),
                },
                num_threads: self.num_threads,
                debug: false,
                provider: Some(provider.clone()),
            },
            embedding: SpeakerEmbeddingExtractorConfig {
                model: Some(path_str(&self.paths.embedding)?),
                num_threads: self.num_threads,
                debug: false,
                provider: Some(provider),
            },
            clustering,
            ..Default::default()
        };

        OfflineSpeakerDiarization::create(&config)
            .ok_or_else(|| Error::Model("failed to create OfflineSpeakerDiarization".into()))
    }
}

impl Diarizer for SherpaDiarizer {
    fn diarize(&self, wav_path: &Path, expected_speakers: Option<i32>) -> Result<Diarization> {
        let audio = wav::read_wav(wav_path)?;
        let sr = audio.sample_rate;
        let window = ((DIARIZE_WINDOW_MS * sr as i64) / 1000) as usize;

        if window == 0 || audio.samples.len() <= window {
            return self.diarize_whole(&audio, expected_speakers);
        }
        self.diarize_chunked(&audio, expected_speakers, window)
    }
}

impl SherpaDiarizer {
    /// Single-pass diarization — the whole clip goes to sherpa at once.
    fn diarize_whole(&self, audio: &WavData, expected: Option<i32>) -> Result<Diarization> {
        let diarizer = self.build_diarizer(expected)?;

        let result = diarizer
            .process(&audio.samples)
            .ok_or_else(|| Error::Model("diarization produced no result".into()))?;

        let segments = result.sort_by_start_time();

        let mut turns = Vec::with_capacity(segments.len());
        let mut speaker_set = std::collections::BTreeSet::new();
        for seg in &segments {
            speaker_set.insert(seg.speaker);
            turns.push(SpeakerTurn {
                local_idx: seg.speaker,
                start_ms: (seg.start as f64 * 1000.0).round() as i64,
                end_ms: (seg.end as f64 * 1000.0).round() as i64,
            });
        }

        // Per-speaker mean embedding for enrollment matching. sherpa's
        // diarization result doesn't hand back the cluster centroids, so we
        // re-extract an embedding over each speaker's concatenated segment audio
        // and average. This reuses the same embedding model the clusterer used.
        let extractor = build_extractor(&self.paths, &self.device, self.num_threads)?;
        let embeddings =
            compute_speaker_embeddings(&extractor, &audio.samples, audio.sample_rate, &turns)?;

        let num_speakers = if result.num_speakers() > 0 {
            result.num_speakers() as usize
        } else {
            speaker_set.len()
        };

        Ok(Diarization {
            turns,
            embeddings,
            num_speakers,
        })
    }

    /// Diarize a long recording one window at a time, stitching the per-window
    /// speaker sets back together by voice similarity.
    ///
    /// Handing sherpa a multi-hour clip in one `process` call overruns its stack
    /// and aborts the process (Windows `0xc0000409`), so long audio has to be
    /// windowed. Each window is clustered independently, which means window 2's
    /// "speaker 0" is unrelated to window 1's, so we re-identify speakers across
    /// windows with the same cosine test the clusterer itself uses: a window
    /// speaker joins the global speaker whose running mean embedding it matches,
    /// or becomes a new one.
    fn diarize_chunked(
        &self,
        audio: &WavData,
        expected: Option<i32>,
        window: usize,
    ) -> Result<Diarization> {
        let diarizer = self.build_diarizer(expected)?;
        let extractor = build_extractor(&self.paths, &self.device, self.num_threads)?;
        let sr = audio.sample_rate;

        let mut turns: Vec<SpeakerTurn> = Vec::new();
        // Running mean embedding per global speaker, with the number of window
        // speakers folded into it so later merges stay weighted correctly.
        let mut centroids: Vec<(Vec<f32>, usize)> = Vec::new();

        let mut start = 0usize;
        while start < audio.samples.len() {
            let end = (start + window).min(audio.samples.len());
            let offset_ms = (start as i64 * 1000) / sr as i64;
            let slice = &audio.samples[start..end];

            // A window with no speech yields no result; that is not an error.
            let Some(result) = diarizer.process(slice) else {
                tracing::debug!(offset_ms, "diarize: window produced no segments");
                start = end;
                continue;
            };

            let local_turns: Vec<SpeakerTurn> = result
                .sort_by_start_time()
                .iter()
                .map(|seg| SpeakerTurn {
                    local_idx: seg.speaker,
                    start_ms: (seg.start as f64 * 1000.0).round() as i64,
                    end_ms: (seg.end as f64 * 1000.0).round() as i64,
                })
                .collect();
            if local_turns.is_empty() {
                start = end;
                continue;
            }

            let local_embs = compute_speaker_embeddings(&extractor, slice, sr, &local_turns)?;

            // Resolve every window speaker to a global one before rewriting the
            // turns, so two window speakers can't collapse onto each other.
            let mut local_ids: Vec<i32> = local_turns.iter().map(|t| t.local_idx).collect();
            local_ids.sort_unstable();
            local_ids.dedup();

            // Total speech per window speaker. Clustering inside a window splits
            // out brief noise and crosstalk as their own "speakers"; an
            // embedding taken from a fraction of a second is unreliable, so it
            // matches nothing and would found a new participant every time. A
            // 2h49m meeting produced 156 speakers that way — five real voices
            // and ~150 fragments of well under a second each.
            let mut speech_ms: HashMap<i32, i64> = HashMap::new();
            for t in &local_turns {
                *speech_ms.entry(t.local_idx).or_insert(0) += (t.end_ms - t.start_ms).max(0);
            }

            let mut mapping: HashMap<i32, i32> = HashMap::new();
            for local_id in local_ids {
                let Some(emb) = local_embs.get(&local_id) else {
                    continue;
                };
                if let Some(global) = match_global_speaker(&mut centroids, emb) {
                    // Recognised: join that speaker however brief this turn was.
                    mapping.insert(local_id, global);
                } else if speech_ms.get(&local_id).copied().unwrap_or(0) >= MIN_NEW_SPEAKER_MS {
                    mapping.insert(local_id, push_global_speaker(&mut centroids, emb));
                }
                // Otherwise: unrecognised *and* too brief to be a new
                // participant — attributed to the nearest speaker below.
            }

            // Nothing in this window could be identified — drop it rather than
            // attribute speech to an arbitrary speaker.
            if mapping.is_empty() {
                tracing::debug!(offset_ms, "diarize: no identifiable speaker in window");
                start = end;
                continue;
            }

            for turn in &local_turns {
                let global = match mapping.get(&turn.local_idx) {
                    Some(g) => *g,
                    // Unidentifiable fragment: attribute it to whoever holds the
                    // nearest identified turn in this window, which is far more
                    // likely to be right than a brand-new speaker.
                    None => nearest_mapped_speaker(&local_turns, &mapping, turn),
                };
                turns.push(SpeakerTurn {
                    local_idx: global,
                    start_ms: turn.start_ms + offset_ms,
                    end_ms: turn.end_ms + offset_ms,
                });
            }

            start = end;
        }

        turns.sort_by_key(|t| (t.start_ms, t.end_ms));

        let mut embeddings = HashMap::new();
        for (idx, (centroid, _)) in centroids.iter().enumerate() {
            if centroid.iter().any(|x| *x != 0.0) {
                embeddings.insert(idx as i32, centroid.clone());
            }
        }

        let num_speakers = centroids.len();
        tracing::info!(
            windows = audio.samples.len().div_ceil(window),
            speakers = num_speakers,
            turns = turns.len(),
            "diarize: chunked long recording"
        );

        Ok(Diarization {
            turns,
            embeddings,
            num_speakers,
        })
    }
}

/// Global speaker of the identified turn closest in time to `turn`.
///
/// Distance is measured between turn midpoints, so a fragment sitting inside a
/// long turn attaches to that turn's speaker. Callers must only invoke this when
/// `mapping` is non-empty.
fn nearest_mapped_speaker(
    turns: &[SpeakerTurn],
    mapping: &HashMap<i32, i32>,
    turn: &SpeakerTurn,
) -> i32 {
    let mid = |t: &SpeakerTurn| (t.start_ms + t.end_ms) / 2;
    let target = mid(turn);
    turns
        .iter()
        .filter_map(|t| mapping.get(&t.local_idx).map(|g| ((mid(t) - target).abs(), *g)))
        .min_by_key(|(d, _)| *d)
        .map(|(_, g)| g)
        // Unreachable while `mapping` is non-empty; fall back to the first
        // known speaker rather than fabricating an index.
        .unwrap_or_else(|| mapping.values().copied().min().unwrap_or(0))
}

/// Match `emb` to an existing global speaker or append a new one, returning its
/// index.
fn assign_global_speaker(centroids: &mut Vec<(Vec<f32>, usize)>, emb: &[f32]) -> i32 {
    match match_global_speaker(centroids, emb) {
        Some(i) => i,
        None => push_global_speaker(centroids, emb),
    }
}

/// Find the global speaker `emb` belongs to, if any. On a match the centroid
/// absorbs `emb` as a weighted mean, so an identity sharpens as more windows
/// contribute to it.
fn match_global_speaker(centroids: &mut [(Vec<f32>, usize)], emb: &[f32]) -> Option<i32> {
    let mut best: Option<(usize, f32)> = None;
    for (i, (centroid, _)) in centroids.iter().enumerate() {
        let sim = cosine(centroid, emb);
        if sim >= CLUSTER_THRESHOLD && best.map(|(_, b)| sim > b).unwrap_or(true) {
            best = Some((i, sim));
        }
    }

    let (i, _) = best?;
    let (centroid, count) = &mut centroids[i];
    let n = *count as f32;
    for (c, e) in centroid.iter_mut().zip(emb.iter()) {
        *c = (*c * n + *e) / (n + 1.0);
    }
    l2_normalize(centroid);
    *count += 1;
    Some(i as i32)
}

/// Append `emb` as a brand-new global speaker.
fn push_global_speaker(centroids: &mut Vec<(Vec<f32>, usize)>, emb: &[f32]) -> i32 {
    centroids.push((emb.to_vec(), 1));
    (centroids.len() - 1) as i32
}

/// Cosine similarity. Returns 0 for empty/zero vectors, so a speaker with no
/// usable embedding never matches anything.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na <= f32::EPSILON || nb <= f32::EPSILON {
        return 0.0;
    }
    dot / (na * nb)
}

/// Build a standalone embedding extractor from the diarization embedding model.
fn build_extractor(
    paths: &DiarizationModelPaths,
    device: &str,
    num_threads: i32,
) -> Result<SpeakerEmbeddingExtractor> {
    let config = SpeakerEmbeddingExtractorConfig {
        model: Some(path_str(&paths.embedding)?),
        num_threads,
        debug: false,
        provider: Some(provider_for(device)),
    };
    SpeakerEmbeddingExtractor::create(&config)
        .ok_or_else(|| Error::Model("failed to create SpeakerEmbeddingExtractor".into()))
}

/// Compute one mean embedding per speaker by extracting an embedding over the
/// audio of each of that speaker's turns and averaging (then L2-normalizing).
///
/// `turns` are relative to `samples`, so this works equally on a whole file or
/// on one window of a chunked run.
fn compute_speaker_embeddings(
    extractor: &SpeakerEmbeddingExtractor,
    samples: &[f32],
    sample_rate: u32,
    turns: &[SpeakerTurn],
) -> Result<HashMap<i32, Vec<f32>>> {
    let sr = sample_rate as i64;
    let mut acc: HashMap<i32, (Vec<f32>, usize)> = HashMap::new();

    for turn in turns {
        let start = ((turn.start_ms.max(0) * sr) / 1000) as usize;
        let end = (((turn.end_ms.max(turn.start_ms)) * sr) / 1000) as usize;
        let end = end.min(samples.len());
        if end <= start {
            continue;
        }
        let slice = &samples[start..end];

        let Some(stream) = extractor.create_stream() else {
            continue;
        };
        stream.accept_waveform(sample_rate as i32, slice);
        stream.input_finished();
        if !extractor.is_ready(&stream) {
            continue;
        }
        let Some(emb) = extractor.compute(&stream) else {
            continue;
        };

        let entry = acc
            .entry(turn.local_idx)
            .or_insert_with(|| (vec![0.0; emb.len()], 0));
        if entry.0.len() != emb.len() {
            entry.0 = vec![0.0; emb.len()];
        }
        for (a, b) in entry.0.iter_mut().zip(emb.iter()) {
            *a += *b;
        }
        entry.1 += 1;
    }

    let mut out = HashMap::new();
    for (idx, (mut sum, count)) in acc {
        if count == 0 {
            continue;
        }
        for x in sum.iter_mut() {
            *x /= count as f32;
        }
        l2_normalize(&mut sum);
        out.insert(idx, sum);
    }
    Ok(out)
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        let mut v = v.to_vec();
        l2_normalize(&mut v);
        v
    }

    /// The same voice appearing in a later window must resolve to the speaker
    /// it already established, not to a new one — otherwise a two-person call
    /// split over six windows would report a dozen speakers.
    #[test]
    fn a_returning_voice_rejoins_its_global_speaker() {
        let mut centroids = Vec::new();
        let alice = unit(&[1.0, 0.0, 0.0]);
        let bob = unit(&[0.0, 1.0, 0.0]);

        assert_eq!(assign_global_speaker(&mut centroids, &alice), 0);
        assert_eq!(assign_global_speaker(&mut centroids, &bob), 1);

        // Window 2: Alice again, slightly different but well within threshold.
        let alice_again = unit(&[0.96, 0.28, 0.0]);
        assert_eq!(assign_global_speaker(&mut centroids, &alice_again), 0);
        assert_eq!(centroids.len(), 2, "must not invent a third speaker");
    }

    /// A genuinely different voice must not be folded into an existing speaker.
    #[test]
    fn a_new_voice_becomes_a_new_speaker() {
        let mut centroids = Vec::new();
        assign_global_speaker(&mut centroids, &unit(&[1.0, 0.0, 0.0]));

        let orthogonal = unit(&[0.0, 0.0, 1.0]);
        assert_eq!(assign_global_speaker(&mut centroids, &orthogonal), 1);
        assert_eq!(centroids.len(), 2);
    }

    /// When several existing speakers are above threshold, the closest wins.
    ///
    /// The two seed voices must be mutually *below* threshold or they would
    /// (correctly) merge into one speaker before the probe is ever tested.
    #[test]
    fn matching_picks_the_closest_speaker() {
        let mut centroids = Vec::new();
        assert_eq!(assign_global_speaker(&mut centroids, &unit(&[1.0, 0.0, 0.0])), 0);
        assert_eq!(assign_global_speaker(&mut centroids, &unit(&[0.0, 1.0, 0.0])), 1);

        // cos 0.8 with speaker 0 and 0.6 with speaker 1 — both clear 0.5, so
        // the tie must be broken by similarity rather than by iteration order.
        let probe = unit(&[0.8, 0.6, 0.0]);
        assert_eq!(assign_global_speaker(&mut centroids, &probe), 0);
        assert_eq!(centroids.len(), 2);
    }

    /// An empty embedding must never be *matched* to an existing speaker.
    /// (The chunked path no longer feeds these in — it routes them through
    /// `nearest_mapped_speaker` — but the guard must hold regardless.)
    #[test]
    fn an_unembeddable_speaker_never_matches() {
        let mut centroids = Vec::new();
        assign_global_speaker(&mut centroids, &unit(&[1.0, 0.0, 0.0]));

        assert_eq!(assign_global_speaker(&mut centroids, &[]), 1);
        // And the zero-length centroid must not swallow a later real voice.
        assert_eq!(assign_global_speaker(&mut centroids, &unit(&[0.0, 1.0, 0.0])), 2);
    }

    fn turn(local_idx: i32, start_ms: i64, end_ms: i64) -> SpeakerTurn {
        SpeakerTurn {
            local_idx,
            start_ms,
            end_ms,
        }
    }

    /// A fragment too short to embed is attributed to the nearest identified
    /// speaker instead of becoming a phantom participant. Before this, a long
    /// meeting reported ~150 speakers that each spoke once.
    #[test]
    fn unidentifiable_fragment_takes_the_nearest_speaker() {
        let turns = vec![
            turn(0, 0, 10_000),      // identified -> global 7
            turn(9, 10_100, 10_300), // fragment, no embedding
            turn(1, 20_000, 30_000), // identified -> global 4
        ];
        let mapping = HashMap::from([(0, 7), (1, 4)]);

        // Sits just after speaker 0's turn, so it belongs to global 7.
        assert_eq!(nearest_mapped_speaker(&turns, &mapping, &turns[1]), 7);

        // A fragment adjacent to the later turn resolves to global 4 instead.
        let late = turn(9, 29_000, 29_200);
        assert_eq!(nearest_mapped_speaker(&turns, &mapping, &late), 4);
    }

    /// `match_global_speaker` must not create anything — the duration gate in
    /// the chunked path depends on being able to test for a match separately
    /// from committing to a new speaker.
    #[test]
    fn matching_alone_never_creates_a_speaker() {
        let mut centroids = vec![(unit(&[1.0, 0.0, 0.0]), 1)];

        // An unrecognised voice reports no match and leaves the set untouched.
        assert_eq!(match_global_speaker(&mut centroids, &unit(&[0.0, 1.0, 0.0])), None);
        assert_eq!(centroids.len(), 1);

        // A recognised one matches, still without growing the set.
        assert_eq!(match_global_speaker(&mut centroids, &unit(&[0.99, 0.14, 0.0])), Some(0));
        assert_eq!(centroids.len(), 1);
    }

    /// The duration gate: a brief unrecognised burst is below the floor and must
    /// not found a participant, while a substantial one must.
    #[test]
    fn only_substantial_speech_founds_a_new_speaker() {
        let brief = MIN_NEW_SPEAKER_MS - 1;
        let substantial = MIN_NEW_SPEAKER_MS;

        assert!(brief < MIN_NEW_SPEAKER_MS, "guard: brief is under the floor");
        assert!(substantial >= MIN_NEW_SPEAKER_MS, "guard: substantial clears it");

        // Mirrors the chunked path's decision for an unrecognised speaker.
        let mut centroids = vec![(unit(&[1.0, 0.0, 0.0]), 1)];
        let stranger = unit(&[0.0, 1.0, 0.0]);

        if match_global_speaker(&mut centroids, &stranger).is_none() && brief >= MIN_NEW_SPEAKER_MS {
            push_global_speaker(&mut centroids, &stranger);
        }
        assert_eq!(centroids.len(), 1, "a brief burst must not add a speaker");

        if match_global_speaker(&mut centroids, &stranger).is_none()
            && substantial >= MIN_NEW_SPEAKER_MS
        {
            push_global_speaker(&mut centroids, &stranger);
        }
        assert_eq!(centroids.len(), 2, "substantial speech must add one");
    }

    #[test]
    fn cosine_is_zero_for_degenerate_input() {
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0, 0.0], &[1.0]), 0.0, "length mismatch");
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
    }
}
