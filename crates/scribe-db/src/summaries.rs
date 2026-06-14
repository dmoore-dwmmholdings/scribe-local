//! `summaries` table: LLM-generated metadata, one row per recording (design §9).

use scribe_core::types::Summary;
use scribe_core::Result;
use serde_json::Value;
use uuid::Uuid;

use crate::db_err;
use crate::row::summary_from_row;
use crate::Db;

const COLS: &str =
    "recording_id, title, summary, action_items, topics, decisions, model, created_at";

impl Db {
    /// Insert or replace the summary for a recording (keyed on `recording_id`).
    /// Re-summarizing overwrites the previous result and refreshes `created_at`.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_summary(
        &self,
        recording_id: Uuid,
        title: Option<&str>,
        summary: Option<&str>,
        action_items: Value,
        topics: Value,
        decisions: Value,
        model: Option<&str>,
    ) -> Result<Summary> {
        let sql = format!(
            "INSERT INTO summaries \
               (recording_id, title, summary, action_items, topics, decisions, model, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
             ON CONFLICT (recording_id) DO UPDATE SET \
               title        = EXCLUDED.title, \
               summary      = EXCLUDED.summary, \
               action_items = EXCLUDED.action_items, \
               topics       = EXCLUDED.topics, \
               decisions    = EXCLUDED.decisions, \
               model        = EXCLUDED.model, \
               created_at   = now() \
             RETURNING {COLS}"
        );
        let row = sqlx::query(&sql)
            .bind(recording_id)
            .bind(title)
            .bind(summary)
            .bind(action_items)
            .bind(topics)
            .bind(decisions)
            .bind(model)
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        summary_from_row(&row)
    }

    /// Fetch a recording's summary, or `None` if not yet generated.
    pub async fn get_summary(&self, recording_id: Uuid) -> Result<Option<Summary>> {
        let sql = format!("SELECT {COLS} FROM summaries WHERE recording_id = $1");
        let row = sqlx::query(&sql)
            .bind(recording_id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        match row {
            Some(r) => Ok(Some(summary_from_row(&r)?)),
            None => Ok(None),
        }
    }
}
