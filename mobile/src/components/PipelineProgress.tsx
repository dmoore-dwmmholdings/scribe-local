/**
 * Pipeline progress for a recording that is still being processed.
 *
 * The server reports a fixed six-stage checklist (transcode → diarize →
 * transcribe → merge → embed → summarize) with a state per stage. Long files
 * can sit in one stage for many minutes, so a single "processing…" line reads
 * as a hang; showing which stage is active, how many are done, and how long the
 * current one has been running makes the wait legible.
 *
 * Older backends don't send `progress`. In that case the caller falls back to
 * the plain status line, so this component is never rendered without data.
 */
import { useEffect, useState } from 'react';
import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';
import { Ionicons } from '@expo/vector-icons';

import { colors, radius } from '../theme';
import type { PipelineProgress as Progress, StageKind, StageState } from '../types';

/** What each stage is actually doing, in the user's terms — not our job names. */
const STAGE_LABELS: Record<StageKind, string> = {
  transcode: 'Preparing audio',
  diarize: 'Identifying speakers',
  transcribe: 'Transcribing speech',
  merge: 'Matching words to speakers',
  embed: 'Building the search index',
  summarize: 'Writing the summary',
};

function formatElapsed(sinceIso: string, now: number): string {
  const secs = Math.max(0, Math.floor((now - new Date(sinceIso).getTime()) / 1000));
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ${secs % 60}s`;
  return `${Math.floor(mins / 60)}h ${mins % 60}m`;
}

function StageIcon({ state }: { state: StageState }) {
  if (state === 'running') return <ActivityIndicator size="small" color={colors.accent} />;
  if (state === 'done') {
    return <Ionicons name="checkmark-circle" size={18} color={colors.statusReady} />;
  }
  if (state === 'failed') {
    return <Ionicons name="alert-circle" size={18} color={colors.accentDeep} />;
  }
  // queued / pending — not started yet.
  return <Ionicons name="ellipse-outline" size={18} color={colors.textDim} />;
}

export function PipelineProgressCard({ progress }: { progress: Progress }) {
  // Re-render once a second so the elapsed timer on the running stage ticks.
  const [now, setNow] = useState(() => Date.now());
  const hasRunning = progress.stages.some((s) => s.state === 'running');
  useEffect(() => {
    if (!hasRunning) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [hasRunning]);

  const failed = progress.stages.find((s) => s.state === 'failed');
  const pct = progress.total > 0 ? Math.round((progress.completed / progress.total) * 100) : 0;

  return (
    <View style={styles.card}>
      <View style={styles.headerRow}>
        <Text style={styles.header}>
          {failed ? 'Processing failed' : 'Processing'}
        </Text>
        <Text style={styles.count}>
          {progress.completed}/{progress.total}
        </Text>
      </View>

      {/* Coarse bar: stage granularity is all the server can tell us honestly. */}
      <View style={styles.track}>
        <View style={[styles.fill, { width: `${pct}%` }]} />
      </View>

      <View style={styles.stages}>
        {progress.stages.map((stage) => {
          const active = stage.state === 'running';
          const elapsed =
            active && stage.started_at ? formatElapsed(stage.started_at, now) : null;
          return (
            <View key={stage.kind} style={styles.stageRow}>
              <View style={styles.stageIcon}>
                <StageIcon state={stage.state} />
              </View>
              <Text
                style={[
                  styles.stageLabel,
                  active && styles.stageLabelActive,
                  stage.state === 'done' && styles.stageLabelDone,
                ]}
              >
                {STAGE_LABELS[stage.kind]}
              </Text>
              {elapsed && <Text style={styles.elapsed}>{elapsed}</Text>}
            </View>
          );
        })}
      </View>

      {failed?.error ? (
        <Text style={styles.error} numberOfLines={3}>
          {failed.error}
        </Text>
      ) : (
        <Text style={styles.hint}>
          Long recordings take a while — speaker identification and transcription
          both scan the whole file. You can leave this screen; it keeps running.
        </Text>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  card: {
    backgroundColor: colors.surface,
    borderRadius: radius.card,
    borderWidth: 1,
    borderColor: colors.border,
    padding: 16,
    marginBottom: 16,
  },
  headerRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginBottom: 10,
  },
  header: {
    color: colors.textPrimary,
    fontSize: 15,
    fontWeight: '600',
  },
  count: {
    color: colors.textMuted,
    fontSize: 12,
    fontVariant: ['tabular-nums'],
  },
  track: {
    height: 4,
    borderRadius: 2,
    backgroundColor: colors.chipInactiveBg,
    overflow: 'hidden',
    marginBottom: 14,
  },
  fill: {
    height: '100%',
    borderRadius: 2,
    backgroundColor: colors.accent,
  },
  stages: {
    gap: 9,
  },
  stageRow: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  stageIcon: {
    width: 24,
    alignItems: 'center',
  },
  stageLabel: {
    flex: 1,
    marginLeft: 6,
    color: colors.textDim,
    fontSize: 13,
  },
  stageLabelActive: {
    color: colors.textPrimary,
    fontWeight: '600',
  },
  stageLabelDone: {
    color: colors.textMuted,
  },
  elapsed: {
    color: colors.accent,
    fontSize: 12,
    fontVariant: ['tabular-nums'],
  },
  hint: {
    marginTop: 14,
    color: colors.caption,
    fontSize: 12,
    lineHeight: 17,
  },
  error: {
    marginTop: 14,
    color: colors.accentDeep,
    fontSize: 12,
    lineHeight: 17,
  },
});
