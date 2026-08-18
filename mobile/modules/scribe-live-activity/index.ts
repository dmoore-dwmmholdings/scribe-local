/**
 * Recording Live Activity (Lock Screen + Dynamic Island).
 *
 * Every function is a no-op that resolves falsy when Live Activities are
 * unavailable — iOS < 16.2, a non-iOS platform, or the user having turned them
 * off for Scribe. A Live Activity is decoration around recording, so failing to
 * show one must never interfere with capture.
 */

import { requireOptionalNativeModule } from 'expo';

interface ScribeLiveActivityNativeModule {
  isSupported(): boolean;
  start(title: string, startedAtMs: number): Promise<boolean>;
  update(
    startedAtMs: number,
    pausedMs: number,
    isPaused: boolean,
    segmentsUploaded: number,
  ): Promise<boolean>;
  end(): Promise<boolean>;
  endAll(): Promise<number>;
}

const native = requireOptionalNativeModule<ScribeLiveActivityNativeModule>('ScribeLiveActivity');

export interface LiveActivityState {
  /** Epoch ms when capture began. */
  startedAtMs: number;
  /** Wall-clock ms spent paused, excluded from the displayed timer. */
  pausedMs: number;
  isPaused: boolean;
  segmentsUploaded: number;
}

/** Whether a Live Activity can be shown right now. */
export function isLiveActivitySupported(): boolean {
  try {
    return native?.isSupported() ?? false;
  } catch {
    return false;
  }
}

export async function startLiveActivity(title: string, startedAtMs: number): Promise<boolean> {
  if (!native) return false;
  try {
    return await native.start(title, startedAtMs);
  } catch {
    return false;
  }
}

export async function updateLiveActivity(state: LiveActivityState): Promise<boolean> {
  if (!native) return false;
  try {
    return await native.update(
      state.startedAtMs,
      state.pausedMs,
      state.isPaused,
      state.segmentsUploaded,
    );
  } catch {
    return false;
  }
}

/**
 * End every Live Activity belonging to this app, including ones started by a
 * previous process.
 *
 * ActivityKit activities outlive the app. If the app crashes mid-recording the
 * Lock Screen keeps claiming a recording is in progress, and the new process
 * holds no handle to it — so neither relaunching nor rebooting the phone clears
 * it. Call this on startup, where no recording can legitimately be running yet.
 *
 * Returns how many stale activities were cleared.
 */
export async function endAllLiveActivities(): Promise<number> {
  if (!native) return 0;
  try {
    return await native.endAll();
  } catch {
    return 0;
  }
}

export async function endLiveActivity(): Promise<boolean> {
  if (!native) return false;
  try {
    return await native.end();
  } catch {
    return false;
  }
}
