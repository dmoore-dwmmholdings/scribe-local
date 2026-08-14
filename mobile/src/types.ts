/**
 * TypeScript types that mirror the server's `scribe_core::types` Rust structs
 * and the HTTP API contract defined in scribe-design.md §6 / §11.
 *
 * All UUIDs are represented as strings (the Rust side serialises uuid::Uuid as
 * lowercase hyphenated UUID strings).  Timestamps are ISO-8601 strings (Rust
 * chrono::DateTime<Utc> serialises to RFC-3339).
 */

// ---------------------------------------------------------------------------
// Recordings
// ---------------------------------------------------------------------------

/** Lifecycle of a captured meeting — mirrors RecordingStatus in types.rs. */
export type RecordingStatus = 'uploading' | 'processing' | 'ready' | 'failed';

/** A captured meeting (`recordings` table). */
export interface Recording {
  id: string;
  title: string | null;
  created_at: string;
  device_id: string | null;
  duration_ms: number | null;
  status: RecordingStatus;
  participants_expected: number | null;
  audio_format: string | null;
  sample_rate: number | null;
  storage_key: string | null;
  /** Free-form organization tags. Optional: absent on older backends. */
  tags?: string[];
  /** Bookmarked moments (ms offsets) captured during recording. */
  marks?: number[];
}

/** Response from `PUT /recordings/{id}/tags`. */
export interface SetTagsResponse {
  id: string;
  tags: string[];
}

// ---------------------------------------------------------------------------
// Segments
// ---------------------------------------------------------------------------

/** One chunked-upload piece (`segments` table). */
export interface Segment {
  id: string;
  recording_id: string;
  seq: number;
  storage_key: string;
  start_ms: number | null;
  duration_ms: number | null;
  bytes: number | null;
  uploaded_at: string;
}

// ---------------------------------------------------------------------------
// Speakers / diarisation
// ---------------------------------------------------------------------------

/** An enrolled, named voice (`speakers` table). */
export interface Speaker {
  id: string;
  display_name: string;
  created_at: string;
}

/**
 * Per-recording diarised speaker, optionally matched to a known Speaker.
 * Mirrors `RecordingSpeaker` in types.rs plus the `display_name` convenience
 * field the API adds.
 */
export interface RecordingSpeaker {
  recording_id: string;
  local_idx: number;
  speaker_id: string | null;
  display_name: string | null;
}

// ---------------------------------------------------------------------------
// Transcript
// ---------------------------------------------------------------------------

/** A single word with timing (stored in `utterances.words` as JSON). */
export interface Word {
  w: string;
  start_ms: number;
  end_ms: number;
  conf: number;
  local_idx?: number;
}

/** One utterance / speaker turn (`utterances` table). */
export interface Utterance {
  id: number;
  recording_id: string;
  local_idx: number | null;
  start_ms: number;
  end_ms: number;
  text: string;
  words: Word[];
  speaker_name: string | null;
}

// ---------------------------------------------------------------------------
// LLM-generated metadata
// ---------------------------------------------------------------------------

/**
 * LLM-generated summary (`summaries` table).
 * `action_items`, `topics`, and `decisions` are stored as JSONB in Postgres;
 * we treat them as string arrays for display purposes.
 */
export interface Summary {
  title: string | null;
  summary: string | null;
  action_items: string[] | null;
  topics: string[] | null;
  decisions: string[] | null;
  /** Which summary template produced this (null for legacy/auto rows). */
  template?: string | null;
}

/** A selectable summary template (`GET /summary-templates`). */
export interface SummaryTemplate {
  id: string;
  label: string;
}

/** Response from `GET /summary-templates`. */
export interface SummaryTemplatesResponse {
  templates: SummaryTemplate[];
}

/** Response from `POST /recordings/{id}/summarize`. */
export interface ResummarizeResponse {
  id: string;
  template: string;
  status: string;
}

// ---------------------------------------------------------------------------
// Search / RAG
// ---------------------------------------------------------------------------

/** One hybrid-search hit returned by `GET /search`. */
export interface SearchHit {
  recording_id: string;
  recording_title: string | null;
  start_ms: number | null;
  end_ms: number | null;
  text: string;
  score: number;
}

/** A citation backing a RAG answer from `POST /ask`. */
export interface Citation {
  recording_id: string;
  recording_title: string | null;
  start_ms: number | null;
  end_ms: number | null;
  snippet: string;
}

// ---------------------------------------------------------------------------
// API request / response shapes
// ---------------------------------------------------------------------------

/** Body for `POST /recordings`. */
export interface CreateRecordingRequest {
  title?: string;
  participants_expected?: number;
  device_id?: string;
  audio_format?: string;
  sample_rate?: number;
}

/**
 * Response from `POST /recordings` (201).
 * The `upload` object contains the segment URL template the client uses to
 * construct PUT targets: `/recordings/{id}/segments/{seq}`.
 */
export interface CreateRecordingResponse {
  id: string;
  status: RecordingStatus;
  upload: {
    segment_url_template: string; // e.g. "/recordings/{id}/segments/{seq}"
  };
}

/** Response from `PUT /recordings/{id}/segments/{seq}?ext=m4a`. */
export interface UploadSegmentResponse {
  seq: number;
  bytes: number;
  storage_key: string;
}

/** Body for `POST /recordings/{id}/complete`. */
export interface CompleteRecordingRequest {
  duration_ms?: number;
  /** Bookmarked moments (ms offsets) captured during recording. */
  marks?: number[];
}

/** Response from `POST /recordings/{id}/complete`. */
export interface CompleteRecordingResponse {
  id: string;
  status: 'processing';
}

/** Response from `GET /recordings?limit=&offset=`. */
export interface ListRecordingsResponse {
  recordings: Recording[];
}

/** Response from `GET /recordings/{id}`. */
export interface RecordingDetailResponse {
  recording: Recording;
  speakers: RecordingSpeaker[];
  utterances: Utterance[];
  /** Legacy single summary (older backends). Prefer `summaries`. */
  summary?: Summary | null;
  /** All generated summary views, one per template (newer backends). */
  summaries?: Summary[];
  /** Per-stage pipeline progress while processing (newer backends only). */
  progress?: PipelineProgress;
}

/** A pipeline stage the server reports while a recording is processing. */
export type StageKind =
  | 'transcode'
  | 'diarize'
  | 'transcribe'
  | 'merge'
  | 'embed'
  | 'summarize';

/**
 * `pending` means the stage has not been enqueued yet — stages are enqueued
 * lazily as their predecessors finish, so this is normal, not an error.
 */
export type StageState = 'pending' | 'queued' | 'running' | 'done' | 'failed';

export interface StageProgress {
  kind: StageKind;
  state: StageState;
  /** When a worker claimed the stage — the basis for the elapsed timer. */
  started_at?: string;
  finished_at?: string;
  attempts: number;
  error?: string;
}

export interface PipelineProgress {
  stages: StageProgress[];
  current?: StageKind;
  completed: number;
  total: number;
}

/** Response from `POST /recordings/{id}/reprocess`. */
export interface ReprocessResponse {
  id: string;
  status: string;
}

/** Response from `POST /recordings/{id}/translate`. */
export interface TranslateResponse {
  lang: string;
  text: string;
}

/** Response from `GET /health`. */
export interface HealthResponse {
  status: string;
  version: string;
  db: string;
}

/** Response from `GET /search`. */
export interface SearchResponse {
  hits: SearchHit[];
}

/** Body for `POST /ask`. */
export interface AskRequest {
  question: string;
  filters?: {
    from?: string;
    to?: string;
    speaker?: string;
    recording?: string;
  };
  top_k?: number;
}

/** Response from `POST /ask`. */
export interface AskResponse {
  answer: string;
  citations: Citation[];
}

/** Body for `POST /recordings/{id}/speakers/{local_idx}/name`. */
export interface NameSpeakerRequest {
  name: string;
  enroll?: boolean;
}

/** Response from `POST /recordings/{id}/speakers/{local_idx}/name`. */
export interface NameSpeakerResponse {
  local_idx: number;
  speaker_id: string;
  display_name: string;
}

// ---------------------------------------------------------------------------
// Local-only types (not mirrored from Rust)
// ---------------------------------------------------------------------------

/** Status of an individual segment in the local upload queue. */
export type SegmentUploadStatus = 'pending' | 'uploading' | 'done' | 'failed';

/** A pending segment held in the local upload queue. */
export interface PendingSegment {
  id: string; // local UUID
  recordingId: string;
  seq: number;
  fileUri: string; // local file:// path
  startMs: number;
  durationMs: number;
  status: SegmentUploadStatus;
  attempts: number;
  lastAttemptAt: number | null;
}

/** App settings persisted to SecureStore / AsyncStorage. */
export interface AppSettings {
  baseUrl: string; // e.g. "https://scribe.<tailnet>.ts.net"
  deviceKey: string; // Bearer token for API auth
  deviceId: string; // stable device identifier sent on POST /recordings
  audioQuality: 'low' | 'medium' | 'high'; // maps to bitrate
  defaultParticipants: number;
  updateToken: string; // Admin bearer token for /admin/* endpoints
  /** Disable the animated ember orb (diagnostic / battery / motion-sensitivity). */
  reduceMotion: boolean;
}

// ---------------------------------------------------------------------------
// Admin / update API types
// ---------------------------------------------------------------------------

/** Response from `GET /admin/info`. */
export interface UpdateInfoResponse {
  version: string;
  target: string;
  update_enabled: boolean;
  restart_mode: string;
  has_backup: boolean;
}

/** Response from `POST /admin/update`. */
export interface UpdateResponse {
  from_version: string;
  to_version: string;
  target: string;
  restarting: true;
  restart_in_ms: number;
}

/** Response from `POST /admin/update/rollback`. */
export interface RollbackResponse {
  restored_version: string;
  restarting: boolean;
  restart_in_ms: number;
}
