/**
 * AVAudioSession interruption reporting.
 *
 * An interruption (phone call, Siri, another app taking audio focus) stops the
 * recorder at the OS level. Nothing tells JS, so without this the app happily
 * reports "recording" while capturing nothing — which is how a meeting goes
 * missing. Subscribe, and restart capture when the interruption ends.
 */

import { requireOptionalNativeModule } from 'expo';
import { EventSubscription } from 'expo-modules-core';

export interface InterruptionEvent {
  type: 'began' | 'ended';
  /** iOS's hint that it expects the app to start audio again. */
  shouldResume: boolean;
  reason: 'interruption' | 'mediaServicesReset';
}

interface ScribeAudioSessionNativeModule {
  addListener(
    event: 'onInterruption',
    listener: (payload: InterruptionEvent) => void,
  ): EventSubscription;
}

const native = requireOptionalNativeModule<ScribeAudioSessionNativeModule>('ScribeAudioSession');

/** Subscribe to interruptions. Returns an unsubscribe function. */
export function onAudioInterruption(
  handler: (event: InterruptionEvent) => void,
): () => void {
  if (!native) return () => {};
  const sub = native.addListener('onInterruption', handler);
  return () => sub.remove();
}
