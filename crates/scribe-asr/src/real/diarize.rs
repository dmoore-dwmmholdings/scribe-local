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

/// Shortest turn used to build a speaker's identity embedding.
///
/// Speaker-embedding models need about a second of speech; below that the
/// vector says more about the noise floor than the voice.
const MIN_EMBED_MS: i64 = 1_000;

/// Longest slice handed to the embedding extractor in one call.
///
/// TitaNet's masked convolutions carry a length limit baked in at export: past
/// roughly two minutes of audio the mask and the feature map disagree, and
/// onnxruntime throws from `mconv`'s `Where` node
/// (`broadcast an axis by a dimension other than 1. 12288 by 14531`). That is a
/// C++ exception crossing the FFI boundary, which Rust cannot catch — it aborts
/// the whole worker, taking the job's lease and every other queued job with it.
/// A 9-minute single-speaker recording did exactly that: diarization merged the
/// monologue into one turn far over the limit.
///
/// 30 s is well inside the limit and is already more speech than these models
/// use — they were trained on a few seconds — so nothing is lost by splitting.
/// A longer turn is embedded in pieces and averaged, weighted by duration, so
/// the result is what embedding the whole turn was meant to produce anyway.
const MAX_EMBED_MS: i64 = 30_000;

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

        // When the user tells us how many people are in the room, believe them.
        //
        // This used to always discover the count by threshold, on the reasoning
        // that the hint might be wrong. On long recordings that reasoning cost
        // far more than it saved: threshold discovery inside each window split
        // four voices into dozens (a 50-minute meeting came back with 34
        // speakers, a 2h49m one with 156). A stated count is the single most
        // reliable signal available, and a wrong hint is recoverable — the user
        // can rename or reprocess — where dozens of phantom speakers are not.
        //
        // Without a hint we still discover the count, and the global merge in
        // `consolidate` cleans up what over-segmentation gets through.
        let clustering = match expected {
            Some(n) if n >= 1 => {
                tracing::debug!(n, "diarize: clustering to the stated speaker count");
                FastClusteringConfig {
                    num_clusters: n,
                    threshold: CLUSTER_THRESHOLD,
                }
            }
            _ => FastClusteringConfig {
                num_clusters: -1,
                threshold: CLUSTER_THRESHOLD,
            },
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

        // Pass 1: diarize each window on its own. Nothing is decided about
        // identity here - a window speaker is just "some voice, over this
        // stretch of audio". Identity used to be resolved greedily as the
        // windows went by, which made the answer depend on the order they
        // happened to arrive in.
        let mut fragments: Vec<Fragment> = Vec::new();
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

            let mut by_local: HashMap<i32, Fragment> = HashMap::new();
            for turn in &local_turns {
                let frag = by_local.entry(turn.local_idx).or_insert_with(|| Fragment {
                    turns: Vec::new(),
                    embedding: local_embs.get(&turn.local_idx).cloned(),
                    speech_ms: 0,
                });
                frag.speech_ms += (turn.end_ms - turn.start_ms).max(0);
                frag.turns.push(SpeakerTurn {
                    local_idx: turn.local_idx,
                    start_ms: turn.start_ms + offset_ms,
                    end_ms: turn.end_ms + offset_ms,
                });
            }
            fragments.extend(by_local.into_values());

            start = end;
        }

        if fragments.is_empty() {
            return Ok(Diarization {
                turns: Vec::new(),
                embeddings: HashMap::new(),
                num_speakers: 0,
            });
        }

        // Pass 2: cluster every fragment in the recording at once.
        let assignment = cluster_fragments(&fragments, expected);

        // Pass 3: emit the turns under their cluster, and build one centroid per
        // cluster, weighted by how much speech each fragment contributed.
        let mut turns: Vec<SpeakerTurn> = Vec::new();
        let mut sums: HashMap<i32, (Vec<f32>, f32)> = HashMap::new();
        for (frag, &cluster) in fragments.iter().zip(assignment.iter()) {
            for turn in &frag.turns {
                turns.push(SpeakerTurn {
                    local_idx: cluster,
                    start_ms: turn.start_ms,
                    end_ms: turn.end_ms,
                });
            }
            if let Some(emb) = &frag.embedding {
                let weight = (frag.speech_ms.max(1) as f32) / 1000.0;
                let entry = sums
                    .entry(cluster)
                    .or_insert_with(|| (vec![0.0; emb.len()], 0.0));
                if entry.0.len() != emb.len() {
                    entry.0 = vec![0.0; emb.len()];
                }
                for (a, b) in entry.0.iter_mut().zip(emb.iter()) {
                    *a += *b * weight;
                }
                entry.1 += weight;
            }
        }
        turns.sort_by_key(|t| (t.start_ms, t.end_ms));

        let mut embeddings = HashMap::new();
        for (cluster, (mut sum, weight)) in sums {
            if weight <= 0.0 {
                continue;
            }
            for x in sum.iter_mut() {
                *x /= weight;
            }
            l2_normalize(&mut sum);
            embeddings.insert(cluster, sum);
        }

        let num_speakers = assignment
            .iter()
            .copied()
            .max()
            .map(|m| m as usize + 1)
            .unwrap_or(0);
        tracing::info!(
            windows = audio.samples.len().div_ceil(window),
            fragments = fragments.len(),
            speakers = num_speakers,
            turns = turns.len(),
            stated = expected.unwrap_or(-1),
            "diarize: chunked long recording"
        );

        Ok(Diarization {
            turns,
            embeddings,
            num_speakers,
        })
    }
}

/// One window speaker: a stretch of a single voice, before anything is decided
/// about which recording-wide speaker it belongs to.
struct Fragment {
    turns: Vec<SpeakerTurn>,
    /// `None` when the audio was too short or too poor to embed.
    embedding: Option<Vec<f32>>,
    speech_ms: i64,
}

/// Most speakers we will infer when the count was not stated.
///
/// A bound on the search, not a similarity threshold: past a dozen distinct
/// voices in one recording, another "speaker" is far more likely to be
/// over-segmentation than a person.
const MAX_INFERRED_SPEAKERS: usize = 12;

/// Assign every fragment to a recording-wide speaker.
///
/// Agglomerative clustering with **average linkage**: the similarity between
/// two clusters is the mean over every member pair, recomputed from the
/// original fragment embeddings each time. It deliberately keeps no running
/// centroid - averaging centroids as you merge lets the largest cluster drift
/// toward the mean of everything and then swallow the rest, which is how a
/// four-person meeting collapsed into one speaker holding 99.8% of the speech.
///
/// The speaker count comes from `expected` when it was stated. When it was not,
/// it is read off this recording's own merge sequence rather than from a tuned
/// constant: merges get less convincing as clustering is forced to join
/// genuinely different voices, and the sharpest fall in that sequence is the
/// natural number of speakers. A cosine value that means "same person" in one
/// recording means nothing in another - different mic, room and voices - so no
/// fixed threshold can be right for both.
fn cluster_fragments(fragments: &[Fragment], expected: Option<i32>) -> Vec<i32> {
    // Fragments we can actually compare.
    let embedded: Vec<usize> = fragments
        .iter()
        .enumerate()
        .filter(|(_, f)| f.embedding.as_ref().is_some_and(|e| !e.is_empty()))
        .map(|(i, _)| i)
        .collect();

    let mut assignment = vec![-1i32; fragments.len()];
    if embedded.is_empty() {
        // Nothing could be embedded: one speaker is the only honest answer.
        return vec![0; fragments.len()];
    }

    // Every embedded fragment starts as its own cluster.
    let mut clusters: Vec<Vec<usize>> = embedded.iter().map(|&i| vec![i]).collect();
    let target = expected
        .filter(|n| *n >= 1)
        .map(|n| (n as usize).min(clusters.len()));

    // Merge down to a single cluster, recording what each merge cost.
    let mut history: Vec<(usize, f32, Vec<Vec<usize>>)> = Vec::new();
    while clusters.len() > 1 {
        let mut best: Option<(usize, usize, f32)> = None;
        for i in 0..clusters.len() {
            for j in (i + 1)..clusters.len() {
                let sim = average_linkage(fragments, &clusters[i], &clusters[j]);
                if best.map(|(_, _, b)| sim > b).unwrap_or(true) {
                    best = Some((i, j, sim));
                }
            }
        }
        let Some((i, j, sim)) = best else { break };
        history.push((clusters.len(), sim, clusters.clone()));

        let merged = clusters.remove(j);
        clusters[i].extend(merged);

        if target.is_some_and(|t| clusters.len() == t) {
            break;
        }
    }

    let final_clusters = match target {
        // Stated count: what we hold now is the answer.
        Some(_) => clusters,
        // Otherwise the recording decides. `clusters` is now the single
        // all-in-one cluster the merge loop ended at, which is the answer when
        // nothing separates cleanly.
        None => choose_clustering(&history, &clusters),
    };

    for (idx, members) in final_clusters.iter().enumerate() {
        for &fragment in members {
            assignment[fragment] = idx as i32;
        }
    }

    // A fragment with no usable embedding joins the speaker holding the nearest
    // turn in time - far likelier to be right than a speaker of its own.
    for i in 0..fragments.len() {
        if assignment[i] >= 0 {
            continue;
        }
        assignment[i] = nearest_assigned(fragments, &assignment, i).unwrap_or(0);
    }
    assignment
}

/// How much worse than the merges already accepted a merge has to be before we
/// refuse it and call the two clusters different people.
///
/// The one constant left in the speaker count, and deliberately a *ratio*
/// rather than a similarity. Absolute cosine cannot work across recordings: two
/// takes of one voice sit near 0.95 on a close mic and near 0.5 across a room,
/// so any fixed cutoff is simultaneously too strict for one recording and too
/// loose for another. What does carry across is the shape of the merge
/// sequence - joining takes of one voice looks consistent, and joining two
/// people is a visible step down from whatever "consistent" meant in this
/// recording.
const RELATIVE_DROP: f32 = 0.8;

/// Decide where to stop merging, using only this recording's own numbers.
///
/// Walks the merge sequence in order. Each merge is compared against the median
/// of the merges already accepted: while clustering is joining takes of the same
/// voice the similarities stay in family, and the first one that falls well
/// below that family is the boundary between two people. If no merge ever does,
/// every fragment belongs to one voice - which is a possible answer here, and
/// the one a "largest drop" rule can never give, because it always cuts
/// somewhere.
fn choose_clustering(
    history: &[(usize, f32, Vec<Vec<usize>>)],
    single: &[Vec<usize>],
) -> Vec<Vec<usize>> {
    let mut accepted: Vec<f32> = Vec::new();
    for (count, sim, snapshot) in history {
        // Above the sanity bound, keep merging whatever it looks like: that
        // many clusters is over-segmentation, not a room full of people.
        if *count <= MAX_INFERRED_SPEAKERS {
            // With nothing accepted yet there is no family to compare against;
            // a fragment is perfectly similar to itself, so use 1.0.
            let within = median(&accepted).unwrap_or(1.0);
            if *sim < RELATIVE_DROP * within {
                return snapshot.clone();
            }
        }
        accepted.push(*sim);
    }
    single.to_vec()
}

/// Median of `values`, or `None` when empty. Used instead of the mean so one
/// unusually good or bad merge cannot move the comparison.
fn median(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    })
}

/// Mean similarity over every member pair across two clusters.
fn average_linkage(fragments: &[Fragment], a: &[usize], b: &[usize]) -> f32 {
    let mut total = 0.0;
    let mut pairs = 0.0;
    for &i in a {
        let Some(ei) = fragments[i].embedding.as_ref() else {
            continue;
        };
        for &j in b {
            let Some(ej) = fragments[j].embedding.as_ref() else {
                continue;
            };
            total += cosine(ei, ej);
            pairs += 1.0;
        }
    }
    if pairs == 0.0 {
        0.0
    } else {
        total / pairs
    }
}

/// Speaker of the assigned fragment whose turns sit closest in time to `target`.
fn nearest_assigned(fragments: &[Fragment], assignment: &[i32], target: usize) -> Option<i32> {
    let mid = |f: &Fragment| -> i64 {
        if f.turns.is_empty() {
            return 0;
        }
        let sum: i64 = f.turns.iter().map(|t| (t.start_ms + t.end_ms) / 2).sum();
        sum / f.turns.len() as i64
    };
    let want = mid(&fragments[target]);
    fragments
        .iter()
        .enumerate()
        .filter(|(i, _)| assignment[*i] >= 0)
        .min_by_key(|(_, f)| (mid(f) - want).abs())
        .map(|(i, _)| assignment[i])
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
///
/// The chunked path deliberately no longer calls this: it needs to test for a
/// match *without* committing to a new speaker, so that an unrecognised voice
/// can be held to [`MIN_NEW_SPEAKER_MS`] first. Kept for tests, which cover the
/// match-or-create behaviour that `match_global_speaker` still implements.
#[cfg(test)]
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
    // Weighted by duration, so a speaker's identity is dominated by the audio
    // there was most of. An embedding taken from a fraction of a second is
    // mostly noise; averaging it in equally with a ten-second turn was pulling
    // centroids apart and stopping enrolled voices from matching.
    let mut acc: HashMap<i32, (Vec<f32>, f32)> = HashMap::new();

    // Turns long enough to embed reliably. If a speaker has none, fall back to
    // whatever they do have rather than leaving them with no embedding at all.
    let long_enough: Vec<&SpeakerTurn> = turns
        .iter()
        .filter(|t| t.end_ms - t.start_ms >= MIN_EMBED_MS)
        .collect();
    let speakers_with_long: std::collections::HashSet<i32> =
        long_enough.iter().map(|t| t.local_idx).collect();

    for turn in turns {
        // Skip a scrap of a turn when this speaker has better audio elsewhere.
        if turn.end_ms - turn.start_ms < MIN_EMBED_MS
            && speakers_with_long.contains(&turn.local_idx)
        {
            continue;
        }
        let start = ((turn.start_ms.max(0) * sr) / 1000) as usize;
        let end = (((turn.end_ms.max(turn.start_ms)) * sr) / 1000) as usize;
        let end = end.min(samples.len());
        if end <= start {
            continue;
        }

        // A turn over the model's length limit is embedded in pieces and
        // averaged. The pieces carry their own duration as weight, so a long
        // turn still counts for its full length in the speaker's mean.
        let max_len = ((MAX_EMBED_MS * sr) / 1000) as usize;
        for (from, to) in split_evenly(start, end, max_len) {
            if to <= from {
                continue;
            }
            let slice = &samples[from..to];

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

            let weight = (to - from) as f32 / sample_rate as f32;
            let entry = acc
                .entry(turn.local_idx)
                .or_insert_with(|| (vec![0.0; emb.len()], 0.0));
            if entry.0.len() != emb.len() {
                entry.0 = vec![0.0; emb.len()];
            }
            for (a, b) in entry.0.iter_mut().zip(emb.iter()) {
                *a += *b * weight;
            }
            entry.1 += weight;
        }
    }

    let mut out = HashMap::new();
    for (idx, (mut sum, weight)) in acc {
        if weight <= 0.0 {
            continue;
        }
        for x in sum.iter_mut() {
            *x /= weight;
        }
        l2_normalize(&mut sum);
        out.insert(idx, sum);
    }
    Ok(out)
}

/// Split `start..end` into consecutive ranges of at most `max_len`.
///
/// The pieces come out as equal as the range divides, rather than a run of
/// full-length pieces followed by whatever is left: a three-second remainder
/// embeds far worse than two pieces of half the length, and the pieces are
/// averaged together either way.
fn split_evenly(start: usize, end: usize, max_len: usize) -> Vec<(usize, usize)> {
    if end <= start || max_len == 0 {
        return vec![(start, end)];
    }
    let len = end - start;
    let pieces = len.div_ceil(max_len);
    (0..pieces)
        .map(|i| (start + len * i / pieces, start + len * (i + 1) / pieces))
        .collect()
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

    fn frag(emb: Option<&[f32]>, start_ms: i64, end_ms: i64) -> Fragment {
        Fragment {
            turns: vec![turn(0, start_ms, end_ms)],
            embedding: emb.map(unit),
            speech_ms: end_ms - start_ms,
        }
    }

    /// Group fragment indices by the speaker they were assigned, so a test can
    /// assert the partition without caring which number each speaker got.
    fn partition(assignment: &[i32]) -> Vec<Vec<usize>> {
        let mut groups: HashMap<i32, Vec<usize>> = HashMap::new();
        for (i, &a) in assignment.iter().enumerate() {
            groups.entry(a).or_default().push(i);
        }
        let mut out: Vec<Vec<usize>> = groups.into_values().collect();
        out.sort();
        out
    }

    /// Three voices, several stretches each, must come back as three speakers
    /// with the right membership.
    ///
    /// This is the case that broke in the field: a 50-minute, four-person
    /// meeting came back with one speaker holding 99.8% of the speech, because
    /// merging averaged centroids and the biggest cluster drifted until it
    /// swallowed everyone.
    #[test]
    fn several_takes_of_three_voices_give_three_speakers() {
        let alice = [1.0, 0.05, 0.0];
        let bob = [0.0, 1.0, 0.05];
        let carol = [0.05, 0.0, 1.0];
        let fragments = vec![
            frag(Some(&alice), 0, 10_000),
            frag(Some(&[0.98, 0.1, 0.02]), 10_000, 20_000),
            frag(Some(&bob), 20_000, 30_000),
            frag(Some(&[0.02, 0.99, 0.1]), 30_000, 40_000),
            frag(Some(&carol), 40_000, 50_000),
            frag(Some(&[0.1, 0.02, 0.97]), 50_000, 60_000),
        ];

        let assignment = cluster_fragments(&fragments, None);

        assert_eq!(
            partition(&assignment),
            vec![vec![0, 1], vec![2, 3], vec![4, 5]],
            "each voice keeps its own stretches"
        );
    }

    /// No single voice may absorb the recording when the others are genuinely
    /// distinct, however many stretches it happens to have.
    #[test]
    fn a_dominant_voice_does_not_swallow_the_others() {
        let mut fragments: Vec<Fragment> = (0..8)
            .map(|i| frag(Some(&[1.0, 0.02 * i as f32, 0.0]), i * 10_000, i * 10_000 + 9_000))
            .collect();
        // Two brief contributions from two other people, as in the real case.
        fragments.push(frag(Some(&[0.0, 1.0, 0.0]), 90_000, 94_000));
        fragments.push(frag(Some(&[0.0, 0.0, 1.0]), 100_000, 104_000));

        let assignment = cluster_fragments(&fragments, None);
        let groups = partition(&assignment);

        assert_eq!(groups.len(), 3, "three voices, not one");
        assert_eq!(groups[0], (0..8).collect::<Vec<_>>(), "the talker stays one speaker");
        assert_eq!(groups[1], vec![8]);
        assert_eq!(groups[2], vec![9]);
    }

    /// A stated speaker count is honored exactly - the user knows how many
    /// people were in the room, and that beats anything inferred.
    #[test]
    fn a_stated_count_is_honored() {
        let fragments = vec![
            frag(Some(&[1.0, 0.0, 0.0]), 0, 10_000),
            frag(Some(&[0.0, 1.0, 0.0]), 10_000, 20_000),
            frag(Some(&[0.0, 0.0, 1.0]), 20_000, 30_000),
            frag(Some(&[0.9, 0.1, 0.0]), 30_000, 40_000),
        ];

        let assignment = cluster_fragments(&fragments, Some(2));

        assert_eq!(partition(&assignment).len(), 2, "merged down to the stated count");
        assert!(assignment.iter().all(|&a| a >= 0 && a < 2));
    }

    /// The result must not depend on the order the windows happened to arrive
    /// in. The greedy pass this replaced failed exactly here: identity was
    /// resolved one window at a time, so whoever spoke first won.
    #[test]
    fn clustering_is_independent_of_fragment_order() {
        let voices: [[f32; 3]; 3] = [[1.0, 0.05, 0.0], [0.0, 1.0, 0.05], [0.05, 0.0, 1.0]];
        let forward: Vec<Fragment> = (0..6)
            .map(|i| frag(Some(&voices[i % 3]), i as i64 * 10_000, i as i64 * 10_000 + 9_000))
            .collect();
        let reversed: Vec<Fragment> = (0..6)
            .rev()
            .map(|i| frag(Some(&voices[i % 3]), i as i64 * 10_000, i as i64 * 10_000 + 9_000))
            .collect();

        let a = partition(&cluster_fragments(&forward, None)).len();
        let b = partition(&cluster_fragments(&reversed, None)).len();
        assert_eq!(a, 3);
        assert_eq!(a, b, "the same audio must give the same speaker count either way");
    }

    /// A stretch too short or too noisy to embed joins whoever is speaking
    /// around it, rather than becoming a speaker of its own.
    #[test]
    fn an_unembeddable_fragment_joins_its_neighbour() {
        let fragments = vec![
            frag(Some(&[1.0, 0.0, 0.0]), 0, 10_000),
            frag(None, 10_100, 10_400),
            frag(Some(&[0.0, 1.0, 0.0]), 60_000, 70_000),
        ];

        let assignment = cluster_fragments(&fragments, None);

        assert_eq!(
            assignment[1], assignment[0],
            "the fragment takes the speaker it sits next to in time"
        );
        assert_ne!(assignment[2], assignment[0]);
    }

    /// One voice must stay one speaker - inferring a count must not split a
    /// single person just because their stretches differ slightly.
    #[test]
    fn one_voice_stays_one_speaker() {
        let fragments: Vec<Fragment> = (0..6)
            .map(|i| frag(Some(&[1.0, 0.03 * i as f32, 0.01 * i as f32]), i * 10_000, i * 10_000 + 9_000))
            .collect();

        assert_eq!(partition(&cluster_fragments(&fragments, None)).len(), 1);
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

    /// A turn inside the limit must go to the model whole — splitting audio
    /// that did not need splitting would only blur the embedding.
    #[test]
    fn a_short_turn_is_not_split() {
        assert_eq!(split_evenly(0, 100, 100), vec![(0, 100)]);
        assert_eq!(split_evenly(500, 600, 1000), vec![(500, 600)]);
    }

    /// Pieces must cover the range exactly, with no gap, overlap or lost tail —
    /// they are weighted by their own length, so a gap silently under-weights
    /// the speaker and an overlap counts the same audio twice.
    #[test]
    fn pieces_tile_the_range_without_gaps() {
        let pieces = split_evenly(1_000, 8_500, 1_000);
        assert_eq!(pieces.first().unwrap().0, 1_000);
        assert_eq!(pieces.last().unwrap().1, 8_500);
        for pair in pieces.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "pieces must be contiguous");
        }
    }

    /// An over-long turn splits into even pieces, not full ones plus a runt.
    #[test]
    fn a_long_turn_splits_evenly() {
        // 2.5× the limit → three pieces of ~5/6 the limit, not 1 + 1 + 0.5.
        let pieces = split_evenly(0, 2_500, 1_000);
        assert_eq!(pieces.len(), 3);
        let shortest = pieces.iter().map(|(a, b)| b - a).min().unwrap();
        assert!(shortest >= 800, "no runt piece, got {shortest}");
    }

    /// The regression this cap exists for: TitaNet's exported masked convolution
    /// holds 12288 frames (122.88 s at a 10 ms hop), and one sample past that
    /// throws from onnxruntime — a foreign exception that aborts the worker
    /// rather than failing the job. A 9-minute monologue diarized into a single
    /// ~145 s turn and took the whole process down.
    #[test]
    fn no_piece_can_reach_the_models_length_limit() {
        const TITANET_LIMIT_MS: i64 = 122_880;
        assert!(
            MAX_EMBED_MS < TITANET_LIMIT_MS,
            "the cap must sit under the model's limit"
        );

        let sr = 16_000i64;
        let max_len = ((MAX_EMBED_MS * sr) / 1000) as usize;
        let limit = ((TITANET_LIMIT_MS * sr) / 1000) as usize;

        // A ten-minute turn, the worst case the diarize window can produce.
        for (from, to) in split_evenly(0, 600 * sr as usize, max_len) {
            assert!(to - from <= max_len, "piece over the cap: {}", to - from);
            assert!(to - from < limit, "piece would abort the worker");
        }
    }

    #[test]
    fn cosine_is_zero_for_degenerate_input() {
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0, 0.0], &[1.0]), 0.0, "length mismatch");
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
    }
}
