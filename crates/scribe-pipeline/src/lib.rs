//! `scribe-pipeline` — the worker crate (design §6–§9).
//!
//! It claims jobs from the Postgres queue and runs the processing DAG:
//! `transcode → {diarize, transcribe} → merge → {embed, summarize} → ready`.
//! Each stage is a module under [`stages`]; the queue-driven driver lives in
//! [`worker`] and an inline (no-queue) driver is [`process_recording_inline`].
//!
//! The queue-driven worker obeys the processing schedule
//! ([`scribe_core::schedule`]) through [`gate`]. The inline driver does not: an
//! explicit CLI invocation is the operator asking for work right now.
//!
//! ## Public entrypoints
//! * [`run_worker`] — the long-running worker (NOTIFY + poll + heartbeat + reaper).
//! * [`process_recording_inline`] — run one recording end-to-end synchronously.
//! * [`ingest_file`] — create a recording from a local audio file.
//! * [`enroll`] — register a named voice from a sample.
//! * [`reindex`] — re-enqueue `embed`/`summarize` over existing recordings.
//!
//! ## Build paths
//! With the default `onnx` feature this pulls real ASR/diarization (sherpa-onnx)
//! and fastembed embeddings. With `--no-default-features` the ASR and embedder
//! degrade to deterministic stubs, so the whole pipeline still runs end-to-end
//! without any models — which is how the integration test exercises it.

mod artifacts;
mod engines;
mod fillers;
mod gate;
mod stages;
mod title;
mod worker;

use std::path::Path;

use scribe_core::config::Config;
use scribe_core::storage;
use scribe_core::types::{JobKind, RecordingStatus};
use scribe_core::{Error, Result};
use scribe_db::Db;
use uuid::Uuid;

pub use worker::run_worker;

use crate::engines::Engines;

/// Process exactly one recording end-to-end inline (no queue). Runs every stage
/// in DAG order on the current task: transcode → diarize → transcribe → merge →
/// embed → summarize, then marks the recording `ready`.
///
/// Used by tests and by `ingest --inline`. Loads engines on entry (cheap with
/// the stubs); a long-lived worker should use [`run_worker`] instead.
pub async fn process_recording_inline(cfg: &Config, db: &Db, recording_id: Uuid) -> Result<()> {
    let engines = Engines::load(cfg)?;
    db.set_recording_status(recording_id, RecordingStatus::Processing)
        .await?;

    // The DAG, flattened to a serial order (diarize/transcribe and embed/summarize
    // are independent but harmless to run sequentially on one task).
    let order = [
        JobKind::Transcode,
        JobKind::Diarize,
        JobKind::Transcribe,
        JobKind::Merge,
        JobKind::Embed,
        JobKind::Summarize,
    ];
    let empty_payload = serde_json::json!({});
    for kind in order {
        if let Err(e) =
            worker::run_stage(cfg, db, &engines, kind, recording_id, &empty_payload).await
        {
            let _ = db
                .set_recording_status(recording_id, RecordingStatus::Failed)
                .await;
            return Err(e);
        }
    }

    db.set_recording_status(recording_id, RecordingStatus::Ready)
        .await?;
    Ok(())
}

/// Ingest a local audio file: create a recording, copy the file in as segment 1,
/// and either enqueue `transcode` (queued path) or run the whole pipeline inline
/// (`inline == true`). Returns the new recording id.
pub async fn ingest_file(
    cfg: &Config,
    db: &Db,
    file: &Path,
    title: Option<String>,
    participants: Option<i32>,
    inline: bool,
) -> Result<Uuid> {
    if !file.exists() {
        return Err(Error::BadRequest(format!(
            "ingest file does not exist: {}",
            file.display()
        )));
    }
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
        .to_string();

    let recording = db
        .create_recording(
            title.as_deref(),
            None,
            participants,
            Some(&ext),
            None,
        )
        .await?;
    let recording_id = recording.id;

    // Copy the source file in as segment seq=1.
    let blobs = cfg.storage.blobs.as_path();
    let seg_path = storage::segment_path(blobs, recording_id, 1, &ext);
    if let Some(parent) = seg_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::copy(file, &seg_path).await.map_err(|e| {
        Error::Storage(format!("copying ingest file to {}: {e}", seg_path.display()))
    })?;
    let bytes = tokio::fs::metadata(&seg_path).await.map(|m| m.len() as i64).ok();
    db.insert_segment(
        recording_id,
        1,
        &storage::segment_key(recording_id, 1, &ext),
        Some(0),
        None,
        bytes,
        None,
    )
    .await?;
    db.set_recording_storage_key(recording_id, &storage::storage_key(recording_id))
        .await?;

    if inline {
        process_recording_inline(cfg, db, recording_id).await?;
    } else {
        db.set_recording_status(recording_id, RecordingStatus::Processing)
            .await?;
        db.enqueue(recording_id, JobKind::Transcode, serde_json::json!({}))
            .await?;
    }

    Ok(recording_id)
}

/// Enroll a known speaker from a voice sample. Transcodes the sample to the
/// canonical 16 kHz mono WAV, computes its speaker embedding, and stores a named
/// [`Speaker`](scribe_core::types::Speaker). Returns the new speaker id.
pub async fn enroll(cfg: &Config, db: &Db, name: &str, audio: &Path) -> Result<Uuid> {
    if !audio.exists() {
        return Err(Error::BadRequest(format!(
            "enroll audio does not exist: {}",
            audio.display()
        )));
    }
    let engines = Engines::load(cfg)?;

    // Transcode the sample to a temp 16 kHz mono WAV next to the source.
    let wav = audio.with_extension("enroll.wav");
    transcode_sample(audio, &wav).await?;

    let wav_owned = wav.clone();
    let engines_for_embed = engines.clone();
    let embedding = tokio::task::spawn_blocking(move || {
        engines_for_embed
            .speech
            .speaker_embedder()
            .embed_speaker(&wav_owned)
    })
    .await
    .map_err(|e| Error::pipeline("enroll", format!("embed task failed: {e}")))??;

    let _ = tokio::fs::remove_file(&wav).await;

    let speaker = db.create_speaker(name, Some(embedding)).await?;
    tracing::info!(speaker_id = %speaker.id, name, "enrolled speaker");
    Ok(speaker.id)
}

/// Transcode any audio file to 16 kHz mono PCM WAV via ffmpeg (used by enroll).
async fn transcode_sample(input: &Path, out: &Path) -> Result<()> {
    let output = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(input)
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg("16000")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(out)
        .output()
        .await
        .map_err(|e| Error::pipeline("enroll", format!("failed to spawn ffmpeg: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::pipeline(
            "enroll",
            format!("ffmpeg failed: {}", stderr.trim()),
        ));
    }
    Ok(())
}

/// Recompute derived data over existing recordings by enqueuing jobs (design §15
/// `reindex`). With `embeddings`, re-enqueue `embed`; with `summaries`,
/// re-enqueue `summarize`. Only recordings whose `merge` already completed are
/// touched — there is nothing to re-derive otherwise. Returns nothing; the
/// worker picks the jobs up.
pub async fn reindex(cfg: &Config, db: &Db, embeddings: bool, summaries: bool) -> Result<()> {
    if !embeddings && !summaries {
        return Ok(());
    }
    let recording_ids = recordings_with_merge_done(db).await?;
    let mut enqueued = 0usize;
    for rid in recording_ids {
        if embeddings {
            db.enqueue(rid, JobKind::Embed, serde_json::json!({"reindex": true}))
                .await?;
            enqueued += 1;
        }
        if summaries {
            db.enqueue(rid, JobKind::Summarize, serde_json::json!({"reindex": true}))
                .await?;
            enqueued += 1;
        }
    }
    tracing::info!(
        enqueued,
        embeddings,
        summaries,
        models_dir = %cfg.worker.models_dir.display(),
        "reindex enqueued"
    );
    Ok(())
}

/// Recordings that have a `done` `merge` job (so embed/summarize can re-run).
async fn recordings_with_merge_done(db: &Db) -> Result<Vec<Uuid>> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT DISTINCT recording_id FROM jobs \
         WHERE kind = 'merge' AND state = 'done' AND recording_id IS NOT NULL",
    )
    .fetch_all(db.pool())
    .await
    .map_err(scribe_db::db_err)?;
    rows.iter()
        .map(|r| r.try_get::<Uuid, _>("recording_id").map_err(scribe_db::db_err))
        .collect()
}
