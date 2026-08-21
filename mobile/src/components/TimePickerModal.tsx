/**
 * A two-column hour/minute picker in a bottom sheet.
 *
 * Deliberately hand-rolled rather than pulled from
 * `@react-native-community/datetimepicker`: that is a native module, and a
 * native dependency means every developer and device needs a dev-client rebuild
 * before the app will even start. A schedule editor is not worth that, and two
 * scroll columns are the same interaction the platform picker offers anyway.
 *
 * Values are minutes after midnight (`0..1439`), which is exactly what the
 * `/processing-schedule` API stores — no date objects, no timezone to get
 * wrong on the way in or out.
 */

import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Modal,
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  ScrollView,
} from 'react-native';
import { colors, mono, radius } from '../theme';

/** Height of one row; also the scroll offset per index. */
const ROW_H = 44;
/** Rows visible in a column at once (odd, so the selection can sit centred). */
const VISIBLE_ROWS = 5;
/** Minute granularity. Five minutes is finer than any schedule needs and keeps
 *  the column to twelve rows. */
const MINUTE_STEP = 5;

const HOURS = Array.from({ length: 24 }, (_, i) => i);
const MINUTES = Array.from({ length: 60 / MINUTE_STEP }, (_, i) => i * MINUTE_STEP);

export function formatMinutes(total: number): string {
  const m = ((total % 1440) + 1440) % 1440;
  const hh = String(Math.floor(m / 60)).padStart(2, '0');
  const mm = String(m % 60).padStart(2, '0');
  return `${hh}:${mm}`;
}

export function TimePickerModal({
  visible,
  title,
  minutes,
  onCancel,
  onConfirm,
}: {
  visible: boolean;
  title: string;
  minutes: number;
  onCancel: () => void;
  onConfirm: (minutes: number) => void;
}) {
  const [hour, setHour] = useState(Math.floor(minutes / 60) % 24);
  const [minute, setMinute] = useState(
    Math.round((minutes % 60) / MINUTE_STEP) * MINUTE_STEP % 60,
  );

  // Re-seed each time the sheet opens: the same component instance serves every
  // row, so without this it would reopen showing the previously edited time.
  useEffect(() => {
    if (!visible) return;
    setHour(Math.floor(minutes / 60) % 24);
    setMinute((Math.round((minutes % 60) / MINUTE_STEP) * MINUTE_STEP) % 60);
  }, [visible, minutes]);

  const preview = useMemo(() => formatMinutes(hour * 60 + minute), [hour, minute]);

  return (
    <Modal visible={visible} transparent animationType="fade" onRequestClose={onCancel}>
      <View style={styles.scrim}>
        <View style={styles.sheet}>
          <Text style={styles.title}>{title}</Text>
          <Text style={styles.preview}>{preview}</Text>

          <View style={styles.columns}>
            <Column values={HOURS} selected={hour} onSelect={setHour} />
            <Text style={styles.colon}>:</Text>
            <Column values={MINUTES} selected={minute} onSelect={setMinute} />
          </View>

          <View style={styles.actions}>
            <TouchableOpacity style={styles.ghost} onPress={onCancel}>
              <Text style={styles.ghostText}>Cancel</Text>
            </TouchableOpacity>
            <TouchableOpacity
              style={styles.primary}
              onPress={() => onConfirm(hour * 60 + minute)}
            >
              <Text style={styles.primaryText}>Set</Text>
            </TouchableOpacity>
          </View>
        </View>
      </View>
    </Modal>
  );
}

/** One scrollable column of two-digit values. */
function Column({
  values,
  selected,
  onSelect,
}: {
  values: number[];
  selected: number;
  onSelect: (v: number) => void;
}) {
  const ref = useRef<ScrollView>(null);

  // Bring the current value into view when the sheet opens. Without this a
  // 17:00 end time opens the column scrolled to 00 and looks unset.
  useEffect(() => {
    const idx = values.indexOf(selected);
    if (idx < 0) return;
    const offset = Math.max(0, (idx - Math.floor(VISIBLE_ROWS / 2)) * ROW_H);
    // A frame's delay: the ScrollView has no content height on first render.
    const t = setTimeout(() => ref.current?.scrollTo({ y: offset, animated: false }), 0);
    return () => clearTimeout(t);
    // Only on mount / when the incoming value changes, not on every tap —
    // re-scrolling under the user's finger would fight the gesture.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <ScrollView
      ref={ref}
      style={styles.column}
      contentContainerStyle={styles.columnContent}
      showsVerticalScrollIndicator={false}
    >
      {values.map((v) => {
        const active = v === selected;
        return (
          <TouchableOpacity
            key={v}
            style={[styles.row, active && styles.rowActive]}
            onPress={() => onSelect(v)}
            accessibilityRole="radio"
            accessibilityState={{ checked: active }}
          >
            <Text style={[styles.rowText, active && styles.rowTextActive]}>
              {String(v).padStart(2, '0')}
            </Text>
          </TouchableOpacity>
        );
      })}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  scrim: {
    flex: 1,
    backgroundColor: colors.scrim,
    justifyContent: 'center',
    paddingHorizontal: 32,
  },
  sheet: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.card,
    padding: 18,
  },
  title: {
    fontSize: 13,
    fontWeight: '700',
    color: colors.textMuted,
    fontFamily: mono,
    letterSpacing: 1.2,
    textAlign: 'center',
  },
  preview: {
    fontSize: 30,
    fontWeight: '700',
    color: colors.accent,
    fontFamily: mono,
    textAlign: 'center',
    marginTop: 8,
    marginBottom: 12,
  },
  columns: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 6,
  },
  colon: {
    fontSize: 22,
    color: colors.textDim,
    fontFamily: mono,
  },
  column: {
    height: ROW_H * VISIBLE_ROWS,
    width: 76,
    backgroundColor: colors.bg,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: radius.input,
  },
  columnContent: {
    paddingVertical: 4,
  },
  row: {
    height: ROW_H,
    alignItems: 'center',
    justifyContent: 'center',
  },
  rowActive: {
    backgroundColor: colors.accentSoft,
  },
  rowText: {
    fontSize: 18,
    color: colors.textMuted,
    fontFamily: mono,
  },
  rowTextActive: {
    color: colors.accent,
    fontWeight: '700',
  },
  actions: {
    flexDirection: 'row',
    gap: 10,
    marginTop: 18,
  },
  ghost: {
    flex: 1,
    borderWidth: 1,
    borderColor: colors.borderButton,
    borderRadius: radius.input,
    paddingVertical: 12,
    alignItems: 'center',
  },
  ghostText: {
    color: colors.textMuted,
    fontWeight: '600',
    fontSize: 13.5,
  },
  primary: {
    flex: 1,
    backgroundColor: colors.accent,
    borderRadius: radius.input,
    paddingVertical: 12,
    alignItems: 'center',
  },
  primaryText: {
    color: colors.white,
    fontWeight: '700',
    fontSize: 13.5,
  },
});
