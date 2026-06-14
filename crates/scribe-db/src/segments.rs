//! `segments` table: chunked-upload pieces (design §10).
//!
//! Inserts are idempotent on `(recording_id, seq)` — a retried tus segment
//! upload must not create a duplicate row, so we upsert.

use scribe_core::types::Segment;
use scribe_core::Result;
use uuid::Uuid;

use crate::db_err;
use crate::row::segment_from_row;
use crate::Db;

const COLS: &str =
    "id, recording_id, seq, storage_key, start_ms, duration_ms, bytes, sha256, uploaded_at";

impl Db {
    /// Insert or update a segment, keyed on `(recording_id, seq)`.
    ///
    /// On conflict the mutable fields are refreshed (a re-uploaded segment may
    /// carry a corrected length/hash) and `uploaded_at` is bumped to now.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_segment(
        &self,
        recording_id: Uuid,
        seq: i32,
        storage_key: &str,
        start_ms: Option<i64>,
        duration_ms: Option<i64>,
        bytes: Option<i64>,
        sha256: Option<&[u8]>,
    ) -> Result<Segment> {
        let sql = format!(
            "INSERT INTO segments (recording_id, seq, storage_key, start_ms, duration_ms, bytes, sha256) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (recording_id, seq) DO UPDATE SET \
               storage_key = EXCLUDED.storage_key, \
               start_ms    = EXCLUDED.start_ms, \
               duration_ms = EXCLUDED.duration_ms, \
               bytes       = EXCLUDED.bytes, \
               sha256      = EXCLUDED.sha256, \
               uploaded_at = now() \
             RETURNING {COLS}"
        );
        let row = sqlx::query(&sql)
            .bind(recording_id)
            .bind(seq)
            .bind(storage_key)
            .bind(start_ms)
            .bind(duration_ms)
            .bind(bytes)
            .bind(sha256)
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        segment_from_row(&row)
    }

    /// All segments for a recording, ordered by sequence (upload/stitch order).
    pub async fn list_segments_by_recording(&self, recording_id: Uuid) -> Result<Vec<Segment>> {
        let sql = format!("SELECT {COLS} FROM segments WHERE recording_id = $1 ORDER BY seq");
        let rows = sqlx::query(&sql)
            .bind(recording_id)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(segment_from_row).collect()
    }

    // --- Incremental transcription (design: live) ---------------------------

    /// Segments not yet transcribed into provisional utterances, in seq order.
    pub async fn list_untranscribed_segments(&self, recording_id: Uuid) -> Result<Vec<Segment>> {
        let sql = format!(
            "SELECT {COLS} FROM segments \
             WHERE recording_id = $1 AND transcribed_at IS NULL ORDER BY seq"
        );
        let rows = sqlx::query(&sql)
            .bind(recording_id)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(segment_from_row).collect()
    }

    /// Mark a segment transcribed, storing its measured global offset + duration.
    pub async fn mark_segment_transcribed(
        &self,
        segment_id: Uuid,
        start_ms: i64,
        duration_ms: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE segments SET transcribed_at = now(), start_ms = $2, duration_ms = $3 \
             WHERE id = $1",
        )
        .bind(segment_id)
        .bind(start_ms)
        .bind(duration_ms)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Count of segments still awaiting incremental transcription.
    pub async fn count_untranscribed_segments(&self, recording_id: Uuid) -> Result<i64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM segments WHERE recording_id = $1 AND transcribed_at IS NULL",
        )
        .bind(recording_id)
        .fetch_one(self.pool())
        .await
        .map_err(db_err)?;
        Ok(n)
    }

    /// The global end time (ms) of the last already-transcribed segment, used as
    /// the timeline offset for the next batch. `None` when nothing is transcribed.
    pub async fn max_transcribed_end_ms(&self, recording_id: Uuid) -> Result<Option<i64>> {
        let v: Option<i64> = sqlx::query_scalar(
            "SELECT max(coalesce(start_ms, 0) + coalesce(duration_ms, 0)) FROM segments \
             WHERE recording_id = $1 AND transcribed_at IS NOT NULL",
        )
        .bind(recording_id)
        .fetch_one(self.pool())
        .await
        .map_err(db_err)?;
        Ok(v)
    }
}
