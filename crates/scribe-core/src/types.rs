//! The domain model. These types mirror the Postgres tables in `migrations/`
//! (design §10) but carry no storage logic — leaf crates map rows to these.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// --------------------------------------------------------------------------
// Recordings
// --------------------------------------------------------------------------

/// Lifecycle of a captured meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingStatus {
    /// Segments are still arriving.
    Uploading,
    /// `complete` fired; the pipeline is running.
    Processing,
    /// Transcript + summary available.
    Ready,
    /// A stage failed terminally.
    Failed,
}

impl RecordingStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RecordingStatus::Uploading => "uploading",
            RecordingStatus::Processing => "processing",
            RecordingStatus::Ready => "ready",
            RecordingStatus::Failed => "failed",
        }
    }
}

impl fmt::Display for RecordingStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RecordingStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "uploading" => Ok(Self::Uploading),
            "processing" => Ok(Self::Processing),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown recording status: {other}")),
        }
    }
}

/// A captured meeting (`recordings` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: Uuid,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub device_id: Option<String>,
    pub duration_ms: Option<i64>,
    pub status: RecordingStatus,
    /// Helps diarization clustering when known up front.
    pub participants_expected: Option<i32>,
    pub audio_format: Option<String>,
    pub sample_rate: Option<i32>,
    pub storage_key: Option<String>,
    /// Free-form organization tags (Postgres `text[]`), normalized lowercase.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Bookmark timestamps (millisecond offsets into the audio) captured when the
    /// user tapped "Mark" while recording. Postgres `integer[]`; empty by default.
    #[serde(default)]
    pub marks: Vec<i32>,
}

/// One chunked-upload piece (`segments` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: Uuid,
    pub recording_id: Uuid,
    pub seq: i32,
    pub storage_key: String,
    pub start_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Vec<u8>>,
    pub uploaded_at: DateTime<Utc>,
}

// --------------------------------------------------------------------------
// Jobs / queue
// --------------------------------------------------------------------------

/// A pipeline stage. The variants double as the queue `kind` column values and
/// form the DAG in design §7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobKind {
    /// Concatenate AAC segments → 16 kHz mono WAV.
    Transcode,
    /// VAD + segmentation + speaker embeddings + clustering.
    Diarize,
    /// ASR with word-level timestamps.
    Transcribe,
    /// Assign each transcribed word a speaker (WhisperX merge).
    Merge,
    /// Chunk transcript → embeddings → pgvector.
    Embed,
    /// LLM title/summary/action items.
    Summarize,
    /// Incremental ("live") transcription: transcribe newly-uploaded segments
    /// into provisional utterances *during* recording. A side-channel job
    /// (triggered per upload), not part of the `complete`-time DAG.
    #[serde(rename = "transcribe_segment")]
    TranscribeSegment,
}

impl JobKind {
    /// Every kind a worker can claim. `["all"]` expands to this. Includes the
    /// side-channel `TranscribeSegment` so a default worker handles live jobs.
    pub const ALL: [JobKind; 7] = [
        JobKind::Transcode,
        JobKind::Diarize,
        JobKind::Transcribe,
        JobKind::Merge,
        JobKind::Embed,
        JobKind::Summarize,
        JobKind::TranscribeSegment,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::Transcode => "transcode",
            JobKind::Diarize => "diarize",
            JobKind::Transcribe => "transcribe",
            JobKind::Merge => "merge",
            JobKind::Embed => "embed",
            JobKind::Summarize => "summarize",
            JobKind::TranscribeSegment => "transcribe_segment",
        }
    }

    /// The stages that must complete before this one can run (DAG edges).
    /// `TranscribeSegment` is side-channel (no DAG edges).
    pub fn predecessors(&self) -> &'static [JobKind] {
        match self {
            JobKind::Transcode => &[],
            JobKind::Diarize => &[JobKind::Transcode],
            JobKind::Transcribe => &[JobKind::Transcode],
            JobKind::Merge => &[JobKind::Diarize, JobKind::Transcribe],
            JobKind::Embed => &[JobKind::Merge],
            JobKind::Summarize => &[JobKind::Merge],
            JobKind::TranscribeSegment => &[],
        }
    }

    /// The stages to enqueue once this one finishes.
    pub fn successors(&self) -> &'static [JobKind] {
        match self {
            JobKind::Transcode => &[JobKind::Diarize, JobKind::Transcribe],
            JobKind::Diarize => &[JobKind::Merge],
            JobKind::Transcribe => &[JobKind::Merge],
            JobKind::Merge => &[JobKind::Embed, JobKind::Summarize],
            JobKind::Embed => &[],
            JobKind::Summarize => &[],
            JobKind::TranscribeSegment => &[],
        }
    }
}

impl fmt::Display for JobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for JobKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "transcode" => Ok(Self::Transcode),
            "diarize" => Ok(Self::Diarize),
            "transcribe" => Ok(Self::Transcribe),
            "merge" => Ok(Self::Merge),
            "embed" => Ok(Self::Embed),
            "summarize" => Ok(Self::Summarize),
            "transcribe_segment" => Ok(Self::TranscribeSegment),
            other => Err(format!("unknown job kind: {other}")),
        }
    }
}

/// Queue state of a job (`jobs.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
}

impl JobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
        }
    }
}

impl fmt::Display for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for JobState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown job state: {other}")),
        }
    }
}

/// A row of the work queue (`jobs` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub recording_id: Option<Uuid>,
    pub kind: JobKind,
    pub state: JobState,
    pub priority: i32,
    pub attempts: i32,
    pub run_after: DateTime<Utc>,
    pub locked_by: Option<String>,
    pub locked_at: Option<DateTime<Utc>>,
    pub payload: serde_json::Value,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// --------------------------------------------------------------------------
// Speakers / diarization
// --------------------------------------------------------------------------

/// An enrolled, named voice (`speakers` table) — cross-recording identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Speaker {
    pub id: Uuid,
    pub display_name: String,
    /// Reference voice embedding (speaker-embedding model dimension).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
}

/// Per-recording diarized speaker, optionally matched to a known [`Speaker`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSpeaker {
    pub recording_id: Uuid,
    /// `Speaker 0`, `Speaker 1`, … within this recording.
    pub local_idx: i32,
    /// Resolved identity, null until named/matched.
    pub speaker_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// Convenience: the resolved display name if matched, else `Speaker N`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

// --------------------------------------------------------------------------
// Transcript
// --------------------------------------------------------------------------

/// A single word with its timing (stored in `utterances.words` as JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Word {
    #[serde(rename = "w")]
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default)]
    pub conf: f32,
    /// Diarized speaker index assigned during merge (None before merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_idx: Option<i32>,
}

/// One utterance / speaker turn (`utterances` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Utterance {
    pub id: i64,
    pub recording_id: Uuid,
    pub local_idx: Option<i32>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub words: Vec<Word>,
    /// Resolved speaker name for display, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_name: Option<String>,
}

// --------------------------------------------------------------------------
// Retrieval / RAG
// --------------------------------------------------------------------------

/// A retrieval chunk (`chunks` table) — text window + embedding for search/RAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: i64,
    pub recording_id: Uuid,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

/// LLM-generated metadata (`summaries` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub recording_id: Uuid,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub action_items: serde_json::Value,
    pub topics: serde_json::Value,
    pub decisions: serde_json::Value,
    pub model: Option<String>,
    /// Summary template id that framed this summary (`general`, `interview`, …).
    /// `None` for summaries written before templates existed.
    pub template: Option<String>,
    pub created_at: DateTime<Utc>,
}

// --------------------------------------------------------------------------
// Search / Q&A view models (API responses)
// --------------------------------------------------------------------------

/// One hybrid-search hit returned by `GET /search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub recording_id: Uuid,
    pub recording_title: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub text: String,
    /// Fused relevance score (higher = better).
    pub score: f32,
}

/// A citation backing a RAG answer (`POST /ask`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub recording_id: Uuid,
    pub recording_title: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub snippet: String,
}

/// The answer to a `POST /ask` question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub answer: String,
    pub citations: Vec<Citation>,
}
