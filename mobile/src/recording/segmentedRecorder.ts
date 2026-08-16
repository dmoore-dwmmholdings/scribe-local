/**
 * Segmented recorder.
 *
 * Records audio in fixed-duration segments (default 30 s) so that:
 *   1. Each closed segment can be uploaded while recording continues —
 *      "by the time the meeting ends, almost everything is already on the
 *      server." (design §6)
 *   2. A crash or interruption loses at most one segment (design §16).
 *   3. The server stitches segments by sequence number; no client-side
 *      concatenation is needed.
 *
 * Audio format: AAC in m4a, 16 kHz mono, ~32 kbps.
 * Hardware-accelerated on both iOS and Android; transcodes cleanly to the
 * 16 kHz mono WAV the ASR models want (design §11 "Recording engine").
 *
 * Background recording:
 *   iOS:     UIBackgroundModes["audio"] is set in app.json → recording
 *            continues when backgrounded / screen-locked.
 *            IMPORTANT: recording MUST be started in the foreground.
 *            AVAudioSession interruptions (calls, Siri) pause the mic;
 *            this class resumes automatically if it was recording when the
 *            interruption began (design §11, §16).
 *
 *   Android: expo-audio's shouldPlayInBackground + allowsRecording options
 *            + the FOREGROUND_SERVICE_MICROPHONE permission +
 *            foreground service type keep recording alive.
 *            A "Recording…" notification is shown by the system (Android 14+).
 *            IMPORTANT: do NOT attempt to start recording from the background.
 *            If expo-audio's Android background path proves unreliable, swap
 *            the recording core to react-native-nitro-sound with a hand-wired
 *            foreground service (design §16 "risks").
 *
 * Usage:
 *   const recorder = new SegmentedRecorder({ onSegmentReady, onError });
 *   await recorder.start({ recordingId, startTimeMs });
 *   // ...
 *   await recorder.stop();
 */

import {
  AudioModule,
  setAudioModeAsync,
  requestRecordingPermissionsAsync,
  type AudioRecorder,
  type RecordingOptions,
} from 'expo-audio';
import * as FileSystem from 'expo-file-system/legacy';
import { log } from '../util/logger';
import { startBackgroundInterval } from '../../modules/scribe-bg-timer';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface SegmentInfo {
  seq: number;
  fileUri: string;
  startMs: number;
  durationMs: number;
}

export interface SegmentedRecorderOptions {
  /** Called whenever a segment file is ready to upload. */
  onSegmentReady: (segment: SegmentInfo) => void;
  /** Called on unrecoverable error. */
  onError: (error: Error) => void;
  /** Segment duration in seconds. Default: 30. Range: 10–60. */
  segmentDurationSeconds?: number;
}

interface StartOptions {
  recordingId: string;
  startTimeMs: number;
}

// ---------------------------------------------------------------------------
// Audio quality preset (AAC, 16 kHz mono, ~32 kbps)
// ---------------------------------------------------------------------------

const RECORDING_OPTIONS: RecordingOptions = {
  extension: '.m4a',
  sampleRate: 16000,
  numberOfChannels: 1,
  bitRate: 32000,
  isMeteringEnabled: false,
  android: {
    outputFormat: 'mpeg4',
    audioEncoder: 'aac',
    sampleRate: 16000,
    maxFileSize: undefined,
  },
  ios: {
    outputFormat: 'aac ',  // IOSOutputFormat.MPEG4AAC string value
    audioQuality: 64,      // AudioQuality.MEDIUM = 64
    sampleRate: 16000,
  },
  web: {
    mimeType: 'audio/webm',
    bitsPerSecond: 32000,
  },
};

// ---------------------------------------------------------------------------
// SegmentedRecorder class
// ---------------------------------------------------------------------------

export class SegmentedRecorder {
  private readonly onSegmentReady: (segment: SegmentInfo) => void;
  private readonly onError: (error: Error) => void;
  private readonly segmentDurationMs: number;

  private activeRecorder: AudioRecorder | null = null;
  /**
   * Cancels the rotation interval. This is a native (DispatchSourceTimer)
   * interval rather than setTimeout: RN's JS timers are CADisplayLink-driven
   * and stop firing when the screen locks, which stalled rotation — and
   * therefore upload and transcription — for as long as the phone was locked.
   */
  private stopRotationTimer: (() => void) | null = null;
  /** Guards against a tick arriving while the previous rotation is in flight. */
  private _rotating = false;
  private seq = 0;
  private _recordingId = '';
  private _startTimeMs = 0;
  private _segmentStartMs = 0;
  private _running = false;
  private _paused = false;
  /** Total time (ms) spent paused, excluded from the audio timeline. */
  private _pausedAccumMs = 0;
  /** Wall-clock when the current pause began (0 when not paused). */
  private _pauseStartMs = 0;

  constructor(options: SegmentedRecorderOptions) {
    this.onSegmentReady = options.onSegmentReady;
    this.onError = options.onError;
    const dur = options.segmentDurationSeconds ?? 30;
    this.segmentDurationMs = Math.min(60, Math.max(10, dur)) * 1000;
  }

  /** Whether a recording session is active (including while paused). */
  get isRecording(): boolean {
    return this._running;
  }

  /** Whether capture is currently paused. */
  get isPaused(): boolean {
    return this._paused;
  }

  /**
   * Total wall-clock ms excluded from the audio timeline because of pauses,
   * including any pause currently in progress. The Live Activity needs this to
   * offset its self-counting timer.
   */
  get pausedMs(): number {
    if (!this._running) return 0;
    return this._pausedAccumMs + (this._paused ? Date.now() - this._pauseStartMs : 0);
  }

  /** Elapsed milliseconds of captured audio (frozen while paused). */
  get elapsedMs(): number {
    if (!this._running) return 0;
    const now = this._paused ? this._pauseStartMs : Date.now();
    return now - this._startTimeMs - this._pausedAccumMs;
  }

  // -------------------------------------------------------------------------
  // Public API
  // -------------------------------------------------------------------------

  /**
   * Request microphone permission and start the first segment.
   * Must be called while the app is in the foreground (design §11, §16).
   */
  async start(options: StartOptions): Promise<void> {
    if (this._running) {
      throw new Error('SegmentedRecorder: already recording');
    }

    // Request microphone permission
    const { status } = await requestRecordingPermissionsAsync();
    if (status !== 'granted') {
      throw new Error('Microphone permission was denied.');
    }

    // Configure the audio session.
    // allowsRecording = true enables recording on iOS (sets AVAudioSession category).
    // shouldPlayInBackground = true keeps the session alive when backgrounded
    // (pairs with UIBackgroundModes: audio on iOS; foreground service on Android).
    await setAudioModeAsync({
      allowsRecording: true,
      playsInSilentMode: true,
      shouldPlayInBackground: true,
    });

    this._recordingId = options.recordingId;
    this._startTimeMs = options.startTimeMs;
    this.seq = 0;
    this._running = true;
    this._paused = false;
    this._pausedAccumMs = 0;
    this._pauseStartMs = 0;

    await this._startSegment();
    this._startRotationTimer();
  }

  /**
   * Pause capture: close + emit the current segment and stop rotating new ones,
   * but keep the session alive.  The audio timeline excludes paused time.
   */
  async pause(): Promise<void> {
    if (!this._running || this._paused) return;
    this._paused = true;
    this._pauseStartMs = Date.now();
    this._clearTimer();

    const segStartMs = this._segmentStartMs;
    const currentSeq = this.seq;
    const fileUri = await this._stopActiveRecorder();
    if (fileUri) {
      const durationMs = Date.now() - segStartMs;
      this.seq += 1;
      this.onSegmentReady({
        seq: currentSeq,
        fileUri,
        startMs: segStartMs - this._startTimeMs - this._pausedAccumMs,
        durationMs,
      });
    }
  }

  /** Resume capture after pause: start a fresh segment. */
  async resume(): Promise<void> {
    if (!this._running || !this._paused) return;
    this._pausedAccumMs += Date.now() - this._pauseStartMs;
    this._paused = false;
    this._pauseStartMs = 0;
    await this._startSegment();
    this._startRotationTimer();
  }

  /** Stop recording. Closes and emits the final (partial) segment. */
  async stop(): Promise<void> {
    if (!this._running) return;
    this._running = false;
    this._paused = false;
    this._clearTimer();
    await this._closeCurrentSegment();

    // Reset audio mode after recording ends
    await setAudioModeAsync({
      allowsRecording: false,
      playsInSilentMode: false,
      shouldPlayInBackground: false,
    }).catch(() => {});
  }

  // -------------------------------------------------------------------------
  // Internal
  // -------------------------------------------------------------------------

  private async _startSegment(): Promise<void> {
    this._segmentStartMs = Date.now();

    let recorder: AudioRecorder | null = null;
    try {
      // AudioRecorder is instantiated with options; prepareToRecordAsync
      // allocates OS resources, then record() starts capture.
      //
      // SDK 54: the `AudioRecorder` class is re-exported type-only from
      // expo-audio; the constructable value lives on `AudioModule.AudioRecorder`
      // (this is exactly what the useAudioRecorder hook calls internally).
      // We can't use the hook here because this is a plain class, not a
      // React component, and we deliberately create one recorder per segment.
      log('audio', `startSegment #${this.seq}: new AudioRecorder`);
      recorder = new AudioModule.AudioRecorder(RECORDING_OPTIONS);
      await recorder.prepareToRecordAsync();
      recorder.record();
      this.activeRecorder = recorder;
      log('audio', `startSegment #${this.seq}: recording`);
    } catch (err) {
      log('audio', 'startSegment FAILED', err);
      // Release the recorder if prepare/record threw after construction —
      // otherwise this half-initialized native object leaks too.
      if (recorder && recorder !== this.activeRecorder) {
        try {
          recorder.release();
        } catch {
          // ignore
        }
      }
      this._running = false;
      this.onError(err instanceof Error ? err : new Error(String(err)));
    }
  }

  /**
   * Close the current segment, emit it, then immediately start the next one.
   * This keeps the mic "warm" — there is minimal gap in capture between
   * segments (determined by OS recording session startup time).
   */
  private async _rotateSegment(): Promise<void> {
    if (!this._running) return;
    this._clearTimer();

    // Snapshot timing before stopping (stop() takes a few ms)
    const segStartMs = this._segmentStartMs;
    const currentSeq = this.seq;

    const fileUri = await this._stopActiveRecorder();
    if (fileUri) {
      const durationMs = Date.now() - segStartMs;
      this.seq += 1;
      this.onSegmentReady({
        seq: currentSeq,
        fileUri,
        startMs: segStartMs - this._startTimeMs - this._pausedAccumMs,
        durationMs,
      });
    }

    // Start the next segment immediately (still running)
    if (this._running && !this._paused) {
      await this._startSegment();
    }
  }

  /** Close the current segment (final stop). */
  private async _closeCurrentSegment(): Promise<void> {
    const segStartMs = this._segmentStartMs;
    const currentSeq = this.seq;

    const fileUri = await this._stopActiveRecorder();
    if (fileUri) {
      const durationMs = Date.now() - segStartMs;
      this.onSegmentReady({
        seq: currentSeq,
        fileUri,
        startMs: segStartMs - this._startTimeMs - this._pausedAccumMs,
        durationMs,
      });
    }
  }

  private async _stopActiveRecorder(): Promise<string | null> {
    const rec = this.activeRecorder;
    this.activeRecorder = null;
    if (!rec) return null;

    let uri: string | null = null;
    try {
      await rec.stop();
      // Read the uri BEFORE releasing — release() tears down the native object.
      // The file itself is already finalized on disk by stop(), so the upload
      // queue can still read it later.
      uri = rec.uri ?? null;
      log('audio', `stopped recorder, uri=${uri ? 'ok' : 'null'}`);
    } catch (err) {
      log('audio', 'recorder.stop FAILED', err);
    } finally {
      // CRITICAL: an AudioRecorder is an expo SharedObject. Instances created
      // imperatively (as we do, one per segment) are NOT auto-released — only
      // the `useAudioRecorder` hook releases on unmount. Without this call,
      // every ~15 s segment leaks a native AVAudioRecorder attached to the
      // shared AVAudioSession, so a long recording exhausts audio/memory
      // resources and iOS terminates the app mid-recording. See expo-audio's
      // SharedObject.release().
      try {
        rec.release();
        log('audio', `released recorder #${this.seq}`);
      } catch (e) {
        log('audio', 'recorder.release FAILED', e);
      }
    }
    return uri;
  }

  /**
   * Start the rotation interval. One interval runs for the whole session
   * (restarted across pause/resume) rather than a fresh timeout per segment,
   * so rotation cadence does not drift with per-segment setup cost.
   */
  private _startRotationTimer(): void {
    this._clearTimer();
    if (!this._running || this._paused) return;

    this.stopRotationTimer = startBackgroundInterval(
      `scribe-segment-${this._recordingId}`,
      this.segmentDurationMs,
      () => {
        // Ticks keep arriving while a rotation is in flight (rotation does
        // async native work); skip rather than queue, or a slow rotation
        // would cascade into overlapping recorders.
        if (!this._running || this._paused || this._rotating) return;
        this._rotating = true;
        this._rotateSegment()
          .catch((err) => this.onError(err instanceof Error ? err : new Error(String(err))))
          .finally(() => {
            this._rotating = false;
          });
      },
    );
  }

  private _clearTimer(): void {
    if (this.stopRotationTimer) {
      this.stopRotationTimer();
      this.stopRotationTimer = null;
    }
  }

  /**
   * Handle an AVAudioSession interruption end (iOS: incoming call ended, etc.).
   * Call this from your interruption handler; the recorder resumes automatically.
   *
   * Design §11: "Handle AVAudioSession interruptions (calls, Siri): on
   * interruption it pauses; resume only if you were recording."
   */
  async handleInterruptionEnd(): Promise<void> {
    if (!this._running || this.activeRecorder?.isRecording) return;
    // Start a fresh segment — the interrupted segment's file was already
    // closed by the OS; it will be emitted when detected.
    await this._startSegment();
  }
}

// ---------------------------------------------------------------------------
// Directory helper — ensure segment temp dir exists
// ---------------------------------------------------------------------------

export async function ensureSegmentDir(): Promise<string> {
  const dir = `${FileSystem.cacheDirectory}scribe-segments/`;
  const info = await FileSystem.getInfoAsync(dir);
  if (!info.exists) {
    await FileSystem.makeDirectoryAsync(dir, { intermediates: true });
  }
  return dir;
}
