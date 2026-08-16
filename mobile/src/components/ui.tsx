/**
 * Small shared Kiln UI primitives used across the screens: a dark safe-area
 * screen wrapper, the big screen title row, the mono section label, and the
 * RDY/PROC-style status chip.
 */

import type { ReactNode } from 'react';
import { View, Text, StyleSheet, TouchableOpacity, type ViewStyle } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { useRouter } from 'expo-router';
import { colors, mono, radius, statusBadge, statusColor } from '../theme';

export function Screen({
  children,
  style,
}: {
  children: ReactNode;
  style?: ViewStyle;
}) {
  return (
    <SafeAreaView edges={['top']} style={[styles.screen, style]}>
      {children}
    </SafeAreaView>
  );
}

/** Big 27px screen title with an optional right-hand accessory (mono count, etc). */
export function ScreenTitle({
  title,
  right,
  style,
}: {
  title: string;
  right?: ReactNode;
  style?: ViewStyle;
}) {
  return (
    <View style={[styles.titleRow, style]}>
      <Text style={styles.title}>{title}</Text>
      {right}
    </View>
  );
}

export function MonoCount({ children }: { children: ReactNode }) {
  return <Text style={styles.monoCount}>{children}</Text>;
}

/**
 * In-screen header with a back chevron — the same one the Recording Detail
 * screen uses, so every pushed screen gets an identical, reliably tappable
 * control instead of the native stack header.
 *
 * `onBack` defaults to popping the stack, falling back to the Settings tab when
 * there is nothing to pop (a deep link straight into this screen).
 */
export function ScreenHeader({
  title,
  onBack,
  right,
}: {
  title: string;
  onBack?: () => void;
  right?: ReactNode;
}) {
  const router = useRouter();
  const goBack = () => {
    if (onBack) return onBack();
    if (router.canGoBack()) router.back();
    else router.replace('/settings');
  };
  return (
    <View style={styles.header}>
      <TouchableOpacity
        onPress={goBack}
        accessibilityRole="button"
        accessibilityLabel="Back"
        style={styles.backBtn}
        hitSlop={12}
      >
        <Text style={styles.backChevron}>‹</Text>
      </TouchableOpacity>
      <Text style={styles.headerTitle} numberOfLines={1}>
        {title}
      </Text>
      {right}
    </View>
  );
}

/** Mono section label, e.g. "SERVER", "TRANSCRIPTION". */
export function SectionLabel({ children }: { children: ReactNode }) {
  return <Text style={styles.sectionLabel}>{children}</Text>;
}

/** RDY / PROC / FAIL status chip in the recording's status color. */
export function StatusChip({ status }: { status: string }) {
  const color = statusColor[status] ?? colors.textMuted;
  const label = statusBadge[status] ?? status.toUpperCase();
  return <Text style={[styles.statusChip, { color }]}>{label}</Text>;
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: colors.bg,
  },
  titleRow: {
    flexDirection: 'row',
    alignItems: 'baseline',
    justifyContent: 'space-between',
    paddingHorizontal: 22,
    paddingTop: 6,
    paddingBottom: 12,
  },
  title: {
    fontSize: 27,
    fontWeight: '700',
    color: colors.textPrimary,
    letterSpacing: -0.5,
  },
  monoCount: {
    fontSize: 11,
    fontWeight: '600',
    color: colors.textMuted,
    fontFamily: mono,
  },
  sectionLabel: {
    fontSize: 11,
    fontWeight: '700',
    letterSpacing: 2,
    color: colors.accent,
    fontFamily: mono,
    paddingLeft: 6,
    paddingTop: 18,
    paddingBottom: 7,
  },
  statusChip: {
    fontSize: 10,
    fontWeight: '700',
    fontFamily: mono,
    letterSpacing: 0.5,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 18,
    paddingTop: 4,
    paddingBottom: 12,
  },
  backBtn: {
    paddingRight: 6,
    paddingVertical: 2,
  },
  backChevron: {
    fontSize: 30,
    fontWeight: '300',
    color: colors.accent,
    lineHeight: 32,
  },
  headerTitle: {
    flex: 1,
    fontSize: 17,
    fontWeight: '700',
    color: colors.textPrimary,
  },
});

export { radius };
