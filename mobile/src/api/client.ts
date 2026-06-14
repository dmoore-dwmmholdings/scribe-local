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

import type {
  AskRequest,
  AskResponse,
  CompleteRecordingRequest,
  CompleteRecordingResponse,
  CreateRecordingRequest,
  CreateRecordingResponse,
  HealthResponse,
  ListRecordingsResponse,
  NameSpeakerRequest,
  NameSpeakerResponse,
  RecordingDetailResponse,
  SearchResponse,
  UploadSegmentResponse,
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

  // -------------------------------------------------------------------------
  // Speakers
  // -------------------------------------------------------------------------

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
};
