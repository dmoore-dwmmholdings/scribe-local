//! Does the worker actually honour the processing schedule?
//!
//! The unit tests in `scribe-core` prove the policy evaluates correctly; this
//! proves the claim loop obeys it — the part that would still be broken if the
//! gate were wired in wrongly.
//!
//! Requires a live Postgres and `ffmpeg` on PATH; skipped when `DATABASE_URL`
//! is unset, like the other DB-backed tests.
//!
//! ```text
//! DATABASE_URL=postgres://scribe:scribe@localhost:5433/scribe_test \
//!   cargo test -p scribe-pipeline --no-default-features --test schedule_gate -- --nocapture
//! ```

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::Utc;
use scribe_core::config::Config;
use scribe_core::schedule::{
    DayWindow, OverrideMode, ProcessingSchedule, ScheduleOverride,
};
use scribe_core::storage;
use scribe_core::types::{JobKind, JobState};
use scribe_db::Db;
use uuid::Uuid;

fn database_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty())
}

/// A tiny silent 16 kHz mono WAV — enough for ffmpeg to transcode quickly.
fn write_test_wav(path: &Path, secs: f32) {
    let sample_rate = 16_000u32;
    let n = (sample_rate as f32 * secs) as usize;
    let data_len = (n * 2) as u32;
    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    buf.extend_from_slice(&vec![0u8; data_len as usize]);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::File::create(path).unwrap().write_all(&buf).unwrap();
}

async fn reset_schema(db: &Db, url: &str) {
    scribe_db::assert_disposable_test_db(url);
    sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .execute(db.pool())
        .await
        .expect("reset schema");
    db.run_migrations().await.expect("run migrations");
}

/// Poll a job until `pred` holds, or give up after `within`.
async fn wait_for(db: &Db, job_id: i64, within: Duration, pred: impl Fn(JobState) -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        let job = db.get_job(job_id).await.expect("get job").expect("job exists");
        if pred(job.state) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn the_worker_waits_for_its_window() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set; skipping schedule-gate test");
        return;
    };

    let blobs = std::env::temp_dir().join(format!("scribe-gate-{}", Uuid::new_v4()));
    let mut cfg = Config::default();
    cfg.database.url = url.clone();
    cfg.storage.blobs = blobs.clone();
    cfg.llm.base_url = "http://127.0.0.1:1".to_string();
    // Only transcode, so the test finishes as soon as that one stage runs
    // instead of dragging the whole DAG behind it.
    cfg.worker.stages = vec!["transcode".to_string()];
    cfg.worker.poll_secs = 1;

    let db = Db::connect(&cfg.database).await.expect("connect db");
    reset_schema(&db, &url).await;

    // A schedule that is enabled with every day switched off: never a window,
    // whatever time the test runs at.
    let closed = ProcessingSchedule {
        enabled: true,
        days: vec![DayWindow::OFF; 7],
        grace_minutes: 0,
        active_override: None,
    };
    db.set_processing_schedule(&closed).await.expect("save schedule");

    // A recording with one staged segment and a queued transcode job.
    let recording = db
        .create_recording(Some("Gated"), None, Some(2), Some("wav"), Some(16_000))
        .await
        .expect("create recording");
    let rid = recording.id;
    write_test_wav(&storage::segment_path(&blobs, rid, 1, "wav"), 1.0);
    db.insert_segment(
        rid,
        1,
        &storage::segment_key(rid, 1, "wav"),
        Some(0),
        Some(1_000),
        None,
        None,
    )
    .await
    .expect("insert segment");
    let job = db
        .enqueue(rid, JobKind::Transcode, serde_json::json!({}))
        .await
        .expect("enqueue transcode");

    // Start the worker. It has work it could do and is capable of doing it —
    // only the schedule should be holding it back.
    let worker = tokio::spawn(run_worker_quietly(cfg.clone()));

    // Give it several poll intervals to misbehave in.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let still = db.get_job(job.id).await.expect("get job").expect("job exists");
    assert_eq!(
        still.state,
        JobState::Queued,
        "a paused worker must not claim a gated job"
    );
    assert!(still.locked_by.is_none(), "and must not hold a lease on it");
    assert_eq!(still.attempts, 0, "and must not burn an attempt on it");

    // Now lift the pause the way the app does: a "process now" override plus a
    // NOTIFY to wake the idle worker.
    let mut running = closed.clone();
    running.active_override = Some(ScheduleOverride {
        mode: OverrideMode::Run,
        until: Utc::now() + chrono::Duration::minutes(10),
    });
    db.set_processing_schedule(&running).await.expect("save override");
    db.notify_workers("schedule").await.expect("notify");

    let done = wait_for(&db, job.id, Duration::from_secs(30), |s| s == JobState::Done).await;
    worker.abort();
    assert!(done, "the job should run once the override opens the gate");
}

/// `run_worker` never returns; this wrapper just gives the spawned task a
/// sensible shape and keeps a panic from being swallowed silently.
async fn run_worker_quietly(cfg: Config) {
    if let Err(e) = scribe_pipeline::run_worker(cfg).await {
        eprintln!("worker exited with an error: {e}");
    }
}
