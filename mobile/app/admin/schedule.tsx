/**
 * Screen: Processing schedule.
 *
 * Controls *when* the worker box is allowed to run the heavy pipeline
 * (transcode / diarize / transcribe / merge / embed / summarize), so it can
 * drain during the working day and leave the GPU alone in the evening.
 *
 * Reachable from Settings -> "Processing schedule...".
 *
 * Two things this screen does NOT gate, and says so, because the surprise would
 * otherwise land during a meeting:
 *   * Live transcription keeps running whenever you are recording. It works on
 *     a few seconds of audio at a time and you asked for it by hitting record.
 *   * Uploads are never blocked. Audio always reaches the server; only the
 *     processing of it waits.
 *
 * API contract:
 *   GET  /processing-schedule            -> ProcessingScheduleResponse
 *   PUT  /processing-schedule            -> ProcessingScheduleResponse
 *   POST /processing-schedule/override   -> ProcessingScheduleResponse
 *
 * All times are minutes after midnight **on the server**, which is the clock
 * "when I'm home" is actually measured against. The phone's timezone is not
 * involved in the windows; it is used only to render the absolute instant the
 * server reports for its next boundary.
 */

import { useState, useEffect, useCallback, useRef } from 'react';
import {
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  ScrollView,
  ActivityIndicator,
  Alert,
  RefreshControl,
} from 'react-native';
import { api, ApiError } from '../../src/api/client';
import type {
  DayWindow,
  ProcessingScheduleResponse,
  ScheduleBacklog,
  ScheduleStatus,
} from '../../src/types';
import { Screen, ScreenHeader, SectionLabel } from '../../src/components/ui';
import { TimePickerModal, formatMinutes } from '../../src/components/TimePickerModal';
import { colors, mono, radius } from '../../src/theme';

// ---------------------------------------------------------------------------
// Constants & helpers
// ---------------------------------------------------------------------------

/** Monday first — the same order the server stores `days` in. */
const DAY_LABELS = ['MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT', 'SUN'] as const;

/** Offered lengths for a run-now / pause-now override, in minutes. */
const OVERRIDE_CHOICES = [
  { label: '30m', minutes: 30 },
  { label: '1h', minutes: 60 },
  { label: '2h', minutes: 120 },
  { label: '4h', minutes: 240 },
] as const;

const GRACE_CHOICES = [0, 5, 10, 30, 60] as const;

/** Seconds -> "2d 3h" / "13h 5m" / "45m" / "30s". */
function formatDuration(secs: number): string {
  if (secs < 60) return `${Math.max(0, Math.round(secs))}s`;
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  const rem = mins % 60;
  if (hours < 24) return rem ? `${hours}h ${rem}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  const remH = hours % 24;
  return remH ? `${days}d ${remH}h` : `${days}d`;
}

/** A window whose end is at or before its start runs into the next day. */
function wrapsMidnight(d: DayWindow): boolean {
  return d.end <= d.start;
}

function sameDays(a: DayWindow[], b: DayWindow[]): boolean {
  return (
    a.length === b.length &&
    a.every((d, i) => d.enabled === b[i].enabled && d.start === b[i].start && d.end === b[i].end)
  );
}

function friendlyError(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 404) {
      return 'This backend does not have the processing schedule yet — update it first.';
    }
    return err.message;
  }
  return err instanceof Error ? err.message : String(err);
}

// ---------------------------------------------------------------------------
// Screen
// ---------------------------------------------------------------------------

export default function ProcessingScheduleScreen() {
  // Server-owned, refreshed by polling.
  const [status, setStatus] = useState<ScheduleStatus | null>(null);
  const [backlog, setBacklog] = useState<ScheduleBacklog | null>(null);

  // Locally edited, seeded from the server on load and after each save. Kept
  // apart from `status`/`backlog` on purpose: the background refresh below must
  // be able to update the readout without overwriting half-finished edits.
  const [enabled, setEnabled] = useState(false);
  const [days, setDays] = useState<DayWindow[]>([]);
  const [grace, setGrace] = useState(10);
  const savedRef = useRef<{ enabled: boolean; days: DayWindow[]; grace: number } | null>(null);

  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [busyAction, setBusyAction] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [picker, setPicker] = useState<{ day: number; edge: 'start' | 'end' } | null>(null);
  const [pendingAction, setPendingAction] = useState<'run' | 'pause' | null>(null);

  const dirty =
    savedRef.current != null &&
    (enabled !== savedRef.current.enabled ||
      grace !== savedRef.current.grace ||
      !sameDays(days, savedRef.current.days));

  /** Apply a server response: always the readout, the editable copy only when
   *  the user has nothing unsaved (or we just saved). */
  const applyResponse = useCallback(
    (data: ProcessingScheduleResponse, seedEditable: boolean) => {
      setStatus(data.status);
      setBacklog(data.backlog);
      if (!seedEditable) return;
      setEnabled(data.schedule.enabled);
      setDays(data.schedule.days);
      setGrace(data.schedule.grace_minutes);
      savedRef.current = {
        enabled: data.schedule.enabled,
        days: data.schedule.days,
        grace: data.schedule.grace_minutes,
      };
    },
    [],
  );

  const load = useCallback(
    async (seedEditable: boolean) => {
      try {
        const data = await api.getProcessingSchedule();
        applyResponse(data, seedEditable);
        setError(null);
      } catch (err) {
        setError(friendlyError(err));
      }
    },
    [applyResponse],
  );

  useEffect(() => {
    (async () => {
      await load(true);
      setLoading(false);
    })();
  }, [load]);

  // Keep the status line and backlog honest while the screen sits open — the
  // countdown is only meaningful if it is actually counting. Background polls
  // never reseed the editable copy; pull-to-refresh is how you deliberately
  // pick up a change made from somewhere else.
  useEffect(() => {
    const id = setInterval(() => void load(false), 20_000);
    return () => clearInterval(id);
  }, [load]);

  // -------------------------------------------------------------------------
  // Mutations
  // -------------------------------------------------------------------------

  const save = useCallback(async () => {
    setSaving(true);
    try {
      const data = await api.setProcessingSchedule({ enabled, days, grace_minutes: grace });
      applyResponse(data, true);
      setError(null);
    } catch (err) {
      Alert.alert('Could not save', friendlyError(err));
    } finally {
      setSaving(false);
    }
  }, [enabled, days, grace, applyResponse]);

  const applyOverride = useCallback(
    async (mode: 'run' | 'pause' | 'clear', minutes?: number) => {
      setBusyAction(true);
      setPendingAction(null);
      try {
        const data = await api.setProcessingOverride({ mode, minutes });
        // Never reseed the editable copy here: an override does not touch the
        // weekly windows, and the user may be mid-edit.
        applyResponse(data, false);
        setError(null);
      } catch (err) {
        Alert.alert('Could not apply', friendlyError(err));
      } finally {
        setBusyAction(false);
      }
    },
    [applyResponse],
  );

  const patchDay = useCallback((index: number, patch: Partial<DayWindow>) => {
    setDays((prev) => prev.map((d, i) => (i === index ? { ...d, ...patch } : d)));
  }, []);

  const copyMondayToWeekdays = useCallback(() => {
    setDays((prev) => (prev.length === 7 ? prev.map((d, i) => (i < 5 ? { ...prev[0] } : d)) : prev));
  }, []);

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------

  if (loading) {
    return (
      <Screen>
        <ScreenHeader title="Processing schedule" />
        <View style={styles.centered}>
          <ActivityIndicator color={colors.accent} />
        </View>
      </Screen>
    );
  }

  const hasOverride =
    status?.reason === 'override_run' || status?.reason === 'override_pause';

  return (
    <Screen>
      <ScreenHeader title="Processing schedule" />
      <ScrollView
        contentContainerStyle={styles.container}
        keyboardShouldPersistTaps="handled"
        refreshControl={
          <RefreshControl
            refreshing={refreshing}
            tintColor={colors.accent}
            onRefresh={async () => {
              setRefreshing(true);
              await load(!dirty);
              setRefreshing(false);
            }}
          />
        }
      >
        {error && <Text style={styles.errorBanner}>{error}</Text>}

        {/* ---------------------------------------------------------------- */}
        {status && <StatusCard status={status} />}

        {/* ---------------------------------------------------------------- */}
        <SectionLabel>BACKLOG</SectionLabel>
        <View style={styles.card}>
          {backlog ? (
            <>
              <View style={styles.statRow}>
                <Stat value={backlog.recordings_waiting} label="RECORDINGS WAITING" />
                <Stat value={backlog.queued} label="JOBS QUEUED" />
                <Stat value={backlog.running} label="RUNNING" />
              </View>
              {backlog.failed > 0 && (
                <Text style={styles.failedNote}>
                  {backlog.failed} job{backlog.failed === 1 ? '' : 's'} failed and will not retry.
                </Text>
              )}
            </>
          ) : (
            <Text style={styles.hint}>No queue information.</Text>
          )}
        </View>

        {/* ---------------------------------------------------------------- */}
        <SectionLabel>RIGHT NOW</SectionLabel>
        <View style={styles.card}>
          {pendingAction ? (
            <>
              <Text style={styles.fieldLabelTop}>
                {pendingAction === 'run' ? 'PROCESS FOR HOW LONG?' : 'PAUSE FOR HOW LONG?'}
              </Text>
              <View style={styles.chipRow}>
                {OVERRIDE_CHOICES.map((c) => (
                  <TouchableOpacity
                    key={c.minutes}
                    style={styles.chip}
                    onPress={() => applyOverride(pendingAction, c.minutes)}
                  >
                    <Text style={styles.chipText}>{c.label}</Text>
                  </TouchableOpacity>
                ))}
              </View>
              <TouchableOpacity style={styles.cancelRow} onPress={() => setPendingAction(null)}>
                <Text style={styles.cancelText}>Cancel</Text>
              </TouchableOpacity>
            </>
          ) : (
            <View style={styles.rowButtons}>
              <TouchableOpacity
                style={styles.ghostButton}
                onPress={() => setPendingAction('run')}
                disabled={busyAction}
              >
                <Text style={styles.ghostButtonText}>Process now</Text>
              </TouchableOpacity>
              <TouchableOpacity
                style={styles.ghostButton}
                onPress={() => setPendingAction('pause')}
                disabled={busyAction}
              >
                <Text style={styles.ghostButtonText}>Pause now</Text>
              </TouchableOpacity>
            </View>
          )}

          {hasOverride && !pendingAction && (
            <>
              <View style={styles.divider} />
              <TouchableOpacity
                style={styles.linkRow}
                onPress={() => applyOverride('clear')}
                disabled={busyAction}
              >
                <View style={styles.flex}>
                  <Text style={styles.rowLabel}>Back to the schedule</Text>
                  <Text style={styles.toggleHint}>
                    Drop the{' '}
                    {status?.reason === 'override_run' ? '"process now"' : '"pause now"'} override
                    now instead of waiting for it to expire
                  </Text>
                </View>
                {busyAction ? (
                  <ActivityIndicator color={colors.accent} />
                ) : (
                  <Text style={styles.chevron}>›</Text>
                )}
              </TouchableOpacity>
            </>
          )}
          <Text style={styles.hint}>
            An override outranks the weekly windows until it expires, then the schedule takes over
            again.
          </Text>
        </View>

        {/* ---------------------------------------------------------------- */}
        <SectionLabel>WEEKLY WINDOWS</SectionLabel>
        <View style={styles.card}>
          <TouchableOpacity
            style={styles.toggleRow}
            onPress={() => setEnabled((v) => !v)}
            accessibilityRole="switch"
            accessibilityState={{ checked: enabled }}
          >
            <View style={styles.flex}>
              <Text style={styles.rowLabel}>Limit processing to a schedule</Text>
              <Text style={styles.toggleHint}>
                Off means process whenever there is work — the original behaviour.
              </Text>
            </View>
            <Toggle on={enabled} />
          </TouchableOpacity>

          <View style={styles.divider} />

          {days.map((d, i) => (
            <DayRow
              key={DAY_LABELS[i]}
              label={DAY_LABELS[i]}
              day={d}
              dimmed={!enabled}
              onToggle={() => patchDay(i, { enabled: !d.enabled })}
              onEditStart={() => setPicker({ day: i, edge: 'start' })}
              onEditEnd={() => setPicker({ day: i, edge: 'end' })}
            />
          ))}

          <TouchableOpacity style={styles.copyRow} onPress={copyMondayToWeekdays}>
            <Text style={styles.copyText}>Copy Monday to Tue–Fri</Text>
          </TouchableOpacity>

          <Text style={styles.hint}>
            Times are the server's local clock ({status?.server_time ?? 'unknown'}). An end at or
            before the start runs overnight into the next day.
          </Text>
        </View>

        {/* ---------------------------------------------------------------- */}
        <SectionLabel>WHEN A WINDOW CLOSES</SectionLabel>
        <View style={styles.card}>
          <Text style={styles.fieldLabelTop}>GRACE PERIOD</Text>
          <View style={styles.chipRow}>
            {GRACE_CHOICES.map((g) => {
              const active = grace === g;
              return (
                <TouchableOpacity
                  key={g}
                  style={[styles.chip, active ? styles.chipActive : styles.chipInactive]}
                  onPress={() => setGrace(g)}
                  accessibilityRole="radio"
                  accessibilityState={{ checked: active }}
                >
                  <Text style={[styles.chipText, !active && styles.chipTextInactive]}>
                    {g === 0 ? 'None' : `${g}m`}
                  </Text>
                </TouchableOpacity>
              );
            })}
          </View>
          <Text style={styles.hint}>
            A stage already running when the window closes gets this long to finish. After that the
            worker puts it back on the queue for the next window — nothing is lost, and it costs no
            retry attempt. A model call already on the GPU runs to the end of that call regardless.
          </Text>
        </View>

        {/* ---------------------------------------------------------------- */}
        <View style={styles.card}>
          <Text style={styles.hint}>
            Live transcription during an active recording is never paused, and uploads are never
            blocked. Only the full pipeline waits for a window.
          </Text>
        </View>

        {dirty && (
          <TouchableOpacity
            style={[styles.saveButton, saving && styles.dimmed]}
            onPress={save}
            disabled={saving}
          >
            {saving ? (
              <ActivityIndicator color={colors.white} />
            ) : (
              <Text style={styles.saveText}>Save schedule</Text>
            )}
          </TouchableOpacity>
        )}
      </ScrollView>

      <TimePickerModal
        visible={picker != null}
        title={
          picker ? `${DAY_LABELS[picker.day]} ${picker.edge === 'start' ? 'START' : 'END'}` : ''
        }
        minutes={picker ? days[picker.day][picker.edge] : 0}
        onCancel={() => setPicker(null)}
        onConfirm={(m) => {
          if (picker) patchDay(picker.day, { [picker.edge]: m } as Partial<DayWindow>);
          setPicker(null);
        }}
      />
    </Screen>
  );
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

function StatusCard({ status }: { status: ScheduleStatus }) {
  const { allowed, reason, next_change_secs } = status;
  const headline = allowed ? 'Processing' : 'Paused';
  const detail: Record<ScheduleStatus['reason'], string> = {
    disabled: 'No schedule set — the pipeline runs whenever there is work.',
    in_window: 'Inside a scheduled window.',
    outside_window: 'Outside every scheduled window.',
    override_run: 'A "process now" override is in force.',
    override_pause: 'A "pause now" override is in force.',
  };
  const boundary =
    next_change_secs == null
      ? null
      : `${allowed ? 'Stops' : 'Resumes'} in ${formatDuration(next_change_secs)}`;

  return (
    <View style={[styles.card, styles.statusCard]}>
      <View style={styles.statusHead}>
        <View
          style={[
            styles.statusDot,
            { backgroundColor: allowed ? colors.statusReady : colors.amber },
          ]}
        />
        <Text style={[styles.statusHeadline, { color: allowed ? colors.statusReady : colors.amber }]}>
          {headline}
        </Text>
        {boundary && <Text style={styles.statusBoundary}>{boundary}</Text>}
      </View>
      <Text style={styles.statusDetail}>{detail[reason]}</Text>
    </View>
  );
}

function DayRow({
  label,
  day,
  dimmed,
  onToggle,
  onEditStart,
  onEditEnd,
}: {
  label: string;
  day: DayWindow;
  dimmed: boolean;
  onToggle: () => void;
  onEditStart: () => void;
  onEditEnd: () => void;
}) {
  const off = !day.enabled;
  return (
    <View style={[styles.dayRow, dimmed && styles.dimmed]}>
      <TouchableOpacity
        style={styles.dayToggle}
        onPress={onToggle}
        accessibilityRole="switch"
        accessibilityLabel={`${label} enabled`}
        accessibilityState={{ checked: day.enabled }}
      >
        <Text style={[styles.dayLabel, off && styles.dayLabelOff]}>{label}</Text>
        <Toggle on={day.enabled} small />
      </TouchableOpacity>

      {off ? (
        <Text style={styles.dayOffText}>No processing</Text>
      ) : (
        <View style={styles.timeGroup}>
          <TouchableOpacity style={styles.timeButton} onPress={onEditStart}>
            <Text style={styles.timeText}>{formatMinutes(day.start)}</Text>
          </TouchableOpacity>
          <Text style={styles.arrow}>→</Text>
          <TouchableOpacity style={styles.timeButton} onPress={onEditEnd}>
            <Text style={styles.timeText}>{formatMinutes(day.end)}</Text>
          </TouchableOpacity>
          {wrapsMidnight(day) && <Text style={styles.nextDay}>+1d</Text>}
        </View>
      )}
    </View>
  );
}

function Toggle({ on, small }: { on: boolean; small?: boolean }) {
  return (
    <View
      style={[
        small ? styles.toggleSm : styles.toggle,
        on ? styles.toggleOn : styles.toggleOff,
      ]}
    >
      <View
        style={[
          small ? styles.toggleKnobSm : styles.toggleKnob,
          on ? styles.toggleKnobOn : styles.toggleKnobOff,
        ]}
      />
    </View>
  );
}

function Stat({ value, label }: { value: number; label: string }) {
  return (
    <View style={styles.stat}>
      <Text style={[styles.statValue, value === 0 && styles.statValueZero]}>{value}</Text>
      <Text style={styles.statLabel}>{label}</Text>
    </View>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  container: { paddingHorizontal: 18, paddingBottom: 44 },
  centered: { flex: 1, alignItems: 'center', justifyContent: 'center' },
  flex: { flex: 1 },
  dimmed: { opacity: 0.45 },

  errorBanner: {
    color: colors.accentDeep,
    fontSize: 12.5,
    marginBottom: 12,
    lineHeight: 18,
  },

  card: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.inner,
    padding: 14,
  },
  statusCard: { marginTop: 4 },
  statusHead: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  statusDot: { width: 8, height: 8, borderRadius: 4 },
  statusHeadline: { fontSize: 15, fontWeight: '700' },
  statusBoundary: {
    flex: 1,
    textAlign: 'right',
    fontSize: 11.5,
    color: colors.textMuted,
    fontFamily: mono,
  },
  statusDetail: { fontSize: 12.5, color: colors.textMuted, marginTop: 7, lineHeight: 18 },

  statRow: { flexDirection: 'row', gap: 10 },
  stat: { flex: 1 },
  statValue: { fontSize: 22, fontWeight: '700', color: colors.accent, fontFamily: mono },
  statValueZero: { color: colors.textDim },
  statLabel: {
    fontSize: 9.5,
    color: colors.textDim,
    fontFamily: mono,
    letterSpacing: 0.8,
    marginTop: 3,
  },
  failedNote: { fontSize: 12, color: colors.accentDeep, marginTop: 12, lineHeight: 17 },

  rowLabel: { fontSize: 13.5, fontWeight: '500', color: colors.textBody },
  hint: { fontSize: 12, color: colors.textDim, marginTop: 10, lineHeight: 17 },
  toggleHint: { fontSize: 11.5, color: colors.textDim, marginTop: 3, lineHeight: 16 },
  fieldLabelTop: {
    fontSize: 11,
    fontWeight: '700',
    color: colors.textMuted,
    fontFamily: mono,
    letterSpacing: 1.2,
    marginBottom: 9,
  },
  divider: { height: 1, backgroundColor: colors.borderDivider, marginVertical: 12 },

  rowButtons: { flexDirection: 'row', gap: 10 },
  ghostButton: {
    flex: 1,
    borderWidth: 1,
    borderColor: colors.chipActiveBorder,
    borderRadius: 11,
    paddingVertical: 11,
    alignItems: 'center',
    backgroundColor: colors.accentFaint,
  },
  ghostButtonText: { color: colors.accent, fontWeight: '600', fontSize: 13 },

  chipRow: { flexDirection: 'row', gap: 8 },
  chip: {
    flex: 1,
    borderWidth: 1,
    borderColor: colors.chipActiveBorder,
    backgroundColor: colors.chipActiveBg,
    borderRadius: 11,
    paddingVertical: 10,
    alignItems: 'center',
  },
  chipActive: { borderColor: colors.chipActiveBorder, backgroundColor: colors.accentSoft },
  chipInactive: { borderColor: colors.borderInput, backgroundColor: colors.bg },
  chipText: { fontSize: 13, fontWeight: '600', color: colors.accent },
  chipTextInactive: { color: colors.textMuted, fontWeight: '400' },
  cancelRow: { alignItems: 'center', paddingTop: 14 },
  cancelText: { color: colors.textMuted, fontSize: 13 },

  linkRow: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: 12 },
  chevron: { color: colors.textDim, fontSize: 18 },

  toggleRow: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: 12 },
  toggle: { width: 40, height: 23, borderRadius: 12, justifyContent: 'center', flexShrink: 0 },
  toggleSm: { width: 32, height: 19, borderRadius: 10, justifyContent: 'center', flexShrink: 0 },
  toggleOn: { backgroundColor: colors.accent },
  toggleOff: { backgroundColor: colors.toggleOff },
  toggleKnob: { width: 18, height: 18, borderRadius: 9, position: 'absolute' },
  toggleKnobSm: { width: 15, height: 15, borderRadius: 8, position: 'absolute' },
  toggleKnobOn: { right: 2.5, backgroundColor: colors.white },
  toggleKnobOff: { left: 2.5, backgroundColor: colors.toggleKnobOff },

  dayRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingVertical: 7,
    gap: 10,
  },
  dayToggle: { flexDirection: 'row', alignItems: 'center', gap: 9 },
  dayLabel: {
    fontSize: 11.5,
    fontWeight: '700',
    color: colors.textBody,
    fontFamily: mono,
    letterSpacing: 1,
    width: 34,
  },
  dayLabelOff: { color: colors.textDim },
  dayOffText: { fontSize: 12, color: colors.textDim, fontStyle: 'italic' },
  timeGroup: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  timeButton: {
    borderWidth: 1,
    borderColor: colors.borderInput,
    backgroundColor: colors.bg,
    borderRadius: 9,
    paddingHorizontal: 10,
    paddingVertical: 6,
  },
  timeText: { fontSize: 13, color: colors.textPrimary, fontFamily: mono },
  arrow: { color: colors.textDim, fontSize: 12 },
  nextDay: { fontSize: 9.5, color: colors.amber, fontFamily: mono, letterSpacing: 0.5 },

  copyRow: { paddingTop: 10, alignItems: 'flex-start' },
  copyText: { color: colors.accent, fontSize: 12.5, fontWeight: '600' },

  saveButton: {
    marginTop: 26,
    backgroundColor: colors.accent,
    borderRadius: radius.inner,
    paddingVertical: 14,
    alignItems: 'center',
  },
  saveText: { color: colors.white, fontSize: 16, fontWeight: '700' },
});
