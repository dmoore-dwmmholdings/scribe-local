//! Speaker naming / enrollment (design §8).
//!
//! `POST /recordings/{id}/speakers/{local_idx}/name` attaches a real name to a
//! diarized "Speaker N". If no enrolled speaker with that name exists we create
//! one; when `enroll` is set we carry the recording-speaker's stored voice
//! embedding onto the new identity so the voice is recognised next time.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use scribe_core::Error;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Body for naming a diarized speaker.
#[derive(Debug, Deserialize)]
pub struct NameBody {
    pub name: String,
    /// When true, copy this recording-speaker's embedding onto the (new) named
    /// speaker so the voice is auto-matched in future recordings.
    #[serde(default)]
    pub enroll: bool,
}

/// `POST /recordings/{id}/speakers/{local_idx}/name`
pub async fn name_speaker(
    State(state): State<AppState>,
    Path((id, local_idx)): Path<(Uuid, i32)>,
    Json(body): Json<NameBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError(Error::BadRequest("name must not be empty".into())));
    }

    // The diarized speaker row must already exist (created by the diarize stage).
    let rec_speakers = state.db.list_recording_speakers(id).await?;
    let rec_speaker = rec_speakers
        .iter()
        .find(|s| s.local_idx == local_idx)
        .ok_or_else(|| {
            Error::NotFound(format!(
                "speaker {local_idx} not found in recording {id}"
            ))
        })?;

    // Reuse an existing enrolled speaker with this exact name, else create one.
    let existing = state
        .db
        .list_speakers()
        .await?
        .into_iter()
        .find(|s| s.display_name.eq_ignore_ascii_case(name));

    let speaker = match existing {
        Some(s) => s,
        None => {
            // Carry the diarized embedding onto the new identity when enrolling.
            let embedding = if body.enroll {
                rec_speaker.embedding.clone()
            } else {
                None
            };
            state.db.create_speaker(name, embedding).await?
        }
    };

    state
        .db
        .set_recording_speaker(id, local_idx, Some(speaker.id))
        .await?;

    Ok(Json(json!({
        "local_idx": local_idx,
        "speaker_id": speaker.id,
        "display_name": speaker.display_name,
    })))
}
