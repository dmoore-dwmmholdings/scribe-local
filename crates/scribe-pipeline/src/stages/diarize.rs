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

/// Similarity threshold for auto-matching a diarized voice to an enrolled one.
const ENROLL_MATCH_THRESHOLD: f32 = 0.5;

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

/// Resolve a recording's diarized voices against the enrolled ones, one to one.
///
/// Matching each voice independently against its own nearest enrolled speaker
/// is the obvious thing and it is wrong: nothing stops two of a recording's
/// voices picking the same person, and then the transcript shows Alice
/// answering Alice. A person is in the room once.
///
/// So every (voice, enrolled) pair above the threshold is considered together,
/// best first, and a pair is taken only while both sides are still free. Greedy
/// rather than optimal — with a handful of speakers the two agree, and "the
/// most confident match wins the tie" is a rule that can be explained to
/// somebody looking at a transcript wondering why it chose that.
///
/// `voices` is `(local_idx, embedding)`; `enrolled` is `(speaker_id, voiceprint)`.
fn resolve_identities(
    voices: &[(i32, Vec<f32>)],
    enrolled: &[(Uuid, Vec<f32>)],
    threshold: f32,
) -> HashMap<i32, (Uuid, f32)> {
    let mut candidates: Vec<(f32, i32, Uuid)> = Vec::new();
    for (local_idx, embedding) in voices {
        for (speaker_id, voiceprint) in enrolled {
            let sim = cosine(embedding, voiceprint);
            if sim >= threshold {
                candidates.push((sim, *local_idx, *speaker_id));
            }
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
    for (sim, local_idx, speaker_id) in candidates {
        if resolved.contains_key(&local_idx) || claimed.contains(&speaker_id) {
            continue;
        }
        resolved.insert(local_idx, (speaker_id, sim));
        claimed.insert(speaker_id);
    }
    resolved
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
    let identities = resolve_identities(&voices, &enrolled, ENROLL_MATCH_THRESHOLD);

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

        let resolved = resolve_identities(&voices, &enrolled, ENROLL_MATCH_THRESHOLD);
        assert_eq!(resolved.len(), 1);
        // The better match keeps her.
        assert_eq!(resolved.get(&0).map(|(s, _)| *s), Some(id(1)));
        assert!(resolved.get(&1).is_none());
    }

    #[test]
    fn each_voice_takes_its_own_person() {
        let voices = vec![(0, unit(&[1.0, 0.05, 0.0])), (1, unit(&[0.0, 1.0, 0.05]))];
        let enrolled = vec![(id(1), unit(&[1.0, 0.0, 0.0])), (id(2), unit(&[0.0, 1.0, 0.0]))];

        let resolved = resolve_identities(&voices, &enrolled, ENROLL_MATCH_THRESHOLD);
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

        let resolved = resolve_identities(&voices, &enrolled, ENROLL_MATCH_THRESHOLD);
        assert_eq!(resolved.get(&1).map(|(s, _)| *s), Some(id(1)), "Alice to voice 1");
        assert_eq!(resolved.get(&0).map(|(s, _)| *s), Some(id(2)), "voice 0 falls to Bob");
    }

    #[test]
    fn a_stranger_stays_anonymous() {
        let voices = vec![(0, unit(&[0.0, 0.0, 1.0]))];
        let enrolled = vec![(id(1), unit(&[1.0, 0.0, 0.0]))];
        assert!(resolve_identities(&voices, &enrolled, ENROLL_MATCH_THRESHOLD).is_empty());
    }

    #[test]
    fn nothing_enrolled_resolves_nothing() {
        let voices = vec![(0, unit(&[1.0, 0.0, 0.0]))];
        assert!(resolve_identities(&voices, &[], ENROLL_MATCH_THRESHOLD).is_empty());
    }

    /// Database row order must not change who gets matched.
    #[test]
    fn the_result_does_not_depend_on_candidate_order() {
        let voices = vec![(0, unit(&[1.0, 0.05, 0.0])), (1, unit(&[0.0, 1.0, 0.05]))];
        let mut enrolled = vec![(id(1), unit(&[1.0, 0.0, 0.0])), (id(2), unit(&[0.0, 1.0, 0.0]))];

        let forward = resolve_identities(&voices, &enrolled, ENROLL_MATCH_THRESHOLD);
        enrolled.reverse();
        let reversed = resolve_identities(&voices, &enrolled, ENROLL_MATCH_THRESHOLD);

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
