//! The work queue (design §7).
//!
//! Postgres *is* the broker. Jobs are claimed atomically with
//! `UPDATE … WHERE id = (SELECT … FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING *`,
//! and an idle worker is woken by `LISTEN scribe_jobs` (the insert/requeue
//! triggers in `0001_init.sql` fire the NOTIFY).
//!
//! Crash recovery follows the design: claim fast, stamp `locked_at`/`locked_by`,
//! do the heavy work outside any transaction, heartbeat `locked_at` periodically,
//! and let [`Db::reap_stuck`] requeue any `running` job whose lease has expired.

use std::time::Duration;

use scribe_core::types::{Job, JobKind};
use scribe_core::Result;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::db_err;
use crate::row::job_from_row;
use crate::Db;

const COLS: &str = "id, recording_id, kind, state, priority, attempts, run_after, \
                    locked_by, locked_at, payload, error, created_at, updated_at";

/// Recorded against a job whose worker stopped renewing its lease.
///
/// Distinct from a stage error on purpose: a stage that returns `Err` reports
/// what went wrong, while this is what is left when the worker never got to
/// report anything at all.
pub const LEASE_EXPIRED: &str =
    "worker lease expired: the worker stopped renewing it (crashed, killed, or lost the database)";

/// A job the reaper took back from a worker that stopped renewing its lease.
#[derive(Debug, Clone)]
pub struct ReapedJob {
    pub id: i64,
    pub recording_id: Option<Uuid>,
    /// True when this reap used the last attempt, so the job is now `failed`
    /// rather than queued for another try.
    pub exhausted: bool,
}

impl Db {
    /// Enqueue a job for `recording_id`/`kind`.
    ///
    /// Idempotent against the partial-unique index
    /// `(recording_id, kind) WHERE state IN ('queued','running')`: if a live job
    /// already exists this is a no-op and the existing job is returned, so a
    /// stage that re-enqueues a successor never creates a duplicate.
    pub async fn enqueue(
        &self,
        recording_id: Uuid,
        kind: JobKind,
        payload: Value,
    ) -> Result<Job> {
        self.enqueue_with_priority(recording_id, kind, payload, 0)
            .await
    }

    /// Like [`enqueue`](Self::enqueue) but with an explicit priority (higher
    /// runs first).
    pub async fn enqueue_with_priority(
        &self,
        recording_id: Uuid,
        kind: JobKind,
        payload: Value,
        priority: i32,
    ) -> Result<Job> {
        // The conflict target is the *partial* unique index, so the predicate
        // must be repeated here for Postgres to match it.
        let sql = format!(
            "INSERT INTO jobs (recording_id, kind, payload, priority) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (recording_id, kind) WHERE state IN ('queued','running') \
             DO NOTHING \
             RETURNING {COLS}"
        );
        let inserted = sqlx::query(&sql)
            .bind(recording_id)
            .bind(kind.as_str())
            .bind(payload)
            .bind(priority)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;

        if let Some(row) = inserted {
            return job_from_row(&row);
        }

        // Conflict → a live job already exists; return it.
        let existing_sql = format!(
            "SELECT {COLS} FROM jobs \
             WHERE recording_id = $1 AND kind = $2 AND state IN ('queued','running') \
             ORDER BY id LIMIT 1"
        );
        let row = sqlx::query(&existing_sql)
            .bind(recording_id)
            .bind(kind.as_str())
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        job_from_row(&row)
    }

    /// Atomically claim the next runnable job whose kind is in `kinds`.
    ///
    /// Implements the design §7 claim query: among `queued` jobs with
    /// `run_after <= now()`, take the highest priority / oldest, skipping any row
    /// another worker already holds (`FOR UPDATE SKIP LOCKED`). Returns `None`
    /// when the queue is empty for these kinds.
    pub async fn claim_one(&self, worker_id: &str, kinds: &[JobKind]) -> Result<Option<Job>> {
        if kinds.is_empty() {
            return Ok(None);
        }
        let kind_strs: Vec<&str> = kinds.iter().map(JobKind::as_str).collect();

        let sql = format!(
            "UPDATE jobs SET state = 'running', locked_by = $1, locked_at = now() \
             WHERE id = ( \
               SELECT id FROM jobs \
               WHERE state = 'queued' AND kind = ANY($2) AND run_after <= now() \
               ORDER BY priority DESC, created_at \
               FOR UPDATE SKIP LOCKED \
               LIMIT 1 \
             ) \
             RETURNING {COLS}"
        );
        let row = sqlx::query(&sql)
            .bind(worker_id)
            .bind(&kind_strs)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        match row {
            Some(r) => Ok(Some(job_from_row(&r)?)),
            None => Ok(None),
        }
    }

    /// Bump `locked_at` on a job this worker still owns (visibility-timeout
    /// heartbeat). The `locked_by` guard prevents a worker from extending a lease
    /// the reaper has already reassigned. Returns `true` if the heartbeat landed.
    pub async fn heartbeat(&self, job_id: i64, worker_id: &str) -> Result<bool> {
        let affected = sqlx::query(
            "UPDATE jobs SET locked_at = now() \
             WHERE id = $1 AND locked_by = $2 AND state = 'running'",
        )
        .bind(job_id)
        .bind(worker_id)
        .execute(self.pool())
        .await
        .map_err(db_err)?
        .rows_affected();
        Ok(affected > 0)
    }

    /// Mark a job `done` and clear its lock.
    pub async fn complete(&self, job_id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE jobs SET state = 'done', locked_by = NULL, locked_at = NULL, error = NULL \
             WHERE id = $1",
        )
        .bind(job_id)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Record a failure. Increments `attempts`; if still under `max_attempts`,
    /// requeue with exponential backoff via `run_after` (so the NOTIFY-driven
    /// worker retries later). Once attempts reach the cap, park it in `failed`
    /// with the error preserved for inspection.
    ///
    /// Returns `true` if the job was requeued, `false` if it was parked failed.
    pub async fn fail(
        &self,
        job_id: i64,
        error: &str,
        max_attempts: i32,
        backoff: Duration,
    ) -> Result<bool> {
        // `attempts` after this failure (the column starts at 0 for a fresh job).
        let next_attempts: i32 = {
            let row = sqlx::query("SELECT attempts FROM jobs WHERE id = $1")
                .bind(job_id)
                .fetch_one(self.pool())
                .await
                .map_err(db_err)?;
            row.try_get::<i32, _>("attempts").map_err(db_err)? + 1
        };

        if next_attempts < max_attempts {
            // Exponential backoff: base * 2^(attempts-1), as a seconds interval.
            let base_secs = backoff.as_secs_f64().max(0.0);
            let delay_secs = base_secs * 2f64.powi(next_attempts - 1);
            sqlx::query(
                "UPDATE jobs SET state = 'queued', attempts = $2, error = $3, \
                   locked_by = NULL, locked_at = NULL, \
                   run_after = now() + make_interval(secs => $4) \
                 WHERE id = $1",
            )
            .bind(job_id)
            .bind(next_attempts)
            .bind(error)
            .bind(delay_secs)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
            Ok(true)
        } else {
            sqlx::query(
                "UPDATE jobs SET state = 'failed', attempts = $2, error = $3, \
                   locked_by = NULL, locked_at = NULL \
                 WHERE id = $1",
            )
            .bind(job_id)
            .bind(next_attempts)
            .bind(error)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
            Ok(false)
        }
    }

    /// Requeue `running` jobs whose `locked_at` is older than `older_than`
    /// (a worker died mid-job). The requeue fires the NOTIFY trigger so another
    /// worker picks them up immediately.
    ///
    /// **The reap counts as an attempt.** A stage that returns `Err` is counted
    /// by [`Db::fail`], but a stage that takes the whole worker process down
    /// never reaches that code — and a native abort does exactly this, since a
    /// C++ exception from onnxruntime cannot be caught in Rust. Without counting
    /// the reap, such a job is immortal: every worker that claims it dies, the
    /// reaper hands it to the next one, `attempts` stays at 0, and the job
    /// starves every other job in the queue for as long as anyone keeps
    /// restarting the worker. Counting the reap gives that loop the same
    /// `max_attempts` bound every other failure has.
    ///
    /// Returns one [`ReapedJob`] per job taken back, flagged with whether this
    /// reap was its last attempt.
    pub async fn reap_stuck(&self, older_than: Duration, max_attempts: i32) -> Result<Vec<ReapedJob>> {
        let secs = older_than.as_secs_f64();
        let rows = sqlx::query(
            "UPDATE jobs SET \
               attempts  = attempts + 1, \
               state     = CASE WHEN attempts + 1 >= $2 THEN 'failed' ELSE 'queued' END, \
               error     = $3, \
               locked_by = NULL, \
               locked_at = NULL \
             WHERE state = 'running' \
               AND locked_at IS NOT NULL \
               AND locked_at < now() - make_interval(secs => $1) \
             RETURNING id, recording_id, state = 'failed' AS exhausted",
        )
        .bind(secs)
        .bind(max_attempts)
        .bind(LEASE_EXPIRED)
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;

        rows.iter()
            .map(|r| {
                Ok(ReapedJob {
                    id: r.try_get("id").map_err(db_err)?,
                    recording_id: r.try_get("recording_id").map_err(db_err)?,
                    exhausted: r.try_get("exhausted").map_err(db_err)?,
                })
            })
            .collect()
    }

    /// True if every predecessor stage of `kind` (per [`JobKind::predecessors`])
    /// has a `done` job for this recording — the gate for running e.g. `merge`.
    /// A kind with no predecessors is trivially ready.
    pub async fn predecessors_done(&self, recording_id: Uuid, kind: JobKind) -> Result<bool> {
        let preds = kind.predecessors();
        if preds.is_empty() {
            return Ok(true);
        }
        let pred_strs: Vec<&str> = preds.iter().map(JobKind::as_str).collect();
        // Count distinct predecessor kinds that have a done job; ready iff all present.
        let row = sqlx::query(
            "SELECT count(DISTINCT kind) AS n FROM jobs \
             WHERE recording_id = $1 AND kind = ANY($2) AND state = 'done'",
        )
        .bind(recording_id)
        .bind(&pred_strs)
        .fetch_one(self.pool())
        .await
        .map_err(db_err)?;
        let done: i64 = row.try_get("n").map_err(db_err)?;
        Ok(done as usize == preds.len())
    }

    /// Delete every queue row for a recording. Used when reprocessing so the
    /// fresh pipeline run isn't gated by stale `done` predecessor jobs from a
    /// prior run (`predecessors_done` / the `ready` check count `done` jobs).
    /// Returns the number of jobs removed.
    pub async fn delete_jobs_by_recording(&self, recording_id: Uuid) -> Result<u64> {
        let affected = sqlx::query("DELETE FROM jobs WHERE recording_id = $1")
            .bind(recording_id)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        Ok(affected)
    }

    /// Every job belonging to `recording_id`, oldest first.
    ///
    /// Used to report pipeline progress to the client: stages are enqueued
    /// lazily (each one's successors only appear once it finishes), so the
    /// caller must treat an absent kind as "not started yet" rather than
    /// assuming the full DAG is present.
    pub async fn list_jobs_by_recording(&self, recording_id: Uuid) -> Result<Vec<Job>> {
        let sql = format!("SELECT {COLS} FROM jobs WHERE recording_id = $1 ORDER BY id");
        let rows = sqlx::query(&sql)
            .bind(recording_id)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(job_from_row).collect()
    }

    /// Fetch a job by id (mostly for tests / observability).
    pub async fn get_job(&self, job_id: i64) -> Result<Option<Job>> {
        let sql = format!("SELECT {COLS} FROM jobs WHERE id = $1");
        let row = sqlx::query(&sql)
            .bind(job_id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        match row {
            Some(r) => Ok(Some(job_from_row(&r)?)),
            None => Ok(None),
        }
    }
}
