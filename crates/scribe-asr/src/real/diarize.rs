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

/// How readily the segmentation model's own clustering joins two stretches
/// within a window.
///
/// This looks inert, because the speaker labels it produces are thrown away —
/// every piece is renumbered and clustered again across the whole recording.
/// It is not: how sherpa clusters changes the segments it emits, and those are
/// kept. Measured, moving it from 0.5 to 0.8 takes a six-voice degraded
/// recording from 94.4% to 95.7% and a recording with somebody moving about
/// from 99.4% to 99.7%, with nothing worse anywhere.
///
/// Higher means it merges less, which is what this code wants for the same
/// reason it discovers each window's voices without a target count: finer
/// segments are repairable downstream and merged ones are not.
///
/// 0.8 rather than 0.9 because 0.9 costs a recording with a television playing
/// in it, and rather than 0.7 because most of the gain is above it. The
/// response is not smooth — 0.6 returns eight speakers where 0.5 and 0.7 both
/// return six — so 0.8 sits in the middle of the stable region, not on an edge.
const CLUSTER_THRESHOLD: f32 = 0.8;

/// How loose the loosest cluster may be, against the median cluster, before the
/// cut is taken to have stopped a merge too late. See the step-back in
/// `cluster_fragments`.
const COHESION_RATIO: f32 = 0.73;

/// Shortest stretch sherpa will call speech, and shortest silence it will split
/// a segment at. These are sherpa's own defaults, carried explicitly so they are
/// measured rather than inherited.
///
/// Swept once against fixtures that repeated each sentence four times, these
/// looked inert, and this comment said so. On fixtures that do not repeat
/// themselves `min_duration_off` is a sharp optimum rather than a flat one, and
/// 0.5 — which is also sherpa's default — is the peak:
///
///   min_duration_off   0.05          0.2           0.5           0.7
///   8 voices, reverb   7 spk 86.6%   7 spk 86.4%   8 spk 98.6%   7 spk 86.7%
///   6 voices, reverb   7 spk 94.2%   6 spk 98.1%   6 spk 98.1%   6 spk 98.1%
///
/// Being carried explicitly rather than inherited is what matters here: the
/// right value happens to be the upstream default, so nothing needed changing,
/// but a future sherpa release moving it would cost a speaker on an eight-voice
/// recording and nothing would say why.
///
/// `MIN_DURATION_ON` really is inert, measured the same way: 0.1 through 0.6
/// are identical on every fixture, and it is live (5.0 collapses six voices to
/// four at 53.3%). The split and the participant floor downstream are stricter
/// than it is, so it never binds.
const MIN_DURATION_ON: f32 = 0.3;
const MIN_DURATION_OFF: f32 = 0.5;

/// How many merges the step-back may undo. Two people merging into one cluster
/// costs one step; the worst measured recording needed two.
const MAX_COHESION_STEPS: usize = 4;

/// Below this many clusters there is no median worth comparing against.
const MIN_CLUSTERS_TO_JUDGE: usize = 3;

/// Diarize at most this much audio in one `process` call.
///
/// sherpa's diarization holds the whole clip's segments and embeddings in
/// native memory and overruns its stack on very long input — a 2h49m recording
/// aborted the worker with `0xc0000409` (STATUS_STACK_BUFFER_OVERRUN) after
/// ~40 minutes of work. Ten minutes is comfortably inside the range that has
/// run reliably, and is still long enough for clustering to separate voices
/// within a window; identity is then carried across windows by embedding.
const DIARIZE_WINDOW_MS: i64 = 10 * 60 * 1000;

/// Longest slice handed to the embedding extractor in one call.
///
/// A guard against a length limit baked into an exported model. TitaNet-large,
/// which this used to install, carries one in its masked convolutions: past
/// roughly two minutes of audio the mask and the feature map disagree, and
/// onnxruntime throws from `mconv`'s `Where` node
/// (`broadcast an axis by a dimension other than 1. 12288 by 14531`). That is a
/// C++ exception crossing the FFI boundary, which Rust cannot catch — it aborts
/// the whole worker, taking the job's lease and every other queued job with it.
/// A 9-minute single-speaker recording did exactly that: diarization merged the
/// monologue into one turn far over the limit.
///
/// 30 s is well inside that limit and is already more speech than any of these
/// models use — they are trained on a few seconds — so nothing is lost by
/// splitting, and the guard costs nothing on a model that would not have needed
/// it.
/// A longer turn is embedded in pieces and averaged, weighted by duration, so
/// the result is what embedding the whole turn was meant to produce anyway.
const MAX_EMBED_MS: i64 = 30_000;

/// Most audio fed to the extractor for any one piece of speech.
///
/// A speaker embedding is an identity, not a summary: these models are trained
/// on a few seconds and stop improving well before a minute, while the cost of
/// running one is linear in the audio handed over. Nothing is gained by pushing
/// a five-minute monologue through in full to produce one 192-dimensional
/// vector.
///
/// This was a budget per *speaker* when a speaker was the unit being embedded.
/// The unit is now a single stretch of speech between two pauses, so the budget
/// follows it. It rarely binds — conversation is full of pauses, and turns are
/// split at every one long enough to be a handover — and exists for the piece
/// that has none.
const EMBED_BUDGET_MS: i64 = 60_000;

/// Silence long enough to be a possible speaker change.
///
/// People do not swap places mid-breath; a handover has a pause in it. This was
/// a quarter of a second on that reasoning, and a quarter of a second is what a
/// handover sounds like in a quiet room. In a reverberant one the tail of the
/// outgoing speaker eats the front of the gap, so the quiet part is shorter
/// than the pause actually was, and requiring 250 ms of it misses the handover
/// entirely — the two speakers are embedded together and become one person.
///
/// It could not be shortened while a cut was allowed to leave a sliver behind:
/// at 160 ms an eleven-minute recording came back as one speaker at 18.1%, the
/// turns cut into pieces too short to embed. With `MIN_PIECE_MS` refusing those
/// cuts, 160 ms is where the numbers are best, and the collapse is gone.
///
/// Measured on a six-voice recording in a bad room, at 10 dB SNR: 87.2% with a
/// speaker lost at 250 ms, 95.7% and the right count at 160. The working band
/// is 140–170 ms; at 180 a noisy recording starts inventing a seventh speaker
/// and at 120 the long recording loses one.
const MIN_SPLIT_SILENCE_MS_DEFAULT: i64 = 160;

/// Experiment hooks for the split, which was chosen by judgement and never
/// measured. See docs/measuring-diarization.md.
fn min_split_silence_ms() -> i64 {
    std::env::var("SCRIBE_SPLIT_SILENCE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MIN_SPLIT_SILENCE_MS_DEFAULT)
}

fn min_piece_ms() -> i64 {
    std::env::var("SCRIBE_MIN_PIECE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MIN_PIECE_MS)
}

fn env_f32_or(key: &str, default: f32) -> f32 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn floor_alpha() -> f32 {
    std::env::var("SCRIBE_FLOOR_ALPHA")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(FLOOR_ALPHA)
}

fn silence_ratio() -> f32 {
    std::env::var("SCRIBE_SILENCE_RATIO")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(SILENCE_RATIO_DEFAULT)
}

/// How quiet, relative to the speech around it, a stretch has to be to count as
/// silence.
///
/// Measured against the turn's own loudness rather than an absolute level, so
/// the same rule works on a close mic and across a room.
const SILENCE_RATIO_DEFAULT: f32 = 0.15;

/// Frame size for the silence scan. Fine enough to place a boundary accurately,
/// coarse enough that one quiet glottal stop is not a pause.
const SILENCE_FRAME_MS: i64 = 20;

/// Where to put the silence threshold between the quietest frame in a turn and
/// the speech in it, as a fraction of the distance in dB. 0 sits on the noise
/// floor, 1 sits on the speech.
///
/// `SILENCE_RATIO_DEFAULT` alone measures silence against the turn's mean,
/// which assumes the gaps are near-silent. True of a clean recording, false of
/// a real room: pink noise at 15 dB SNR puts the floor at 0.178 of the speech
/// level, above a threshold of 0.15, so no frame is ever quiet and no turn is
/// ever split. Two people who spoke one after the other are then embedded as
/// one voice and cluster as one person.
///
/// That was not an edge case. On a six-voice fixture in a reverberant room it
/// cost a whole speaker every time — 5 found instead of 6, 83.2% of words to
/// the right person — and the same pair merged at every noise level. Reading
/// the floor off the audio instead of assuming it:
///
///   snr/reverb   measured floor   assumed floor
///   25dB / 0.2   6 spk  99.8%     6 spk  98.1%
///   20dB / 0.3   6 spk  99.6%     6 spk  91.7%
///   15dB / 0.4   6 spk  97.4%     5 spk  83.2%
///   10dB / 0.5   6 spk  87.2%     6 spk  81.2%
///    5dB / 0.6   6 spk  82.2%     6 spk  73.9%
///
/// Clean recordings are untouched, by construction: their quietest frame is
/// near zero, so the adaptive threshold lands below the mean-relative one and
/// `max` keeps the old behaviour. Measured across every other fixture, nothing
/// moved by more than 0.1 of a point.
///
/// 0.35 is the middle of the working band. Below 0.3 the handover is missed
/// again; at 0.45 turns are cut into pieces too short to embed and the score
/// falls back to where it started.
const FLOOR_ALPHA: f32 = 0.35;

/// Shortest piece a split may leave behind.
///
/// A cut that leaves a sliver is worse than no cut at all. The piece is too
/// short to embed well, the embedding lands it on whoever it happens to
/// resemble, and enough of them drag real speakers together — an eleven-minute
/// degraded recording came back with 3 speakers at 75.9% instead of 4 at 99.5%.
///
/// Refusing those cuts is what makes a shorter `MIN_SPLIT_SILENCE_MS_DEFAULT`
/// usable, and it is the shorter split that finds the handover in a room with
/// reverb in it. On its own, at the old 250 ms split, this changes nothing.
const MIN_PIECE_MS: i64 = 400;

/// Where the speech level is read from — high enough to sit inside the speech
/// rather than on a trailing syllable.
const SPEECH_PERCENTILE: f32 = 0.85;

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
            threshold: std::env::var("SCRIBE_CLUSTER_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(CLUSTER_THRESHOLD),
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
            min_duration_on: env_f32_or("SCRIBE_MIN_DURATION_ON", MIN_DURATION_ON),
            min_duration_off: env_f32_or("SCRIBE_MIN_DURATION_OFF", MIN_DURATION_OFF),
        };

        OfflineSpeakerDiarization::create(&config)
            .ok_or_else(|| Error::Model("failed to create OfflineSpeakerDiarization".into()))
    }
}

impl Diarizer for SherpaDiarizer {
    fn diarize(&self, wav_path: &Path, expected_speakers: Option<i32>) -> Result<Diarization> {
        let audio = wav::read_wav(wav_path)?;

        // Somebody dictating has told us there is one voice, and segmentation
        // and clustering have nothing left to decide. Running them anyway is
        // not merely wasted work: the pyannote segmentation model reads out of
        // bounds on some single-speaker recordings and takes the process down
        // with SIGBUS, which in the worker kills every job in flight. Measured
        // over eighteen one-voice recordings in reverberant, noisy rooms, four
        // crashed; over ten multi-speaker ones, none did.
        //
        // Only when the count was *stated*. Discovering one voice is a weaker
        // claim than being told there is one, and this must never swallow a
        // recording that turns out to hold two.
        if expected_speakers == Some(1) {
            return self.single_speaker(&audio);
        }

        self.diarize_windowed(&audio, expected_speakers)
    }
}

impl SherpaDiarizer {
    /// The whole recording as one speaker, without asking the segmentation
    /// model anything.
    ///
    /// Turns come from the same silence split the normal path uses, so the
    /// merge stage still breaks utterances where the speaker stopped talking
    /// and a word timed into silence is still visible as one. The embedding is
    /// still computed — that is what a voiceprint match needs, and it is the
    /// segmentation model that crashes, not the extractor — so a dictation can
    /// still be named from an enrolled voice.
    fn single_speaker(&self, audio: &WavData) -> Result<Diarization> {
        let whole = SpeakerTurn { local_idx: 0, start_ms: 0, end_ms: audio.duration_ms() };
        let mut turns = split_turns_at_silence(&[whole], &audio.samples, audio.sample_rate);
        for t in turns.iter_mut() {
            t.local_idx = 0;
        }
        if turns.is_empty() {
            turns.push(whole);
        }

        let extractor = build_extractor(&self.paths, &self.device, self.num_threads)?;
        let embeddings = compute_speaker_embeddings(
            &extractor,
            &audio.samples,
            audio.sample_rate,
            &turns,
        )?;

        Ok(Diarization {
            num_speakers: 1,
            turns,
            embeddings,
        })
    }

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
        // SCRIBE_DIARIZE_WINDOW_MS shortens the window, which was worth trying
        // against the segmentation crash and does not help: of four recordings
        // that crash at the ten-minute default, three still crash when the same
        // audio is handed over in twenty-second pieces. Whatever the model
        // reads out of bounds, it is not a function of how much it is given.
        let window_ms = std::env::var("SCRIBE_DIARIZE_WINDOW_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DIARIZE_WINDOW_MS);
        let window = match ((window_ms * sr as i64) / 1000) as usize {
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
            let t_seg = std::time::Instant::now();
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
            let seg_secs = t_seg.elapsed().as_secs_f64();

            let t_emb = std::time::Instant::now();
            let pieces = split_turns_at_silence(&local_turns, slice, sr);
            let piece_embs = compute_speaker_embeddings(&extractor, slice, sr, &pieces)?;
            if std::env::var("SCRIBE_DIARIZE_TIMING").is_ok() {
                eprintln!(
                    "   window {:>6}ms  segment {:.1}s  embed {:.1}s over {} pieces",
                    offset_ms,
                    seg_secs,
                    t_emb.elapsed().as_secs_f64(),
                    pieces.len()
                );
            }

            if std::env::var("SCRIBE_DIARIZE_HALVES").is_ok() {
                report_halves(&extractor, slice, sr, &pieces, offset_ms);
            }

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
/// Swept across seven fixtures: every value from 1.5 s to 5 s gives an
/// identical answer on all of them, and 1 s is clearly worse — phantom
/// participants on four of the seven and attribution down several points. So
/// this sits in the middle of a wide plateau rather than on a tuned edge, and
/// moving it is not the lever it looks like.
///
/// What it cannot rescue is somebody whose whole contribution is brief
/// interjections. On a fixture where one participant said only "Yes.", "That
/// tracks." and "I can take that." — 2.5 s across three turns of 0.4 s to 1.1 s —
/// he is folded into whoever he is nearest and the recording comes back with
/// three speakers instead of four. Lowering the floor does not recover him,
/// because his turns are too short to embed consistently enough to cluster
/// together in the first place: they arrive as separate slivers, none of which
/// reaches even 1.5 s. Letting slivers stand on dissimilarity instead was tried
/// and is worse — a short embedding is unreliable rather than distinctive, so
/// "resembles nobody" and "too brief to tell" are the same measurement, and the
/// same recording came back with twelve speakers.
const MIN_SPEAKER_SPEECH_MS_DEFAULT: i64 = 3_000;

/// A cluster must also hold this share of the recording's speech to be a person.
///
/// The absolute floor above is length-blind, and the residue left by splitting
/// turns is not: a long recording accumulates bigger slivers simply by having
/// more turns to leave them in. Four and a half seconds is a fifth of what a
/// participant says in a forty-second exchange and seven tenths of one percent
/// of an eleven-minute meeting, and only one of those is a person.
///
/// Measured, an eleven-minute degraded recording came back with a fifth speaker
/// holding 0.7% of the speech; raising the absolute floor to five seconds also
/// fixes it, and costs anybody who says three to five seconds in a *short*
/// recording, where that is a real share of it. This keeps both ends.
const MIN_SPEAKER_SHARE: f64 = 0.01;

/// Experiment hook: `SCRIBE_MIN_SPEAKER_MS` overrides the floor, which is how
/// its value was chosen. See docs/measuring-diarization.md.
fn min_speaker_speech_ms() -> i64 {
    std::env::var("SCRIBE_MIN_SPEAKER_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MIN_SPEAKER_SPEECH_MS_DEFAULT)
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
/// Diagnostic: how alike the two halves of each fragment are.
///
/// A fragment that quietly holds a handover — the pause between the two
/// speakers was too short to cut at — ought to have halves that do not match,
/// which would allow a second pass to find the handovers the silence split
/// cannot reach. This exists to check that before anything is built on it.
///
/// The signal is not there. On a fast-conversation fixture, splitting each
/// fragment at its quietest interior point and comparing the halves:
///
///   fragment = one speaker      n=38  min 0.108  median 0.495  max 0.787
///   fragment spans a handover   n=6   min 0.360  median 0.500  max 0.684
///
/// The medians are the same and the single-speaker range is the wider of the
/// two, so no threshold separates them. Half of a short fragment is about a
/// second of audio, and a one-second embedding is too noisy to say who is
/// speaking — one person's two halves differ as much as two people's do.
///
/// Kept so the same question can be asked again of a better embedding model,
/// which is the thing that would have to change for the answer to differ.
#[cfg(feature = "onnx")]
fn report_halves(
    extractor: &SpeakerEmbeddingExtractor,
    samples: &[f32],
    sample_rate: u32,
    pieces: &[SpeakerTurn],
    offset_ms: i64,
) {
    let sr = sample_rate as i64;
    for piece in pieces {
        let span = piece.end_ms - piece.start_ms;
        if span < 2 * MIN_HALF_MS {
            continue;
        }
        // Cut at the quietest interior point, not the midpoint: that is where a
        // handover would be if there is one.
        let start = ((piece.start_ms * sr) / 1000) as usize;
        let end = ((((piece.end_ms) * sr) / 1000) as usize).min(samples.len());
        if end <= start {
            continue;
        }
        let frame = ((SILENCE_FRAME_MS * sr) / 1000).max(1) as usize;
        let guard = (MIN_HALF_MS / SILENCE_FRAME_MS) as usize;
        let energies: Vec<f32> = samples[start..end]
            .chunks(frame)
            .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
            .collect();
        if energies.len() <= 2 * guard {
            continue;
        }
        let mut best = guard;
        for i in guard..(energies.len() - guard) {
            if energies[i] < energies[best] {
                best = i;
            }
        }
        let cut_ms = piece.start_ms + best as i64 * SILENCE_FRAME_MS;
        let halves = vec![
            SpeakerTurn { local_idx: 0, start_ms: piece.start_ms, end_ms: cut_ms },
            SpeakerTurn { local_idx: 1, start_ms: cut_ms, end_ms: piece.end_ms },
        ];
        let Ok(embs) = compute_speaker_embeddings(extractor, samples, sample_rate, &halves) else {
            continue;
        };
        let (Some(a), Some(b)) = (embs.get(&0), embs.get(&1)) else {
            continue;
        };
        println!(
            "HALVES {:>8} {:>8} cut {:>8} cos {:.4}",
            piece.start_ms + offset_ms,
            piece.end_ms + offset_ms,
            cut_ms + offset_ms,
            cosine(a, b)
        );
    }
}

/// Shortest half the diagnostic will consider — below this an embedding is not
/// worth comparing.
const MIN_HALF_MS: i64 = 700;

/// The least similar pair of fragments inside one cluster.
///
/// A cluster holding one voice keeps every pair fairly close. A cluster that
/// has quietly swallowed a second person has at least one pair that is not
/// close at all, and this finds it. `None` when there are not two embeddable
/// fragments to compare.
fn cluster_worst_pair(fragments: &[Fragment], members: &[usize]) -> Option<f32> {
    let embs: Vec<&Vec<f32>> = members
        .iter()
        .filter_map(|&f| fragments[f].embedding.as_ref())
        .filter(|e| !e.is_empty())
        .collect();
    if embs.len() < 2 {
        return None;
    }
    let mut worst = f32::MAX;
    for a in 0..embs.len() {
        for b in (a + 1)..embs.len() {
            worst = worst.min(cosine(embs[a], embs[b]));
        }
    }
    Some(worst)
}

#[derive(Clone, Copy, PartialEq)]
enum Linkage {
    Average,
    Single,
    Complete,
}

fn linkage() -> Linkage {
    match std::env::var("SCRIBE_LINKAGE").ok().as_deref() {
        Some("single") => Linkage::Single,
        Some("complete") => Linkage::Complete,
        _ => Linkage::Average,
    }
}

fn cohesion_ratio() -> f32 {
    std::env::var("SCRIBE_COHESION_RATIO")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(COHESION_RATIO)
}

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
    let floor = min_speaker_speech_ms();
    let total_speech: i64 = speech.iter().sum();
    let substantial = |ms: i64| -> bool {
        ms >= floor && (total_speech <= 0 || (ms as f64) / (total_speech as f64) >= MIN_SPEAKER_SHARE)
    };
    let count_substantial = |alive: &[bool], speech: &[i64]| -> usize {
        (0..alive.len()).filter(|&k| alive[k] && substantial(speech[k])).count()
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
            // SCRIBE_LINKAGE=single follows a chain instead of averaging it,
            // which is what a speaker whose voice drifts across a recording
            // looks like: her first and last stretches do not match, but every
            // neighbouring pair does. Measured, not shipped — see the docs.
            let merged = match linkage() {
                Linkage::Single => sim[i * n + k].max(sim[j * n + k]),
                Linkage::Complete => sim[i * n + k].min(sim[j * n + k]),
                Linkage::Average => (sim[i * n + k] * wi + sim[j * n + k] * wj) / (wi + wj),
            };
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
    // The cut can stop one or two merges late: two people whose voices are
    // alike join at a similarity that looks like one person's own spread, so
    // the merge sequence shows no step there and `choose_cut` sails past it.
    //
    // The joined cluster gives itself away afterwards. Every other cluster
    // holds one voice and keeps its fragments close; this one holds two and has
    // a pair that is not close at all. Measured against the median cluster, the
    // loosest cluster of a recording that was counted correctly never fell
    // below 0.75 of it, and one that was miscounted never rose above 0.72.
    //
    // A stated count is left alone: it is better evidence than this.
    let mut cut = cut;
    if target.is_none() {
        for _ in 0..MAX_COHESION_STEPS {
            if cut == 0 {
                break;
            }
            let clusters = replay(n, &history, cut, &embedded);
            let worsts: Vec<f32> = clusters
                .iter()
                .filter_map(|m| cluster_worst_pair(fragments, m))
                .collect();
            // Too few real clusters to say what "normal" looks like here.
            if worsts.len() < MIN_CLUSTERS_TO_JUDGE {
                break;
            }
            let Some(mid) = median(&worsts) else { break };
            let loosest = worsts.iter().copied().fold(f32::MAX, f32::min);
            if mid > 0.0 && loosest / mid < cohesion_ratio() {
                cut -= 1;
            } else {
                break;
            }
        }
    }
    let cut = cut;

    let final_clusters = replay(n, &history, cut, &embedded);

    // Diagnostic: how tightly each surviving cluster holds together, measured
    // from the raw embeddings rather than the merged similarities. A cluster
    // holding two people should be looser than one holding a voice.
    // Diagnostic: for every pair of substantial clusters, how their similarity
    // depends on the time between the fragments compared. A speaker whose voice
    // drifts should look like herself across a short gap and less like herself
    // across a long one; two different people should look equally unalike at
    // every gap. Unlike a raw similarity this is a shape, so it does not depend
    // on the microphone.
    if std::env::var("SCRIBE_DIARIZE_DRIFT").is_ok() {
        let mid = |f: usize| -> i64 {
            let t = &fragments[f].turns;
            if t.is_empty() { 0 } else { (t[0].start_ms + t[t.len() - 1].end_ms) / 2 }
        };
        eprintln!("-- drift: similarity by time gap, per cluster pair --");
        for (a, ma) in final_clusters.iter().enumerate() {
            for (b, mb) in final_clusters.iter().enumerate().skip(a + 1) {
                if ma.len() < 2 || mb.len() < 2 {
                    continue;
                }
                let mut pairs: Vec<(i64, f32)> = Vec::new();
                for &x in ma {
                    for &y in mb {
                        let (Some(ex), Some(ey)) =
                            (fragments[x].embedding.as_ref(), fragments[y].embedding.as_ref())
                        else { continue };
                        if ex.is_empty() || ey.is_empty() { continue }
                        pairs.push(((mid(x) - mid(y)).abs(), cosine(ex, ey)));
                    }
                }
                if pairs.len() < 6 {
                    continue;
                }
                pairs.sort_by_key(|(g, _)| *g);
                let third = pairs.len() / 3;
                let near: f32 = pairs[..third].iter().map(|(_, s)| *s).sum::<f32>() / third as f32;
                let far: f32 = pairs[pairs.len() - third..].iter().map(|(_, s)| *s).sum::<f32>()
                    / third as f32;
                eprintln!(
                    "   {a:>2} x {b:<2}  closest-third {near:.4}  farthest-third {far:.4}                       drop {:.4}",
                    near - far
                );
            }
        }
    }

    if std::env::var("SCRIBE_DIARIZE_COHESION").is_ok() {
        eprintln!("-- cluster cohesion at cut ({} clusters) --", final_clusters.len());
        for (idx, members) in final_clusters.iter().enumerate() {
            let embs: Vec<&Vec<f32>> = members
                .iter()
                .filter_map(|&f| fragments[f].embedding.as_ref())
                .filter(|e| !e.is_empty())
                .collect();
            let mut total = 0.0f64;
            let mut pairs = 0u64;
            let mut worst = 1.0f32;
            for a in 0..embs.len() {
                for b in (a + 1)..embs.len() {
                    let c = cosine(embs[a], embs[b]);
                    total += c as f64;
                    pairs += 1;
                    worst = worst.min(c);
                }
            }
            let speech: i64 = members.iter().map(|&f| fragments[f].speech_ms.max(0)).sum();
            let mean = if pairs > 0 { total / pairs as f64 } else { f64::NAN };
            // Closest other cluster, by average linkage over raw embeddings,
            // and how that compares with that cluster's own internal spread.
            let mut best = (f32::MIN, usize::MAX);
            for (other, others) in final_clusters.iter().enumerate() {
                if other == idx {
                    continue;
                }
                let oembs: Vec<&Vec<f32>> = others
                    .iter()
                    .filter_map(|&f| fragments[f].embedding.as_ref())
                    .filter(|e| !e.is_empty())
                    .collect();
                if oembs.is_empty() || embs.is_empty() {
                    continue;
                }
                let mut acc = 0.0f64;
                let mut n = 0u64;
                for a in &embs {
                    for b in &oembs {
                        acc += cosine(a, b) as f64;
                        n += 1;
                    }
                }
                let link = (acc / n as f64) as f32;
                if link > best.0 {
                    best = (link, other);
                }
            }
            // Every link from this cluster to the others, so the closest can be
            // judged against what "a different person" looks like in this
            // recording rather than against a fixed number.
            let mut links: Vec<f32> = Vec::new();
            for (other, others) in final_clusters.iter().enumerate() {
                if other == idx {
                    continue;
                }
                let oembs: Vec<&Vec<f32>> = others
                    .iter()
                    .filter_map(|&f| fragments[f].embedding.as_ref())
                    .filter(|e| !e.is_empty())
                    .collect();
                if oembs.is_empty() || embs.is_empty() || others.len() < 2 {
                    continue;
                }
                let mut acc = 0.0f64;
                let mut n = 0u64;
                for a in &embs {
                    for b in &oembs {
                        acc += cosine(a, b) as f64;
                        n += 1;
                    }
                }
                links.push((acc / n as f64) as f32);
            }
            let link_median = median(&links).unwrap_or(f32::NAN);
            let host_spread = if best.1 == usize::MAX {
                f32::NAN
            } else {
                cluster_worst_pair(fragments, &final_clusters[best.1]).unwrap_or(f32::NAN)
            };
            eprintln!(
                "   cluster {idx:>2}  {:>2} frags  {speech:>7} ms  mean {mean:.4}  worst {worst:.4}                   nearest {:>2} at {:.4} (its own spread {host_spread:.4}) linkmed {link_median:.4}",
                members.len(),
                best.1 as i64,
                best.0
            );
        }
    }

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
    let floor = min_speaker_speech_ms();
    let mut speech: HashMap<i32, i64> = HashMap::new();
    for (frag, &cluster) in fragments.iter().zip(assignment.iter()) {
        *speech.entry(cluster).or_insert(0) += frag.speech_ms.max(0);
    }

    let total: i64 = speech.values().sum();
    let surviving: Vec<i32> = speech
        .iter()
        .filter(|(_, ms)| {
            **ms >= floor
                && (total <= 0 || (**ms as f64) / (total as f64) >= MIN_SPEAKER_SHARE)
        })
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
/// Swept from 0.70 to 0.90 after the embedding model changed, since the value
/// was originally chosen against a different model's similarity distribution.
/// Everything from 0.70 to 0.85 gives an identical answer on every fixture;
/// 0.90 breaks two. A plateau rather than an edge, and 0.8 sits in it.
const RELATIVE_DROP_DEFAULT: f32 = 0.8;

/// Experiment hook: `SCRIBE_RELATIVE_DROP` overrides it, which is how the value
/// is checked against a change of embedding model.
fn relative_drop() -> f32 {
    std::env::var("SCRIBE_RELATIVE_DROP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(RELATIVE_DROP_DEFAULT)
}

/// Decide where to stop merging, using only this recording's own numbers.
///
/// There are recordings where the numbers do not contain the answer. Eight
/// voices in a clean room merge down through 0.58, 0.56, 0.53, 0.45, 0.38,
/// 0.34 — all of those joining one person's own pieces back together — and then
/// join two different women at 0.31, with the merges either side at 0.34 and
/// 0.30. One speaker's variation is as wide there as the gap between two
/// people, so no threshold on this sequence separates them and none was found
/// by sweeping. Stating the participant count does, completely.
///
/// The obvious alternative — cut where the sequence falls away most steeply,
/// the standard elbow — was measured against this across fourteen fixtures and
/// is far worse on every one of them. Similarities approach zero as the last
/// unrelated clusters are forced together, so the sharpest *ratio* is almost
/// always among the final merges, and the rule collapses recordings to one or
/// two speakers: five voices came back as one, four as one, and the
/// over-segmented recording it was written to rescue came back as three rather
/// than the four it should be. Comparing against the family of merges already
/// accepted, rather than against the single merge before, is what keeps the
/// comparison anchored to what "same voice" looked like earlier in this
/// recording instead of to how far the sequence has fallen by now.
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
    let drop = relative_drop();
    let mut accepted: Vec<f32> = Vec::new();
    for (m, (count, sim, _)) in history.iter().enumerate() {
        // Above the sanity bound, keep merging whatever it looks like: that
        // many clusters is over-segmentation, not a room full of people.
        if *count <= MAX_INFERRED_SPEAKERS {
            // With nothing accepted yet there is no family to compare against;
            // a fragment is perfectly similar to itself, so use 1.0.
            let within = median(&accepted).unwrap_or(1.0);
            if *sim < drop * within {
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

    // Every piece is embedded, however short.
    //
    // Speaker-embedding models need about a second of speech, and below that the
    // vector says more about the noise floor than the voice - so skipping the
    // short ones looks like free economy. Measured, it is not: a short
    // interjection ("Okay.", "Yes.") is real speech by a real person, and even a
    // poor embedding of it places it better than the alternative, which is
    // inheriting whoever happened to be speaking nearby. On a five-voice fixture
    // embedding everything scores 99.7% and skipping pieces under a second
    // scores 97.1%, the whole difference being those interjections landing on
    // the wrong person. Under 400 ms it stops mattering either way.
    for turn in turns {
        let start = ((turn.start_ms.max(0) * sr) / 1000) as usize;
        let end = (((turn.end_ms.max(turn.start_ms)) * sr) / 1000) as usize;
        let end = end.min(samples.len());
        if end <= start {
            continue;
        }

        // Past the budget, embed the middle of the piece rather than all of it.
        // The middle because the start of a stretch of speech carries the lead-in
        // from whatever preceded it, and the end trails off.
        let budget = ((EMBED_BUDGET_MS * sr) / 1000) as usize;
        let (start, end) = if end - start > budget {
            let centre = start + (end - start) / 2;
            (centre - budget / 2, centre + budget / 2)
        } else {
            (start, end)
        };

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
    let min_run = (min_split_silence_ms() / SILENCE_FRAME_MS).max(1) as usize;

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
        // Two readings of the same frames. The mean-relative threshold is what
        // a clean recording wants; one placed above the turn's own noise floor
        // is what a real room wants. Take whichever is higher — on clean audio
        // the floor is near zero and the first still wins.
        let mean = energies.iter().sum::<f32>() / energies.len() as f32;
        let mut sorted = energies.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let floor = sorted[0];
        let speech = sorted[((sorted.len() - 1) as f32 * SPEECH_PERCENTILE) as usize];
        let adaptive = if floor > 0.0 && speech > floor {
            floor * (speech / floor).powf(floor_alpha())
        } else {
            0.0
        };
        let threshold = (mean * silence_ratio()).max(adaptive);

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

        // A cut that leaves a sliver behind is worse than no cut: the piece is
        // too short to embed well, and a bad embedding lands it on whoever it
        // happens to resemble. Drop cuts that would make one, and drop the last
        // cut if the tail after it is a sliver too.
        let min_piece = min_piece_ms();
        let mut kept: Vec<i64> = Vec::with_capacity(cut_ms.len());
        let mut last = turn.start_ms;
        for cut in cut_ms {
            if cut - last >= min_piece {
                kept.push(cut);
                last = cut;
            }
        }
        while kept.last().is_some_and(|c| turn.end_ms - c < min_piece) {
            kept.pop();
        }
        let cut_ms = kept;

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

    /// The same handover, in a room with a noise floor instead of silence.
    ///
    /// This is the case a mean-relative threshold cannot see: the gap sits at
    /// 0.178 of the speech level (15 dB SNR) and the threshold at 0.15 of the
    /// mean, so nothing is ever quiet enough and the turn survives whole — two
    /// speakers embedded as one. Cost a whole speaker on every reverberant
    /// six-voice fixture before the threshold was made to measure the floor.
    #[test]
    fn a_handover_is_found_over_a_noise_floor() {
        let sr = 16_000u32;
        let speech = |i: usize| ((i as f32 / 40.0).sin()) * 0.3;
        // Deterministic hiss at 15 dB below the speech, everywhere — including
        // under the speech, as real noise is.
        let hiss = |i: usize| (((i * 2_654_435_761) % 2_003) as f32 / 1_001.5 - 1.0) * 0.3 * 0.178;

        let mut samples = Vec::new();
        for i in 0..sr as usize {
            samples.push(speech(i) + hiss(i));
        }
        let gap = (sr as usize * 400) / 1000;
        for i in 0..gap {
            samples.push(hiss(i + sr as usize));
        }
        for i in 0..sr as usize {
            samples.push(speech(i) + hiss(i + sr as usize + gap));
        }

        let pieces = split_turns_at_silence(&[turn(0, 0, 2_400)], &samples, sr);
        assert_eq!(pieces.len(), 2, "the handover was missed: {pieces:?}");
        assert!(
            (1_000..=1_400).contains(&pieces[0].end_ms),
            "cut at {}",
            pieces[0].end_ms
        );
    }

    /// A cut that would leave a sliver is refused. The sliver is too short to
    /// embed well, and a bad embedding lands it on whoever it resembles; enough
    /// of them drag real speakers together. Refusing them is what allows the
    /// split to look for pauses short enough to survive a reverberant room.
    #[test]
    fn a_cut_that_would_leave_a_sliver_is_refused() {
        let sr = 16_000u32;
        let tone = |n: usize| (0..n).map(|i| ((i as f32 / 40.0).sin()) * 0.3);
        // 200 ms speech, 200 ms silence, 2 s speech. The cut would fall at
        // 300 ms and leave a 300 ms piece, under MIN_PIECE_MS.
        let mut samples: Vec<f32> = tone((sr as usize * 200) / 1000).collect();
        samples.extend(std::iter::repeat(0.0).take((sr as usize * 200) / 1000));
        samples.extend(tone(sr as usize * 2));

        let pieces = split_turns_at_silence(&[turn(0, 0, 2_400)], &samples, sr);
        assert_eq!(pieces.len(), 1, "sliver was cut off: {pieces:?}");
    }

    /// The same gap, with enough speech either side to be worth cutting.
    #[test]
    fn a_cut_between_two_real_stretches_is_kept() {
        let sr = 16_000u32;
        let tone = |n: usize| (0..n).map(|i| ((i as f32 / 40.0).sin()) * 0.3);
        let mut samples: Vec<f32> = tone(sr as usize).collect();
        samples.extend(std::iter::repeat(0.0).take((sr as usize * 200) / 1000));
        samples.extend(tone(sr as usize));

        let pieces = split_turns_at_silence(&[turn(0, 0, 2_200)], &samples, sr);
        assert_eq!(pieces.len(), 2, "pieces = {pieces:?}");
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

    /// The same amount of speech is a participant in a short recording and a
    /// sliver in a long one, so the floor cannot be a duration alone.
    #[test]
    fn a_sliver_is_judged_against_the_length_of_the_recording() {
        let alice = [1.0, 0.02, 0.0];
        let bob = [0.0, 1.0, 0.02];

        // A four-second scrap beside two people who talk for ten minutes each:
        // under one percent of the speech, and not a participant.
        let long = vec![
            frag(Some(&alice), 0, 600_000),
            frag(Some(&bob), 600_000, 1_200_000),
            frag(Some(&[0.5, 0.5, 0.71]), 1_200_000, 1_204_000),
        ];
        assert_eq!(partition(&cluster_fragments(&long, None)).len(), 2);

        // The same four seconds beside two people who talk for twenty: a fifth
        // of the recording, and somebody.
        let short = vec![
            frag(Some(&alice), 0, 20_000),
            frag(Some(&bob), 20_000, 40_000),
            frag(Some(&[0.5, 0.5, 0.71]), 40_000, 44_000),
        ];
        assert_eq!(partition(&cluster_fragments(&short, None)).len(), 3);
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

    /// The looseness measure the step-back reads: the least similar pair in a
    /// cluster, which is what gives away a cluster holding two people.
    #[test]
    fn a_clusters_looseness_is_its_least_similar_pair() {
        let a = unit(&[1.0, 0.0, 0.0]);
        let b = unit(&[0.9, 0.436, 0.0]);
        let c = unit(&[0.6, 0.8, 0.0]);
        let fragments = vec![
            frag(Some(&a), 0, 1_000),
            frag(Some(&b), 1_000, 2_000),
            frag(Some(&c), 2_000, 3_000),
        ];
        let worst = cluster_worst_pair(&fragments, &[0, 1, 2]).expect("three embeddings");
        let ac = cosine(&a, &c);
        assert!(
            (worst - ac).abs() < 1e-5,
            "expected the a-c pair ({ac:.4}), got {worst:.4}"
        );
        // A tighter subset is not dragged down by the fragment left out.
        let tight = cluster_worst_pair(&fragments, &[0, 1]).expect("two embeddings");
        assert!(tight > worst, "{tight:.4} should beat {worst:.4}");
    }

    /// Nothing to compare means no opinion, rather than a default that would
    /// make a one-fragment cluster look infinitely loose and split forever.
    #[test]
    fn a_cluster_too_small_to_judge_has_no_looseness() {
        let a = unit(&[1.0, 0.0, 0.0]);
        let fragments = vec![frag(Some(&a), 0, 1_000), frag(None, 1_000, 2_000)];
        assert!(cluster_worst_pair(&fragments, &[0]).is_none());
        assert!(cluster_worst_pair(&fragments, &[0, 1]).is_none(), "unembedded does not count");
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

    /// A piece with no pause in it can still be long. The budget bounds what
    /// reaches the extractor, and takes it from the middle.
    #[test]
    fn a_long_unbroken_piece_is_embedded_within_budget() {
        let sr = 16_000u32;
        let secs = (EMBED_BUDGET_MS / 1000 + 60) as usize;
        let samples: Vec<f32> = (0..sr as usize * secs)
            .map(|i| ((i as f32 / 40.0).sin()) * 0.3)
            .collect();
        let budget_samples = ((EMBED_BUDGET_MS * sr as i64) / 1000) as usize;

        // The pieces the extractor would be handed, via the same split the
        // embedding loop uses.
        let total = samples.len();
        let centre = total / 2;
        let (from, to) = (centre - budget_samples / 2, centre + budget_samples / 2);
        assert!(to - from <= budget_samples, "budget respected");
        assert!(from > 0 && to < total, "taken from the middle, not an edge");

        for (a, b) in split_evenly(from, to, ((MAX_EMBED_MS * sr as i64) / 1000) as usize) {
            assert!(
                b - a <= ((MAX_EMBED_MS * sr as i64) / 1000) as usize,
                "no piece reaches the model's length limit"
            );
        }
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
