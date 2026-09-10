//! Stage 2 — diarize (design §7/§8).
//!
//! Run the diarizer over the transcoded WAV to get speaker turns + per-speaker
//! embeddings. For each diarized speaker we persist its embedding to
//! `recording_speakers`, then try to match it against an enrolled voice
//! (cosine ≥ 0.5); a hit attaches the known `speaker_id`. The turns are parked
//! in the scratch artifacts table for the merge stage.

use std::collections::BTreeSet;

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

    for &local_idx in &speakers {
        let embedding = diarization.embeddings.get(&local_idx);

        // Match to an enrolled voice, when there is a voiceprint to match on.
        let matched = match embedding {
            Some(e) => db.match_speaker_by_embedding(e, ENROLL_MATCH_THRESHOLD).await?,
            None => None,
        };
        let speaker_id = matched.as_ref().map(|(s, _)| s.id);
        if let Some((s, sim)) = &matched {
            tracing::info!(
                %recording_id, local_idx, speaker = %s.display_name, similarity = sim,
                "diarize: matched enrolled speaker"
            );
        }
        db.upsert_recording_speaker(recording_id, local_idx, speaker_id, embedding.cloned())
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
