/**
 * Background-safe repeating timer.
 *
 * React Native's `setTimeout` is driven by a CADisplayLink, which halts when
 * the screen turns off — so anything scheduled with it stalls while the phone
 * is locked. This module is backed by a `DispatchSourceTimer`, which keeps
 * firing as long as the process is alive (which, during a recording, it is:
 * `UIBackgroundModes: audio` keeps the app running rather than suspended).
 *
 * iOS only. On other platforms `isAvailable` is false and the helpers fall
 * back to `setInterval`, which is correct there (Android keeps timers running
 * under a foreground service).
 */

import { requireOptionalNativeModule } from 'expo';
import { EventSubscription } from 'expo-modules-core';

export interface TickEvent {
  /** The timer id passed to `start`. */
  id: string;
  /** 1-based tick counter for this timer. */
  count: number;
}

interface ScribeBgTimerNativeModule {
  start(id: string, intervalMs: number): void;
  stop(id: string): void;
  stopAll(): void;
  addListener(event: 'onTick', listener: (payload: TickEvent) => void): EventSubscription;
}

const native = requireOptionalNativeModule<ScribeBgTimerNativeModule>('ScribeBgTimer');

/** Whether the native background-safe timer is available on this platform. */
export const isAvailable = native != null;

/**
 * Start a repeating timer that survives the screen locking.
 *
 * Returns a stop function. Starting an id that is already running replaces it,
 * so a missed `stop` cannot leave two timers racing.
 */
export function startBackgroundInterval(
  id: string,
  intervalMs: number,
  onTick: () => void,
): () => void {
  if (!native) {
    // Non-iOS fallback. setInterval is fine where the platform keeps timers
    // alive in the background (Android foreground service).
    const handle = setInterval(onTick, intervalMs);
    return () => clearInterval(handle);
  }

  const subscription = native.addListener('onTick', (payload) => {
    if (payload.id === id) onTick();
  });
  native.start(id, intervalMs);

  let stopped = false;
  return () => {
    if (stopped) return;
    stopped = true;
    native.stop(id);
    subscription.remove();
  };
}

/** Cancel every native timer. Used on teardown. */
export function stopAllBackgroundIntervals(): void {
  native?.stopAll();
}
