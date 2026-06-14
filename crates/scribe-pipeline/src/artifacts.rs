//! Scratch artifacts: transient stage-to-stage hand-offs (migration 0003).
//!
//! The DAG (design §7) splits transcribe and diarize into separate jobs that
//! later converge at `merge`. Their raw outputs are not durable products, so we
//! park them in `recording_artifacts` (jsonb keyed on `(recording_id, kind)`)
//! and the merge stage reads both back. We talk to that table with raw `sqlx`
//! against `db.pool()` since `scribe-db` has no typed accessor for it.

use scribe_core::types::Word;
use scribe_core::{Error, Result};
use scribe_db::{db_err, Db};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

/// `kind` value for the transcribe stage's stored output.
pub const KIND_TRANSCRIPT: &str = "transcript";
/// `kind` value for the diarize stage's stored output.
pub const KIND_DIARIZATION: &str = "diarization";

/// The transcribe stage's persisted result: flat text + word-level timing.
/// Mirrors [`scribe_asr::Transcript`] but uses the core [`Word`] type so the
/// merge stage works against one vocabulary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranscriptArtifact {
    pub text: String,
    pub words: Vec<Word>,
}

/// One diarized speaker turn (mirrors [`scribe_asr::SpeakerTurn`]).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TurnArtifact {
    pub local_idx: i32,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// The diarize stage's persisted result: the speaker turns the merge needs.
/// Embeddings are persisted directly to `recording_speakers` by the diarize
/// stage, so only the turns travel through the scratch table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiarizationArtifact {
    pub turns: Vec<TurnArtifact>,
    pub num_speakers: usize,
}

/// Upsert a JSON artifact for `(recording_id, kind)`.
pub async fn put(db: &Db, recording_id: Uuid, kind: &str, data: &serde_json::Value) -> Result<()> {
    sqlx::query(
        "INSERT INTO recording_artifacts (recording_id, kind, data, created_at) \
         VALUES ($1, $2, $3, now()) \
         ON CONFLICT (recording_id, kind) DO UPDATE SET \
           data = EXCLUDED.data, created_at = now()",
    )
    .bind(recording_id)
    .bind(kind)
    .bind(data)
    .execute(db.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Fetch a JSON artifact for `(recording_id, kind)`, or `None` if absent.
pub async fn get(db: &Db, recording_id: Uuid, kind: &str) -> Result<Option<serde_json::Value>> {
    let row = sqlx::query(
        "SELECT data FROM recording_artifacts WHERE recording_id = $1 AND kind = $2",
    )
    .bind(recording_id)
    .bind(kind)
    .fetch_optional(db.pool())
    .await
    .map_err(db_err)?;
    match row {
        Some(r) => Ok(Some(r.try_get::<serde_json::Value, _>("data").map_err(db_err)?)),
        None => Ok(None),
    }
}

/// Store the transcribe stage's result.
pub async fn put_transcript(db: &Db, recording_id: Uuid, t: &TranscriptArtifact) -> Result<()> {
    put(db, recording_id, KIND_TRANSCRIPT, &serde_json::to_value(t)?).await
}

/// Store the diarize stage's turns.
pub async fn put_diarization(db: &Db, recording_id: Uuid, d: &DiarizationArtifact) -> Result<()> {
    put(db, recording_id, KIND_DIARIZATION, &serde_json::to_value(d)?).await
}

/// Read the transcribe stage's result back at merge time.
pub async fn get_transcript(db: &Db, recording_id: Uuid) -> Result<TranscriptArtifact> {
    let v = get(db, recording_id, KIND_TRANSCRIPT).await?.ok_or_else(|| {
        Error::pipeline("merge", "transcript artifact missing (transcribe not done?)")
    })?;
    Ok(serde_json::from_value(v)?)
}

/// Read the diarize stage's turns back at merge time.
pub async fn get_diarization(db: &Db, recording_id: Uuid) -> Result<DiarizationArtifact> {
    let v = get(db, recording_id, KIND_DIARIZATION).await?.ok_or_else(|| {
        Error::pipeline("merge", "diarization artifact missing (diarize not done?)")
    })?;
    Ok(serde_json::from_value(v)?)
}
