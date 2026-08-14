/**
 * Transcript / summary export (Phase 0.1 of the parity roadmap).
 *
 * Pure formatters that turn a RecordingDetailResponse into shareable text in a
 * few common formats. There is no I/O here — the caller hands the result to the
 * OS share sheet (React Native `Share`), which works on the current dev client
 * with no extra native module.
 *
 * Follow-ups (see docs/parity-roadmap.md): true file export (write a real .md /
 * .srt file and share it via expo-sharing) and server-side DOCX/PDF + bulk
 * export. Those need a dev-client rebuild / backend work; this slice does not.
 */

import type { RecordingDetailResponse, RecordingSpeaker, Utterance } from '../types';

export type ExportFormat = 'markdown' | 'text' | 'srt';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Resolve a display name for an utterance: explicit name → enrolled → fallback. */
function speakerLabel(u: Utterance, speakers: RecordingSpeaker[]): string {
  if (u.speaker_name) return u.speaker_name;
  if (u.local_idx == null) return 'Unknown';
  const sp = speakers.find((s) => s.local_idx === u.local_idx);
  return sp?.display_name ?? `Speaker ${u.local_idx}`;
}

/** Human-readable mm:ss (or h:mm:ss) for transcript timestamps. */
function clock(ms: number): string {
  const total = Math.floor(Math.max(0, ms) / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** SRT cue timecode: HH:MM:SS,mmm. */
function srtTime(ms: number): string {
  const c = Math.max(0, Math.round(ms));
  const pad = (n: number, w = 2) => String(n).padStart(w, '0');
  return `${pad(Math.floor(c / 3_600_000))}:${pad(Math.floor((c % 3_600_000) / 60_000))}:${pad(
    Math.floor((c % 60_000) / 1000),
  )},${pad(c % 1000, 3)}`;
}

function formatDate(iso: string): string {
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

function exportTitle(d: RecordingDetailResponse): string {
  return d.summary?.title ?? d.recording.title ?? 'Untitled recording';
}

function slug(s: string): string {
  return (
    s
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/(^-|-$)/g, '')
      .slice(0, 60) || 'recording'
  );
}

// ---------------------------------------------------------------------------
// Formatters
// ---------------------------------------------------------------------------

function toMarkdown(d: RecordingDetailResponse): string {
  const lines: string[] = [`# ${exportTitle(d)}`, ''];
  const meta = [formatDate(d.recording.created_at)];
  if (d.recording.duration_ms) meta.push(clock(d.recording.duration_ms));
  lines.push(`_${meta.join(' · ')}_`, '');

  const s = d.summary;
  if (s?.summary) lines.push('## Summary', '', s.summary, '');
  if (s?.action_items?.length) {
    lines.push('## Action items', '');
    for (const a of s.action_items) lines.push(`- [ ] ${a}`);
    lines.push('');
  }
  if (s?.decisions?.length) {
    lines.push('## Decisions', '');
    for (const dec of s.decisions) lines.push(`- ${dec}`);
    lines.push('');
  }
  if (s?.topics?.length) {
    lines.push('## Topics', '', s.topics.map((t) => `\`${t}\``).join(' '), '');
  }

  lines.push('## Transcript', '');
  for (const u of d.utterances) {
    lines.push(`**${speakerLabel(u, d.speakers)}** _(${clock(u.start_ms)})_`);
    lines.push(u.text.trim(), '');
  }
  return `${lines.join('\n').trim()}\n`;
}

function toText(d: RecordingDetailResponse): string {
  const lines: string[] = [exportTitle(d)];
  const meta = [formatDate(d.recording.created_at)];
  if (d.recording.duration_ms) meta.push(clock(d.recording.duration_ms));
  lines.push(meta.join('  ·  '), '');

  const s = d.summary;
  if (s?.summary) lines.push('SUMMARY', s.summary, '');
  if (s?.action_items?.length) {
    lines.push('ACTION ITEMS');
    for (const a of s.action_items) lines.push(`  - ${a}`);
    lines.push('');
  }
  if (s?.decisions?.length) {
    lines.push('DECISIONS');
    for (const dec of s.decisions) lines.push(`  - ${dec}`);
    lines.push('');
  }

  lines.push('TRANSCRIPT', '');
  for (const u of d.utterances) {
    lines.push(`[${clock(u.start_ms)}] ${speakerLabel(u, d.speakers)}: ${u.text.trim()}`);
  }
  return `${lines.join('\n').trim()}\n`;
}

function toSrt(d: RecordingDetailResponse): string {
  const cues: string[] = [];
  d.utterances.forEach((u, i) => {
    // Guard against zero/negative durations so the cue is always visible.
    const end = u.end_ms > u.start_ms ? u.end_ms : u.start_ms + 2000;
    cues.push(
      String(i + 1),
      `${srtTime(u.start_ms)} --> ${srtTime(end)}`,
      `${speakerLabel(u, d.speakers)}: ${u.text.trim()}`,
      '',
    );
  });
  return cues.join('\n');
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/** Build the export payload for a recording in the requested format. */
export function buildExport(
  detail: RecordingDetailResponse,
  format: ExportFormat,
): { content: string; filename: string } {
  const base = slug(exportTitle(detail));
  switch (format) {
    case 'markdown':
      return { content: toMarkdown(detail), filename: `${base}.md` };
    case 'srt':
      return { content: toSrt(detail), filename: `${base}.srt` };
    case 'text':
    default:
      return { content: toText(detail), filename: `${base}.txt` };
  }
}
