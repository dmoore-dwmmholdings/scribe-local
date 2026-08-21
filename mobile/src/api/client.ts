/**
 * Typed API client for the Scribe server.
 *
 * All methods inject `Authorization: Bearer <deviceKey>` and resolve paths
 * against the configured `baseUrl` (a Tailscale MagicDNS HTTPS URL).
 *
 * The client is intentionally thin: it is a fetch wrapper, not an ORM or
 * cache layer.  State management and caching live in src/state/.
 *
 * Upload contract note:
 *   v1 uses a direct PUT per segment (simple, works).  The design's preferred
 *   path for production is tus resumable uploads (`tus-js-client` v4 against a
 *   rustus sidecar at /files).  Swap the `uploadSegment` method to tus when
 *   you add rustus.
 */

// SDK 54: createUploadTask / FileSystemUploadType (the binary upload + progress
// API) moved to the /legacy entry point. Importing from there preserves the
// exact upload behavior and progress callbacks the update screen relies on.
import * as FileSystem from 'expo-file-system/legacy';
import type {
  AskRequest,
  AskResponse,
  CompleteRecordingRequest,
  CompleteRecordingResponse,
  CreateRecordingRequest,
  CreateRecordingResponse,
  EnrolledSpeaker,
  HealthResponse,
  ListRecordingsResponse,
  ListSpeakersResponse,
  NameSpeakerRequest,
  NameSpeakerResponse,
  OverrideRequest,
  ProcessingSchedule,
  ProcessingScheduleResponse,
  RecordingDetailResponse,
  ReprocessResponse,
  ResummarizeResponse,
  RollbackResponse,
  SearchResponse,
  SetTagsResponse,
  SummaryTemplatesResponse,
  TranslateResponse,
  UpdateInfoResponse,
  UpdateResponse,
  UploadSegmentResponse,
  Utterance,
} from '../types';
import { useSettingsStore } from '../state/settingsStore';

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/** Build the absolute URL from a path (e.g. "/recordings"). */
function url(baseUrl: string, path: string): string {
  return `${baseUrl.replace(/\/$/, '')}${path}`;
}

interface RequestOptions {
  method?: string;
  headers?: Record<string, string>;
  body?: BodyInit | null;
  signal?: AbortSignal;
}

async function apiFetch<T>(
  path: string,
  options: RequestOptions = {},
): Promise<T> {
  const { baseUrl, deviceKey } = useSettingsStore.getState();

  if (!baseUrl) {
    throw new Error('Scribe server URL is not configured. Go to Settings.');
  }

  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    Authorization: `Bearer ${deviceKey}`,
    ...options.headers,
  };

  const response = await fetch(url(baseUrl, path), {
    method: options.method ?? 'GET',
    headers,
    body: options.body ?? null,
    signal: options.signal,
  });

  if (!response.ok) {
    const text = await response.text().catch(() => '');
    throw new ApiError(response.status, text || response.statusText, path);
  }

  // Some endpoints return 204 No Content
  if (response.status === 204) {
    return undefined as unknown as T;
  }

  return response.json() as Promise<T>;
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    public readonly path: string,
  ) {
    super(`API ${status} on ${path}: ${message}`);
    this.name = 'ApiError';
  }
}

// ---------------------------------------------------------------------------
// Public API methods
// ---------------------------------------------------------------------------

export const api = {
  // -------------------------------------------------------------------------
  // Health
  // -------------------------------------------------------------------------

  health(): Promise<HealthResponse> {
    // Health endpoint does NOT require auth per the spec.
    const { baseUrl } = useSettingsStore.getState();
    return fetch(url(baseUrl, '/health')).then((r) => r.json() as Promise<HealthResponse>);
  },

  // -------------------------------------------------------------------------
  // Recordings
  // -------------------------------------------------------------------------

  createRecording(req: CreateRecordingRequest): Promise<CreateRecordingResponse> {
    return apiFetch<CreateRecordingResponse>('/recordings', {
      method: 'POST',
      body: JSON.stringify(req),
    });
  },

  listRecordings(params?: {
    limit?: number;
    offset?: number;
  }): Promise<ListRecordingsResponse> {
    const qs = new URLSearchParams();
    if (params?.limit != null) qs.set('limit', String(params.limit));
    if (params?.offset != null) qs.set('offset', String(params.offset));
    const query = qs.toString() ? `?${qs}` : '';
    return apiFetch<ListRecordingsResponse>(`/recordings${query}`);
  },

  getRecording(id: string): Promise<RecordingDetailResponse> {
    return apiFetch<RecordingDetailResponse>(`/recordings/${id}`);
  },

  /**
   * Delete a recording, everything derived from it, and its audio blobs.
   * Irreversible: the server removes the blob directory before the row.
   */
  deleteRecording(id: string): Promise<void> {
    return apiFetch<void>(`/recordings/${id}`, { method: 'DELETE' });
  },

  completeRecording(
    id: string,
    req: CompleteRecordingRequest = {},
  ): Promise<CompleteRecordingResponse> {
    return apiFetch<CompleteRecordingResponse>(`/recordings/${id}/complete`, {
      method: 'POST',
      body: JSON.stringify(req),
    });
  },

  // -------------------------------------------------------------------------
  // Summary templates
  // -------------------------------------------------------------------------

  /** GET /summary-templates — the selectable summary templates. */
  summaryTemplates(): Promise<SummaryTemplatesResponse> {
    return apiFetch<SummaryTemplatesResponse>('/summary-templates');
  },

  /**
   * POST /recordings/{id}/summarize — (re)generate the summary with a template.
   * Enqueues a summarize job; poll GET /recordings/{id} until `summary.template`
   * reflects the requested template.
   */
  resummarize(id: string, template: string): Promise<ResummarizeResponse> {
    return apiFetch<ResummarizeResponse>(`/recordings/${id}/summarize`, {
      method: 'POST',
      body: JSON.stringify({ template }),
    });
  },

  // -------------------------------------------------------------------------
  // Segment upload
  // -------------------------------------------------------------------------

  /**
   * Upload a single audio segment as a raw binary PUT.
   *
   * v1 direct PUT — no resumable semantics.  For production, swap this to a
   * tus PATCH sequence against `rustus` at /files (see design §11 "Upload").
   * The `tus-js-client` v4 or `@cuvent/react-native-better-tus-client` are
   * the recommended client libs.
   *
   * Optional timing headers let the server store segment metadata without
   * reparsing the audio file.
   */
  async uploadSegment(params: {
    recordingId: string;
    seq: number;
    fileUri: string;
    startMs?: number;
    durationMs?: number;
    signal?: AbortSignal;
  }): Promise<UploadSegmentResponse> {
    const { baseUrl, deviceKey } = useSettingsStore.getState();

    if (!baseUrl) {
      throw new Error('Scribe server URL is not configured. Go to Settings.');
    }

    // Fetch the local file as a blob so we can PUT raw bytes
    const fileResponse = await fetch(params.fileUri);
    const blob = await fileResponse.blob();

    const headers: Record<string, string> = {
      'Content-Type': 'audio/mp4',
      Authorization: `Bearer ${deviceKey}`,
    };

    if (params.startMs != null) {
      headers['X-Segment-Start-Ms'] = String(params.startMs);
    }
    if (params.durationMs != null) {
      headers['X-Segment-Duration-Ms'] = String(params.durationMs);
    }

    const path = `/recordings/${params.recordingId}/segments/${params.seq}?ext=m4a`;
    const response = await fetch(url(baseUrl, path), {
      method: 'PUT',
      headers,
      body: blob,
      signal: params.signal,
    });

    if (!response.ok) {
      const text = await response.text().catch(() => '');
      throw new ApiError(response.status, text || response.statusText, path);
    }

    return response.json() as Promise<UploadSegmentResponse>;
  },

  /**
   * Import an existing audio file as a single segment (seq 0) of a recording,
   * for transcription of audio not captured in-app (voice memos, prior
   * recordings, other apps' exports).
   *
   * Uses expo-file-system's binary upload so the file streams from disk — large
   * imports don't get buffered into memory like the blob-based `uploadSegment`.
   * The server's segment route has its body limit disabled, so size is bounded
   * only by storage/time. `ext` is the file's extension (mp3/m4a/wav/…) so the
   * server's ffmpeg transcode picks the right demuxer.
   */
  async importAudioSegment(params: {
    recordingId: string;
    fileUri: string;
    ext: string;
    contentType: string;
    durationMs?: number;
  }): Promise<UploadSegmentResponse> {
    const { baseUrl, deviceKey } = useSettingsStore.getState();
    if (!baseUrl) {
      throw new Error('Scribe server URL is not configured. Go to Settings.');
    }

    const headers: Record<string, string> = {
      Authorization: `Bearer ${deviceKey}`,
      'Content-Type': params.contentType,
      'X-Segment-Start-Ms': '0',
    };
    if (params.durationMs != null) {
      headers['X-Segment-Duration-Ms'] = String(params.durationMs);
    }

    const path = `/recordings/${params.recordingId}/segments/0?ext=${encodeURIComponent(params.ext)}`;
    const task = FileSystem.createUploadTask(url(baseUrl, path), params.fileUri, {
      httpMethod: 'PUT',
      uploadType: FileSystem.FileSystemUploadType.BINARY_CONTENT,
      headers,
    });

    const result = await task.uploadAsync();
    if (!result) throw new Error('Upload returned no response');
    if (result.status < 200 || result.status >= 300) {
      throw new ApiError(result.status, result.body || 'Upload failed', path);
    }
    return JSON.parse(result.body) as UploadSegmentResponse;
  },

  // -------------------------------------------------------------------------
  // Speakers
  // -------------------------------------------------------------------------

  /**
   * POST /recordings/{id}/speakers/{local_idx}/name — tag a diarized speaker.
   *
   * Pass `speaker_id` to reuse someone already tagged in an earlier recording,
   * or `name` to reuse-by-name / create a new identity. `enroll` stores the
   * voiceprint, which is what makes the tag apply to recordings uploaded later.
   */
  nameSpeaker(
    recordingId: string,
    localIdx: number,
    req: NameSpeakerRequest,
  ): Promise<NameSpeakerResponse> {
    return apiFetch<NameSpeakerResponse>(
      `/recordings/${recordingId}/speakers/${localIdx}/name`,
      {
        method: 'POST',
        body: JSON.stringify(req),
      },
    );
  },

  /**
   * DELETE /recordings/{id}/speakers/{local_idx}/name — untag this recording's
   * speaker. The enrolled speaker stays in the library; the line reverts to
   * "Speaker N".
   */
  unnameSpeaker(recordingId: string, localIdx: number): Promise<void> {
    return apiFetch<void>(`/recordings/${recordingId}/speakers/${localIdx}/name`, {
      method: 'DELETE',
    });
  },

  /** GET /speakers — the enrolled speaker library, shared across recordings. */
  listSpeakers(): Promise<ListSpeakersResponse> {
    return apiFetch<ListSpeakersResponse>('/speakers');
  },

  /** PATCH /speakers/{id} — rename a speaker in every recording at once. */
  renameSpeaker(speakerId: string, name: string): Promise<EnrolledSpeaker> {
    return apiFetch<EnrolledSpeaker>(`/speakers/${speakerId}`, {
      method: 'PATCH',
      body: JSON.stringify({ name }),
    });
  },

  /** DELETE /speakers/{id} — forget a speaker; tagged recordings go anonymous. */
  deleteSpeaker(speakerId: string): Promise<void> {
    return apiFetch<void>(`/speakers/${speakerId}`, { method: 'DELETE' });
  },

  // -------------------------------------------------------------------------
  // Tags & transcript editing
  // -------------------------------------------------------------------------

  /** PUT /recordings/{id}/tags — replace the recording's tag set. */
  setTags(id: string, tags: string[]): Promise<SetTagsResponse> {
    return apiFetch<SetTagsResponse>(`/recordings/${id}/tags`, {
      method: 'PUT',
      body: JSON.stringify({ tags }),
    });
  },

  /**
   * PUT /recordings/{id}/participants — state how many people speak in this
   * recording. Diarization pins its clustering to this count, so it is the
   * strongest correction for a recording that came back with the wrong number
   * of speakers. Takes effect on the next reprocess.
   */
  setParticipants(id: string, participantsExpected: number | null): Promise<void> {
    return apiFetch<void>(`/recordings/${id}/participants`, {
      method: 'PUT',
      body: JSON.stringify({ participants_expected: participantsExpected }),
    });
  },

  /** PATCH /recordings/{id}/utterances/{utteranceId} — correct an utterance's text. */
  editUtterance(recordingId: string, utteranceId: number, text: string): Promise<Utterance> {
    return apiFetch<Utterance>(`/recordings/${recordingId}/utterances/${utteranceId}`, {
      method: 'PATCH',
      body: JSON.stringify({ text }),
    });
  },

  /**
   * POST /recordings/{id}/reprocess — re-run the whole pipeline on an existing
   * recording (transcription/diarization/summary). Useful to recover recordings
   * processed before a fix. Poll GET /recordings/{id} until status is `ready`.
   */
  reprocess(id: string): Promise<ReprocessResponse> {
    return apiFetch<ReprocessResponse>(`/recordings/${id}/reprocess`, { method: 'POST' });
  },

  /** POST /recordings/{id}/translate — translate the summary into `lang` (LLM). */
  translate(id: string, lang: string): Promise<TranslateResponse> {
    return apiFetch<TranslateResponse>(`/recordings/${id}/translate`, {
      method: 'POST',
      body: JSON.stringify({ lang }),
    });
  },

  // -------------------------------------------------------------------------
  // Processing schedule
  // -------------------------------------------------------------------------

  /**
   * GET /processing-schedule — the weekly windows, what they evaluate to on the
   * server right now, and the queue backlog waiting on them.
   */
  getProcessingSchedule(): Promise<ProcessingScheduleResponse> {
    return apiFetch<ProcessingScheduleResponse>('/processing-schedule');
  },

  /**
   * PUT /processing-schedule — replace the weekly windows.
   *
   * `days` must be exactly 7 entries, Monday first. A live override is NOT sent
   * and NOT cleared: the server carries it across the write, so saving a window
   * edit cannot cancel a pause set moments earlier.
   */
  setProcessingSchedule(
    schedule: Pick<ProcessingSchedule, 'enabled' | 'days' | 'grace_minutes'>,
  ): Promise<ProcessingScheduleResponse> {
    return apiFetch<ProcessingScheduleResponse>('/processing-schedule', {
      method: 'PUT',
      body: JSON.stringify(schedule),
    });
  },

  /**
   * POST /processing-schedule/override — bend the schedule for a while.
   *
   * `run` drains the backlog regardless of the windows, `pause` stops heavy
   * processing even inside one, `clear` drops whichever is in force. Both timed
   * modes default to 60 minutes and are capped at 24 hours by the server.
   */
  setProcessingOverride(req: OverrideRequest): Promise<ProcessingScheduleResponse> {
    return apiFetch<ProcessingScheduleResponse>('/processing-schedule/override', {
      method: 'POST',
      body: JSON.stringify(req),
    });
  },

  // -------------------------------------------------------------------------
  // Search
  // -------------------------------------------------------------------------

  search(params: {
    q: string;
    from?: string;
    to?: string;
    speaker?: string;
    recording?: string;
    limit?: number;
  }): Promise<SearchResponse> {
    const qs = new URLSearchParams({ q: params.q });
    if (params.from) qs.set('from', params.from);
    if (params.to) qs.set('to', params.to);
    if (params.speaker) qs.set('speaker', params.speaker);
    if (params.recording) qs.set('recording', params.recording);
    if (params.limit != null) qs.set('limit', String(params.limit));
    return apiFetch<SearchResponse>(`/search?${qs}`);
  },

  // -------------------------------------------------------------------------
  // Ask (RAG)
  // -------------------------------------------------------------------------

  ask(req: AskRequest): Promise<AskResponse> {
    return apiFetch<AskResponse>('/ask', {
      method: 'POST',
      body: JSON.stringify(req),
    });
  },

  // -------------------------------------------------------------------------
  // Audio playback URL helpers
  // -------------------------------------------------------------------------

  /**
   * Returns the full URL for streaming audio at a specific moment.
   * The caller sets the `Range` header for HTTP byte-range seeking.
   */
  audioUrl(recordingId: string): string {
    const { baseUrl } = useSettingsStore.getState();
    return url(baseUrl, `/recordings/${recordingId}/audio`);
  },

  segmentAudioUrl(recordingId: string, seq: number): string {
    const { baseUrl } = useSettingsStore.getState();
    return url(baseUrl, `/recordings/${recordingId}/segments/${seq}`);
  },

  /** Authorization header value for use with the audio player. */
  authHeader(): string {
    const { deviceKey } = useSettingsStore.getState();
    return `Bearer ${deviceKey}`;
  },

  // -------------------------------------------------------------------------
  // Admin / update endpoints  (Authorization: Bearer <updateToken>)
  // -------------------------------------------------------------------------

  /**
   * GET /admin/info — returns the current backend version and update metadata.
   * Uses the update token, NOT the device key.
   */
  getUpdateInfo(): Promise<UpdateInfoResponse> {
    const { baseUrl, updateToken } = useSettingsStore.getState();
    if (!baseUrl) throw new Error('Scribe server URL is not configured. Go to Settings.');
    if (!updateToken) throw new Error('Update token is not configured. Go to Settings.');
    return fetch(url(baseUrl, '/admin/info'), {
      headers: { Authorization: `Bearer ${updateToken}` },
    }).then(async (r) => {
      if (!r.ok) {
        const text = await r.text().catch(() => '');
        throw new ApiError(r.status, text || r.statusText, '/admin/info');
      }
      return r.json() as Promise<UpdateInfoResponse>;
    });
  },

  /**
   * POST /admin/update — uploads a .tar.gz package as raw bytes and triggers
   * an in-place backend update + restart.
   *
   * Uses expo-file-system uploadAsync for native binary upload with progress
   * callbacks (BINARY_CONTENT method).  Falls back to a plain fetch if the
   * file cannot be read as a blob.
   *
   * @param fileUri  - local file:// URI from expo-document-picker
   * @param onProgress - optional callback receiving 0..1 upload fraction
   */
  async uploadUpdatePackage(
    fileUri: string,
    onProgress?: (fraction: number) => void,
  ): Promise<UpdateResponse> {
    const { baseUrl, updateToken } = useSettingsStore.getState();
    if (!baseUrl) throw new Error('Scribe server URL is not configured. Go to Settings.');
    if (!updateToken) throw new Error('Update token is not configured. Go to Settings.');

    const endpoint = url(baseUrl, '/admin/update');

    // expo-file-system uploadAsync gives us native progress + background
    // capability.  It POSTs raw binary when httpMethod is POST and
    // uploadType is BINARY_CONTENT.
    const task = FileSystem.createUploadTask(
      endpoint,
      fileUri,
      {
        httpMethod: 'POST',
        uploadType: FileSystem.FileSystemUploadType.BINARY_CONTENT,
        headers: {
          Authorization: `Bearer ${updateToken}`,
          'Content-Type': 'application/gzip',
        },
      },
      (event) => {
        if (onProgress && event.totalBytesSent > 0 && event.totalBytesExpectedToSend > 0) {
          onProgress(event.totalBytesSent / event.totalBytesExpectedToSend);
        }
      },
    );

    const result = await task.uploadAsync();
    if (!result) throw new Error('Upload returned no response');

    if (result.status < 200 || result.status >= 300) {
      throw new ApiError(result.status, result.body || 'Upload failed', '/admin/update');
    }

    return JSON.parse(result.body) as UpdateResponse;
  },

  /**
   * POST /admin/update/rollback — reverts to the previous backup binary and
   * triggers a restart.  Only available when `has_backup` is true.
   */
  rollbackUpdate(): Promise<RollbackResponse> {
    const { baseUrl, updateToken } = useSettingsStore.getState();
    if (!baseUrl) throw new Error('Scribe server URL is not configured. Go to Settings.');
    if (!updateToken) throw new Error('Update token is not configured. Go to Settings.');
    return fetch(url(baseUrl, '/admin/update/rollback'), {
      method: 'POST',
      headers: { Authorization: `Bearer ${updateToken}` },
    }).then(async (r) => {
      if (!r.ok) {
        const text = await r.text().catch(() => '');
        throw new ApiError(r.status, text || r.statusText, '/admin/update/rollback');
      }
      return r.json() as Promise<RollbackResponse>;
    });
  },

  /**
   * Polls GET /health until the server responds with status 200 or the
   * timeout elapses.  Use this after an update/rollback to detect when
   * the backend has restarted into the new binary.
   *
   * Resolves with the final HealthResponse, or rejects with an error if
   * the timeout expires.
   */
  async waitForHealthy(timeoutMs: number = 60_000): Promise<HealthResponse> {
    const { baseUrl } = useSettingsStore.getState();
    if (!baseUrl) throw new Error('Scribe server URL is not configured. Go to Settings.');

    const deadline = Date.now() + timeoutMs;
    const POLL_INTERVAL_MS = 2_000;

    while (Date.now() < deadline) {
      try {
        const r = await fetch(url(baseUrl, '/health'), { signal: AbortSignal.timeout(4_000) });
        if (r.ok) {
          return r.json() as Promise<HealthResponse>;
        }
      } catch {
        // backend is still down — keep polling
      }
      // Wait before next poll (skip if we've already hit the deadline)
      const remaining = deadline - Date.now();
      if (remaining <= 0) break;
      await new Promise<void>((resolve) =>
        setTimeout(resolve, Math.min(POLL_INTERVAL_MS, remaining)),
      );
    }

    throw new Error(`Backend did not become healthy within ${timeoutMs / 1000}s`);
  },
};
