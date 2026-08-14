//! `summaries` table: LLM-generated metadata, one row PER TEMPLATE per
//! recording (design §9). A recording accumulates multiple summaries — one per
//! template — all retained; re-summarizing with a new template adds a view.

use scribe_core::types::Summary;
use scribe_core::Result;
use serde_json::Value;
use uuid::Uuid;

use crate::db_err;
use crate::row::summary_from_row;
use crate::Db;

const COLS: &str =
    "recording_id, title, summary, action_items, topics, decisions, model, template, created_at";

impl Db {
    /// Insert or replace the summary for a recording **and template**, keyed on
    /// `(recording_id, template)`. Re-summarizing with the same template
    /// overwrites that view and refreshes `created_at`; a new template adds a
    /// fresh row, leaving the others intact. `template` defaults to "general"
    /// when not supplied (matching the column default).
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
        template: Option<&str>,
    ) -> Result<Summary> {
        // `template` is NOT NULL with a "general" default; normalize an absent
        // value to that default so the conflict target always has a value.
        let template = template.unwrap_or("general");
        let sql = format!(
            "INSERT INTO summaries \
               (recording_id, title, summary, action_items, topics, decisions, model, template, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now()) \
             ON CONFLICT (recording_id, template) DO UPDATE SET \
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
            .bind(template)
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        summary_from_row(&row)
    }

    /// Fetch one of a recording's summaries (the earliest by `created_at`), or
    /// `None` if none generated yet. With multiple per-template views this is
    /// ambiguous; prefer [`Db::list_summaries_by_recording`]. Retained for
    /// callers/tests that only need "some summary exists".
    pub async fn get_summary(&self, recording_id: Uuid) -> Result<Option<Summary>> {
        let sql = format!(
            "SELECT {COLS} FROM summaries WHERE recording_id = $1 \
             ORDER BY created_at, template LIMIT 1"
        );
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

    /// All of a recording's summaries — one per template — oldest first. Each
    /// carries its `template`, so the client can present multiple views.
    pub async fn list_summaries_by_recording(&self, recording_id: Uuid) -> Result<Vec<Summary>> {
        let sql = format!(
            "SELECT {COLS} FROM summaries WHERE recording_id = $1 \
             ORDER BY created_at, template"
        );
        let rows = sqlx::query(&sql)
            .bind(recording_id)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(summary_from_row).collect()
    }

    /// Delete every summary (all templates) for a recording — used when
    /// reprocessing clears derived data. Returns the number of rows removed.
    pub async fn delete_summaries_by_recording(&self, recording_id: Uuid) -> Result<u64> {
        let affected = sqlx::query("DELETE FROM summaries WHERE recording_id = $1")
            .bind(recording_id)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        Ok(affected)
    }
}
