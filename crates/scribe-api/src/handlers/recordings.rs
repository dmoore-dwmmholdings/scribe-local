//! Recording lifecycle endpoints (design §6, §11):
//!
//! * `POST   /recordings`                 — create (status `uploading`)
//! * `GET    /recordings`                 — list
//! * `GET    /recordings/{id}`            — detail (transcript + speakers + summaries)
//! * `POST   /recordings/{id}/complete`   — finalize → enqueue transcode
//! * `POST   /recordings/{id}/reprocess`  — re-run the whole pipeline

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use scribe_core::storage;
use scribe_core::summary_template;
use chrono::{DateTime, Utc};
use scribe_core::types::{
    Job, JobKind, JobState, Recording, RecordingSpeaker, RecordingStatus, Summary, Utterance,
};
use scribe_core::Error;
use scribe_llm::ChatMessage;

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
    /// Optional tag filter: only recordings carrying this tag are returned.
    pub tag: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub recordings: Vec<Recording>,
}

/// `GET /recordings?limit=&offset=&tag=`
pub async fn list_recordings(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<ListResponse>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    // Treat a blank/whitespace tag as "no filter".
    let tag = q.tag.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let recordings = state.db.list_recordings(limit, offset, tag).await?;
    Ok(Json(ListResponse { recordings }))
}

#[derive(Debug, Serialize)]
pub struct RecordingDetail {
    pub recording: Recording,
    pub speakers: Vec<RecordingSpeaker>,
    pub utterances: Vec<Utterance>,
    /// Every generated summary view — one per template — each carrying its
    /// `template`. Empty until the first summarize runs (Feature C).
    pub summaries: Vec<Summary>,
    /// Per-stage pipeline progress, so the client can show what is actually
    /// happening instead of an opaque "processing". `None` once the recording
    /// is `ready` and for recordings that never entered the pipeline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<PipelineProgress>,
}

/// The `complete`-time DAG in display order. `TranscribeSegment` is deliberately
/// absent: it is a side-channel live-transcription job, not a pipeline stage,
/// and showing it would make an import look like it runs ASR twice.
const PIPELINE_STAGES: [JobKind; 6] = [
    JobKind::Transcode,
    JobKind::Diarize,
    JobKind::Transcribe,
    JobKind::Merge,
    JobKind::Embed,
    JobKind::Summarize,
];

/// One stage of the pipeline as reported to the client.
#[derive(Debug, Serialize)]
pub struct StageProgress {
    pub kind: JobKind,
    /// `queued` / `running` / `done` / `failed`, or `pending` when the stage has
    /// not been enqueued yet (its predecessors are still running).
    pub state: String,
    /// When a worker claimed it — lets the client show elapsed time on the
    /// stage that is currently running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    pub attempts: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pipeline progress for a recording that is still being processed.
#[derive(Debug, Serialize)]
pub struct PipelineProgress {
    pub stages: Vec<StageProgress>,
    /// The stage currently running, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<JobKind>,
    pub completed: usize,
    pub total: usize,
}

/// Fold this recording's jobs into a fixed six-stage checklist.
///
/// Stages are enqueued lazily, so a kind missing from `jobs` has simply not
/// started yet — it is reported as `pending` rather than omitted, which keeps
/// the client's list a stable length instead of one that grows as work starts.
fn build_progress(jobs: &[Job]) -> PipelineProgress {
    let stages: Vec<StageProgress> = PIPELINE_STAGES
        .iter()
        .map(|kind| {
            // Last job wins: a retried or reprocessed stage has several rows.
            match jobs.iter().filter(|j| j.kind == *kind).next_back() {
                Some(job) => StageProgress {
                    kind: *kind,
                    state: job.state.as_str().to_string(),
                    started_at: job.locked_at,
                    finished_at: match job.state {
                        JobState::Done | JobState::Failed => Some(job.updated_at),
                        _ => None,
                    },
                    attempts: job.attempts,
                    error: job.error.clone(),
                },
                None => StageProgress {
                    kind: *kind,
                    state: "pending".to_string(),
                    started_at: None,
                    finished_at: None,
                    attempts: 0,
                    error: None,
                },
            }
        })
        .collect();

    let completed = stages.iter().filter(|s| s.state == "done").count();
    let current = stages
        .iter()
        .find(|s| s.state == "running")
        .map(|s| s.kind);

    PipelineProgress {
        stages,
        current,
        completed,
        total: PIPELINE_STAGES.len(),
    }
}

/// `GET /recordings/{id}` → recording + diarized speakers + transcript +
/// summaries (one per template).
pub async fn get_recording(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<RecordingDetail>> {
    let recording = state.db.get_recording(id).await?;
    let speakers = state.db.list_recording_speakers(id).await?;
    let utterances = state.db.list_utterances_by_recording(id).await?;
    let summaries = state.db.list_summaries_by_recording(id).await?;

    // Only meaningful while work is outstanding; a `ready` recording would just
    // report six done stages on every poll.
    let progress = match recording.status {
        RecordingStatus::Processing | RecordingStatus::Failed => {
            let jobs = state.db.list_jobs_by_recording(id).await?;
            Some(build_progress(&jobs))
        }
        _ => None,
    };

    Ok(Json(RecordingDetail {
        recording,
        speakers,
        utterances,
        summaries,
        progress,
    }))
}

/// Body for `POST /recordings/{id}/complete`.
#[derive(Debug, Default, Deserialize)]
pub struct CompleteBody {
    pub duration_ms: Option<i64>,
    /// Bookmark timestamps (ms offsets into the audio) captured during recording.
    /// When present, replaces the recording's marks; absent leaves the default.
    pub marks: Option<Vec<i32>>,
}

/// `POST /recordings/{id}/complete` → flip to `processing`, record duration and
/// bookmark marks if supplied, and enqueue the `transcode` job (the DB trigger
/// fires NOTIFY). 409 if the recording isn't in `uploading`.
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
    if let Some(marks) = &body.marks {
        state.db.set_recording_marks(id, marks).await?;
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

/// `POST /recordings/{id}/reprocess` → re-run the entire processing pipeline on
/// an existing recording (recovers recordings transcribed before bug fixes).
///
/// 404 if the recording doesn't exist. Otherwise, in one DB transaction, delete
/// the recording's DERIVED data — utterances, chunks, summaries, and pipeline
/// scratch artifacts — and its stale queue jobs (so the re-run isn't gated by
/// old `done` predecessors), flip the status to `processing`, then enqueue a
/// fresh `transcode` job which cascades the rest of the DAG. The transcoded
/// audio/segments already on disk are reused. Responds 202.
pub async fn reprocess_recording(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    // 404 if the recording doesn't exist (mirrors other recording routes).
    state.db.get_recording(id).await?;

    // Atomically clear derived data + stale jobs and flip to `processing`.
    state.db.reset_for_reprocess(id).await?;

    // Kick off the pipeline from the top. enqueue is idempotent per
    // (recording, kind); the prior jobs were just deleted, so this inserts a
    // fresh transcode job that cascades through diarize/transcribe/merge/etc.
    state.db.enqueue(id, JobKind::Transcode, json!({})).await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "id": id,
            "status": RecordingStatus::Processing.as_str(),
        })),
    ))
}

/// Body for `POST /recordings/{id}/summarize`. `template` is optional; absent →
/// `general`.
#[derive(Debug, Default, Deserialize)]
pub struct SummarizeBody {
    pub template: Option<String>,
}

/// `POST /recordings/{id}/summarize` → enqueue a (re-)summarize with the chosen
/// template. 404 if the recording doesn't exist; 400 for an unknown template id.
/// Enqueue is idempotent per `(recording, summarize)`: if a summarize job is
/// already queued/running this is treated as success (it will run). 202.
pub async fn summarize_recording(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: Option<Json<SummarizeBody>>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let body = body.map(|Json(b)| b).unwrap_or_default();

    // Resolve the template id: trimmed, defaults to `general`. A non-empty but
    // unknown id is a client error (400) so typos surface rather than silently
    // falling back.
    let raw = body.template.as_deref().unwrap_or("").trim().to_string();
    let template = if raw.is_empty() {
        summary_template::DEFAULT_TEMPLATE_ID.to_string()
    } else if summary_template::is_known(&raw) {
        summary_template::canonical_id(&raw).to_string()
    } else {
        return Err(ApiError(Error::BadRequest(format!(
            "unknown summary template `{raw}`"
        ))));
    };

    // 404 if the recording doesn't exist (mirrors other recording routes).
    state.db.get_recording(id).await?;

    // Enqueue ONE summarize job carrying the template. Idempotent against the
    // partial-unique index: a live summarize job is reused, so this is success.
    state
        .db
        .enqueue(id, JobKind::Summarize, json!({ "template": template }))
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "id": id,
            "template": template,
            "status": "queued",
        })),
    ))
}

/// `GET /summary-templates` → the built-in registry (id + label) so clients
/// don't hardcode the list.
pub async fn list_summary_templates() -> Json<serde_json::Value> {
    let templates: Vec<serde_json::Value> = summary_template::TEMPLATES
        .iter()
        .map(|t| json!({ "id": t.id, "label": t.label }))
        .collect();
    Json(json!({ "templates": templates }))
}

/// Body for `PUT /recordings/{id}/tags`.
#[derive(Debug, Default, Deserialize)]
pub struct SetTagsBody {
    #[serde(default)]
    pub tags: Vec<String>,
}

/// `PUT /recordings/{id}/tags` → replace the recording's tags with the supplied
/// (normalized) set. 404 if the recording doesn't exist. 200 with the stored,
/// normalized tag list.
pub async fn set_recording_tags(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: Option<Json<SetTagsBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let tags = state.db.set_recording_tags(id, &body.tags).await?;
    Ok(Json(json!({ "id": id, "tags": tags })))
}

/// `GET /tags` → every distinct tag in use, sorted alphabetically.
pub async fn list_tags(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let tags = state.db.distinct_tags().await?;
    Ok(Json(json!({ "tags": tags })))
}

/// Body for `PATCH /recordings/{id}/utterances/{utterance_id}`.
#[derive(Debug, Deserialize)]
pub struct EditUtteranceBody {
    pub text: String,
}

/// `PATCH /recordings/{id}/utterances/{utterance_id}` → correct an utterance's
/// text. Empty/whitespace text → 400. No matching utterance → 404. 200 with the
/// updated utterance in the same shape as `GET /recordings/{id}`'s
/// `utterances[]` entries.
pub async fn edit_utterance(
    State(state): State<AppState>,
    Path((id, utterance_id)): Path<(Uuid, i64)>,
    Json(body): Json<EditUtteranceBody>,
) -> ApiResult<Json<Utterance>> {
    let text = body.text.trim();
    if text.is_empty() {
        return Err(ApiError(Error::BadRequest(
            "utterance text must not be empty".into(),
        )));
    }

    let updated = state
        .db
        .update_utterance_text(id, utterance_id, text)
        .await?
        .ok_or_else(|| {
            Error::NotFound(format!(
                "utterance {utterance_id} not found in recording {id}"
            ))
        })?;

    Ok(Json(updated))
}

/// Body for `POST /recordings/{id}/translate`. `lang` is a free-form target
/// language NAME (e.g. `"Spanish"`); empty/whitespace → 400.
#[derive(Debug, Deserialize)]
pub struct TranslateBody {
    pub lang: String,
}

/// Response for `POST /recordings/{id}/translate`.
#[derive(Debug, Serialize)]
pub struct TranslateResponse {
    pub lang: String,
    pub text: String,
}

/// `POST /recordings/{id}/translate` → translate the recording's summary into the
/// requested language via the LLM, returning the model's text verbatim.
///
/// 400 if `lang` is empty/whitespace. 404 if the recording has no summary. 503 if
/// the LLM is unreachable (we don't hang or fall back to a placeholder). 200 with
/// `{ "lang", "text" }` on success.
pub async fn translate_summary(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<TranslateBody>,
) -> ApiResult<Json<TranslateResponse>> {
    let lang = body.lang.trim();
    if lang.is_empty() {
        return Err(ApiError(Error::BadRequest(
            "target language must not be empty".into(),
        )));
    }

    // Load the recording's summaries and pick the `general` one (else the first).
    // Empty list → no summary to translate (404).
    let summaries = state.db.list_summaries_by_recording(id).await?;
    let summary = summaries
        .iter()
        .find(|s| s.template.as_deref() == Some(summary_template::DEFAULT_TEMPLATE_ID))
        .or_else(|| summaries.first())
        .ok_or_else(|| Error::NotFound("no summary to translate".into()))?;

    let block = render_summary_block(summary);

    // Ask the LLM to translate. Mirror `/ask`'s LLM access exactly: the shared
    // `state.ollama` client + `state.cfg.llm.summarize_model`. Unlike `/ask` (which
    // degrades to a placeholder), this endpoint surfaces an unreachable LLM as 503
    // so the caller gets a clear, non-hanging error.
    let system = "You are a translator.";
    let user = format!(
        "Translate the following meeting summary into {lang}. Preserve the structure \
         and meaning. Output only the translation, no preamble.\n\n{block}"
    );
    let messages = [ChatMessage::system(system), ChatMessage::user(user)];

    let text = match state
        .ollama
        .chat(&state.cfg.llm.summarize_model, &messages)
        .await
    {
        Ok(text) => text.trim().to_string(),
        Err(e) => {
            tracing::warn!(error = %e, "ollama chat failed; returning 503 for translate");
            return Err(ApiError(Error::ServiceUnavailable(format!(
                "the language model is unavailable: {e}"
            ))));
        }
    };

    Ok(Json(TranslateResponse {
        lang: lang.to_string(),
        text,
    }))
}

/// Render a [`Summary`] into a plain-text block for translation: the title, the
/// summary prose, then "Action items:", "Decisions:" and "Topics:" sections.
/// Empty sections are skipped. The list fields are stored as JSON arrays of
/// strings (`serde_json::Value`); non-string entries are stringified.
fn render_summary_block(summary: &Summary) -> String {
    let mut out = String::new();

    if let Some(title) = summary.title.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(title);
        out.push_str("\n\n");
    }
    if let Some(body) = summary.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(body);
        out.push_str("\n\n");
    }

    push_list(&mut out, "Action items:", &summary.action_items);
    push_list(&mut out, "Decisions:", &summary.decisions);

    // Topics render as a single comma-joined line rather than a bulleted list.
    let topics = json_strings(&summary.topics);
    if !topics.is_empty() {
        out.push_str("Topics: ");
        out.push_str(&topics.join(", "));
        out.push('\n');
    }

    out.trim_end().to_string()
}

/// Append a `header` followed by one `- item` line per entry, skipping empty
/// lists. No-op when the JSON array is empty/missing.
fn push_list(out: &mut String, header: &str, value: &serde_json::Value) {
    let items = json_strings(value);
    if items.is_empty() {
        return;
    }
    out.push_str(header);
    out.push('\n');
    for item in items {
        out.push_str("- ");
        out.push_str(&item);
        out.push('\n');
    }
    out.push('\n');
}

/// Extract the non-empty string entries of a JSON array; non-string entries are
/// stringified. Returns an empty `Vec` for anything that isn't a JSON array.
fn json_strings(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|i| match i {
                serde_json::Value::String(s) => {
                    let t = s.trim();
                    (!t.is_empty()).then(|| t.to_string())
                }
                other => Some(other.to_string()),
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(kind: JobKind, state: JobState) -> Job {
        let now = Utc::now();
        Job {
            id: 1,
            recording_id: None,
            kind,
            state,
            priority: 0,
            attempts: 0,
            run_after: now,
            locked_by: None,
            locked_at: None,
            payload: serde_json::Value::Null,
            error: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Stages are enqueued lazily, so early in a run most kinds have no row at
    /// all. They must still appear, as `pending`, or the client's checklist
    /// would grow from one item to six as work progresses.
    #[test]
    fn stages_not_yet_enqueued_are_reported_as_pending() {
        let jobs = vec![
            job(JobKind::Transcode, JobState::Done),
            job(JobKind::Diarize, JobState::Running),
        ];

        let p = build_progress(&jobs);

        assert_eq!(p.total, 6);
        assert_eq!(p.stages.len(), 6);
        assert_eq!(p.completed, 1);
        assert_eq!(p.current, Some(JobKind::Diarize));
        let by_kind = |k: JobKind| {
            p.stages.iter().find(|s| s.kind == k).unwrap().state.clone()
        };
        assert_eq!(by_kind(JobKind::Transcode), "done");
        assert_eq!(by_kind(JobKind::Diarize), "running");
        assert_eq!(by_kind(JobKind::Merge), "pending");
        assert_eq!(by_kind(JobKind::Summarize), "pending");
    }

    /// A reprocess leaves several rows for the same kind; the newest one is the
    /// live state, so an old `done` row must not mask a fresh `running` one.
    #[test]
    fn latest_job_per_kind_wins() {
        let mut old = job(JobKind::Transcribe, JobState::Done);
        old.id = 1;
        let mut new = job(JobKind::Transcribe, JobState::Running);
        new.id = 2;

        let p = build_progress(&[old, new]);

        let stage = p.stages.iter().find(|s| s.kind == JobKind::Transcribe).unwrap();
        assert_eq!(stage.state, "running");
        assert_eq!(p.current, Some(JobKind::Transcribe));
        assert_eq!(p.completed, 0);
    }

    /// The live side-channel job is not a pipeline stage and must never appear
    /// in the checklist — otherwise an import looks like it transcribes twice.
    #[test]
    fn transcribe_segment_is_excluded() {
        let jobs = vec![job(JobKind::TranscribeSegment, JobState::Done)];

        let p = build_progress(&jobs);

        assert_eq!(p.stages.len(), 6);
        assert!(p.stages.iter().all(|s| s.kind != JobKind::TranscribeSegment));
        assert_eq!(p.completed, 0);
    }

    /// A failed stage surfaces its error so the client can show why, rather
    /// than a bare "processing failed".
    #[test]
    fn failed_stage_carries_its_error() {
        let mut failed = job(JobKind::Diarize, JobState::Failed);
        failed.error = Some("model load failed".to_string());

        let p = build_progress(&[job(JobKind::Transcode, JobState::Done), failed]);

        let stage = p.stages.iter().find(|s| s.kind == JobKind::Diarize).unwrap();
        assert_eq!(stage.state, "failed");
        assert_eq!(stage.error.as_deref(), Some("model load failed"));
        assert_eq!(p.current, None);
    }
}
