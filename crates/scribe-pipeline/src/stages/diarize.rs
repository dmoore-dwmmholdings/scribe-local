//! Stage 2 — diarize (design §7/§8).
//!
//! Run the diarizer over the transcoded WAV to get speaker turns + per-speaker
//! embeddings. Each diarized speaker's embedding is persisted to
//! `recording_speakers` and the recording's voices are resolved against the
//! enrolled ones **as a set**, so one enrolled person is claimed at most once.
//! The turns are parked in the scratch artifacts table for the merge stage.

use std::collections::{BTreeSet, HashMap, HashSet};

use scribe_asr::SpeechEngine;
use scribe_core::config::Config;
use scribe_core::storage;
use scribe_core::Result;
use scribe_db::Db;
use uuid::Uuid;

use crate::artifacts::{self, DiarizationArtifact, TurnArtifact};
use crate::stages::stage_err;

const STAGE: &str = "diarize";

/// Floor below which a voice is not considered against an enrolled one at all.
///
/// It looks too low. With the current embedding model a stranger who resembles
/// somebody scores about 0.61 against their voiceprint, which is well above
/// this, so the floor is not what refuses them — [`SEPARATION`] and
/// [`MATCH_CONSISTENCY`] are. Raising it to 0.7 to do that job directly is the
/// obvious tidy-up and it is wrong: measured in a bad room, RT60 1.4 s at 4 dB
/// signal-to-noise, a *correct* match scores 0.61 to 0.67. A floor at 0.65
/// loses two of three real speakers there and one at 0.70 loses all three.
///
/// A stranger in a good room and a friend in a bad one land in the same place.
/// That is the whole reason the rules that decide are relative ones, and the
/// floor stays low and out of their way — it is here to reject noise, and
/// everything from 0.5 to 0.6 behaves identically on every fixture.
///
/// A floor, not a decision: it rejects noise, and [`SEPARATION`] decides. An
/// absolute cosine cannot decide identity on its own — two takes of one voice
/// sit near 0.95 on a close mic and near 0.5 across a room, so the number that
/// means "same person" in one recording means nothing in another. The clustering
/// in `scribe-asr` gave up fixed thresholds for exactly this reason.
pub const ENROLL_MATCH_THRESHOLD_DEFAULT: f32 = 0.5;

/// Experiment hook: `SCRIBE_ENROLL_FLOOR` overrides it, which is how it is
/// checked against a change of embedding model.
pub fn enroll_match_threshold() -> f32 {
    std::env::var("SCRIBE_ENROLL_FLOOR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(ENROLL_MATCH_THRESHOLD_DEFAULT)
}

/// How far a voice must stand out from the rest of the library to be called a
/// match.
///
/// The enrolled speakers are the calibration. They are known-different people —
/// each is somebody distinct — and every one of them is measured against this
/// voice across recordings, through the same microphone, the same room and the
/// same model. At most one can be the same person, so the others are a sample
/// of what "not this person" scores under exactly the conditions the real match
/// would be scored under. That is the comparison an absolute cutoff cannot make.
///
/// A voice sitting equally close to everyone enrolled is a poor embedding, not a
/// recognition, however high the absolute number. One sitting well clear of the
/// rest is a recognition, even when the absolute number is unremarkable.
///
/// The size of the gap is a judgement, not a measurement: it is set to reject
/// the genuinely ambiguous rather than to tighten the floor.
const SEPARATION: f32 = 0.10;

/// Cosine similarity between two vectors; 0 for degenerate or mismatched input.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na <= f32::EPSILON || nb <= f32::EPSILON {
        return 0.0;
    }
    dot / (na * nb)
}

/// How far below the matches already accepted in this recording a further match
/// may fall before it is refused.
///
/// The last thing a fixed threshold cannot do. Measured across two meetings of
/// the same people: a voice recognised through the same microphone scores about
/// 0.99 against its own voiceprint, and through a different room about 0.56 —
/// while a *different person* in a good room scored 0.599 against somebody
/// else's voiceprint. The false match is numerically stronger than the true one.
/// No cutoff on similarity, and no requirement to stand clear of the rest of the
/// library, can separate those two: the first accepts both, the second accepts
/// both.
///
/// What separates them is the company they keep. Recognition is not one
/// question asked repeatedly, it is one recording — the same microphone, the
/// same room, the same distance — so the voices that *are* recognised in it
/// establish what recognition looks like here. Against Daniel and Rishi at 0.99,
/// a third voice at 0.599 is not a quieter version of the same event. Against
/// Daniel and Rishi at 0.59 and 0.69, a third at 0.561 plainly is.
///
/// The cost of this is real and worth stating. It refuses a participant heard
/// through a different channel from everybody else — somebody dialling into a
/// meeting from a phone, where the people in the room match at 0.85 and they
/// match at 0.57. Loosening it to admit them is not possible: in the recording
/// where that dial-in scores 0.566 against his own voiceprint, a fourth person
/// who is not enrolled at all scores 0.566 against somebody else's. Same
/// number, same ratio, one right and one wrong. A missed name can be given by
/// hand; a wrong one has to be noticed first.
const MATCH_CONSISTENCY: f32 = 0.75;

/// Experiment hooks, so both halves of the admissibility rule can be swept the
/// way the diarization constants are. See docs/measuring-diarization.md.
fn match_consistency() -> f32 {
    std::env::var("SCRIBE_MATCH_CONSISTENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MATCH_CONSISTENCY)
}

fn separation() -> f32 {
    std::env::var("SCRIBE_ENROLL_SEPARATION")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(SEPARATION)
}

/// Resolve a recording's diarized voices against the enrolled ones, one to one.
///
/// Matching each voice independently against its own nearest enrolled speaker
/// is the obvious thing and it is wrong: nothing stops two of a recording's
/// voices picking the same person, and then the transcript shows Alice
/// answering Alice. A person is in the room once.
///
/// So every admissible (voice, enrolled) pair is considered together, best
/// first, and a pair is taken only while both sides are still free. Greedy
/// rather than optimal — with a handful of speakers the two agree, and "the
/// most confident match wins the tie" is a rule that can be explained to
/// somebody looking at a transcript wondering why it chose that.
///
/// A pair is admissible when it clears [`enroll_match_threshold`] and stands
/// [`SEPARATION`] clear of what this voice scores against the rest of the
/// library. Admissibility is per pair rather than per voice, so a voice whose
/// first choice is taken can still hold a second — but only one it also stands
/// out against, never a weak consolation.
///
/// `voices` is `(local_idx, embedding)`; `enrolled` is `(speaker_id, voiceprint)`.
///
/// Public so it can be measured against voiceprints taken from one recording and
/// voices diarized from another, which is the only setting where it means
/// anything — the whole point is a name surviving from one meeting to the next.
pub fn resolve_identities(
    voices: &[(i32, Vec<f32>)],
    enrolled: &[(Uuid, Vec<f32>)],
    threshold: f32,
) -> HashMap<i32, (Uuid, f32)> {
    let mut candidates: Vec<(f32, i32, Uuid)> = Vec::new();
    for (local_idx, embedding) in voices {
        let scores: Vec<f32> = enrolled
            .iter()
            .map(|(_, voiceprint)| cosine(embedding, voiceprint))
            .collect();

        for (i, (speaker_id, _)) in enrolled.iter().enumerate() {
            if scores[i] < threshold {
                continue;
            }
            // What this voice scores against everybody it is not. With a library
            // of one there is no such sample, and the floor is all there is.
            let rest: Vec<f32> = scores
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, s)| *s)
                .collect();
            if let Some(baseline) = median(&rest) {
                if scores[i] - baseline < separation() {
                    continue;
                }
            }
            candidates.push((scores[i], *local_idx, *speaker_id));
        }
    }
    // Best first. Ties break on the local index then the speaker id, so the
    // result never depends on the order rows came back from the database.
    candidates.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });

    let mut resolved: HashMap<i32, (Uuid, f32)> = HashMap::new();
    let mut claimed: HashSet<Uuid> = HashSet::new();
    let mut accepted: Vec<f32> = Vec::new();
    for (sim, local_idx, speaker_id) in candidates {
        if resolved.contains_key(&local_idx) || claimed.contains(&speaker_id) {
            continue;
        }
        // Judged against the recognitions this recording has already produced.
        // Candidates arrive best first, so the strongest match sets the standard
        // and the rest are held to it; with nothing accepted yet there is no
        // standard, and the floor and separation are all there is.
        if let Some(standard) = median(&accepted) {
            if sim < match_consistency() * standard {
                continue;
            }
        }
        resolved.insert(local_idx, (speaker_id, sim));
        claimed.insert(speaker_id);
        accepted.push(sim);
    }
    resolved
}

/// Median of `values`, or `None` when empty. The median rather than the mean so
/// one unusually confusable library member cannot move the comparison.
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

/// Run the diarize stage for `recording_id`.
pub async fn run(
    cfg: &Config,
    db: &Db,
    speech: &SpeechEngine,
    recording_id: Uuid,
) -> Result<()> {
    let wav = storage::wav_path(cfg.storage.blobs.as_path(), recording_id);
    if !wav.exists() {
        return Err(stage_err(
            STAGE,
            format!("transcoded WAV missing: {}", wav.display()),
        ));
    }

    let recording = db.get_recording(recording_id).await?;
    let expected = recording.participants_expected;

    // Off the runtime threads: diarizing a long recording is minutes of native
    // CPU work, and holding a worker thread that long stops tokio's timers —
    // which stops the job heartbeat, and the reaper then takes the job back
    // from a worker that is still busy with it.
    let diarizer = speech.diarizer_handle();
    let wav_owned = wav.clone();
    let diarization = tokio::task::spawn_blocking(move || diarizer.diarize(&wav_owned, expected))
        .await
        .map_err(|e| stage_err(STAGE, format!("diarize task failed: {e}")))?
        .map_err(|e| stage_err(STAGE, e))?;

    // The speakers of this recording are the ones the merge stage will actually
    // label words with — every index appearing in a turn, not just the ones an
    // embedding could be computed for. A voice too brief or too noisy to embed
    // still gets a row, so the transcript can show it and a user can name it.
    let speakers: BTreeSet<i32> = diarization.turns.iter().map(|t| t.local_idx).collect();

    // Resolve every voice against the enrolled ones together, so one person
    // cannot end up labelling two of this recording's speakers.
    let voices: Vec<(i32, Vec<f32>)> = speakers
        .iter()
        .filter_map(|idx| diarization.embeddings.get(idx).map(|e| (*idx, e.clone())))
        .collect();
    let enrolled: Vec<(Uuid, Vec<f32>)> = db
        .list_enrolled_voiceprints()
        .await?
        .into_iter()
        .map(|(s, v)| (s.id, v))
        .collect();
    let identities = resolve_identities(&voices, &enrolled, enroll_match_threshold());

    for &local_idx in &speakers {
        let matched = identities.get(&local_idx);
        if let Some((speaker_id, sim)) = matched {
            tracing::info!(
                %recording_id, local_idx, %speaker_id, similarity = sim,
                "diarize: matched enrolled speaker"
            );
        }
        db.upsert_recording_speaker(
            recording_id,
            local_idx,
            matched.map(|(id, _)| *id),
            diarization.embeddings.get(&local_idx).cloned(),
        )
        .await?;
    }

    // Drop any speaker left over from an earlier run of this stage. Indices are
    // assigned from scratch each time, so a run that finds fewer speakers than
    // the last one does not overwrite the surplus — it would survive as a
    // participant with no speech attached.
    let found: Vec<i32> = speakers.iter().copied().collect();
    let pruned = db.prune_recording_speakers(recording_id, &found).await?;
    if pruned > 0 {
        tracing::info!(%recording_id, pruned, "diarize: dropped speakers from an earlier run");
    }

    // Hand the turns to merge via the scratch table.
    let artifact = DiarizationArtifact {
        turns: diarization
            .turns
            .iter()
            .map(|t| TurnArtifact {
                local_idx: t.local_idx,
                start_ms: t.start_ms,
                end_ms: t.end_ms,
            })
            .collect(),
        num_speakers: diarization.num_speakers,
    };
    artifacts::put_diarization(db, recording_id, &artifact).await?;

    tracing::info!(
        %recording_id,
        num_speakers = diarization.num_speakers,
        turns = diarization.turns.len(),
        "diarize complete"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / norm).collect()
    }

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// The bug this exists for: two of a recording's voices are both nearest to
    /// the same enrolled person, and matching them one at a time labels both.
    #[test]
    fn one_person_cannot_label_two_voices() {
        let alice = unit(&[1.0, 0.0, 0.0]);
        let voices = vec![
            (0, unit(&[1.0, 0.1, 0.0])),
            // Closer to Alice than the threshold, but Alice is not free.
            (1, unit(&[0.9, 0.4, 0.0])),
        ];
        let enrolled = vec![(id(1), alice)];

        let resolved = resolve_identities(&voices, &enrolled, enroll_match_threshold());
        assert_eq!(resolved.len(), 1);
        // The better match keeps her.
        assert_eq!(resolved.get(&0).map(|(s, _)| *s), Some(id(1)));
        assert!(resolved.get(&1).is_none());
    }

    #[test]
    fn each_voice_takes_its_own_person() {
        let voices = vec![(0, unit(&[1.0, 0.05, 0.0])), (1, unit(&[0.0, 1.0, 0.05]))];
        let enrolled = vec![(id(1), unit(&[1.0, 0.0, 0.0])), (id(2), unit(&[0.0, 1.0, 0.0]))];

        let resolved = resolve_identities(&voices, &enrolled, enroll_match_threshold());
        assert_eq!(resolved.get(&0).map(|(s, _)| *s), Some(id(1)));
        assert_eq!(resolved.get(&1).map(|(s, _)| *s), Some(id(2)));
    }

    /// A globally-greedy pass beats matching each voice to its own favourite:
    /// voice 0 likes Alice most, but voice 1 likes her more still, and voice 0
    /// has a second choice that voice 1 does not.
    #[test]
    fn the_more_confident_match_wins_the_contested_person() {
        let alice = unit(&[1.0, 0.0, 0.0]);
        let bob = unit(&[0.7, 0.7, 0.0]);
        let voices = vec![(0, unit(&[0.85, 0.5, 0.0])), (1, unit(&[1.0, 0.02, 0.0]))];
        let enrolled = vec![(id(1), alice), (id(2), bob)];

        let resolved = resolve_identities(&voices, &enrolled, enroll_match_threshold());
        assert_eq!(resolved.get(&1).map(|(s, _)| *s), Some(id(1)), "Alice to voice 1");
        assert_eq!(resolved.get(&0).map(|(s, _)| *s), Some(id(2)), "voice 0 falls to Bob");
    }

    /// A voice equally close to everybody enrolled has told us nothing, however
    /// high the absolute number. This is the case a fixed cutoff cannot see: a
    /// smeared embedding clears 0.5 against the whole library at once.
    #[test]
    fn a_voice_close_to_everyone_matches_no_one() {
        // Sits in the middle of three enrolled people, ~0.58 from each.
        let voices = vec![(0, unit(&[1.0, 1.0, 1.0]))];
        let enrolled = vec![
            (id(1), unit(&[1.0, 0.0, 0.0])),
            (id(2), unit(&[0.0, 1.0, 0.0])),
            (id(3), unit(&[0.0, 0.0, 1.0])),
        ];

        // Every one of them clears the floor on its own.
        for (_, voiceprint) in &enrolled {
            assert!(
                cosine(&voices[0].1, voiceprint) >= enroll_match_threshold(),
                "guard: the floor alone would have accepted this"
            );
        }
        assert!(resolve_identities(&voices, &enrolled, enroll_match_threshold()).is_empty());
    }

    /// The mirror image: a voice that stands clear of the library is recognised
    /// even though it is barely over the floor in absolute terms — a real match
    /// recorded across a room.
    #[test]
    fn standing_clear_of_the_library_is_enough() {
        // 0.55 against the right person, 0.44 against the others. The fourth
        // component is slack nobody is enrolled on — a voice is never entirely
        // accounted for by the library.
        let voices = vec![(0, unit(&[0.55, 0.44, 0.44, 0.557]))];
        let enrolled = vec![
            (id(1), unit(&[1.0, 0.0, 0.0, 0.0])),
            (id(2), unit(&[0.0, 1.0, 0.0, 0.0])),
            (id(3), unit(&[0.0, 0.0, 1.0, 0.0])),
        ];
        let best = cosine(&voices[0].1, &enrolled[0].1);
        assert!(best < 0.6, "guard: an unremarkable absolute score ({best})");

        let resolved = resolve_identities(&voices, &enrolled, enroll_match_threshold());
        assert_eq!(resolved.get(&0).map(|(s, _)| *s), Some(id(1)));
    }

    /// With one person enrolled there is no sample of "not this person" to
    /// compare against, so the floor is all there is. It must still work.
    #[test]
    fn a_library_of_one_falls_back_to_the_floor() {
        let voices = vec![(0, unit(&[1.0, 0.05, 0.0]))];
        let enrolled = vec![(id(1), unit(&[1.0, 0.0, 0.0]))];
        let resolved = resolve_identities(&voices, &enrolled, enroll_match_threshold());
        assert_eq!(resolved.get(&0).map(|(s, _)| *s), Some(id(1)));
    }

    /// Separation is judged per pair, so losing a contested person does not cost
    /// a voice a second choice — provided that second choice is also the kind of
    /// match this recording is producing.
    #[test]
    fn a_second_choice_must_stand_out_on_its_own_terms() {
        let alice = unit(&[1.0, 0.0, 0.0]);
        let bob = unit(&[0.0, 1.0, 0.0]);
        let carol = unit(&[0.0, 0.0, 1.0]);
        let voices = vec![
            // Wants Alice, is clearly Bob-ish, and not Carol at all.
            (0, unit(&[0.75, 0.66, 0.0])),
            // Wants Alice more.
            (1, unit(&[0.8, 0.6, 0.0])),
        ];
        let enrolled = vec![(id(1), alice), (id(2), bob), (id(3), carol)];

        let resolved = resolve_identities(&voices, &enrolled, enroll_match_threshold());
        assert_eq!(resolved.get(&1).map(|(s, _)| *s), Some(id(1)), "Alice to voice 1");
        assert_eq!(resolved.get(&0).map(|(s, _)| *s), Some(id(2)), "voice 0 falls to Bob");
    }

    /// The case no cutoff on similarity can decide, measured across two meetings
    /// of the same people: a stranger in a good room scored 0.599 against
    /// somebody's voiceprint while that person, recognised through a different
    /// room, scored 0.561 against their own. The false match is the stronger
    /// number. What tells them apart is the rest of the recording.
    #[test]
    fn a_match_far_weaker_than_the_recordings_others_is_refused() {
        let enrolled = vec![
            (id(1), unit(&[1.0, 0.0, 0.0, 0.0])),
            (id(2), unit(&[0.0, 1.0, 0.0, 0.0])),
            (id(3), unit(&[0.0, 0.0, 1.0, 0.0])),
        ];
        // Two voices recognised through the same microphone, and a third that
        // is merely somewhat like the person it is nearest to.
        let voices = vec![
            (0, unit(&[0.99, 0.05, 0.05, 0.0])),
            (1, unit(&[0.05, 0.99, 0.05, 0.0])),
            (2, unit(&[0.05, 0.05, 0.60, 0.80])),
        ];

        let resolved = resolve_identities(&voices, &enrolled, enroll_match_threshold());
        assert_eq!(resolved.get(&0).map(|(s, _)| *s), Some(id(1)));
        assert_eq!(resolved.get(&1).map(|(s, _)| *s), Some(id(2)));
        assert!(
            resolved.get(&2).is_none(),
            "0.60 is not the same event as 0.99, in one recording"
        );
    }

    /// The same weak match stands when it is what the recording is producing —
    /// everybody heard through a bad room, rather than one odd voice.
    #[test]
    fn a_uniformly_weak_recording_still_recognises_everyone() {
        let enrolled = vec![
            (id(1), unit(&[1.0, 0.0, 0.0, 0.0])),
            (id(2), unit(&[0.0, 1.0, 0.0, 0.0])),
            (id(3), unit(&[0.0, 0.0, 1.0, 0.0])),
        ];
        let voices = vec![
            (0, unit(&[0.60, 0.05, 0.05, 0.79])),
            (1, unit(&[0.05, 0.62, 0.05, 0.78])),
            (2, unit(&[0.05, 0.05, 0.58, 0.81])),
        ];

        let resolved = resolve_identities(&voices, &enrolled, enroll_match_threshold());
        assert_eq!(resolved.len(), 3, "resolved = {resolved:?}");
    }

    #[test]
    fn median_handles_both_parities() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[0.4]), Some(0.4));
        assert_eq!(median(&[0.2, 0.4, 0.9]), Some(0.4));
        assert_eq!(median(&[0.2, 0.4, 0.6, 0.8]), Some(0.5));
    }

    #[test]
    fn a_stranger_stays_anonymous() {
        let voices = vec![(0, unit(&[0.0, 0.0, 1.0]))];
        let enrolled = vec![(id(1), unit(&[1.0, 0.0, 0.0]))];
        assert!(resolve_identities(&voices, &enrolled, enroll_match_threshold()).is_empty());
    }

    #[test]
    fn nothing_enrolled_resolves_nothing() {
        let voices = vec![(0, unit(&[1.0, 0.0, 0.0]))];
        assert!(resolve_identities(&voices, &[], enroll_match_threshold()).is_empty());
    }

    /// Database row order must not change who gets matched.
    #[test]
    fn the_result_does_not_depend_on_candidate_order() {
        let voices = vec![(0, unit(&[1.0, 0.05, 0.0])), (1, unit(&[0.0, 1.0, 0.05]))];
        let mut enrolled = vec![(id(1), unit(&[1.0, 0.0, 0.0])), (id(2), unit(&[0.0, 1.0, 0.0]))];

        let forward = resolve_identities(&voices, &enrolled, enroll_match_threshold());
        enrolled.reverse();
        let reversed = resolve_identities(&voices, &enrolled, enroll_match_threshold());

        assert_eq!(
            forward.get(&0).map(|(s, _)| *s),
            reversed.get(&0).map(|(s, _)| *s)
        );
        assert_eq!(
            forward.get(&1).map(|(s, _)| *s),
            reversed.get(&1).map(|(s, _)| *s)
        );
    }
}
