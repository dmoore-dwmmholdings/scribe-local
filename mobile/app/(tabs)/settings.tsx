/**
 * Screen 6: Settings — Kiln self-hosted config.
 *
 * Grouped cards in the design's language: a device identity header, then
 * SERVER (base URL, device key, update token, connection test, backend
 * update), RECORDING (audio quality, default participants) and DEVICE (id).
 * Sensitive values persist to SecureStore; the rest to AsyncStorage.
 */

import { useState, useEffect, useCallback } from 'react';
import {
  View,
  Text,
  StyleSheet,
  TextInput,
  TouchableOpacity,
  ScrollView,
  Alert,
  ActivityIndicator,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useRouter } from 'expo-router';
import 'react-native-get-random-values'; // for crypto.randomUUID()
import { useSettingsStore } from '../../src/state/settingsStore';
import { api } from '../../src/api/client';
import type { AppSettings } from '../../src/types';
import { Screen, ScreenTitle, SectionLabel } from '../../src/components/ui';
import { colors, mono, radius } from '../../src/theme';

// ---------------------------------------------------------------------------

type Quality = AppSettings['audioQuality'];

const QUALITY_OPTIONS: { label: string; value: Quality }[] = [
  { label: 'Low', value: 'low' },
  { label: 'Medium', value: 'medium' },
  { label: 'High', value: 'high' },
];

// ---------------------------------------------------------------------------

export default function SettingsScreen() {
  const store = useSettingsStore();
  const router = useRouter();

  const [baseUrl, setBaseUrl] = useState(store.baseUrl);
  const [deviceKey, setDeviceKey] = useState(store.deviceKey);
  const [deviceId, setDeviceId] = useState(store.deviceId);
  const [updateToken, setUpdateToken] = useState(store.updateToken);
  const [audioQuality, setAudioQuality] = useState<Quality>(store.audioQuality);
  const [defaultParticipants, setDefaultParticipants] = useState(String(store.defaultParticipants));
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);
  const [connected, setConnected] = useState(false);

  // The store hydrates from storage asynchronously (see app/_layout.tsx), so
  // this screen can mount before the saved settings have loaded. The useState
  // initialisers above would then capture empty defaults permanently — showing
  // blank fields, and persisting them back over the saved values on Save (or
  // on Test connection, which also writes baseUrl). Re-seed once hydrated.
  useEffect(() => {
    if (!store.hydrated) return;
    setBaseUrl(store.baseUrl);
    setDeviceKey(store.deviceKey);
    setDeviceId(store.deviceId);
    setUpdateToken(store.updateToken);
    setAudioQuality(store.audioQuality);
    setDefaultParticipants(String(store.defaultParticipants));
    // Keyed on `hydrated` alone: a one-shot startup sync, not a subscription
    // that would clobber in-progress edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store.hydrated]);

  useEffect(() => {
    if (!store.deviceId && !deviceId) setDeviceId(crypto.randomUUID());
  }, [store.deviceId, deviceId]);

  const save = useCallback(async () => {
    setSaving(true);
    setTestResult(null);
    try {
      await store.update({
        baseUrl: baseUrl.trim().replace(/\/$/, ''),
        deviceKey: deviceKey.trim(),
        deviceId: deviceId.trim() || crypto.randomUUID(),
        audioQuality,
        defaultParticipants: parseInt(defaultParticipants, 10) || 2,
        updateToken: updateToken.trim(),
      });
      Alert.alert('Saved', 'Settings saved successfully.');
    } catch (err) {
      Alert.alert('Error', `Could not save settings: ${String(err)}`);
    } finally {
      setSaving(false);
    }
  }, [store, baseUrl, deviceKey, deviceId, audioQuality, defaultParticipants, updateToken]);

  const testConnection = useCallback(async () => {
    if (!baseUrl.trim()) {
      Alert.alert('No URL', 'Enter a server URL first.');
      return;
    }
    setTesting(true);
    setTestResult(null);
    try {
      // Persist the typed URL first so the test (and the rest of the app, which
      // all read baseUrl from the store) actually uses what's in the box — not a
      // stale saved value. Otherwise "Test connection" silently tests nothing.
      await store.update({ baseUrl: baseUrl.trim().replace(/\/$/, '') });
      const health = await api.health();
      setConnected(true);
      setTestResult(`Connected · v${health.version} · DB ${health.db}`);
    } catch (err) {
      setConnected(false);
      setTestResult(`Failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setTesting(false);
    }
  }, [store, baseUrl]);

  const deviceInitial = (deviceId || 'S').slice(0, 2).toUpperCase();

  return (
    <Screen>
      <ScreenTitle title="Settings" />
      <ScrollView contentContainerStyle={styles.container} keyboardShouldPersistTaps="handled">
        {/* Device identity header */}
        <View style={styles.accountCard}>
          <View style={styles.avatar}>
            <Text style={styles.avatarText}>{deviceInitial}</Text>
          </View>
          <View style={styles.accountBody}>
            <Text style={styles.accountName}>This device</Text>
            <Text style={styles.accountSub} numberOfLines={1}>
              {deviceId || 'New device — id assigned on save'}
            </Text>
          </View>
        </View>

        {/* Server */}
        <SectionLabel>SERVER</SectionLabel>
        <View style={styles.card}>
          <View style={styles.statusRow}>
            <Text style={styles.rowLabel}>Connection</Text>
            <View style={styles.statusValue}>
              <View
                style={[styles.statusDot, { backgroundColor: connected ? colors.statusReady : colors.textDim }]}
              />
              <Text style={[styles.statusText, { color: connected ? colors.statusReady : colors.textMuted }]}>
                {connected ? 'Connected' : 'Not connected'}
              </Text>
            </View>
          </View>
          <View style={styles.divider} />

          <Text style={styles.fieldLabel}>BASE URL</Text>
          <TextInput
            style={styles.input}
            placeholder="https://scribe.example.ts.net"
            placeholderTextColor={colors.textDim}
            value={baseUrl}
            onChangeText={setBaseUrl}
            autoCapitalize="none"
            autoCorrect={false}
            keyboardType="url"
            returnKeyType="done"
          />
          <Text style={styles.hint}>
            Your Tailscale MagicDNS HTTPS URL. The Tailscale app must be running on this device.
          </Text>

          <Text style={styles.fieldLabel}>DEVICE API KEY</Text>
          <TextInput
            style={styles.input}
            placeholder="scribe_sk_…"
            placeholderTextColor={colors.textDim}
            value={deviceKey}
            onChangeText={setDeviceKey}
            autoCapitalize="none"
            autoCorrect={false}
            secureTextEntry
            returnKeyType="done"
          />

          <Text style={styles.fieldLabel}>UPDATE TOKEN</Text>
          <TextInput
            style={styles.input}
            placeholder="scribe_update_…"
            placeholderTextColor={colors.textDim}
            value={updateToken}
            onChangeText={setUpdateToken}
            autoCapitalize="none"
            autoCorrect={false}
            secureTextEntry
            returnKeyType="done"
          />
          <Text style={styles.hint}>
            Admin secret for the /admin/* endpoints (backend self-update). Stored securely.
          </Text>

          <View style={styles.rowButtons}>
            <TouchableOpacity style={styles.ghostButton} onPress={testConnection} disabled={testing}>
              {testing ? (
                <ActivityIndicator color={colors.accent} />
              ) : (
                <Text style={styles.ghostButtonText}>Test connection</Text>
              )}
            </TouchableOpacity>
            <TouchableOpacity style={styles.ghostButton} onPress={() => router.push('/admin/update')}>
              <Text style={styles.ghostButtonText}>Backend update…</Text>
            </TouchableOpacity>
          </View>
          {testResult && (
            <Text
              style={[styles.testResult, testResult.startsWith('Failed') && { color: colors.accentDeep }]}
            >
              {testResult}
            </Text>
          )}
        </View>

        {/* Recording */}
        <SectionLabel>RECORDING</SectionLabel>
        <View style={styles.card}>
          <Text style={styles.fieldLabel}>AUDIO QUALITY</Text>
          <View style={styles.qualityGroup}>
            {QUALITY_OPTIONS.map((opt) => {
              const active = audioQuality === opt.value;
              return (
                <TouchableOpacity
                  key={opt.value}
                  style={[styles.qualityOption, active ? styles.qualityActive : styles.qualityInactive]}
                  onPress={() => setAudioQuality(opt.value)}
                  accessibilityRole="radio"
                  accessibilityState={{ checked: active }}
                >
                  <Text style={[styles.qualityText, active && styles.qualityTextActive]}>{opt.label}</Text>
                </TouchableOpacity>
              );
            })}
          </View>
          <Text style={styles.hint}>All options record 16 kHz mono AAC; only the bitrate changes.</Text>

          <Text style={styles.fieldLabel}>DEFAULT PARTICIPANTS</Text>
          <TextInput
            style={styles.input}
            value={defaultParticipants}
            onChangeText={setDefaultParticipants}
            keyboardType="number-pad"
            returnKeyType="done"
            placeholder="2"
            placeholderTextColor={colors.textDim}
          />
          <Text style={styles.hint}>Pre-fills the Record screen; improves speaker diarisation.</Text>

          <View style={styles.divider} />
          <TouchableOpacity
            style={styles.linkRow}
            onPress={() => router.push('/speakers')}
            accessibilityRole="button"
          >
            <View style={styles.toggleText}>
              <Text style={styles.rowLabel}>Speakers</Text>
              <Text style={styles.toggleHint}>
                Names and voiceprints reused across recordings
              </Text>
            </View>
            <Text style={styles.chevron}>›</Text>
          </TouchableOpacity>
        </View>

        {/* Processing */}
        <SectionLabel>PROCESSING</SectionLabel>
        <View style={styles.card}>
          <TouchableOpacity
            style={styles.linkRow}
            onPress={() => router.push('/admin/schedule')}
            accessibilityRole="button"
          >
            <View style={styles.toggleText}>
              <Text style={styles.rowLabel}>Processing schedule</Text>
              <Text style={styles.toggleHint}>
                Hours the server may transcribe, so it stays off your GPU at home
              </Text>
            </View>
            <Text style={styles.chevron}>›</Text>
          </TouchableOpacity>
        </View>

        {/* Diagnostics */}
        <SectionLabel>DIAGNOSTICS</SectionLabel>
        <View style={styles.card}>
          <TouchableOpacity
            style={styles.toggleRow}
            onPress={() => store.update({ reduceMotion: !store.reduceMotion })}
            accessibilityRole="switch"
            accessibilityState={{ checked: store.reduceMotion }}
          >
            <View style={styles.toggleText}>
              <Text style={styles.rowLabel}>Reduce motion</Text>
              <Text style={styles.toggleHint}>Disable the animated ember orb (also useful to isolate crashes)</Text>
            </View>
            <View style={[styles.toggle, store.reduceMotion ? styles.toggleOn : styles.toggleOff]}>
              <View
                style={[styles.toggleKnob, store.reduceMotion ? styles.toggleKnobOn : styles.toggleKnobOff]}
              />
            </View>
          </TouchableOpacity>
          <View style={styles.divider} />
          <TouchableOpacity
            style={styles.linkRow}
            onPress={() => router.push('/admin/logs')}
            accessibilityRole="button"
          >
            <Text style={styles.rowLabel}>View debug log</Text>
            <Text style={styles.chevron}>›</Text>
          </TouchableOpacity>
        </View>

        {/* Device */}
        <SectionLabel>DEVICE</SectionLabel>
        <View style={styles.card}>
          <Text style={styles.fieldLabel}>DEVICE ID</Text>
          <Text style={styles.deviceId} selectable>
            {deviceId || '(assigned on save)'}
          </Text>
        </View>

        {/* Save */}
        <TouchableOpacity style={[styles.saveButton, saving && styles.dimmed]} onPress={save} disabled={saving}>
          {saving ? (
            <ActivityIndicator color={colors.white} />
          ) : (
            <View style={styles.saveInner}>
              <Ionicons name="checkmark" size={18} color={colors.white} />
              <Text style={styles.saveText}>Save settings</Text>
            </View>
          )}
        </TouchableOpacity>
      </ScrollView>
    </Screen>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  container: {
    paddingHorizontal: 18,
    paddingBottom: 44,
  },
  accountCard: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.card,
    padding: 13,
  },
  avatar: {
    width: 42,
    height: 42,
    borderRadius: 21,
    backgroundColor: colors.accent,
    alignItems: 'center',
    justifyContent: 'center',
  },
  avatarText: {
    fontSize: 15,
    fontWeight: '700',
    color: colors.white,
  },
  accountBody: {
    flex: 1,
    minWidth: 0,
  },
  accountName: {
    fontSize: 14,
    fontWeight: '600',
    color: colors.textPrimary,
  },
  accountSub: {
    fontSize: 11.5,
    color: colors.textMuted,
    fontFamily: mono,
    marginTop: 2,
  },
  card: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.inner,
    padding: 14,
  },
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  rowLabel: {
    fontSize: 13.5,
    fontWeight: '500',
    color: colors.textBody,
  },
  statusValue: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  statusDot: {
    width: 7,
    height: 7,
    borderRadius: 4,
  },
  statusText: {
    fontSize: 12.5,
    fontWeight: '600',
  },
  divider: {
    height: 1,
    backgroundColor: colors.borderDivider,
    marginVertical: 12,
  },
  fieldLabel: {
    fontSize: 11,
    fontWeight: '700',
    color: colors.textMuted,
    fontFamily: mono,
    letterSpacing: 1.2,
    marginTop: 14,
    marginBottom: 7,
  },
  input: {
    backgroundColor: colors.bg,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: 11,
    paddingHorizontal: 13,
    paddingVertical: 11,
    fontSize: 14,
    color: colors.textPrimary,
  },
  hint: {
    fontSize: 12,
    color: colors.textDim,
    marginTop: 7,
    lineHeight: 17,
  },
  rowButtons: {
    flexDirection: 'row',
    gap: 10,
    marginTop: 16,
  },
  ghostButton: {
    flex: 1,
    borderWidth: 1,
    borderColor: colors.chipActiveBorder,
    borderRadius: 11,
    paddingVertical: 11,
    alignItems: 'center',
    backgroundColor: colors.accentFaint,
  },
  ghostButtonText: {
    color: colors.accent,
    fontWeight: '600',
    fontSize: 13,
  },
  testResult: {
    marginTop: 10,
    fontSize: 12.5,
    color: colors.statusReady,
    fontFamily: mono,
  },
  qualityGroup: {
    flexDirection: 'row',
    gap: 8,
  },
  qualityOption: {
    flex: 1,
    borderWidth: 1,
    borderRadius: 11,
    paddingVertical: 10,
    alignItems: 'center',
  },
  qualityActive: {
    borderColor: colors.chipActiveBorder,
    backgroundColor: colors.accentSoft,
  },
  qualityInactive: {
    borderColor: colors.borderInput,
    backgroundColor: colors.bg,
  },
  qualityText: {
    fontSize: 13,
    color: colors.textMuted,
  },
  qualityTextActive: {
    color: colors.accent,
    fontWeight: '600',
  },
  deviceId: {
    fontFamily: mono,
    fontSize: 12,
    color: colors.textMuted,
  },
  toggleRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 12,
  },
  toggleText: { flex: 1 },
  toggleHint: { fontSize: 11.5, color: colors.textDim, marginTop: 3, lineHeight: 16 },
  toggle: { width: 40, height: 23, borderRadius: 12, justifyContent: 'center', flexShrink: 0 },
  toggleOn: { backgroundColor: colors.accent },
  toggleOff: { backgroundColor: colors.toggleOff },
  toggleKnob: { width: 18, height: 18, borderRadius: 9, position: 'absolute' },
  toggleKnobOn: { right: 2.5, backgroundColor: colors.white },
  toggleKnobOff: { left: 2.5, backgroundColor: colors.toggleKnobOff },
  linkRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  chevron: { color: colors.textDim, fontSize: 18 },
  saveButton: {
    marginTop: 28,
    backgroundColor: colors.accent,
    borderRadius: radius.inner,
    paddingVertical: 14,
    alignItems: 'center',
  },
  saveInner: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  saveText: {
    color: colors.white,
    fontSize: 16,
    fontWeight: '700',
  },
  dimmed: {
    opacity: 0.6,
  },
});
