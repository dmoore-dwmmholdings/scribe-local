/**
 * Diagnostics — view / share / clear the crash-surviving debug log.
 *
 * The log is written by src/util/logger.ts (console + on-disk ring buffer). It
 * persists across a hard crash, so after the app dies you can come here, read
 * the last breadcrumbs, and share them.  For NATIVE crashes the device
 * `adb logcat` is still the source of truth — the breadcrumbs here tell you
 * what the app was doing at that instant.
 */

import { useState, useCallback, useEffect } from 'react';
import { View, Text, StyleSheet, TouchableOpacity, ScrollView, Share } from 'react-native';
import { readLog, clearLog, logFilePath } from '../../src/util/logger';
import { Screen, ScreenHeader } from '../../src/components/ui';
import { colors, mono } from '../../src/theme';

export default function LogsScreen() {
  const [text, setText] = useState('(loading…)');

  const refresh = useCallback(async () => {
    setText(await readLog());
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const share = useCallback(async () => {
    try {
      await Share.share({ message: await readLog() });
    } catch {
      // user cancelled
    }
  }, []);

  const clear = useCallback(async () => {
    await clearLog();
    await refresh();
  }, [refresh]);

  return (
    <Screen>
      <ScreenHeader title="Diagnostics" />
      <View style={styles.bar}>
        <TouchableOpacity style={styles.btn} onPress={refresh}>
          <Text style={styles.btnText}>Refresh</Text>
        </TouchableOpacity>
        <TouchableOpacity style={styles.btn} onPress={share}>
          <Text style={styles.btnText}>Share</Text>
        </TouchableOpacity>
        <TouchableOpacity style={styles.btn} onPress={clear}>
          <Text style={[styles.btnText, { color: colors.accentDeep }]}>Clear</Text>
        </TouchableOpacity>
      </View>
      <Text style={styles.path}>{logFilePath()}</Text>
      <ScrollView style={styles.scroll} contentContainerStyle={styles.scrollContent}>
        <Text style={styles.log} selectable>
          {text}
        </Text>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: colors.bg },
  bar: {
    flexDirection: 'row',
    gap: 10,
    paddingHorizontal: 16,
    paddingTop: 12,
    paddingBottom: 8,
  },
  btn: {
    borderWidth: 1,
    borderColor: colors.chipActiveBorder,
    backgroundColor: colors.accentFaint,
    borderRadius: 10,
    paddingHorizontal: 14,
    paddingVertical: 8,
  },
  btnText: { color: colors.accent, fontWeight: '600', fontSize: 13 },
  path: {
    fontSize: 10,
    color: colors.textDim,
    fontFamily: mono,
    paddingHorizontal: 16,
    paddingBottom: 8,
  },
  scroll: { flex: 1, backgroundColor: colors.surface, marginHorizontal: 12, borderRadius: 12 },
  scrollContent: { padding: 12 },
  log: { fontSize: 10.5, lineHeight: 15, color: colors.textLight, fontFamily: mono },
});
