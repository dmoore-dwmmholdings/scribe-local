/**
 * Upload queue — offline-first, per-segment retry with exponential backoff.
 *
 * Design requirements (§11, §16):
 *   - Keep recording with no connectivity; upload segments when back on the
 *     tailnet.
 *   - Each closed segment is queued and retried independently.
 *   - Upload progress is surfaced to the UI via the recordings store.
 *
 * Implementation:
 *   - Pending segments are persisted to AsyncStorage so they survive app
 *     restarts.
 *   - A single concurrent uploader loop runs while the queue has items,
 *     pausing on network errors and retrying with exponential backoff.
 *   - On success, the segment's local file is deleted to free cache space.
 *
 * v1 upload protocol: direct PUT per segment (simple, reliable).
 * Production upgrade path: swap uploadSegment() to tus-js-client v4 PATCH
 * sequence against a rustus sidecar at /files.  The queue structure is
 * protocol-agnostic; only the api.uploadSegment() call changes (design §11).
 */

import AsyncStorage from '@react-native-async-storage/async-storage';
import * as FileSystem from 'expo-file-system';
import { api } from '../api/client';
import { useRecordingsStore } from '../state/recordingsStore';
import type { PendingSegment } from '../types';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const QUEUE_STORAGE_KEY = 'scribe_upload_queue';

/** Maximum upload attempts before a segment is marked failed. */
const MAX_ATTEMPTS = 8;

/** Base delay for exponential backoff (ms). Doubles each attempt. */
const BACKOFF_BASE_MS = 2_000;

/** Cap on backoff delay (ms). */
const BACKOFF_CAP_MS = 5 * 60_000; // 5 minutes

// ---------------------------------------------------------------------------
// UploadQueue class
// ---------------------------------------------------------------------------

export class UploadQueue {
  private queue: PendingSegment[] = [];
  private uploading = false;
  private stopRequested = false;

  // -------------------------------------------------------------------------
  // Lifecycle
  // -------------------------------------------------------------------------

  /** Restore persisted queue from AsyncStorage. Call once on app start. */
  async hydrate(): Promise<void> {
    try {
      const stored = await AsyncStorage.getItem(QUEUE_STORAGE_KEY);
      if (stored) {
        const parsed: PendingSegment[] = JSON.parse(stored);
        // Reset any "uploading" items back to "pending" (interrupted mid-upload)
        this.queue = parsed.map((s) =>
          s.status === 'uploading' ? { ...s, status: 'pending' } : s,
        );
        await this._persist();
        this._drainLoop();
      }
    } catch {
      // Non-fatal: start with an empty queue
    }
  }

  /** Add a segment to the queue and trigger upload. */
  async enqueue(segment: Omit<PendingSegment, 'status' | 'attempts' | 'lastAttemptAt'>): Promise<void> {
    const item: PendingSegment = {
      ...segment,
      status: 'pending',
      attempts: 0,
      lastAttemptAt: null,
    };
    this.queue.push(item);
    await this._persist();
    this._updateProgress(segment.recordingId);
    this._drainLoop();
  }

  /** Stop the drain loop (e.g. on sign-out). */
  stop(): void {
    this.stopRequested = true;
  }

  /** Resume after stop(). */
  resume(): void {
    this.stopRequested = false;
    this._drainLoop();
  }

  /** All pending+uploading+failed segments for a given recording. */
  pendingFor(recordingId: string): PendingSegment[] {
    return this.queue.filter((s) => s.recordingId === recordingId);
  }

  // -------------------------------------------------------------------------
  // Drain loop
  // -------------------------------------------------------------------------

  private _drainLoop(): void {
    if (this.uploading || this.stopRequested) return;
    const pending = this.queue.filter((s) => s.status === 'pending');
    if (pending.length === 0) return;

    this.uploading = true;
    this._processNext().finally(() => {
      this.uploading = false;
      // Re-enter the loop if there are more items
      const remaining = this.queue.filter((s) => s.status === 'pending');
      if (remaining.length > 0 && !this.stopRequested) {
        this._drainLoop();
      }
    });
  }

  private async _processNext(): Promise<void> {
    const item = this.queue.find((s) => s.status === 'pending');
    if (!item) return;

    item.status = 'uploading';
    item.attempts += 1;
    item.lastAttemptAt = Date.now();
    await this._persist();

    try {
      await api.uploadSegment({
        recordingId: item.recordingId,
        seq: item.seq,
        fileUri: item.fileUri,
        startMs: item.startMs,
        durationMs: item.durationMs,
      });

      // Success — remove from queue and clean up the temp file
      this.queue = this.queue.filter((s) => s.id !== item.id);
      await this._persist();
      this._updateProgress(item.recordingId);

      // Best-effort delete the cached segment file
      FileSystem.deleteAsync(item.fileUri, { idempotent: true }).catch(() => {});
    } catch (err) {
      if (item.attempts >= MAX_ATTEMPTS) {
        item.status = 'failed';
        console.warn(
          `[UploadQueue] segment ${item.recordingId}#${item.seq} ` +
            `failed after ${MAX_ATTEMPTS} attempts:`,
          err,
        );
      } else {
        // Back off and retry
        const delay = Math.min(
          BACKOFF_BASE_MS * Math.pow(2, item.attempts - 1),
          BACKOFF_CAP_MS,
        );
        item.status = 'pending';
        await this._persist();

        await sleep(delay);
        // Re-trigger drain loop for this item
        this._drainLoop();
      }
    }
  }

  // -------------------------------------------------------------------------
  // Progress
  // -------------------------------------------------------------------------

  private _updateProgress(recordingId: string): void {
    const segments = this.queue.filter((s) => s.recordingId === recordingId);
    if (segments.length === 0) {
      useRecordingsStore.getState().setUploadProgress(recordingId, 1);
      return;
    }
    const done = segments.filter((s) => s.status === 'done').length;
    useRecordingsStore
      .getState()
      .setUploadProgress(recordingId, done / segments.length);
  }

  // -------------------------------------------------------------------------
  // Persistence
  // -------------------------------------------------------------------------

  private async _persist(): Promise<void> {
    try {
      await AsyncStorage.setItem(QUEUE_STORAGE_KEY, JSON.stringify(this.queue));
    } catch {
      // Non-fatal
    }
  }
}

// ---------------------------------------------------------------------------
// Singleton instance
// ---------------------------------------------------------------------------

export const uploadQueue = new UploadQueue();

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
