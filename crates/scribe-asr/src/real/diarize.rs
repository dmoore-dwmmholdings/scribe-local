//! Real diarization via `sherpa_onnx::OfflineSpeakerDiarization`.
//!
//! Silero VAD + pyannote segmentation + speaker-embedding extraction +
//! FastClustering, all inside one sherpa-onnx object (design §8).
//!
//! sherpa sees the recording a window at a time and answers only for the window
//! in front of it. Who is in the room is a fact about the whole recording, so
//! this module asks sherpa for voices, not for people: each window is clustered
//! by threshold with no target count, and the recording's speaker set is then
//! settled once, over every window's voices at once, in `cluster_fragments` —
//! which is also where a stated participant count is applied.

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

/// How much of a speaker's speech is enough to establish their voice.
///
/// A speaker embedding is an identity, not a summary — these models are trained
/// on a few seconds and stop improving well before a minute. Embedding every
/// turn a speaker takes therefore buys nothing after the first stretch, and it
/// is the single most expensive thing diarization does: a speaker who talks for
/// twenty minutes of an hour-long meeting had twenty minutes of audio pushed
/// through the extractor to produce one 192-dimensional vector.
///
/// A minute per speaker, taken from their longest turns first, so what is
/// embedded is also their cleanest continuous speech rather than whatever
/// happened to come first.
const EMBED_BUDGET_MS: i64 = 60_000;

/// Silence long enough to be a possible speaker change.
///
/// People do not swap places mid-breath; a handover has a pause in it. Below a
/// quarter of a second a gap is punctuation inside one person's sentence.
const MIN_SPLIT_SILENCE_MS: i64 = 250;

/// How quiet, relative to the speech around it, a stretch has to be to count as
/// silence.
///
/// Measured against the turn's own loudness rather than an absolute level, so
/// the same rule works on a close mic and across a room.
const SILENCE_RATIO: f32 = 0.15;

/// Frame size for the silence scan. Fine enough to place a boundary accurately,
/// coarse enough that one quiet glottal stop is not a pause.
const SILENCE_FRAME_MS: i64 = 20;

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

    /// Build the per-window diarizer.
    ///
    /// It never takes a speaker count: a window is a slice of the recording,
    /// and how many people are in the room is not a fact about a slice. sherpa
    /// discovers whatever voices the window holds, by threshold, and
    /// `cluster_fragments` settles the recording's speaker set afterwards.
    fn build_diarizer(&self) -> Result<OfflineSpeakerDiarization> {
        let provider = provider_for(&self.device);
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
        self.diarize_windowed(&audio, expected_speakers)
    }
}

impl SherpaDiarizer {
    /// Diarize in windows, then decide the recording's speaker set once, over
    /// every window at once.
    ///
    /// Handing sherpa a multi-hour clip in one `process` call overruns its stack
    /// and aborts the process (Windows `0xc0000409`), so long audio has to be
    /// windowed. Each window is clustered independently, which means window 2's
    /// "speaker 0" is unrelated to window 1's — identity is resolved afterwards,
    /// by voice, across the whole recording.
    ///
    /// Short recordings take the same path as one window rather than a
    /// shortcut through sherpa's own answer. They used to be trusted directly,
    /// which left the commonest recording of all — a meeting under ten minutes —
    /// as the only one with no repair for sherpa's over-segmentation, and made
    /// a recording's speaker count depend on which side of ten minutes it fell.
    fn diarize_windowed(&self, audio: &WavData, expected: Option<i32>) -> Result<Diarization> {
        let sr = audio.sample_rate;
        let window = match ((DIARIZE_WINDOW_MS * sr as i64) / 1000) as usize {
            // A sample rate so low the window rounds to nothing: one window.
            0 => audio.samples.len().max(1),
            w => w,
        };

        // Each window is diarized without a target count, however many people
        // the recording as a whole has.
        //
        // Pinning `num_clusters` to the stated count per window forced every
        // window to produce exactly that many voices — so a stretch where only
        // one person talks came back split into four arbitrary pieces of that
        // one voice, and a stretch where a fifth person joined had someone
        // folded in with someone else. Those splits are what the clustering
        // below then worked from, which is why a stated count of four could
        // still land on four clusters that were not four people. The count
        // belongs to the recording, not to the window, so it is applied once,
        // globally, in `cluster_fragments`. Over-segmentation here is the cheap
        // error: clustering merges takes of one voice back together, but it can
        // never split two people a window has already merged.
        let diarizer = self.build_diarizer()?;
        let extractor = build_extractor(&self.paths, &self.device, self.num_threads)?;

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

            // Split at any pause long enough to be a handover, and treat each
            // piece as its own voice. Grouping by the segmentation model's own
            // speaker label instead makes its mistakes permanent: two similar
            // voices merged into one turn embed to a blend of the two, and no
            // amount of clustering afterwards can take them apart again.
            let pieces = split_turns_at_silence(&local_turns, slice, sr);
            let piece_embs = compute_speaker_embeddings(&extractor, slice, sr, &pieces)?;

            for piece in &pieces {
                fragments.push(Fragment {
                    turns: vec![SpeakerTurn {
                        local_idx: piece.local_idx,
                        start_ms: piece.start_ms + offset_ms,
                        end_ms: piece.end_ms + offset_ms,
                    }],
                    embedding: piece_embs.get(&piece.local_idx).cloned(),
                    speech_ms: (piece.end_ms - piece.start_ms).max(0),
                });
            }

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
            "diarize complete"
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

/// Total speech a cluster must hold, across the whole recording, to stand as a
/// participant when the count was not stated.
///
/// Speaker-embedding models need about a second of speech to say anything, and
/// a second or two split across a recording is not a person — it is the residue
/// of splitting turns finely enough to catch a handover. Someone real, however
/// quiet, clears this; a sliver left over from over-segmentation does not.
///
/// A stated count is a count of people who clear this, not of clusters — see
/// the stop condition in `cluster_fragments`.
const MIN_SPEAKER_SPEECH_MS: i64 = 3_000;

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

    // Pairwise similarity over the embeddable fragments, computed once.
    //
    // The merge loop used to recompute average linkage from the raw embeddings
    // on every iteration, which is O(n^3) 192-dimensional dot products - fine
    // for the handful of fragments a window-pinned diarizer produced, hopeless
    // now that each window reports however many voices it actually heard. With
    // the matrix in hand a merge only ever touches scalars.
    let n = embedded.len();
    let mut sim = vec![0.0f32; n * n];
    for a in 0..n {
        let Some(ea) = fragments[embedded[a]].embedding.as_ref() else {
            continue;
        };
        for b in (a + 1)..n {
            let Some(eb) = fragments[embedded[b]].embedding.as_ref() else {
                continue;
            };
            let s = cosine(ea, eb);
            sim[a * n + b] = s;
            sim[b * n + a] = s;
        }
    }

    let target = expected.filter(|v| *v >= 1).map(|v| (v as usize).min(n));

    // Every embedded fragment starts as its own cluster. `alive[i]` marks a
    // cluster that has not been absorbed; `size[i]` counts its members.
    let mut alive = vec![true; n];
    let mut size = vec![1.0f32; n];
    // Speech behind each cluster, so the stated count can be read as a number of
    // participants rather than a number of clusters.
    let mut speech: Vec<i64> = embedded
        .iter()
        .map(|&i| fragments[i].speech_ms.max(0))
        .collect();
    let mut live = n;

    // The merge sequence: clusters before the merge, what it cost, and which
    // pair joined. Recording the pair rather than a snapshot of the whole
    // partition keeps this linear in the number of merges.
    let mut history: Vec<(usize, f32, (usize, usize))> = Vec::new();
    // How many clusters hold enough speech to be somebody, after each merge.
    // `substantial[k]` is that count once `k` merges have been applied — which
    // is how a stated number of *people* is located in a sequence of merges
    // between *clusters*.
    let count_substantial = |alive: &[bool], speech: &[i64]| -> usize {
        (0..alive.len())
            .filter(|&k| alive[k] && speech[k] >= MIN_SPEAKER_SPEECH_MS)
            .count()
    };
    let mut substantial: Vec<usize> = vec![count_substantial(&alive, &speech)];

    while live > 1 {
        let mut best: Option<(usize, usize, f32)> = None;
        for i in 0..n {
            if !alive[i] {
                continue;
            }
            for j in (i + 1)..n {
                if !alive[j] {
                    continue;
                }
                let s = sim[i * n + j];
                if best.map(|(_, _, b)| s > b).unwrap_or(true) {
                    best = Some((i, j, s));
                }
            }
        }
        let Some((i, j, s)) = best else { break };
        history.push((live, s, (i, j)));

        // Lance-Williams update for average linkage: the merged cluster's
        // similarity to every other is the size-weighted mean of the two
        // originals', which is exactly the mean over member pairs the old code
        // recomputed by hand. Still average linkage, still no running centroid -
        // averaging centroids as you merge lets the largest cluster drift toward
        // the mean of everything and then swallow the rest, which is how a
        // four-person meeting collapsed into one speaker holding 99.8% of the
        // speech.
        let (wi, wj) = (size[i], size[j]);
        for k in 0..n {
            if !alive[k] || k == i || k == j {
                continue;
            }
            let merged = (sim[i * n + k] * wi + sim[j * n + k] * wj) / (wi + wj);
            sim[i * n + k] = merged;
            sim[k * n + i] = merged;
        }
        size[i] = wi + wj;
        speech[i] += speech[j];
        alive[j] = false;
        live -= 1;
        substantial.push(count_substantial(&alive, &speech));
    }

    if std::env::var("SCRIBE_DIARIZE_MERGES").is_ok() {
        eprintln!("-- merge sequence ({n} fragments) --");
        for (count, sim, _) in &history {
            eprintln!("   {count:>3} clusters -> {:<3}  sim {sim:.4}", count - 1);
        }
    }

    // Where to stop. The recording's own merge sequence decides, and a stated
    // count picks a point in that sequence instead.
    //
    // The count used to halt the merge the moment it was reached, which stopped
    // it mid-sort: on a five-person meeting one speaker was still split in two
    // while two others had already been joined, and the answer came out worse
    // than taking no hint at all. Merging now always runs to completion, and the
    // count selects the *last* point at which that many people were present —
    // the most-merged partition holding the stated number, rather than the first
    // partition to stumble into it.
    let cut = {
        let discovered = choose_cut(&history);
        match target {
            None => discovered,
            // No point holds the stated number: the audio does not support it,
            // and what the recording itself says is the better answer.
            Some(t) => (0..substantial.len())
                .rev()
                .find(|&k| substantial[k] == t)
                .unwrap_or(discovered),
        }
    };
    let final_clusters = replay(n, &history, cut, &embedded);

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

    fold_slight_speakers(fragments, &mut assignment);
    assignment
}

/// Fold clusters holding too little speech to be a participant into the voice
/// they most resemble.
///
/// Where the merge sequence has a clean step, `choose_cut` finds it. Where it
/// does not — two people whose voices are genuinely alike, so that joining them
/// looks much like joining two stretches of one of them — it stops early and
/// leaves slivers standing beside the real speakers. They are easy to tell apart
/// afterwards even though they were not during merging: a participant holds a
/// share of the conversation, and a sliver holds a second or two.
///
/// Each is folded into the surviving cluster whose voice it is closest to, not
/// the nearest in time: a sliver is usually a fragment of somebody already
/// present, and its embedding says which.
fn fold_slight_speakers(fragments: &[Fragment], assignment: &mut [i32]) {
    let mut speech: HashMap<i32, i64> = HashMap::new();
    for (frag, &cluster) in fragments.iter().zip(assignment.iter()) {
        *speech.entry(cluster).or_insert(0) += frag.speech_ms.max(0);
    }

    let surviving: Vec<i32> = speech
        .iter()
        .filter(|(_, ms)| **ms >= MIN_SPEAKER_SPEECH_MS)
        .map(|(c, _)| *c)
        .collect();
    // Everything is slight - a very short recording. Nothing to fold into.
    if surviving.is_empty() || surviving.len() == speech.len() {
        return;
    }

    // A voice per surviving cluster, weighted by how much speech backs it.
    let mut centroids: HashMap<i32, (Vec<f32>, f32)> = HashMap::new();
    for (frag, &cluster) in fragments.iter().zip(assignment.iter()) {
        let Some(emb) = &frag.embedding else { continue };
        if !surviving.contains(&cluster) {
            continue;
        }
        let weight = (frag.speech_ms.max(1) as f32) / 1000.0;
        let entry = centroids
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
    for (sum, weight) in centroids.values_mut() {
        if *weight > 0.0 {
            for x in sum.iter_mut() {
                *x /= *weight;
            }
        }
        l2_normalize(sum);
    }

    let mut folded = 0usize;
    for i in 0..fragments.len() {
        if surviving.contains(&assignment[i]) {
            continue;
        }
        let best = fragments[i].embedding.as_ref().and_then(|emb| {
            let mut ranked: Vec<(i32, f32)> = centroids
                .iter()
                .map(|(c, (centroid, _))| (*c, cosine(centroid, emb)))
                .collect();
            // Ties break on the cluster index, never on hash order.
            ranked.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.0.cmp(&b.0))
            });
            ranked.first().map(|(c, _)| *c)
        });
        assignment[i] = best.unwrap_or_else(|| *surviving.iter().min().unwrap());
        folded += 1;
    }

    // Renumber to a contiguous 0..k-1, so a speaker index is still a count.
    let mut seen: Vec<i32> = assignment.to_vec();
    seen.sort_unstable();
    seen.dedup();
    let renumber: HashMap<i32, i32> = seen
        .iter()
        .enumerate()
        .map(|(new, old)| (*old, new as i32))
        .collect();
    for a in assignment.iter_mut() {
        *a = renumber[a];
    }

    if folded > 0 {
        tracing::debug!(
            folded,
            speakers = seen.len(),
            "diarize: folded slight clusters into the voices they resemble"
        );
    }
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
///
/// Returns how many merges to keep.
fn choose_cut(history: &[(usize, f32, (usize, usize))]) -> usize {
    let mut accepted: Vec<f32> = Vec::new();
    for (m, (count, sim, _)) in history.iter().enumerate() {
        // Above the sanity bound, keep merging whatever it looks like: that
        // many clusters is over-segmentation, not a room full of people.
        if *count <= MAX_INFERRED_SPEAKERS {
            // With nothing accepted yet there is no family to compare against;
            // a fragment is perfectly similar to itself, so use 1.0.
            let within = median(&accepted).unwrap_or(1.0);
            if *sim < RELATIVE_DROP * within {
                return m;
            }
        }
        accepted.push(*sim);
    }
    history.len()
}

/// Rebuild the partition that holding `cut` merges of `history` produces,
/// mapping cluster members back to fragment indices through `embedded`.
fn replay(
    n: usize,
    history: &[(usize, f32, (usize, usize))],
    cut: usize,
    embedded: &[usize],
) -> Vec<Vec<usize>> {
    let mut members: Vec<Vec<usize>> = (0..n).map(|i| vec![embedded[i]]).collect();
    let mut alive = vec![true; n];
    for &(_, _, (i, j)) in history.iter().take(cut) {
        let absorbed = std::mem::take(&mut members[j]);
        members[i].extend(absorbed);
        alive[j] = false;
    }
    (0..n)
        .filter(|&i| alive[i])
        .map(|i| std::mem::take(&mut members[i]))
        .collect()
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

    let chosen = choose_turns_to_embed(turns);

    for turn in chosen {
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

/// Which of each speaker's turns to actually embed.
///
/// Longest first, up to [`EMBED_BUDGET_MS`] per speaker: the extractor's cost is
/// linear in the audio fed to it, and a speaker's identity stops sharpening long
/// before their speech runs out. Turns too short to embed reliably are skipped
/// when the speaker has better audio elsewhere, and used when that is all they
/// said — better a noisy voiceprint than none.
fn choose_turns_to_embed(turns: &[SpeakerTurn]) -> Vec<&SpeakerTurn> {
    let duration = |t: &SpeakerTurn| (t.end_ms - t.start_ms).max(0);

    let mut by_speaker: HashMap<i32, Vec<&SpeakerTurn>> = HashMap::new();
    for turn in turns {
        by_speaker.entry(turn.local_idx).or_default().push(turn);
    }

    // Speakers in index order, so the result does not depend on hash order.
    let mut speakers: Vec<i32> = by_speaker.keys().copied().collect();
    speakers.sort_unstable();

    let mut chosen: Vec<&SpeakerTurn> = Vec::new();
    for idx in speakers {
        let mut speaker_turns = by_speaker.remove(&idx).unwrap_or_default();
        speaker_turns.sort_by_key(|t| (std::cmp::Reverse(duration(t)), t.start_ms));
        let has_long = speaker_turns
            .first()
            .is_some_and(|t| duration(t) >= MIN_EMBED_MS);

        let mut spent = 0i64;
        for turn in speaker_turns {
            if has_long && duration(turn) < MIN_EMBED_MS {
                break;
            }
            chosen.push(turn);
            spent += duration(turn);
            if spent >= EMBED_BUDGET_MS {
                break;
            }
        }
    }
    chosen
}

/// Split every turn at any silence inside it long enough to be a handover.
///
/// The segmentation model decides where speech starts and stops, and it is
/// wrong about that in a specific, repeatable way: two people whose voices are
/// alike, one following the other across a short pause, come back as a single
/// turn attributed to whichever of them it preferred. Nothing downstream can
/// undo that — the turn is one unit by then, it embeds to a blend of two
/// people, and both the clustering and the transcript inherit the mistake.
///
/// But the pause is in the audio, whatever the model made of it. Splitting there
/// costs nothing when the model was right: a pause inside one person's speech
/// yields two pieces of that person, which the clustering puts straight back
/// together. It is the same trade as everywhere else here — over-segmentation is
/// repairable and a merge is not.
///
/// Each piece is numbered uniquely, so a piece is embedded and clustered on its
/// own rather than inheriting the segmentation model's opinion of who spoke.
fn split_turns_at_silence(turns: &[SpeakerTurn], samples: &[f32], sample_rate: u32) -> Vec<SpeakerTurn> {
    let sr = sample_rate as i64;
    let frame = ((SILENCE_FRAME_MS * sr) / 1000).max(1) as usize;
    let min_run = (MIN_SPLIT_SILENCE_MS / SILENCE_FRAME_MS).max(1) as usize;

    let mut out = Vec::with_capacity(turns.len());
    let mut next_idx = 0i32;
    let mut emit = |start_ms: i64, end_ms: i64, out: &mut Vec<SpeakerTurn>| {
        if end_ms > start_ms {
            out.push(SpeakerTurn { local_idx: next_idx, start_ms, end_ms });
            next_idx += 1;
        }
    };

    for turn in turns {
        let start = ((turn.start_ms.max(0) * sr) / 1000) as usize;
        let end = (((turn.end_ms.max(turn.start_ms)) * sr) / 1000) as usize;
        let end = end.min(samples.len());
        if end <= start || end - start < frame * 2 {
            emit(turn.start_ms, turn.end_ms, &mut out);
            continue;
        }

        // Frame energies, and the turn's own speech level to judge them against.
        let energies: Vec<f32> = samples[start..end]
            .chunks(frame)
            .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
            .collect();
        let mean = energies.iter().sum::<f32>() / energies.len() as f32;
        let threshold = mean * SILENCE_RATIO;

        // Runs of quiet frames long enough to be a handover; the boundary goes
        // in the middle of each, so neither side carries the other's silence.
        let mut cut_ms: Vec<i64> = Vec::new();
        let mut run_start: Option<usize> = None;
        for i in 0..=energies.len() {
            let quiet = i < energies.len() && energies[i] <= threshold;
            match (quiet, run_start) {
                (true, None) => run_start = Some(i),
                (false, Some(from)) => {
                    if i - from >= min_run {
                        let mid = from + (i - from) / 2;
                        cut_ms.push(turn.start_ms + mid as i64 * SILENCE_FRAME_MS);
                    }
                    run_start = None;
                }
                _ => {}
            }
        }

        let mut piece_start = turn.start_ms;
        for cut in cut_ms {
            if cut > piece_start {
                emit(piece_start, cut, &mut out);
                piece_start = cut;
            }
        }
        emit(piece_start, turn.end_ms, &mut out);
    }
    out
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

    /// A window that heard only one voice, over-segmented into pieces, must not
    /// leave those pieces standing as separate speakers.
    ///
    /// This is the shape of the bug that made a stated speaker count worse than
    /// no count at all. Every window was clustered to the stated count, so a
    /// stretch where only Alice talks was forced to yield two "speakers" —
    /// two arbitrary halves of Alice. Windows now discover their own voices and
    /// the count is applied once, here, so the halves rejoin and the two real
    /// people come out as two people.
    #[test]
    fn over_segmented_pieces_of_one_voice_rejoin() {
        let alice = [1.0, 0.05, 0.0];
        let bob = [0.0, 1.0, 0.05];
        let fragments = vec![
            // Window 1: Alice only, split into two by an over-eager clusterer.
            frag(Some(&alice), 0, 30_000),
            frag(Some(&[1.0, 0.10, 0.0]), 30_000, 60_000),
            // Window 2: Alice and Bob, likewise split.
            frag(Some(&[0.98, 0.0, 0.0]), 60_000, 90_000),
            frag(Some(&bob), 90_000, 120_000),
            frag(Some(&[0.0, 0.97, 0.10]), 120_000, 150_000),
        ];

        let groups = partition(&cluster_fragments(&fragments, Some(2)));
        assert_eq!(groups, vec![vec![0, 1, 2], vec![3, 4]], "groups = {groups:?}");
    }

    /// Clustering has to stay cheap enough that letting each window report
    /// every voice it heard is affordable.
    ///
    /// Windows used to be pinned to the speaker count, which capped the
    /// fragment count at a handful. They are not any more, so a long recording
    /// arrives here with hundreds of fragments — and the old merge loop
    /// recomputed average linkage from the raw embeddings on every iteration,
    /// which at this size is billions of floating-point operations. A debug
    /// build finishing this comfortably is the guard.
    #[test]
    fn clustering_scales_to_a_long_recordings_fragments() {
        let mut fragments = Vec::new();
        for i in 0..300 {
            // Six voices, each nudged slightly per fragment so nothing is exactly
            // equal and the merge order has real work to do.
            let voice = i % 6;
            let mut emb = vec![0.0f32; 6];
            emb[voice] = 1.0;
            emb[(voice + 1) % 6] = 0.02 * (i / 6) as f32;
            let t = i as i64 * 10_000;
            fragments.push(frag(Some(&emb), t, t + 9_000));
        }

        let started = std::time::Instant::now();
        let groups = partition(&cluster_fragments(&fragments, Some(6)));
        let elapsed = started.elapsed();

        assert_eq!(groups.len(), 6);
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "clustering 300 fragments took {elapsed:?}"
        );
    }

    /// A pause long enough to be a handover splits the turn, so two people the
    /// segmentation model ran together can still be told apart afterwards.
    #[test]
    fn a_turn_is_split_at_a_pause() {
        let sr = 16_000u32;
        // 1s tone, 400 ms silence, 1s tone — one "turn" holding two stretches.
        let mut samples = vec![0.0f32; 0];
        for i in 0..sr as usize {
            samples.push(((i as f32 / 40.0).sin()) * 0.3);
        }
        samples.extend(std::iter::repeat(0.0).take((sr as usize * 400) / 1000));
        for i in 0..sr as usize {
            samples.push(((i as f32 / 40.0).sin()) * 0.3);
        }

        let pieces = split_turns_at_silence(&[turn(0, 0, 2_400)], &samples, sr);
        assert_eq!(pieces.len(), 2, "pieces = {pieces:?}");
        assert_ne!(pieces[0].local_idx, pieces[1].local_idx, "numbered separately");
        // The boundary lands inside the silence, not at either edge of it.
        assert!(
            (1_000..=1_400).contains(&pieces[0].end_ms),
            "cut at {}",
            pieces[0].end_ms
        );
    }

    #[test]
    fn continuous_speech_is_left_whole() {
        let sr = 16_000u32;
        let samples: Vec<f32> = (0..sr as usize * 2)
            .map(|i| ((i as f32 / 40.0).sin()) * 0.3)
            .collect();
        let pieces = split_turns_at_silence(&[turn(0, 0, 2_000)], &samples, sr);
        assert_eq!(pieces.len(), 1);
    }

    #[test]
    fn a_brief_gap_is_punctuation_not_a_handover() {
        let sr = 16_000u32;
        let mut samples: Vec<f32> = (0..sr as usize)
            .map(|i| ((i as f32 / 40.0).sin()) * 0.3)
            .collect();
        // 100 ms — under MIN_SPLIT_SILENCE_MS.
        samples.extend(std::iter::repeat(0.0).take((sr as usize * 100) / 1000));
        samples.extend((0..sr as usize).map(|i| ((i as f32 / 40.0).sin()) * 0.3));

        let pieces = split_turns_at_silence(&[turn(0, 0, 2_100)], &samples, sr);
        assert_eq!(pieces.len(), 1, "pieces = {pieces:?}");
    }

    /// Splitting finely enough to catch a handover leaves slivers. A second of
    /// speech across a whole recording is not a participant.
    #[test]
    fn a_sliver_is_folded_into_the_voice_it_resembles() {
        let alice = [1.0, 0.02, 0.0];
        let bob = [0.0, 1.0, 0.02];
        let mut fragments = vec![
            frag(Some(&alice), 0, 20_000),
            frag(Some(&bob), 20_000, 40_000),
        ];
        // A 0.5 s scrap of Alice, well under the floor to stand alone.
        fragments.push(frag(Some(&[0.99, 0.10, 0.0]), 40_000, 40_500));

        let assignment = cluster_fragments(&fragments, None);
        let groups = partition(&assignment);
        assert_eq!(groups.len(), 2, "groups = {groups:?}");
        assert_eq!(
            assignment[2], assignment[0],
            "the scrap joins Alice, not Bob"
        );
    }

    /// Speaker indices must stay a contiguous 0..k-1 after folding, or a count
    /// read off the maximum index is wrong.
    #[test]
    fn folding_leaves_contiguous_speaker_numbers() {
        let mut fragments = vec![
            frag(Some(&[1.0, 0.0, 0.0]), 0, 20_000),
            frag(Some(&[0.0, 1.0, 0.0]), 20_000, 40_000),
        ];
        for i in 0..4 {
            let t = 40_000 + i * 1_000;
            fragments.push(frag(Some(&[0.0, 0.0, 1.0]), t, t + 400));
        }
        let assignment = cluster_fragments(&fragments, None);
        let mut seen: Vec<i32> = assignment.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, (0..seen.len() as i32).collect::<Vec<_>>(), "seen = {seen:?}");
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

    /// A turn inside the limit must go to the model whole — splitting audio
    /// that did not need splitting would only blur the embedding.
    /// Embedding cost is linear in audio, so a speaker who talks all meeting
    /// must not have all of it pushed through the extractor.
    #[test]
    fn a_talkative_speaker_is_embedded_up_to_the_budget() {
        let turns: Vec<SpeakerTurn> = (0..40)
            .map(|i| turn(0, i * 30_000, i * 30_000 + 20_000))
            .collect();

        let chosen = choose_turns_to_embed(&turns);
        let total: i64 = chosen.iter().map(|t| t.end_ms - t.start_ms).sum();

        assert!(total >= EMBED_BUDGET_MS, "budget not met: {total}");
        // One turn of slack past the budget, not thirteen minutes of it.
        assert!(total < EMBED_BUDGET_MS + 20_000, "far over budget: {total}");
    }

    #[test]
    fn the_longest_turns_are_the_ones_embedded() {
        let turns = vec![
            turn(0, 0, 2_000),
            turn(0, 10_000, 55_000),
            turn(0, 60_000, 63_000),
        ];
        let chosen = choose_turns_to_embed(&turns);
        assert_eq!(chosen[0].start_ms, 10_000, "longest first");
    }

    #[test]
    fn every_speaker_gets_their_own_budget() {
        let mut turns: Vec<SpeakerTurn> = Vec::new();
        for speaker in 0..3 {
            for i in 0..20 {
                let t = (speaker as i64 * 1_000_000) + i * 30_000;
                turns.push(turn(speaker, t, t + 20_000));
            }
        }
        let chosen = choose_turns_to_embed(&turns);
        for speaker in 0..3 {
            let total: i64 = chosen
                .iter()
                .filter(|t| t.local_idx == speaker)
                .map(|t| t.end_ms - t.start_ms)
                .sum();
            assert!(total >= EMBED_BUDGET_MS, "speaker {speaker} short: {total}");
        }
    }

    /// A speaker with nothing but scraps still needs a voiceprint.
    #[test]
    fn a_speaker_with_only_short_turns_is_still_embedded() {
        let turns = vec![turn(0, 0, 400), turn(0, 1_000, 1_300)];
        let chosen = choose_turns_to_embed(&turns);
        assert_eq!(chosen.len(), 2);
    }

    /// But a scrap is ignored when the same speaker has real speech elsewhere.
    #[test]
    fn scraps_are_dropped_when_the_speaker_has_better_audio() {
        let turns = vec![turn(0, 0, 200), turn(0, 1_000, 6_000)];
        let chosen = choose_turns_to_embed(&turns);
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].start_ms, 1_000);
    }

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
