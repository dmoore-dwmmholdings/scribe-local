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
                    participants_expected, audio_format, sample_rate, storage_key";

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
        let sql = format!(
            "INSERT INTO recordings (title, device_id, participants_expected, audio_format, sample_rate) \
             VALUES ($1, $2, $3, $4, $5) RETURNING {COLS}"
        );
        let row = sqlx::query(&sql)
            .bind(title)
            .bind(device_id)
            .bind(participants_expected)
            .bind(audio_format)
            .bind(sample_rate)
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

    /// List recordings, newest first, paginated.
    pub async fn list_recordings(&self, limit: i64, offset: i64) -> Result<Vec<Recording>> {
        let sql = format!(
            "SELECT {COLS} FROM recordings ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        );
        let rows = sqlx::query(&sql)
            .bind(limit)
            .bind(offset)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
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
}

fn not_found_if_zero(affected: u64, id: Uuid) -> Result<()> {
    if affected == 0 {
        Err(Error::NotFound(format!("recording {id}")))
    } else {
        Ok(())
    }
}
