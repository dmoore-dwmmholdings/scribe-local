//! `recordings` table: meeting metadata + lifecycle (design §10).

use scribe_core::types::{Recording, RecordingStatus};
use scribe_core::{Error, Result};
use sqlx::Row;
use uuid::Uuid;

use crate::db_err;
use crate::row::recording_from_row;
use crate::Db;

/// Columns selected for a full [`Recording`] row, in one place so every query
/// stays in sync with [`recording_from_row`].
const COLS: &str = "id, title, created_at, device_id, duration_ms, status, \
                    participants_expected, audio_format, sample_rate, storage_key, tags, marks";

impl Db {
    /// Insert a new recording in the `uploading` state and return it.
    pub async fn create_recording(
        &self,
        title: Option<&str>,
        device_id: Option<&str>,
        participants_expected: Option<i32>,
        audio_format: Option<&str>,
        sample_rate: Option<i32>,
    ) -> Result<Recording> {
        // `title_auto` is set when the user gave no title, so the LLM may name it.
        let sql = format!(
            "INSERT INTO recordings (title, device_id, participants_expected, audio_format, sample_rate, title_auto) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {COLS}"
        );
        let row = sqlx::query(&sql)
            .bind(title)
            .bind(device_id)
            .bind(participants_expected)
            .bind(audio_format)
            .bind(sample_rate)
            .bind(title.is_none())
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        recording_from_row(&row)
    }

    /// Fetch a recording by id. Returns [`Error::NotFound`] when absent.
    pub async fn get_recording(&self, id: Uuid) -> Result<Recording> {
        let sql = format!("SELECT {COLS} FROM recordings WHERE id = $1");
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?
            .ok_or_else(|| Error::NotFound(format!("recording {id}")))?;
        recording_from_row(&row)
    }

    /// List recordings, newest first, paginated. When `tag` is `Some`, only
    /// recordings carrying that tag are returned (`$tag = ANY(tags)`).
    pub async fn list_recordings(
        &self,
        limit: i64,
        offset: i64,
        tag: Option<&str>,
    ) -> Result<Vec<Recording>> {
        let rows = match tag {
            Some(tag) => {
                let sql = format!(
                    "SELECT {COLS} FROM recordings WHERE $3 = ANY(tags) \
                     ORDER BY created_at DESC LIMIT $1 OFFSET $2"
                );
                sqlx::query(&sql)
                    .bind(limit)
                    .bind(offset)
                    .bind(tag)
                    .fetch_all(self.pool())
                    .await
                    .map_err(db_err)?
            }
            None => {
                let sql = format!(
                    "SELECT {COLS} FROM recordings ORDER BY created_at DESC LIMIT $1 OFFSET $2"
                );
                sqlx::query(&sql)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(self.pool())
                    .await
                    .map_err(db_err)?
            }
        };
        rows.iter().map(recording_from_row).collect()
    }

    /// Update the lifecycle status. Returns [`Error::NotFound`] if the id is unknown.
    pub async fn set_recording_status(&self, id: Uuid, status: RecordingStatus) -> Result<()> {
        let affected = sqlx::query("UPDATE recordings SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status.as_str())
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        not_found_if_zero(affected, id)
    }

    /// Record the measured duration (ms) once transcoded.
    pub async fn set_recording_duration(&self, id: Uuid, duration_ms: i64) -> Result<()> {
        let affected = sqlx::query("UPDATE recordings SET duration_ms = $2 WHERE id = $1")
            .bind(id)
            .bind(duration_ms)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        not_found_if_zero(affected, id)
    }

    /// Persist the recording's bookmark marks (millisecond offsets into the
    /// audio), replacing any existing set. Captured by the client during
    /// recording and supplied at completion. Returns [`Error::NotFound`] if the
    /// id is unknown.
    pub async fn set_recording_marks(&self, id: Uuid, marks: &[i32]) -> Result<()> {
        let affected = sqlx::query("UPDATE recordings SET marks = $1 WHERE id = $2")
            .bind(marks)
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        not_found_if_zero(affected, id)
    }

    /// Set the recording's display title (used to auto-name from the LLM summary
    /// when the user didn't provide one).
    pub async fn set_recording_title(&self, id: Uuid, title: &str) -> Result<()> {
        let affected = sqlx::query("UPDATE recordings SET title = $2 WHERE id = $1")
            .bind(id)
            .bind(title)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        not_found_if_zero(affected, id)
    }

    /// Set an **auto-generated** title (continuous regeneration + final summary).
    /// Only overwrites a title that is itself auto-generated or empty — never a
    /// user-provided one. Marks the title auto so later passes may keep updating
    /// it. Returns whether a row was updated (false when a user title was kept).
    pub async fn set_recording_title_auto(&self, id: Uuid, title: &str) -> Result<bool> {
        let affected = sqlx::query(
            "UPDATE recordings SET title = $2, title_auto = true \
             WHERE id = $1 AND (title_auto OR title IS NULL OR title = '')",
        )
        .bind(id)
        .bind(title)
        .execute(self.pool())
        .await
        .map_err(db_err)?
        .rows_affected();
        Ok(affected > 0)
    }

    /// Set the blob-store key prefix for this recording's audio.
    pub async fn set_recording_storage_key(&self, id: Uuid, storage_key: &str) -> Result<()> {
        let affected = sqlx::query("UPDATE recordings SET storage_key = $2 WHERE id = $1")
            .bind(id)
            .bind(storage_key)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        not_found_if_zero(affected, id)
    }

    /// Count recordings (handy for paging UIs).
    pub async fn count_recordings(&self) -> Result<i64> {
        let row = sqlx::query("SELECT count(*) AS n FROM recordings")
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        row.try_get::<i64, _>("n").map_err(db_err)
    }

    /// Replace a recording's tags with a normalized set and return the stored
    /// list. Tags are trimmed, lowercased, empties dropped, and de-duplicated
    /// preserving first-seen order. Returns [`Error::NotFound`] if the id is
    /// unknown (mirrors [`Db::get_recording`]).
    pub async fn set_recording_tags(&self, id: Uuid, tags: &[String]) -> Result<Vec<String>> {
        let normalized = normalize_tags(tags);
        let row = sqlx::query("UPDATE recordings SET tags = $1 WHERE id = $2 RETURNING tags")
            .bind(&normalized)
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?
            .ok_or_else(|| Error::NotFound(format!("recording {id}")))?;
        row.try_get::<Vec<String>, _>("tags").map_err(db_err)
    }

    /// Delete the recording's pipeline scratch artifacts (`recording_artifacts`:
    /// the transcript/diarization hand-offs). Used when reprocessing so the
    /// re-run regenerates them from scratch. Returns the number of rows removed.
    pub async fn delete_artifacts_by_recording(&self, recording_id: Uuid) -> Result<u64> {
        let affected = sqlx::query("DELETE FROM recording_artifacts WHERE recording_id = $1")
            .bind(recording_id)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        Ok(affected)
    }

    /// Reset a recording for a full pipeline re-run (Feature D — reprocess).
    ///
    /// In a single transaction, delete every piece of DERIVED data —
    /// utterances, chunks, summaries (all templates), pipeline scratch
    /// artifacts, and the recording's queue jobs (so stale `done` predecessors
    /// don't gate the re-run) — then flip the recording to `processing`. The
    /// transcoded audio/segments on disk are left untouched. The caller enqueues
    /// the `transcode` job afterwards, which cascades the rest of the DAG.
    ///
    /// Returns [`Error::NotFound`] if the recording id is unknown (the status
    /// update affects zero rows).
    pub async fn reset_for_reprocess(&self, id: Uuid) -> Result<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;

        for table in [
            "utterances",
            "chunks",
            "summaries",
            "recording_artifacts",
            "jobs",
        ] {
            sqlx::query(&format!("DELETE FROM {table} WHERE recording_id = $1"))
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }

        let affected = sqlx::query("UPDATE recordings SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(RecordingStatus::Processing.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?
            .rows_affected();
        not_found_if_zero(affected, id)?;

        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    /// Every distinct tag in use across all recordings, sorted alphabetically.
    pub async fn distinct_tags(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT DISTINCT unnest(tags) AS t FROM recordings WHERE tags <> '{}' ORDER BY t",
        )
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter()
            .map(|r| r.try_get::<String, _>("t").map_err(db_err))
            .collect()
    }
}

/// Normalize a tag list: trim, lowercase, drop empties, dedupe preserving the
/// order tags were first seen.
fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(tags.len());
    for tag in tags {
        let t = tag.trim().to_lowercase();
        if t.is_empty() {
            continue;
        }
        if seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out
}

fn not_found_if_zero(affected: u64, id: Uuid) -> Result<()> {
    if affected == 0 {
        Err(Error::NotFound(format!("recording {id}")))
    } else {
        Ok(())
    }
}
