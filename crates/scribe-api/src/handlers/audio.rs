//! Transcoded-audio playback (design §10).
//!
//! `GET /recordings/{id}/audio` serves the worker-produced 16 kHz mono WAV
//! (`storage::wav_path`) with HTTP range support for scrub/seek. The WAV is a
//! regenerable cache that only exists once the `transcode` stage has run, so the
//! endpoint is a 404 until then.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use uuid::Uuid;

use scribe_core::storage;

use crate::error::ApiResult;
use crate::range::serve_file_range;
use crate::state::AppState;

/// `GET /recordings/{id}/audio` — serve the transcoded WAV with range support,
/// or 404 if the worker hasn't produced it yet.
pub async fn get_audio(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    // Confirm the recording exists for a clean 404 (vs. just a missing file).
    let _ = state.db.get_recording(id).await?;
    let path = storage::wav_path(&state.blobs, id);
    serve_file_range(&path, "audio/wav", &headers).await
}
