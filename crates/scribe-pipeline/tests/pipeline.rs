//! End-to-end integration test for the pipeline using the **stub** engines.
//!
//! Requires a live Postgres (the pgvector container). It is skipped when
//! `DATABASE_URL` is unset so `cargo test` stays green on machines without a DB.
//!
//! Run (stub engines, from the repo root):
//! ```text
//! DATABASE_URL=postgres://scribe:scribe@localhost:5433/scribe \
//!   cargo test -p scribe-pipeline --no-default-features --test pipeline -- --nocapture
//! ```
//!
//! The test resets the schema, re-applies all migrations (incl. 0003),
//! synthesizes a small 16 kHz mono WAV, creates a recording, stages the WAV as
//! segment 1, and runs [`scribe_pipeline::process_recording_inline`]. It then
//! asserts that utterances, chunks, recording_speakers, and a summary row all
//! exist and the recording is `ready`.

use std::f32::consts::PI;
use std::io::Write;
use std::path::Path;

use scribe_core::config::Config;
use scribe_core::storage;
use scribe_core::types::RecordingStatus;
use scribe_db::Db;
use sqlx::Row;
use uuid::Uuid;

/// The DB URL the test connects to, or `None` to skip the whole test.
fn database_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty())
}

/// Write a minimal 16 kHz mono 16-bit PCM WAV (a quiet tone). Self-contained so
/// the test doesn't depend on the `#[cfg(test)]`-only writer in `scribe-asr`.
fn write_test_wav(path: &Path, secs: f32) {
    let sample_rate = 16_000u32;
    let n = (sample_rate as f32 * secs) as usize;
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sample_rate as f32;
        let v = (2.0 * PI * 220.0 * t).sin() * 0.2;
        samples.push((v * i16::MAX as f32) as i16);
    }
    let data_len = (samples.len() * 2) as u32;
    let byte_rate = sample_rate * 2;
    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for s in &samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&buf).unwrap();
}

/// Drop and recreate the `public` schema, then re-apply all migrations.
async fn reset_schema(db: &Db) {
    sqlx::query("DROP SCHEMA public CASCADE")
        .execute(db.pool())
        .await
        .expect("drop schema");
    sqlx::query("CREATE SCHEMA public")
        .execute(db.pool())
        .await
        .expect("create schema");
    db.run_migrations().await.expect("run migrations");
}

async fn count(db: &Db, sql: &str, rid: Uuid) -> i64 {
    let row = sqlx::query(sql)
        .bind(rid)
        .fetch_one(db.pool())
        .await
        .unwrap();
    row.try_get::<i64, _>("n").unwrap()
}

#[tokio::test]
async fn pipeline_runs_end_to_end_with_stub_engines() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL not set; skipping pipeline integration test");
        return;
    };

    // Config: point the DB at the live container and the blob root at a temp dir.
    let blobs = std::env::temp_dir().join(format!("scribe-test-{}", Uuid::new_v4()));
    let mut cfg = Config::default();
    cfg.database.url = url;
    cfg.storage.blobs = blobs.clone();
    // Ollama is not running in the test; summarize degrades to an empty row.
    cfg.llm.ollama_url = "http://127.0.0.1:1".to_string();

    let db = Db::connect(&cfg.database).await.expect("connect db");
    reset_schema(&db).await;

    // Create a recording with 2 expected participants (drives stub diarization).
    let recording = db
        .create_recording(Some("Integration Test"), None, Some(2), Some("wav"), Some(16_000))
        .await
        .expect("create recording");
    let rid = recording.id;

    // Stage a ~30s WAV as segment 1 so the real transcode (ffmpeg) path runs.
    let seg = storage::segment_path(&blobs, rid, 1, "wav");
    write_test_wav(&seg, 30.0);
    db.insert_segment(
        rid,
        1,
        &storage::segment_key(rid, 1, "wav"),
        Some(0),
        Some(30_000),
        None,
        None,
    )
    .await
    .expect("insert segment");

    // Run the whole pipeline inline (stub ASR/diar/embed, ffmpeg transcode).
    scribe_pipeline::process_recording_inline(&cfg, &db, rid)
        .await
        .expect("process inline");

    // Assertions ----------------------------------------------------------
    let utterances = count(
        &db,
        "SELECT count(*) AS n FROM utterances WHERE recording_id = $1",
        rid,
    )
    .await;
    assert!(utterances > 0, "expected utterances, got {utterances}");

    let chunks = count(
        &db,
        "SELECT count(*) AS n FROM chunks WHERE recording_id = $1",
        rid,
    )
    .await;
    assert!(chunks > 0, "expected chunks, got {chunks}");

    let speakers = count(
        &db,
        "SELECT count(*) AS n FROM recording_speakers WHERE recording_id = $1",
        rid,
    )
    .await;
    assert_eq!(speakers, 2, "expected 2 diarized speakers, got {speakers}");

    let summaries = count(
        &db,
        "SELECT count(*) AS n FROM summaries WHERE recording_id = $1",
        rid,
    )
    .await;
    assert_eq!(summaries, 1, "expected a summary row, got {summaries}");

    // Recording must be marked ready with a measured duration.
    let updated = db.get_recording(rid).await.expect("get recording");
    assert_eq!(updated.status, RecordingStatus::Ready, "status not ready");
    assert!(
        updated.duration_ms.unwrap_or(0) > 0,
        "duration not recorded: {:?}",
        updated.duration_ms
    );

    // Scratch artifacts were written by transcribe + diarize.
    let artifacts = count(
        &db,
        "SELECT count(*) AS n FROM recording_artifacts WHERE recording_id = $1",
        rid,
    )
    .await;
    assert_eq!(artifacts, 2, "expected transcript + diarization artifacts");

    println!(
        "OK: utterances={utterances} chunks={chunks} speakers={speakers} \
         summaries={summaries} duration_ms={:?}",
        updated.duration_ms
    );

    // Cleanup the temp blob dir.
    let _ = std::fs::remove_dir_all(&blobs);
}
