/**
 * Recording session — top-level orchestrator that ties together:
 *   - SegmentedRecorder (captures audio in fixed segments)
 *   - UploadQueue (uploads each closed segment with retry)
 *   - The server API (creates the recording, calls complete)
 *   - The recordings store (surfaces optimistic state to the UI)
 *
 * Usage (from the Record screen):
 *
 *   const session = new RecordingSession();
 *
 *   // Start (must be in the foreground)
 *   await session.start({ title: 'Sprint planning', participantsExpected: 4 });
 *
 *   // Stop (can be called from background via the foreground service)
 *   await session.stop();
 *
 * The session object is meant to be held in React state / ref for the
 * lifetime of a single recording.  Create a new one for the next recording.
 */

import 'react-native-get-random-values'; // for crypto.randomUUID()
import { SegmentedRecorder, SegmentInfo } from './segmentedRecorder';
import { uploadQueue } from './uploadQueue';
import { api } from '../api/client';
import { useRecordingsStore } from '../state/recordingsStore';
import { useSettingsStore } from '../state/settingsStore';
import type { Recording } from '../types';

// ---------------------------------------------------------------------------

export interface StartSessionOptions {
  title?: string;
  participantsExpected?: number;
}

export type SessionState =
  | 'idle'
  | 'starting'
  | 'recording'
  | 'stopping'
  | 'done'
  | 'error';

export type SessionEventListener = (event: SessionEvent) => void;

export type SessionEvent =
  | { type: 'state'; state: SessionState }
  | { type: 'tick'; elapsedMs: number }
  | { type: 'segment'; seq: number; uploaded: boolean }
  | { type: 'error'; error: Error };

// ---------------------------------------------------------------------------

export class RecordingSession {
  private _state: SessionState = 'idle';
  private _recordingId: string | null = null;
  private _startTimeMs = 0;
  private _segmentCount = 0;
  private _listeners: SessionEventListener[] = [];
  private _tickTimer: ReturnType<typeof setInterval> | null = null;

  private recorder: SegmentedRecorder = new SegmentedRecorder({
    onSegmentReady: (seg) => this._handleSegmentReady(seg),
    onError: (err) => this._handleError(err),
    segmentDurationSeconds: 30,
  });

  // -------------------------------------------------------------------------
  // Public
  // -------------------------------------------------------------------------

  get state(): SessionState {
    return this._state;
  }

  get recordingId(): string | null {
    return this._recordingId;
  }

  get elapsedMs(): number {
    return this.recorder.elapsedMs;
  }

  get segmentCount(): number {
    return this._segmentCount;
  }

  on(listener: SessionEventListener): () => void {
    this._listeners.push(listener);
    return () => {
      this._listeners = this._listeners.filter((l) => l !== listener);
    };
  }

  /** Start capturing. Must be called while the app is in the foreground. */
  async start(options: StartSessionOptions = {}): Promise<void> {
    if (this._state !== 'idle') {
      throw new Error(`RecordingSession: cannot start from state "${this._state}"`);
    }

    this._setState('starting');

    try {
      const { deviceId } = useSettingsStore.getState();

      // Create the recording on the server
      const created = await api.createRecording({
        title: options.title,
        participants_expected: options.participantsExpected,
        device_id: deviceId || undefined,
        audio_format: 'aac',
        sample_rate: 16000,
      });

      this._recordingId = created.id;

      // Optimistic local state so Library shows the new recording immediately
      const optimisticRecording: Recording = {
        id: created.id,
        title: options.title ?? null,
        created_at: new Date().toISOString(),
        device_id: deviceId || null,
        duration_ms: null,
        status: 'uploading',
        participants_expected: options.participantsExpected ?? null,
        audio_format: 'aac',
        sample_rate: 16000,
        storage_key: null,
      };
      useRecordingsStore.getState().upsertRecording(optimisticRecording);

      this._startTimeMs = Date.now();
      await this.recorder.start({
        recordingId: created.id,
        startTimeMs: this._startTimeMs,
      });

      this._setState('recording');
      this._startTick();
    } catch (err) {
      this._setState('error');
      const error = err instanceof Error ? err : new Error(String(err));
      this._emit({ type: 'error', error });
      throw error;
    }
  }

  /** Stop capturing and finalise the recording on the server. */
  async stop(): Promise<void> {
    if (this._state !== 'recording') return;

    this._setState('stopping');
    this._stopTick();

    const durationMs = Date.now() - this._startTimeMs;

    try {
      await this.recorder.stop();
    } catch {
      // Best effort: continue to complete even if stop had an issue
    }

    if (this._recordingId) {
      try {
        await api.completeRecording(this._recordingId, { duration_ms: durationMs });
        // Update local state to processing
        const cached = useRecordingsStore.getState().getRecording(this._recordingId);
        if (cached) {
          useRecordingsStore.getState().upsertRecording({
            ...cached,
            status: 'processing',
            duration_ms: durationMs,
          });
        }
      } catch (err) {
        // Complete call failed — the recording is on the server but not marked
        // complete.  The user can retry; don't block the UI.
        console.warn('[RecordingSession] /complete failed:', err);
      }
    }

    this._setState('done');
  }

  // -------------------------------------------------------------------------
  // Internal
  // -------------------------------------------------------------------------

  private _handleSegmentReady(seg: SegmentInfo): void {
    this._segmentCount += 1;
    this._emit({ type: 'segment', seq: seg.seq, uploaded: false });

    if (!this._recordingId) return;

    uploadQueue
      .enqueue({
        id: crypto.randomUUID(),
        recordingId: this._recordingId!,
        seq: seg.seq,
        fileUri: seg.fileUri,
        startMs: seg.startMs,
        durationMs: seg.durationMs,
      })
      .then(() => {
        this._emit({ type: 'segment', seq: seg.seq, uploaded: true });
      })
      .catch((err) => {
        console.warn('[RecordingSession] enqueue failed:', err);
      });
  }

  private _handleError(error: Error): void {
    this._setState('error');
    this._stopTick();
    this._emit({ type: 'error', error });
  }

  private _setState(state: SessionState): void {
    this._state = state;
    this._emit({ type: 'state', state });
  }

  private _emit(event: SessionEvent): void {
    for (const l of this._listeners) {
      try {
        l(event);
      } catch {
        // ignore listener errors
      }
    }
  }

  private _startTick(): void {
    this._tickTimer = setInterval(() => {
      this._emit({ type: 'tick', elapsedMs: this.recorder.elapsedMs });
    }, 500);
  }

  private _stopTick(): void {
    if (this._tickTimer != null) {
      clearInterval(this._tickTimer);
      this._tickTimer = null;
    }
  }
}
