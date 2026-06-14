//! `recording_speakers` table: per-recording diarization result and its mapping
//! to enrolled [`Speaker`](scribe_core::types::Speaker)s (design §8/§10).

use pgvector::Vector;
use scribe_core::types::RecordingSpeaker;
use scribe_core::{Error, Result};
use sqlx::postgres::PgRow;
use sqlx::Row;
use uuid::Uuid;

use crate::db_err;
use crate::Db;

impl Db {
    /// Upsert a diarized speaker for a recording, keyed on
    /// `(recording_id, local_idx)`. Re-running diarization refreshes the
    /// embedding and (re)resolved identity.
    pub async fn upsert_recording_speaker(
        &self,
        recording_id: Uuid,
        local_idx: i32,
        speaker_id: Option<Uuid>,
        embedding: Option<Vec<f32>>,
    ) -> Result<()> {
        let vec = embedding.map(Vector::from);
        sqlx::query(
            "INSERT INTO recording_speakers (recording_id, local_idx, speaker_id, embedding) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (recording_id, local_idx) DO UPDATE SET \
               speaker_id = EXCLUDED.speaker_id, \
               embedding  = EXCLUDED.embedding",
        )
        .bind(recording_id)
        .bind(local_idx)
        .bind(speaker_id)
        .bind(vec)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// List the diarized speakers for a recording, ordered by `local_idx`, with
    /// each `display_name` filled from the joined [`Speaker`] when matched and
    /// defaulting to `"Speaker {local_idx}"` otherwise.
    pub async fn list_recording_speakers(
        &self,
        recording_id: Uuid,
    ) -> Result<Vec<RecordingSpeaker>> {
        let rows = sqlx::query(
            "SELECT rs.recording_id, rs.local_idx, rs.speaker_id, rs.embedding, s.display_name \
             FROM recording_speakers rs \
             LEFT JOIN speakers s ON s.id = rs.speaker_id \
             WHERE rs.recording_id = $1 \
             ORDER BY rs.local_idx",
        )
        .bind(recording_id)
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(recording_speaker_from_row).collect()
    }

    /// Attach (or clear, with `None`) the enrolled-speaker mapping for one
    /// diarized index. [`Error::NotFound`] if the `(recording_id, local_idx)`
    /// row doesn't exist.
    pub async fn set_recording_speaker(
        &self,
        recording_id: Uuid,
        local_idx: i32,
        speaker_id: Option<Uuid>,
    ) -> Result<()> {
        let affected = sqlx::query(
            "UPDATE recording_speakers SET speaker_id = $3 \
             WHERE recording_id = $1 AND local_idx = $2",
        )
        .bind(recording_id)
        .bind(local_idx)
        .bind(speaker_id)
        .execute(self.pool())
        .await
        .map_err(db_err)?
        .rows_affected();
        if affected == 0 {
            Err(Error::NotFound(format!(
                "recording_speaker ({recording_id}, {local_idx})"
            )))
        } else {
            Ok(())
        }
    }
}

fn recording_speaker_from_row(row: &PgRow) -> Result<RecordingSpeaker, Error> {
    let local_idx: i32 = row.try_get("local_idx").map_err(db_err)?;
    let resolved: Option<String> = row.try_get("display_name").map_err(db_err)?;
    let embedding: Option<Vector> = row.try_get("embedding").map_err(db_err)?;
    Ok(RecordingSpeaker {
        recording_id: row.try_get("recording_id").map_err(db_err)?,
        local_idx,
        speaker_id: row.try_get("speaker_id").map_err(db_err)?,
        embedding: embedding.map(|v| v.to_vec()),
        // Resolved name if matched, else the anonymous "Speaker N" label.
        display_name: Some(resolved.unwrap_or_else(|| format!("Speaker {local_idx}"))),
    })
}
