/**
 * Crash-surviving debug logger.
 *
 * Every log line is (a) printed to the console — so it appears in Metro and in
 * `adb logcat` under the ReactNativeJS tag, interleaved with any native crash —
 * and (b) appended to an on-disk ring buffer that survives a hard crash, so the
 * last breadcrumbs before the app died can be read back in-app (Settings →
 * Diagnostics) or pulled off the device.
 *
 * It also installs a global JS error handler so uncaught JS errors and unhandled
 * rejections are recorded with their stack before the process goes down.
 *
 * Native crashes (Skia / audio codec / Reanimated) can't be caught in JS — for
 * those the `adb logcat` line naming the faulting library is the source of
 * truth; the breadcrumbs here tell you what the app was doing at that instant.
 */

import * as FileSystem from 'expo-file-system/legacy';

const LOG_FILE = `${FileSystem.documentDirectory ?? ''}scribe-debug.log`;
const PREV_FILE = `${FileSystem.documentDirectory ?? ''}scribe-debug.prev.log`;
/**
 * How many prior sessions to keep, beyond PREV_FILE.
 *
 * One generation is not enough. After a crash the user typically relaunches
 * (rotation 1: the crash log lands in PREV_FILE) and then force-quits or
 * reboots and opens the app again (rotation 2: a trivial session overwrites
 * PREV_FILE). The crash breadcrumbs are gone before anyone can read them —
 * which is exactly what happened to the 2026-08-17 crash. Keeping a few
 * generations makes the evidence survive normal panicky user behaviour.
 */
const ARCHIVE_GENERATIONS = 5;
const archivePath = (n: number) =>
  `${FileSystem.documentDirectory ?? ''}scribe-debug.${n}.log`;
const MAX_LINES = 1000;

let buffer: string[] = [];
let flushing = false;
let dirty = false;
// Run-once-per-launch log rotation. The previous session's file is copied to
// PREV_FILE before this session first overwrites LOG_FILE — so the breadcrumbs
// from before a hard native crash survive the relaunch. (Previously the buffer
// reset + file overwrite on startup destroyed them, which is why the log
// "reset" on every launch.)
let rotatePromise: Promise<void> | null = null;

function stamp(): string {
  // Local wall-clock; fine in app runtime (not a workflow sandbox).
  const d = new Date();
  const p = (n: number, w = 2) => String(n).padStart(w, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

function safe(data: unknown): string {
  if (data == null) return '';
  if (typeof data === 'string') return data;
  if (data instanceof Error) return `${data.message}\n${data.stack ?? ''}`;
  try {
    return JSON.stringify(data);
  } catch {
    return String(data);
  }
}

/**
 * Copy the prior session's log to PREV_FILE exactly once per launch, BEFORE we
 * first overwrite LOG_FILE. Memoized, so concurrent flushes share one rotation.
 */
function ensureRotated(): Promise<void> {
  if (!rotatePromise) {
    rotatePromise = (async () => {
      try {
        const info = await FileSystem.getInfoAsync(LOG_FILE);
        if (!info.exists) return;
        const prev = await FileSystem.readAsStringAsync(LOG_FILE);
        if (!prev.trim()) return;

        // Shift the archive down (…3 -> 4, 2 -> 3, 1 -> 2) so the oldest falls
        // off the end rather than the newest overwriting the only slot.
        for (let n = ARCHIVE_GENERATIONS - 1; n >= 1; n--) {
          try {
            const from = archivePath(n);
            if ((await FileSystem.getInfoAsync(from)).exists) {
              await FileSystem.copyAsync({ from, to: archivePath(n + 1) });
            }
          } catch {
            // a gap in the archive is not worth failing the rotation over
          }
        }
        await FileSystem.writeAsStringAsync(archivePath(1), prev);
        // Keep PREV_FILE too: it is what the in-app Diagnostics view reads.
        await FileSystem.writeAsStringAsync(PREV_FILE, prev);
      } catch {
        // best-effort: a missing/unreadable prior log just means no carry-over
      }
    })();
  }
  return rotatePromise;
}

async function flush(): Promise<void> {
  if (flushing) {
    dirty = true;
    return;
  }
  flushing = true;
  try {
    await ensureRotated();
    await FileSystem.writeAsStringAsync(LOG_FILE, buffer.join('\n'));
  } catch {
    // best-effort
  } finally {
    flushing = false;
    if (dirty) {
      dirty = false;
      void flush();
    }
  }
}

/** Record a breadcrumb. `tag` groups by subsystem (e.g. "rec", "audio", "orb"). */
export function log(tag: string, msg: string, data?: unknown): void {
  const extra = data === undefined ? '' : ` ${safe(data)}`;
  const line = `${stamp()} [${tag}] ${msg}${extra}`;
  buffer.push(line);
  if (buffer.length > MAX_LINES) buffer = buffer.slice(-MAX_LINES);
  // eslint-disable-next-line no-console
  console.log(`[scribe] ${line}`);
  void flush();
}

/** Read the full on-disk log (returns the live buffer if the file is empty). */
export async function readLog(): Promise<string> {
  const parts: string[] = [];
  // Previous session first (the breadcrumbs from before the last crash/restart).
  try {
    const prevInfo = await FileSystem.getInfoAsync(PREV_FILE);
    if (prevInfo.exists) {
      const prev = await FileSystem.readAsStringAsync(PREV_FILE);
      if (prev.trim()) {
        parts.push('===== previous session (before last crash / restart) =====');
        parts.push(prev.trim());
        parts.push('===== current session =====');
      }
    }
  } catch {
    // ignore — previous-session carry-over is best-effort
  }
  // Current session.
  try {
    const info = await FileSystem.getInfoAsync(LOG_FILE);
    if (info.exists) {
      const text = await FileSystem.readAsStringAsync(LOG_FILE);
      if (text.trim()) {
        parts.push(text.trim());
        return parts.join('\n');
      }
    }
  } catch {
    // fall through to in-memory buffer
  }
  parts.push(buffer.join('\n') || '(no logs yet)');
  return parts.join('\n');
}

export async function clearLog(): Promise<void> {
  buffer = [];
  try {
    await FileSystem.writeAsStringAsync(LOG_FILE, '');
    await FileSystem.writeAsStringAsync(PREV_FILE, '');
  } catch {
    // best-effort
  }
}

export function logFilePath(): string {
  return LOG_FILE;
}

/** Install the global JS error / rejection handlers (idempotent). */
export function installCrashLogging(): void {
  const g = global as unknown as {
    __scribeCrashLogging?: boolean;
    ErrorUtils?: {
      getGlobalHandler?: () => (e: unknown, fatal?: boolean) => void;
      setGlobalHandler?: (h: (e: unknown, fatal?: boolean) => void) => void;
    };
    HermesInternal?: unknown;
  };
  if (g.__scribeCrashLogging) return;
  g.__scribeCrashLogging = true;

  // Preserve the prior session's log immediately, before any new write lands.
  void ensureRotated();

  const eu = g.ErrorUtils;
  if (eu?.setGlobalHandler) {
    const prev = eu.getGlobalHandler?.();
    eu.setGlobalHandler((error: unknown, isFatal?: boolean) => {
      const err = error instanceof Error ? error : new Error(String(error));
      log('FATAL', `${isFatal ? 'fatal ' : ''}JS error: ${err.message}`, err.stack);
      // Fire-and-forget flush; the prior handler may tear the app down next.
      void flush();
      prev?.(error, isFatal);
    });
  }

  log('app', 'crash logging installed', { hermes: !!g.HermesInternal });
}
