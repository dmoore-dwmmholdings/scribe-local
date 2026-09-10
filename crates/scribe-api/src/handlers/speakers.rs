//! Speaker naming / enrollment (design §8).
//!
//! Two layers of identity:
//!
//! * `recording_speakers` — the diarized "Speaker 0/1/2…" of one recording.
//! * `speakers` — the enrolled, cross-recording identity ("Dawson"), optionally
//!   carrying a reference voiceprint.
//!
//! `POST /recordings/{id}/speakers/{local_idx}/name` binds the first to the
//! second, either by `speaker_id` (pick someone already tagged before) or by
//! `name` (reuse a matching name, else create it). With `enroll` set we copy the
//! diarized voice embedding onto that identity, which is what lets the diarize
//! stage recognise the same person in recordings uploaded later.
//!
//! `GET/PATCH/DELETE /speakers` manage that shared library. Renaming propagates
//! everywhere, because transcripts resolve names through `speaker_id` at read
//! time rather than storing a copy.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use scribe_core::types::{JobKind, Speaker};
use scribe_db::DUPLICATE_VOICE_SIMILARITY;
use scribe_core::Error;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// `DELETE /recordings/{id}/speakers/{local_idx}` → this voice was not a
/// participant; remove it and everything it said.
///
/// Diarization separates voices, and a room can hold one that is not a person
/// in the meeting: a television left on, a conversation through a wall, a phone
/// on speaker. Measured, a television audible in the pauses comes back as the
/// speaker with the *most* speech in the recording, because it talks steadily
/// through every lull. Telling diarization how many people are present does not
/// help — that constrains how many voices there are, not which are people, and
/// the television is usually the most distinct voice in the room.
///
/// Clearing the name (`DELETE .../name`) leaves the lines in the transcript,
/// and from there in the summary and the search index, so the meeting ends up
/// summarised partly from the weather. This drops them and rebuilds both.
///
/// 404 if the recording or that speaker index is unknown. Responds 200 with the
/// number of lines removed. The audio is untouched: `reprocess` brings the
/// speaker back, since this corrects one run's output rather than excluding a
/// voice permanently.
pub async fn delete_recording_speaker(
    State(state): State<AppState>,
    Path((id, local_idx)): Path<(Uuid, i32)>,
) -> ApiResult<Json<serde_json::Value>> {
    state.db.get_recording(id).await?;
    let removed = state.db.delete_recording_speaker(id, local_idx).await?;

    // The summary and the search index were built from a transcript that
    // included this voice. Rebuild both; merge is already `done`, so each
    // stage's predecessor gate is satisfied.
    for kind in [JobKind::Embed, JobKind::Summarize] {
        state.db.enqueue(id, kind, json!({})).await?;
    }

    Ok(Json(json!({
        "id": id,
        "local_idx": local_idx,
        "utterances_removed": removed,
    })))
}

/// An enrolled speaker as the clients see it.
///
/// The raw embedding never leaves the server — clients only need to know
/// whether one exists, since that is the difference between a label and an
/// identity that is recognised automatically next time.
#[derive(Debug, Serialize)]
pub struct SpeakerView {
    pub id: Uuid,
    pub display_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// True when this speaker has a reference voiceprint.
    pub has_voiceprint: bool,
    /// How many recordings this speaker is tagged in.
    pub recording_count: i64,
}

impl SpeakerView {
    fn new(speaker: &Speaker, recording_count: i64) -> Self {
        Self {
            id: speaker.id,
            display_name: speaker.display_name.clone(),
            created_at: speaker.created_at,
            has_voiceprint: speaker.embedding.is_some(),
            recording_count,
        }
    }
}

/// Body for naming a diarized speaker. Supply `speaker_id` to reuse an existing
/// identity, or `name` to reuse-by-name / create one.
#[derive(Debug, Deserialize)]
pub struct NameBody {
    #[serde(default)]
    pub name: Option<String>,
    /// Existing enrolled speaker to attach. Takes precedence over `name`.
    #[serde(default)]
    pub speaker_id: Option<Uuid>,
    /// When true, store this recording-speaker's voice embedding on the
    /// identity so the voice is auto-matched in future recordings.
    #[serde(default)]
    pub enroll: bool,
    /// Give this recording's voice to the speaker even if they already have one.
    ///
    /// A voiceprint is otherwise written once and never changed, so a first
    /// sample that was short or noisy is permanent — and enrolling the same
    /// person again under a second name stops them being recognised at all, so
    /// there was no way out. Measured, replacing a poor voiceprint with a good
    /// one took recognition in a different room from 0.69 to 0.82, against a
    /// stranger at about 0.61.
    #[serde(default)]
    pub replace_voiceprint: bool,
}

/// `POST /recordings/{id}/speakers/{local_idx}/name`
pub async fn name_speaker(
    State(state): State<AppState>,
    Path((id, local_idx)): Path<(Uuid, i32)>,
    Json(body): Json<NameBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let name = body.name.as_deref().map(str::trim).unwrap_or_default();
    if name.is_empty() && body.speaker_id.is_none() {
        return Err(ApiError(Error::BadRequest(
            "one of name or speaker_id is required".into(),
        )));
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

    let speaker = match body.speaker_id {
        // Tag with someone already in the library.
        Some(speaker_id) => state.db.get_speaker(speaker_id).await?,
        None => {
            // Reuse an existing enrolled speaker with this exact name, else create one.
            let existing = state
                .db
                .list_speakers()
                .await?
                .into_iter()
                .find(|s| s.display_name.eq_ignore_ascii_case(name));

            match existing {
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
            }
        }
    };

    // Enrolling an identity that already existed (picked from the library, or
    // matched by name) still has to learn the voice — otherwise a name given in
    // recording 2 would never carry to recording 3.
    let mut enrolled = speaker.embedding.is_some();
    let mut already_enrolled_as: Option<String> = None;
    if body.enroll {
        if let Some(embedding) = rec_speaker.embedding.as_ref() {
            if !enrolled || body.replace_voiceprint {
                // Is this voice already in the library under a different name?
                //
                // Two entries for one voice do not merely give inconsistent
                // names, they stop that person being recognised at all: a match
                // must stand clear of the rest of the library, and a voice
                // cannot stand clear of itself. Measured, one sample enrolled
                // twice took recognition from three of three to none.
                //
                // The enrolment still goes ahead — the caller asked for it, and
                // twins exist — but the answer says whose voice it resembles so
                // a client can offer to use that name instead.
                if let Ok(Some((other, similarity))) = state
                    .db
                    .nearest_enrolled(embedding, DUPLICATE_VOICE_SIMILARITY)
                    .await
                {
                    if other.id != speaker.id {
                        tracing::warn!(
                            %id, new_speaker = %speaker.display_name,
                            existing = %other.display_name, similarity,
                            "voice already enrolled under another name; two entries for \
                             one voice stop it being recognised"
                        );
                        already_enrolled_as = Some(other.display_name);
                    }
                }
                state.db.set_speaker_embedding(speaker.id, embedding).await?;
                enrolled = true;
            }
        }
    }

    state
        .db
        .set_recording_speaker(id, local_idx, Some(speaker.id))
        .await?;

    Ok(Json(json!({
        "local_idx": local_idx,
        "already_enrolled_as": already_enrolled_as,
        "speaker_id": speaker.id,
        "display_name": speaker.display_name,
        "enrolled": enrolled,
    })))
}

/// `DELETE /recordings/{id}/speakers/{local_idx}/name` — drop the identity from
/// this diarized speaker. The enrolled speaker itself is untouched; the line
/// reverts to "Speaker N".
pub async fn unname_speaker(
    State(state): State<AppState>,
    Path((id, local_idx)): Path<(Uuid, i32)>,
) -> ApiResult<Json<serde_json::Value>> {
    state.db.set_recording_speaker(id, local_idx, None).await?;
    Ok(Json(json!({ "local_idx": local_idx, "speaker_id": null })))
}

/// `GET /speakers` — the enrolled speaker library.
pub async fn list_speakers(
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    let speakers: Vec<SpeakerView> = state
        .db
        .list_speakers_with_usage()
        .await?
        .iter()
        .map(|(s, count)| SpeakerView::new(s, *count))
        .collect();
    Ok(Json(json!({ "speakers": speakers })))
}

/// Body for `PATCH /speakers/{id}`.
#[derive(Debug, Deserialize)]
pub struct RenameBody {
    pub name: String,
}

/// `PATCH /speakers/{id}` — rename an enrolled speaker everywhere at once.
pub async fn rename_speaker(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<RenameBody>,
) -> ApiResult<Json<SpeakerView>> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError(Error::BadRequest("name must not be empty".into())));
    }
    state.db.rename_speaker(id, name).await?;
    let speaker = state.db.get_speaker(id).await?;
    let count = state
        .db
        .list_speakers_with_usage()
        .await?
        .iter()
        .find(|(s, _)| s.id == id)
        .map(|(_, c)| *c)
        .unwrap_or(0);
    Ok(Json(SpeakerView::new(&speaker, count)))
}

/// `DELETE /speakers/{id}` — forget a speaker. Recordings tagged with them fall
/// back to "Speaker N" (`recording_speakers.speaker_id` is `ON DELETE SET NULL`).
pub async fn delete_speaker(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    state.db.delete_speaker(id).await?;
    Ok(Json(json!({ "id": id, "deleted": true })))
}
