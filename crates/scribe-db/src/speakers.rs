//! `speakers` table: enrolled, cross-recording voice identity (design §8).
//!
//! Embeddings are `vector(192)`. Matching uses pgvector cosine distance
//! (`<=>`); we convert distance `d` to cosine similarity `1 - d` and accept a
//! match when similarity ≥ threshold.

use pgvector::Vector;
use scribe_core::types::Speaker;
use scribe_core::{Error, Result};
use sqlx::Row;
use uuid::Uuid;

use crate::db_err;
use crate::row::speaker_from_row;
use crate::Db;

const COLS: &str = "id, display_name, embedding, created_at";

impl Db {
    /// Enrol a new named voice. `embedding` is the speaker-embedding vector
    /// (192-dim); `None` for a name-only placeholder.
    pub async fn create_speaker(
        &self,
        display_name: &str,
        embedding: Option<Vec<f32>>,
    ) -> Result<Speaker> {
        let vec = embedding.map(Vector::from);
        let sql = format!(
            "INSERT INTO speakers (display_name, embedding) VALUES ($1, $2) RETURNING {COLS}"
        );
        let row = sqlx::query(&sql)
            .bind(display_name)
            .bind(vec)
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        speaker_from_row(&row)
    }

    /// All enrolled speakers, alphabetical.
    pub async fn list_speakers(&self) -> Result<Vec<Speaker>> {
        let sql = format!("SELECT {COLS} FROM speakers ORDER BY display_name");
        let rows = sqlx::query(&sql)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(speaker_from_row).collect()
    }

    /// All enrolled speakers with the number of recordings each is tagged in.
    ///
    /// Same order as [`Db::list_speakers`]; the count comes from
    /// `recording_speakers`, so it reflects both manual tags and voiceprint
    /// auto-matches.
    pub async fn list_speakers_with_usage(&self) -> Result<Vec<(Speaker, i64)>> {
        let sql = format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM recording_speakers rs WHERE rs.speaker_id = s.id) \
                 AS recording_count \
             FROM speakers s ORDER BY s.display_name"
        );
        let rows = sqlx::query(&sql)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter()
            .map(|row| {
                let count: i64 = row.try_get("recording_count").map_err(db_err)?;
                Ok((speaker_from_row(row)?, count))
            })
            .collect()
    }

    /// Attach (or replace) an enrolled speaker's reference voiceprint.
    ///
    /// This is what makes a name stick across recordings: the diarize stage
    /// matches future recordings against these vectors, so a speaker without one
    /// is only ever a manual label.
    pub async fn set_speaker_embedding(&self, id: Uuid, embedding: &[f32]) -> Result<()> {
        let vec = Vector::from(embedding.to_vec());
        let affected = sqlx::query("UPDATE speakers SET embedding = $2 WHERE id = $1")
            .bind(id)
            .bind(vec)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        if affected == 0 {
            Err(Error::NotFound(format!("speaker {id}")))
        } else {
            Ok(())
        }
    }

    /// Fetch a speaker by id. [`Error::NotFound`] when absent.
    pub async fn get_speaker(&self, id: Uuid) -> Result<Speaker> {
        let sql = format!("SELECT {COLS} FROM speakers WHERE id = $1");
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?
            .ok_or_else(|| Error::NotFound(format!("speaker {id}")))?;
        speaker_from_row(&row)
    }

    /// Find the closest enrolled speaker to `embedding` by cosine similarity.
    ///
    /// Returns `Some((speaker, similarity))` when the best match's similarity is
    /// ≥ `threshold`, else `None`. Only speakers with a non-null embedding are
    /// considered. `similarity = 1 - cosine_distance`.
    pub async fn match_speaker_by_embedding(
        &self,
        embedding: &[f32],
        threshold: f32,
    ) -> Result<Option<(Speaker, f32)>> {
        let query_vec = Vector::from(embedding.to_vec());
        // `<=>` is cosine distance; smaller is closer. Order ascending, take top.
        let sql = format!(
            "SELECT {COLS}, 1 - (embedding <=> $1) AS similarity \
             FROM speakers \
             WHERE embedding IS NOT NULL \
             ORDER BY embedding <=> $1 \
             LIMIT 1"
        );
        let row = sqlx::query(&sql)
            .bind(query_vec)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        let Some(row) = row else {
            return Ok(None);
        };
        // similarity comes back as float8.
        let similarity: f64 = row.try_get("similarity").map_err(db_err)?;
        if (similarity as f32) < threshold {
            return Ok(None);
        }
        let speaker = speaker_from_row(&row)?;
        Ok(Some((speaker, similarity as f32)))
    }

    /// Rename a speaker. [`Error::NotFound`] if the id is unknown.
    pub async fn rename_speaker(&self, id: Uuid, display_name: &str) -> Result<()> {
        let affected = sqlx::query("UPDATE speakers SET display_name = $2 WHERE id = $1")
            .bind(id)
            .bind(display_name)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        if affected == 0 {
            Err(Error::NotFound(format!("speaker {id}")))
        } else {
            Ok(())
        }
    }

    /// Delete a speaker. Referencing `recording_speakers.speaker_id` rows are
    /// `ON DELETE SET NULL` (per the schema), so they revert to anonymous.
    pub async fn delete_speaker(&self, id: Uuid) -> Result<()> {
        let affected = sqlx::query("DELETE FROM speakers WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        if affected == 0 {
            Err(Error::NotFound(format!("speaker {id}")))
        } else {
            Ok(())
        }
    }
}
