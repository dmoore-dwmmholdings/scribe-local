//! Server-side settings: the `app_settings` key/value table (migration 0009).
//!
//! One JSONB document per key, read by whichever role cares. Today that is the
//! processing schedule — the worker reads it to decide whether it may claim
//! heavy jobs, the API reads and writes it for the mobile app.
//!
//! Reads never fail on bad data. A settings row is edited through a validating
//! API, but it is also plain JSONB that an operator can `psql` into, and a
//! worker that refuses to start because someone fat-fingered a window is worse
//! than one that logs the problem and falls back to the default (which is
//! "no schedule", i.e. process everything).

use scribe_core::schedule::{self, ProcessingSchedule};
use scribe_core::Result;
use serde_json::Value;
use sqlx::Row;

use crate::db_err;
use crate::Db;

impl Db {
    /// Read one settings document. `None` when the key was never written.
    pub async fn get_setting(&self, key: &str) -> Result<Option<Value>> {
        let row = sqlx::query("SELECT value FROM app_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        match row {
            Some(r) => Ok(Some(r.try_get("value").map_err(db_err)?)),
            None => Ok(None),
        }
    }

    /// Insert or replace one settings document.
    pub async fn put_setting(&self, key: &str, value: &Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO app_settings (key, value, updated_at) VALUES ($1, $2, now()) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(key)
        .bind(value)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// The processing schedule, normalized. Falls back to
    /// [`ProcessingSchedule::default`] (schedule off → everything runs) when the
    /// row is absent or unreadable.
    pub async fn processing_schedule(&self) -> Result<ProcessingSchedule> {
        let raw = self.get_setting(schedule::SETTING_KEY).await?;
        let mut sched = match raw {
            Some(v) => match serde_json::from_value::<ProcessingSchedule>(v) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "stored processing schedule is unreadable; falling back to no schedule"
                    );
                    ProcessingSchedule::default()
                }
            },
            None => ProcessingSchedule::default(),
        };
        sched.normalize();
        Ok(sched)
    }

    /// Persist the processing schedule (normalized first).
    pub async fn set_processing_schedule(&self, sched: &ProcessingSchedule) -> Result<ProcessingSchedule> {
        let mut sched = sched.clone();
        sched.normalize();
        let value = serde_json::to_value(&sched)
            .map_err(|e| scribe_core::Error::Internal(format!("serializing schedule: {e}")))?;
        self.put_setting(schedule::SETTING_KEY, &value).await?;
        Ok(sched)
    }
}
