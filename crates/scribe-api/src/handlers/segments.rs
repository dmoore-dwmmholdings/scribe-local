//! Segment upload + raw-segment download (design §6, §10).
//!
//! `PUT /recordings/{id}/segments/{seq}` streams a raw audio chunk to disk
//! without buffering the whole body in memory: bytes are written to a temp file
//! (sha256 hashed on the way through), then atomically renamed into the final
//! segment path. The body limit is disabled on this route (see `lib.rs`) so the
//! `DefaultBodyLimit` doesn't truncate large segments.
//!
//! `GET /recordings/{id}/segments/{seq}` serves a stored segment with range
//! support for playback.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use scribe_core::storage;
use scribe_core::Error;

use crate::error::{ApiError, ApiResult};
use crate::range::serve_file_range;
use crate::state::AppState;

/// Query for the upload: the container extension when the client knows it.
#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    pub ext: Option<String>,
}

/// `PUT /recordings/{id}/segments/{seq}` — stream the raw body to the segment
/// path. Extension comes from `?ext=`, else the `Content-Type`, else `m4a`.
/// Optional `X-Segment-Start-Ms` / `X-Segment-Duration-Ms` headers carry timing.
pub async fn put_segment(
    State(state): State<AppState>,
    Path((id, seq)): Path<(Uuid, i32)>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Json<serde_json::Value>> {
    // The recording must exist (FK + a clearer 404 than a constraint error).
    let recording = state.db.get_recording(id).await?;

    let ext = resolve_ext(q.ext.as_deref(), &headers);
    let start_ms = header_i64(&headers, "x-segment-start-ms");
    let duration_ms = header_i64(&headers, "x-segment-duration-ms");

    let seg_dir = storage::segments_dir(&state.blobs, id);
    tokio::fs::create_dir_all(&seg_dir)
        .await
        .map_err(|e| Error::Storage(format!("creating blob dir {}: {e}", seg_dir.display())))?;

    let final_path = storage::segment_path(&state.blobs, id, seq, &ext);
    // Temp file in the same directory so the rename is atomic (same filesystem).
    let tmp_path = seg_dir.join(format!(".{seq:06}.{ext}.partial"));

    // Stream body → temp file, hashing as we go. Never hold the whole body.
    let mut hasher = Sha256::new();
    let mut bytes_written: u64 = 0;
    {
        let mut file = tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|e| Error::Storage(format!("creating temp {}: {e}", tmp_path.display())))?;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Error::BadRequest(format!("reading upload body: {e}")))?;
            hasher.update(&chunk);
            bytes_written += chunk.len() as u64;
            if let Err(e) = file.write_all(&chunk).await {
                // Best-effort cleanup of the partial file before bailing.
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(ApiError(Error::Storage(format!(
                    "writing segment {}: {e}",
                    tmp_path.display()
                ))));
            }
        }
        file.flush()
            .await
            .map_err(|e| Error::Storage(format!("flushing {}: {e}", tmp_path.display())))?;
    }

    // Atomic publish.
    tokio::fs::rename(&tmp_path, &final_path).await.map_err(|e| {
        Error::Storage(format!(
            "renaming {} → {}: {e}",
            tmp_path.display(),
            final_path.display()
        ))
    })?;

    let sha256 = hasher.finalize();
    let storage_key = storage::segment_key(id, seq, &ext);

    // Idempotent on (recording_id, seq).
    state
        .db
        .insert_segment(
            id,
            seq,
            &storage_key,
            start_ms,
            duration_ms,
            Some(bytes_written as i64),
            Some(sha256.as_slice()),
        )
        .await?;

    // Live transcription: while the recording is still in progress, kick off an
    // incremental transcribe pass so the user sees text during recording. The
    // per-recording unique index keeps at most one live job in flight; it drains
    // all pending segments and the worker re-enqueues if more arrive. Best-effort
    // — a queue hiccup must not fail the upload.
    if recording.status == scribe_core::types::RecordingStatus::Uploading {
        if let Err(e) = state
            .db
            .enqueue(id, scribe_core::types::JobKind::TranscribeSegment, json!({}))
            .await
        {
            tracing::warn!(%id, error = %e, "could not enqueue live transcription");
        }
    }

    Ok(Json(json!({
        "seq": seq,
        "bytes": bytes_written,
        "storage_key": storage_key,
    })))
}

/// `GET /recordings/{id}/segments/{seq}` — serve a stored raw segment with range
/// support. The extension is looked up from the recorded `storage_key`.
pub async fn get_segment(
    State(state): State<AppState>,
    Path((id, seq)): Path<(Uuid, i32)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let segments = state.db.list_segments_by_recording(id).await?;
    let segment = segments
        .into_iter()
        .find(|s| s.seq == seq)
        .ok_or_else(|| Error::NotFound(format!("segment {seq} of recording {id}")))?;

    let path = storage::resolve_key(&state.blobs, &segment.storage_key);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("m4a")
        .to_ascii_lowercase();
    let content_type = audio_content_type(&ext);
    serve_file_range(&path, content_type, &headers).await
}

/// Pick the container extension: explicit `?ext=` wins, then a known audio
/// `Content-Type`, else the default `m4a`.
fn resolve_ext(ext_q: Option<&str>, headers: &HeaderMap) -> String {
    if let Some(ext) = ext_q {
        let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
        if !ext.is_empty() {
            return ext;
        }
    }
    if let Some(ct) = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        // Ignore any `; charset=…` parameters.
        let ct = ct.split(';').next().unwrap_or("").trim();
        if let Some(ext) = ext_from_content_type(ct) {
            return ext.to_string();
        }
    }
    "m4a".to_string()
}

/// Map a small set of audio MIME types to file extensions.
fn ext_from_content_type(ct: &str) -> Option<&'static str> {
    match ct {
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "audio/aac" => Some("m4a"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        "audio/ogg" | "audio/opus" => Some("ogg"),
        "audio/webm" => Some("webm"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        _ => None,
    }
}

/// Content-Type to advertise when serving a stored segment by extension.
fn audio_content_type(ext: &str) -> &'static str {
    match ext {
        "m4a" | "mp4" | "aac" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "webm" => "audio/webm",
        "flac" => "audio/flac",
        _ => "application/octet-stream",
    }
}

/// Parse a positive-or-any integer header, ignoring malformed values.
fn header_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<i64>().ok())
}
