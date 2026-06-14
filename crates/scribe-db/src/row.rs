//! Manual row → domain-type mappers.
//!
//! We use runtime sqlx queries (no compile-time macro), so each `SELECT *`
//! result is decoded here by column name into the [`scribe_core::types`]
//! structs. Enum text columns (`status`, `kind`, `state`) go through `FromStr`;
//! a bad value becomes a [`Error::Database`] rather than a panic.

use pgvector::Vector;
use scribe_core::types::{
    Job, JobKind, JobState, Recording, RecordingStatus, Segment, Speaker, Summary,
};
use scribe_core::Error;
use sqlx::postgres::PgRow;
use sqlx::Row;

/// Helper: parse a text enum column, turning a parse error into `Database`.
fn parse_enum<T: std::str::FromStr<Err = String>>(s: &str, what: &str) -> Result<T, Error> {
    s.parse::<T>()
        .map_err(|e| Error::Database(format!("invalid {what} `{s}`: {e}")))
}

pub fn recording_from_row(row: &PgRow) -> Result<Recording, Error> {
    let status: String = row.try_get("status").map_err(crate::db_err)?;
    Ok(Recording {
        id: row.try_get("id").map_err(crate::db_err)?,
        title: row.try_get("title").map_err(crate::db_err)?,
        created_at: row.try_get("created_at").map_err(crate::db_err)?,
        device_id: row.try_get("device_id").map_err(crate::db_err)?,
        duration_ms: row.try_get("duration_ms").map_err(crate::db_err)?,
        status: parse_enum::<RecordingStatus>(&status, "recording status")?,
        participants_expected: row
            .try_get("participants_expected")
            .map_err(crate::db_err)?,
        audio_format: row.try_get("audio_format").map_err(crate::db_err)?,
        sample_rate: row.try_get("sample_rate").map_err(crate::db_err)?,
        storage_key: row.try_get("storage_key").map_err(crate::db_err)?,
    })
}

pub fn segment_from_row(row: &PgRow) -> Result<Segment, Error> {
    Ok(Segment {
        id: row.try_get("id").map_err(crate::db_err)?,
        recording_id: row.try_get("recording_id").map_err(crate::db_err)?,
        seq: row.try_get("seq").map_err(crate::db_err)?,
        storage_key: row.try_get("storage_key").map_err(crate::db_err)?,
        start_ms: row.try_get("start_ms").map_err(crate::db_err)?,
        duration_ms: row.try_get("duration_ms").map_err(crate::db_err)?,
        bytes: row.try_get("bytes").map_err(crate::db_err)?,
        sha256: row.try_get("sha256").map_err(crate::db_err)?,
        uploaded_at: row.try_get("uploaded_at").map_err(crate::db_err)?,
    })
}

pub fn job_from_row(row: &PgRow) -> Result<Job, Error> {
    let kind: String = row.try_get("kind").map_err(crate::db_err)?;
    let state: String = row.try_get("state").map_err(crate::db_err)?;
    Ok(Job {
        id: row.try_get("id").map_err(crate::db_err)?,
        recording_id: row.try_get("recording_id").map_err(crate::db_err)?,
        kind: parse_enum::<JobKind>(&kind, "job kind")?,
        state: parse_enum::<JobState>(&state, "job state")?,
        priority: row.try_get("priority").map_err(crate::db_err)?,
        attempts: row.try_get("attempts").map_err(crate::db_err)?,
        run_after: row.try_get("run_after").map_err(crate::db_err)?,
        locked_by: row.try_get("locked_by").map_err(crate::db_err)?,
        locked_at: row.try_get("locked_at").map_err(crate::db_err)?,
        payload: row.try_get("payload").map_err(crate::db_err)?,
        error: row.try_get("error").map_err(crate::db_err)?,
        created_at: row.try_get("created_at").map_err(crate::db_err)?,
        updated_at: row.try_get("updated_at").map_err(crate::db_err)?,
    })
}

pub fn speaker_from_row(row: &PgRow) -> Result<Speaker, Error> {
    // `embedding` is a nullable vector(192); decode as Option<Vector>.
    let embedding: Option<Vector> = row.try_get("embedding").map_err(crate::db_err)?;
    Ok(Speaker {
        id: row.try_get("id").map_err(crate::db_err)?,
        display_name: row.try_get("display_name").map_err(crate::db_err)?,
        embedding: embedding.map(|v| v.to_vec()),
        created_at: row.try_get("created_at").map_err(crate::db_err)?,
    })
}

pub fn summary_from_row(row: &PgRow) -> Result<Summary, Error> {
    Ok(Summary {
        recording_id: row.try_get("recording_id").map_err(crate::db_err)?,
        title: row.try_get("title").map_err(crate::db_err)?,
        summary: row.try_get("summary").map_err(crate::db_err)?,
        action_items: row.try_get("action_items").map_err(crate::db_err)?,
        topics: row.try_get("topics").map_err(crate::db_err)?,
        decisions: row.try_get("decisions").map_err(crate::db_err)?,
        model: row.try_get("model").map_err(crate::db_err)?,
        created_at: row.try_get("created_at").map_err(crate::db_err)?,
    })
}
