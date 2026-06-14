//! Stage 1 — transcode (design §7).
//!
//! Concatenate the uploaded audio segments (in `seq` order) and decode them to
//! the canonical **16 kHz mono PCM WAV** the speech models want, via `ffmpeg`'s
//! concat demuxer. Then read the WAV back to record the measured duration.
//!
//! Edge cases handled:
//! * a single-segment ingest (the common `scribe ingest` path),
//! * the WAV already being in place (the inline/test path may pre-stage
//!   `audio.wav`) — if there are no segments but a WAV exists, we keep it.

use std::path::Path;

use scribe_asr::read_wav;
use scribe_core::config::Config;
use scribe_core::storage;
use scribe_core::Result;
use scribe_db::Db;
use uuid::Uuid;

use crate::stages::stage_err;

const STAGE: &str = "transcode";

/// Run the transcode stage for `recording_id`.
pub async fn run(cfg: &Config, db: &Db, recording_id: Uuid) -> Result<()> {
    let blobs = cfg.storage.blobs.as_path();
    let wav = storage::wav_path(blobs, recording_id);
    let segments = db.list_segments_by_recording(recording_id).await?;

    if segments.is_empty() {
        // No raw segments. Tolerate a pre-staged WAV (inline/test path); else fail.
        if !wav.exists() {
            return Err(stage_err(
                STAGE,
                format!("recording {recording_id} has no segments and no audio.wav"),
            ));
        }
        tracing::info!(%recording_id, "transcode: no segments, using pre-staged audio.wav");
    } else {
        // Resolve each segment's absolute path from its stored (relative) key.
        let inputs: Vec<std::path::PathBuf> = segments
            .iter()
            .map(|s| storage::resolve_key(blobs, &s.storage_key))
            .collect();
        for p in &inputs {
            if !p.exists() {
                return Err(stage_err(
                    STAGE,
                    format!("segment file missing: {}", p.display()),
                ));
            }
        }
        transcode_to_wav(&inputs, &wav).await?;
    }

    // Record the storage key (id prefix) and measured duration.
    db.set_recording_storage_key(recording_id, &storage::storage_key(recording_id))
        .await?;

    let wav_owned = wav.clone();
    let decoded = tokio::task::spawn_blocking(move || read_wav(&wav_owned))
        .await
        .map_err(|e| stage_err(STAGE, format!("wav read task failed: {e}")))??;
    db.set_recording_duration(recording_id, decoded.duration_ms())
        .await?;

    tracing::info!(
        %recording_id,
        duration_ms = decoded.duration_ms(),
        sample_rate = decoded.sample_rate,
        "transcode complete"
    );
    Ok(())
}

/// Concatenate `inputs` and decode to a 16 kHz mono PCM WAV at `out` via ffmpeg.
async fn transcode_to_wav(inputs: &[std::path::PathBuf], out: &Path) -> Result<()> {
    if let Some(parent) = out.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Fast path: a single input needs no concat list.
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y").arg("-hide_banner").arg("-loglevel").arg("error");

    // We always build a concat list so multi-segment and single-segment paths
    // share one code path. The list lives next to the WAV.
    let list_path = out
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("concat.txt");
    let mut list = String::new();
    for p in inputs {
        // ffmpeg concat demuxer wants `file '<path>'`; single-quote-escape.
        let s = p.to_string_lossy().replace('\'', "'\\''");
        list.push_str(&format!("file '{s}'\n"));
    }
    tokio::fs::write(&list_path, list).await?;

    cmd.arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&list_path)
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg("16000")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(out);

    let output = cmd
        .output()
        .await
        .map_err(|e| stage_err(STAGE, format!("failed to spawn ffmpeg: {e}")))?;

    // Clean up the scratch list regardless of outcome.
    let _ = tokio::fs::remove_file(&list_path).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stage_err(
            STAGE,
            format!("ffmpeg exited with {}: {}", output.status, stderr.trim()),
        ));
    }
    if !out.exists() {
        return Err(stage_err(STAGE, "ffmpeg produced no output WAV"));
    }
    Ok(())
}
