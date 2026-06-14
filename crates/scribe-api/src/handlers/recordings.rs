//! Recording lifecycle endpoints (design §6, §11):
//!
//! * `POST   /recordings`                 — create (status `uploading`)
//! * `GET    /recordings`                 — list
//! * `GET    /recordings/{id}`            — detail (transcript + speakers + summary)
//! * `POST   /recordings/{id}/complete`   — finalize → enqueue transcode

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use scribe_core::storage;
use scribe_core::types::{
    JobKind, Recording, RecordingSpeaker, RecordingStatus, Summary, Utterance,
};
use scribe_core::Error;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Body for `POST /recordings`. Every field is optional — the phone may not know
/// the title or participant count up front.
#[derive(Debug, Default, Deserialize)]
pub struct CreateRecordingBody {
    pub title: Option<String>,
    pub participants_expected: Option<i32>,
    pub device_id: Option<String>,
    pub audio_format: Option<String>,
    pub sample_rate: Option<i32>,
}

/// `POST /recordings` → 201 with the new id and the segment-upload template.
pub async fn create_recording(
    State(state): State<AppState>,
    body: Option<Json<CreateRecordingBody>>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    // Tolerate an empty/absent body — all fields are optional.
    let body = body.map(|Json(b)| b).unwrap_or_default();

    let audio_format = body.audio_format.as_deref().or(Some("m4a"));
    let recording = state
        .db
        .create_recording(
            body.title.as_deref(),
            body.device_id.as_deref(),
            body.participants_expected,
            audio_format,
            body.sample_rate,
        )
        .await?;

    // Storage key is the recording id; create the on-disk blob dirs now so the
    // first segment PUT just writes into them.
    let key = storage::storage_key(recording.id);
    state
        .db
        .set_recording_storage_key(recording.id, &key)
        .await?;
    let seg_dir = storage::segments_dir(&state.blobs, recording.id);
    tokio::fs::create_dir_all(&seg_dir)
        .await
        .map_err(|e| Error::Storage(format!("creating blob dir {}: {e}", seg_dir.display())))?;

    let resp = json!({
        "id": recording.id,
        "status": RecordingStatus::Uploading.as_str(),
        "upload": {
            "segment_url_template": format!("/recordings/{}/segments/{{seq}}", recording.id),
        }
    });
    Ok((StatusCode::CREATED, Json(resp)))
}

/// Query for `GET /recordings`.
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub recordings: Vec<Recording>,
}

/// `GET /recordings?limit=&offset=`
pub async fn list_recordings(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<ListResponse>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let recordings = state.db.list_recordings(limit, offset).await?;
    Ok(Json(ListResponse { recordings }))
}

#[derive(Debug, Serialize)]
pub struct RecordingDetail {
    pub recording: Recording,
    pub speakers: Vec<RecordingSpeaker>,
    pub utterances: Vec<Utterance>,
    pub summary: Option<Summary>,
}

/// `GET /recordings/{id}` → recording + diarized speakers + transcript + summary.
pub async fn get_recording(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<RecordingDetail>> {
    let recording = state.db.get_recording(id).await?;
    let speakers = state.db.list_recording_speakers(id).await?;
    let utterances = state.db.list_utterances_by_recording(id).await?;
    let summary = state.db.get_summary(id).await?;
    Ok(Json(RecordingDetail {
        recording,
        speakers,
        utterances,
        summary,
    }))
}

/// Body for `POST /recordings/{id}/complete`.
#[derive(Debug, Default, Deserialize)]
pub struct CompleteBody {
    pub duration_ms: Option<i64>,
}

/// `POST /recordings/{id}/complete` → flip to `processing`, record duration if
/// supplied, and enqueue the `transcode` job (the DB trigger fires NOTIFY).
/// 409 if the recording isn't in `uploading`.
pub async fn complete_recording(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: Option<Json<CompleteBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let body = body.map(|Json(b)| b).unwrap_or_default();

    let recording = state.db.get_recording(id).await?;
    if recording.status != RecordingStatus::Uploading {
        return Err(ApiError(Error::Conflict(format!(
            "recording {id} is `{}`, not `uploading`; cannot complete",
            recording.status
        ))));
    }

    if let Some(duration_ms) = body.duration_ms {
        state.db.set_recording_duration(id, duration_ms).await?;
    }
    state
        .db
        .set_recording_status(id, RecordingStatus::Processing)
        .await?;
    // Kick off the pipeline. enqueue is idempotent per (recording, kind).
    state.db.enqueue(id, JobKind::Transcode, json!({})).await?;

    Ok(Json(json!({
        "id": id,
        "status": RecordingStatus::Processing.as_str(),
    })))
}
