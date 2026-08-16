/**
 * Karaoke highlighting — light the word that is being spoken right now.
 *
 * The server already stores per-word timings (`utterances.words`), so the work
 * here is threefold:
 *
 * 1. **Align text to timings.** `utterance.text` is normally the words joined by
 *    spaces, but the merge stage's LLM correction pass and manual transcript
 *    edits can rewrite it. So we align the displayed tokens against the timed
 *    words tolerantly and interpolate whatever is left over, instead of trusting
 *    a 1:1 index. Recordings whose words all carry zero timings (an old Whisper
 *    chunking bug) fall back to spreading the utterance span over the tokens.
 *
 * 2. **Run a fine clock.** `useAudioPlayerStatus` fires ~2×/s, which is far too
 *    coarse for word highlighting, and re-rendering a whole transcript at word
 *    rate would stutter. We poll `player.currentTime` on a short interval and
 *    extrapolate between samples using the playback rate.
 *
 * 3. **Re-render one row, not the list.** The active word lives in a tiny
 *    external store; each transcript row subscribes to *its own* word index via
 *    `useSyncExternalStore`, so a tick only re-renders the line being spoken.
 */

import { useEffect, useMemo, useRef, useSyncExternalStore } from 'react';
import type { AudioPlayer } from 'expo-audio';
import type { Utterance, Word } from '../types';

/** How often the karaoke clock samples playback position. */
const TICK_MS = 60;

/**
 * Never extrapolate further than this past the last native sample — if playback
 * stalls (buffering, an OS interruption) the highlight stops rather than runs
 * away from the audio.
 */
const MAX_EXTRAPOLATION_MS = 750;

/** How far a word's highlight lingers after its end, to bridge tiny gaps. */
const LINGER_MS = 120;

/** Alignment lookahead: how far ahead in the timed words we hunt for a match. */
const LOOKAHEAD = 4;

// ---------------------------------------------------------------------------
// Text ↔ timing alignment
// ---------------------------------------------------------------------------

/** One display token with the moment it is spoken. */
export interface TimedToken {
  /** The token exactly as it appears in the utterance text. */
  text: string;
  startMs: number;
  endMs: number;
}

/** An utterance reduced to what the clock needs. */
export interface UtteranceTiming {
  id: number;
  startMs: number;
  endMs: number;
  tokens: TimedToken[];
}

/**
 * Compare-key for a token: lowercased, stripped of surrounding punctuation.
 * Falls back to the raw lowercase form for scripts the ASCII class drops
 * entirely, so non-Latin transcripts still align.
 */
function normalize(token: string): string {
  const lower = token.toLowerCase();
  const stripped = lower.replace(/[^a-z0-9']/g, '');
  return stripped || lower;
}

/** Split display text the same way it is rendered: on whitespace. */
function tokenize(text: string): string[] {
  return text.split(/\s+/).filter((t) => t.length > 0);
}

/**
 * Spread `[startMs, endMs)` across `tokens`, weighted by token length — the
 * fallback when there is nothing better to go on. Longer words take longer to
 * say, which makes this a good deal closer than an even split.
 */
function interpolate(tokens: string[], startMs: number, endMs: number): TimedToken[] {
  const span = Math.max(0, endMs - startMs);
  const totalWeight = tokens.reduce((sum, t) => sum + t.length, 0) || tokens.length || 1;
  let cursor = startMs;
  return tokens.map((text) => {
    const share = (text.length || 1) / totalWeight;
    const tokenStart = cursor;
    cursor = tokenStart + span * share;
    return { text, startMs: Math.round(tokenStart), endMs: Math.round(cursor) };
  });
}

/**
 * Build timed tokens for one utterance.
 *
 * Words whose timings are degenerate (`end <= start`) are ignored as anchors:
 * they carry no information and would pin tokens to the wrong instant.
 */
export function buildTokens(utterance: Utterance): TimedToken[] {
  const tokens = tokenize(utterance.text);
  if (tokens.length === 0) return [];

  const timed: Word[] = (utterance.words ?? []).filter((w) => w.end_ms > w.start_ms);
  if (timed.length === 0) {
    // No usable word timings: fall back to the utterance span, when it has one.
    if (utterance.end_ms <= utterance.start_ms) return [];
    return interpolate(tokens, utterance.start_ms, utterance.end_ms);
  }

  // Greedy forward alignment: each display token claims the next matching timed
  // word, skipping over words the correction pass dropped or rephrased.
  const anchors: (Word | null)[] = new Array(tokens.length).fill(null);
  let next = 0;
  for (let i = 0; i < tokens.length && next < timed.length; i++) {
    const key = normalize(tokens[i]);
    const limit = Math.min(timed.length, next + LOOKAHEAD);
    for (let k = next; k < limit; k++) {
      if (normalize(timed[k].w) === key) {
        anchors[i] = timed[k];
        next = k + 1;
        break;
      }
    }
  }

  // Fill the unanchored runs by interpolating between their neighbours.
  const out: TimedToken[] = new Array(tokens.length);
  let i = 0;
  let prevEnd = Math.min(utterance.start_ms, timed[0].start_ms);
  while (i < tokens.length) {
    const anchor = anchors[i];
    if (anchor) {
      out[i] = { text: tokens[i], startMs: anchor.start_ms, endMs: anchor.end_ms };
      prevEnd = anchor.end_ms;
      i++;
      continue;
    }
    // Run of unanchored tokens [i, j) — bounded by the next anchor, or the end
    // of the utterance when there is none.
    let j = i;
    while (j < tokens.length && !anchors[j]) j++;
    const nextStart = j < tokens.length ? anchors[j]!.start_ms : Math.max(prevEnd, utterance.end_ms);
    const filled = interpolate(tokens.slice(i, j), prevEnd, Math.max(prevEnd, nextStart));
    for (let k = 0; k < filled.length; k++) out[i + k] = filled[k];
    prevEnd = nextStart;
    i = j;
  }
  return out;
}

/** Reduce a transcript to a time-ordered timeline the clock can search. */
export function buildTimeline(utterances: Utterance[]): UtteranceTiming[] {
  return utterances
    .map((u) => ({
      id: u.id,
      startMs: u.start_ms,
      endMs: u.end_ms,
      tokens: buildTokens(u),
    }))
    .sort((a, b) => a.startMs - b.startMs);
}

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

/** Index of the last entry whose `startMs` is at or before `ms` (-1 if none). */
function lastStartedBefore(items: { startMs: number }[], ms: number): number {
  let lo = 0;
  let hi = items.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (items[mid].startMs <= ms) {
      found = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return found;
}

/**
 * Returned to a row that is not the line being spoken. Distinct from -1, which
 * means "this line is being spoken, but between two words" — a row uses the
 * difference to keep its line highlighted through the pauses.
 */
export const NOT_SPEAKING = -2;

/** The token being spoken at `ms`, or -1 between/outside tokens. */
export function activeTokenIndex(tokens: TimedToken[], ms: number): number {
  const idx = lastStartedBefore(tokens, ms);
  if (idx < 0) return -1;
  return ms < tokens[idx].endMs + LINGER_MS ? idx : -1;
}

// ---------------------------------------------------------------------------
// Marks
// ---------------------------------------------------------------------------

/**
 * Tie each mark (a ms offset bookmarked during recording) to the word that was
 * being spoken at that instant, so the transcript can show where it landed.
 *
 * A mark is placed by a person reacting to what they just heard, so it can fall
 * in a silence between two lines. Rather than drop it, we attach it to the
 * closest line — a mark that shows nowhere is worse than one a second early.
 *
 * Returns token indices per utterance id, in ascending order.
 */
export function anchorMarks(
  timeline: UtteranceTiming[],
  marks: number[],
): Map<number, number[]> {
  const out = new Map<number, number[]>();
  if (timeline.length === 0) return out;

  for (const markMs of marks) {
    // The line containing the mark, else the one whose edge is nearest to it.
    let line = timeline[Math.max(0, lastStartedBefore(timeline, markMs))];
    if (markMs > line.endMs) {
      const after = timeline.find((t) => t.startMs > markMs);
      if (after && after.startMs - markMs < markMs - line.endMs) line = after;
    }
    if (line.tokens.length === 0) continue;

    const idx = Math.max(0, lastStartedBefore(line.tokens, markMs));
    const list = out.get(line.id);
    if (list) {
      if (!list.includes(idx)) list.push(idx);
    } else {
      out.set(line.id, [idx]);
    }
  }
  for (const list of out.values()) list.sort((a, b) => a - b);
  return out;
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/**
 * Holds "which word of which line is being spoken" outside React, so a tick
 * re-renders only the rows whose own answer changed.
 */
export class KaraokeStore {
  private utteranceId: number | null = null;
  private tokenIdx = -1;
  private listeners = new Set<() => void>();
  /** Notified when the spoken *line* changes — drives follow-along scrolling. */
  onUtteranceChange: ((utteranceId: number | null) => void) | null = null;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /**
   * Word index within `utteranceId`, [`NOT_SPEAKING`] when playback is
   * elsewhere, or -1 while this line is spoken but between two words.
   */
  wordIndexFor = (utteranceId: number): number =>
    this.utteranceId === utteranceId ? this.tokenIdx : NOT_SPEAKING;

  get activeUtteranceId(): number | null {
    return this.utteranceId;
  }

  set(utteranceId: number | null, tokenIdx: number): void {
    if (this.utteranceId === utteranceId && this.tokenIdx === tokenIdx) return;
    const lineChanged = this.utteranceId !== utteranceId;
    this.utteranceId = utteranceId;
    this.tokenIdx = tokenIdx;
    for (const listener of this.listeners) listener();
    if (lineChanged) this.onUtteranceChange?.(utteranceId);
  }
}

// ---------------------------------------------------------------------------
// Hooks
// ---------------------------------------------------------------------------

/**
 * Drive a [`KaraokeStore`] from an audio player.
 *
 * `statusPositionMs` / `statusUpdatedAt` come from `useAudioPlayerStatus`; they
 * only re-seed the clock, which then runs at `TICK_MS` off `player.currentTime`.
 * Returns the store to hand to the transcript rows.
 */
export function useKaraoke(params: {
  player: AudioPlayer;
  utterances: Utterance[];
  playing: boolean;
  positionMs: number;
  rate: number;
  enabled: boolean;
}): {
  store: KaraokeStore;
  tokensById: Map<number, TimedToken[]>;
  timeline: UtteranceTiming[];
} {
  const { player, utterances, playing, positionMs, rate, enabled } = params;
  const storeRef = useRef<KaraokeStore | null>(null);
  if (!storeRef.current) storeRef.current = new KaraokeStore();
  const store = storeRef.current;

  const timeline = useMemo(() => buildTimeline(utterances), [utterances]);
  const tokensById = useMemo(
    () => new Map(timeline.map((t) => [t.id, t.tokens])),
    [timeline],
  );

  // Apply a position to the store: find the line, then the word inside it.
  // Held in a ref so the running clock always sees the current transcript
  // without having to be torn down and restarted.
  const applyRef = useRef<(ms: number) => void>(() => {});
  useEffect(() => {
    applyRef.current = (ms: number) => {
      const idx = lastStartedBefore(timeline, ms);
      if (idx < 0) {
        store.set(null, -1);
        return;
      }
      const line = timeline[idx];
      if (ms >= line.endMs + LINGER_MS) {
        store.set(null, -1);
        return;
      }
      store.set(line.id, activeTokenIndex(line.tokens, ms));
    };
  }, [timeline, store]);

  // Coarse sync: the status hook fires on load, seek and pause. While playing
  // the fine clock below owns the position, so we only apply this when it isn't.
  useEffect(() => {
    if (!enabled) {
      store.set(null, -1);
      return;
    }
    if (!playing) applyRef.current(positionMs);
  }, [enabled, playing, positionMs, store]);

  // The fine clock. Runs only while audio is actually playing.
  useEffect(() => {
    if (!enabled || !playing) return;

    // `player` is a native shared object. Reading it after expo-audio has
    // released it (a reload, a source swap, the OS reclaiming the session)
    // throws from the JSI getter — and a throw inside a bare interval callback
    // is an unhandled error, which is a crash in a release build. Reading is
    // never worth that: if it fails, stop the clock and leave the last
    // highlight where it is.
    const readNativeMs = (): number | null => {
      try {
        const t = player.currentTime;
        return typeof t === 'number' && Number.isFinite(t) ? Math.round(t * 1000) : null;
      } catch {
        return null;
      }
    };

    const initial = readNativeMs();
    if (initial == null) return;
    let sampleMs = initial;
    let sampleAt = Date.now();

    const id = setInterval(() => {
      const nativeMs = readNativeMs();
      if (nativeMs == null) {
        clearInterval(id);
        return;
      }
      if (nativeMs !== sampleMs) {
        sampleMs = nativeMs;
        sampleAt = Date.now();
      }
      // Between native samples, carry the clock forward ourselves.
      const drift = Math.min(MAX_EXTRAPOLATION_MS, Date.now() - sampleAt) * rate;
      applyRef.current(sampleMs + drift);
    }, TICK_MS);
    return () => clearInterval(id);
  }, [enabled, playing, rate, player]);

  return { store, tokensById, timeline };
}

/**
 * Subscribe one transcript row to its own active-word index. Returns -1 unless
 * this line is the one being spoken, so unrelated rows never re-render.
 */
export function useActiveWordIndex(store: KaraokeStore, utteranceId: number): number {
  return useSyncExternalStore(
    store.subscribe,
    () => store.wordIndexFor(utteranceId),
    () => NOT_SPEAKING,
  );
}
