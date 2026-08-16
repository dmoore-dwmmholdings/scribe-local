//! The worker loop (design §7).
//!
//! Connects nothing itself — the caller hands a live [`Db`] — but owns the
//! claim/run/complete cycle: NOTIFY-driven wakeups with a poll backstop, a
//! per-job heartbeat task that keeps the visibility lease fresh, a background
//! reaper that requeues jobs whose lease expired, and bounded-concurrency
//! dispatch via a [`Semaphore`].

use std::sync::Arc;
use std::time::Duration;

use scribe_core::config::Config;
use scribe_core::types::{Job, JobKind, RecordingStatus};
use scribe_core::Result;
use scribe_db::Db;
use serde_json::Value;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::engines::Engines;
use crate::stages;

/// Dispatch one stage by kind. Shared by the worker and the inline driver so the
/// stage code path is identical regardless of how it was triggered. `payload` is
/// the job's payload jsonb (empty `{}` for the inline driver), carrying per-job
/// options such as the summarize `template`.
pub(crate) async fn run_stage(
    cfg: &Config,
    db: &Db,
    engines: &Engines,
    kind: JobKind,
    recording_id: Uuid,
    payload: &Value,
) -> Result<()> {
    match kind {
        JobKind::Transcode => stages::transcode::run(cfg, db, recording_id).await,
        JobKind::Diarize => stages::diarize::run(cfg, db, &engines.speech, recording_id).await,
        JobKind::Transcribe => {
            stages::transcribe::run(cfg, db, &engines.speech, recording_id).await
        }
        JobKind::Merge => stages::merge::run(cfg, db, &engines.ollama, recording_id).await,
        JobKind::Embed => stages::embed::run(cfg, db, &engines.embedder, recording_id).await,
        JobKind::Summarize => {
            let template = summarize_template(cfg, payload);
            stages::summarize::run(cfg, db, &engines.ollama, recording_id, &template).await
        }
        JobKind::TranscribeSegment => {
            stages::transcribe_segment::run(cfg, db, engines, recording_id).await
        }
    }
}

/// Resolve the summary template id for a summarize job: the job payload's
/// `"template"` key, else `cfg.llm.summary_template`, else `"general"`.
fn summarize_template(cfg: &Config, payload: &Value) -> String {
    let from_payload = payload
        .get("template")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(t) = from_payload {
        return t.to_string();
    }
    let from_cfg = cfg.llm.summary_template.trim();
    if !from_cfg.is_empty() {
        return from_cfg.to_string();
    }
    scribe_core::summary_template::DEFAULT_TEMPLATE_ID.to_string()
}

/// After a terminal stage (`embed` or `summarize`) finishes, flip the recording
/// to `ready` once **both** of those stages have a `done` job (design §7).
pub(crate) async fn maybe_mark_ready(db: &Db, recording_id: Uuid) -> Result<()> {
    if embed_and_summarize_done(db, recording_id).await? {
        db.set_recording_status(recording_id, RecordingStatus::Ready)
            .await?;
        tracing::info!(%recording_id, "recording ready");
    }
    Ok(())
}

/// True when both `embed` and `summarize` have a `done` job for this recording.
async fn embed_and_summarize_done(db: &Db, recording_id: Uuid) -> Result<bool> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT count(DISTINCT kind) AS n FROM jobs \
         WHERE recording_id = $1 AND kind = ANY($2) AND state = 'done'",
    )
    .bind(recording_id)
    .bind(&["embed", "summarize"][..])
    .fetch_one(db.pool())
    .await
    .map_err(scribe_db::db_err)?;
    let n: i64 = row.try_get("n").map_err(scribe_db::db_err)?;
    Ok(n == 2)
}

/// Run the worker forever: load engines once, spawn the reaper, then loop
/// claiming and running jobs.
pub async fn run_worker(cfg: Config) -> Result<()> {
    let db = Db::connect(&cfg.database).await?;
    let engines = Engines::load(&cfg)?;
    let cfg = Arc::new(cfg);

    let worker_id = derive_worker_id();
    tracing::info!(worker_id = %worker_id, stages = ?cfg.effective_stages(), "worker starting");

    // Background reaper: requeue jobs whose lease is older than 3× heartbeat.
    spawn_reaper(db.clone(), cfg.clone());

    let heartbeat = Duration::from_secs(cfg.worker.heartbeat_secs.max(1));
    let poll = Duration::from_secs(cfg.worker.poll_secs.max(1));
    let concurrency = cfg.worker.concurrency.max(1);
    let semaphore = Arc::new(Semaphore::new(concurrency));

    let mut listener = db.job_listener(&cfg.database).await?;
    let kinds = cfg.effective_stages();

    loop {
        // Acquire a concurrency permit before claiming, so we never claim work we
        // can't immediately start. Held by the spawned task until it finishes.
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore not closed");

        match db.claim_one(&worker_id, &kinds).await? {
            Some(job) => {
                let db = db.clone();
                let cfg = cfg.clone();
                let engines = engines.clone();
                let worker_id = worker_id.clone();
                tokio::spawn(async move {
                    let _permit = permit; // released on task end
                    if let Err(e) =
                        process_job(&cfg, &db, &engines, &worker_id, job, heartbeat).await
                    {
                        tracing::error!(error = %e, "job processing error");
                    }
                });
            }
            None => {
                // Nothing to claim: release the permit and sleep until a NOTIFY or
                // the poll backstop fires, then loop.
                drop(permit);
                tokio::select! {
                    _ = listener.recv() => {
                        tracing::trace!("woken by NOTIFY");
                    }
                    _ = tokio::time::sleep(poll) => {
                        tracing::trace!("poll backstop tick");
                    }
                }
            }
        }
    }
}

/// Run one claimed job: heartbeat while it runs, dispatch the stage, then on
/// success complete it + enqueue ready successors; on failure record it.
async fn process_job(
    cfg: &Config,
    db: &Db,
    engines: &Engines,
    worker_id: &str,
    job: Job,
    heartbeat: Duration,
) -> Result<()> {
    let Some(recording_id) = job.recording_id else {
        // A job with no recording is malformed; mark it done so it isn't retried.
        tracing::warn!(job_id = job.id, "job has no recording_id; completing as no-op");
        return db.complete(job.id).await;
    };

    tracing::info!(job_id = job.id, kind = %job.kind, %recording_id, "running stage");

    // Heartbeat task: bump the lease until the stage finishes (or the guard drops).
    let hb_db = db.clone();
    let hb_worker = worker_id.to_string();
    let job_id = job.id;
    let hb = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(heartbeat);
        ticker.tick().await; // immediate first tick, skip
        loop {
            ticker.tick().await;
            match hb_db.heartbeat(job_id, &hb_worker).await {
                Ok(true) => {}
                Ok(false) => break, // lease lost (reaped) — stop bumping
                Err(e) => {
                    tracing::warn!(error = %e, job_id, "heartbeat failed");
                    break;
                }
            }
        }
    });

    let result = run_stage(cfg, db, engines, job.kind, recording_id, &job.payload).await;
    hb.abort();

    match result {
        Ok(()) => {
            db.complete(job.id).await?;
            enqueue_successors(db, job.kind, recording_id).await?;
            if matches!(job.kind, JobKind::Embed | JobKind::Summarize) {
                maybe_mark_ready(db, recording_id).await?;
            }
            // Live transcription drains itself: if more segments arrived while we
            // were transcribing this batch, queue another pass (now that this job
            // is `done`, the per-recording unique index allows the new one).
            //
            // CRITICAL: only re-enqueue while the recording is still UPLOADING.
            // The stage no-ops once the recording is complete, so any straggler
            // segments uploaded around `/complete` stay untranscribed — without
            // this status guard the `count > 0` check would re-enqueue forever
            // (a runaway loop that created millions of no-op jobs).
            if matches!(job.kind, JobKind::TranscribeSegment)
                && db.get_recording(recording_id).await?.status == RecordingStatus::Uploading
                && db.count_untranscribed_segments(recording_id).await? > 0
            {
                db.enqueue(recording_id, JobKind::TranscribeSegment, serde_json::json!({}))
                    .await?;
            }
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            tracing::error!(job_id = job.id, kind = %job.kind, %recording_id, error = %msg, "stage failed");
            let backoff = Duration::from_secs(cfg.worker.poll_secs.max(1));
            let requeued = db
                .fail(job.id, &msg, cfg.worker.max_attempts, backoff)
                .await?;
            if !requeued {
                // Attempts exhausted: park the recording as failed for inspection.
                let _ = db
                    .set_recording_status(recording_id, RecordingStatus::Failed)
                    .await;
            }
            Ok(())
        }
    }
}

/// Enqueue each successor of `kind` whose predecessors are all `done`.
async fn enqueue_successors(db: &Db, kind: JobKind, recording_id: Uuid) -> Result<()> {
    for succ in kind.successors() {
        if db.predecessors_done(recording_id, *succ).await? {
            db.enqueue(recording_id, *succ, serde_json::json!({})).await?;
        }
    }
    Ok(())
}

/// Spawn the reaper: every heartbeat interval, requeue jobs whose lease is older
/// than 3× the heartbeat (a worker that died mid-job — design §7).
///
/// A reap counts as an attempt, so a stage that kills the worker outright rather
/// than returning an error is bounded by `max_attempts` like any other failure.
/// When a reap uses the last attempt the job is parked `failed`, and the
/// recording goes with it — otherwise the recording would sit in its old status
/// forever, showing the user a stage that nothing is working on.
fn spawn_reaper(db: Db, cfg: Arc<Config>) {
    let heartbeat = Duration::from_secs(cfg.worker.heartbeat_secs.max(1));
    let lease = heartbeat * 3;
    let max_attempts = cfg.worker.max_attempts;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(heartbeat);
        loop {
            ticker.tick().await;
            let reaped = match db.reap_stuck(lease, max_attempts).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "reaper scan failed");
                    continue;
                }
            };
            if reaped.is_empty() {
                continue;
            }
            tracing::warn!(reaped = reaped.len(), "took back jobs with an expired lease");

            for job in reaped.iter().filter(|j| j.exhausted) {
                tracing::error!(
                    job_id = job.id,
                    recording_id = ?job.recording_id,
                    "job exhausted its attempts without a worker ever reporting back"
                );
                let Some(recording_id) = job.recording_id else {
                    continue;
                };
                if let Err(e) = db
                    .set_recording_status(recording_id, RecordingStatus::Failed)
                    .await
                {
                    tracing::warn!(error = %e, %recording_id, "could not mark recording failed");
                }
            }
        }
    });
}

/// A stable-ish worker identity: `hostname-pid`, falling back to a uuid.
fn derive_worker_id() -> String {
    let host = hostname();
    let pid = std::process::id();
    match host {
        Some(h) => format!("{h}-{pid}"),
        None => format!("worker-{}", Uuid::new_v4()),
    }
}

/// Best-effort hostname without pulling a new dependency.
fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME") // Windows
        .or_else(|_| std::env::var("HOSTNAME")) // most Unix shells
        .ok()
        .filter(|s| !s.is_empty())
}
