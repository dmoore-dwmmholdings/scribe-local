/**
 * Screen 6: Settings
 *
 * Server base URL (tailnet MagicDNS name), device API key, audio quality,
 * and default participant count.  Persisted via SecureStore / AsyncStorage.
 *
 * The device ID is generated once on first launch and sent with every
 * POST /recordings as `device_id` (used for per-device auth in the future).
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
import { useRouter } from 'expo-router';
import '../../src/polyfills/crypto'; // for crypto.randomUUID()
import { useSettingsStore } from '../../src/state/settingsStore';
import { api } from '../../src/api/client';
import type { AppSettings } from '../../src/types';

// ---------------------------------------------------------------------------

type Quality = AppSettings['audioQuality'];

const QUALITY_OPTIONS: { label: string; value: Quality }[] = [
  { label: 'Low (~24 kbps)', value: 'low' },
  { label: 'Medium (~32 kbps)', value: 'medium' },
  { label: 'High (~48 kbps)', value: 'high' },
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
  const [defaultParticipants, setDefaultParticipants] = useState(
    String(store.defaultParticipants),
  );
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);

  // The store hydrates from storage asynchronously (see app/_layout.tsx), so
  // this screen can mount before the saved settings have loaded.  The useState
  // initialisers above would then capture empty defaults permanently — showing
  // blank fields, and overwriting the saved settings with them on Save.
  // Re-seed the form once hydration completes.
  useEffect(() => {
    if (!store.hydrated) return;
    setBaseUrl(store.baseUrl);
    setDeviceKey(store.deviceKey);
    setDeviceId(store.deviceId);
    setUpdateToken(store.updateToken);
    setAudioQuality(store.audioQuality);
    setDefaultParticipants(String(store.defaultParticipants));
    // Intentionally keyed on `hydrated` alone: this is a one-shot sync at
    // startup, not a subscription that would clobber in-progress edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store.hydrated]);

  // Generate a device ID on first launch
  useEffect(() => {
    if (!store.deviceId && !deviceId) {
      const newId = crypto.randomUUID();
      setDeviceId(newId);
    }
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
  }, [store, baseUrl, deviceKey, deviceId, audioQuality, defaultParticipants]);

  const testConnection = useCallback(async () => {
    if (!baseUrl.trim()) {
      Alert.alert('No URL', 'Enter a server URL first.');
      return;
    }
    setTesting(true);
    setTestResult(null);
    try {
      // Test what is currently in the form, not the last-saved value — so
      // "Test connection" is meaningful before "Save settings" is tapped.
      const health = await api.health(baseUrl);
      setTestResult(`Connected · v${health.version} · DB: ${health.db}`);
    } catch (err) {
      setTestResult(`Failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setTesting(false);
    }
  }, [baseUrl]);

  return (
    <ScrollView contentContainerStyle={styles.container} keyboardShouldPersistTaps="handled">
      {/* Server */}
      <Text style={styles.sectionHeader}>Server</Text>

      <Text style={styles.label}>Base URL</Text>
      <TextInput
        style={styles.input}
        placeholder="https://scribe.example.ts.net"
        value={baseUrl}
        onChangeText={setBaseUrl}
        autoCapitalize="none"
        autoCorrect={false}
        keyboardType="url"
        returnKeyType="done"
      />
      <Text style={styles.hint}>
        Your Tailscale MagicDNS HTTPS URL (e.g. https://scribe.mango-slug.ts.net).
        The Tailscale app must be running on this device.
      </Text>

      <Text style={styles.label}>Device API Key</Text>
      <TextInput
        style={styles.input}
        placeholder="scribe_sk_…"
        value={deviceKey}
        onChangeText={setDeviceKey}
        autoCapitalize="none"
        autoCorrect={false}
        secureTextEntry
        returnKeyType="done"
      />
      <Text style={styles.hint}>
        Bearer token issued by your Scribe server (device enrollment).
        Stored in the device's secure enclave.
      </Text>

      <Text style={styles.label}>Update token</Text>
      <TextInput
        style={styles.input}
        placeholder="scribe_update_…"
        value={updateToken}
        onChangeText={setUpdateToken}
        autoCapitalize="none"
        autoCorrect={false}
        secureTextEntry
        returnKeyType="done"
      />
      <Text style={styles.hint}>
        Admin secret for the /admin/* endpoints (backend self-update and rollback).
        Separate from the device key. Stored in the device's secure enclave.
      </Text>

      {/* Test connection */}
      <TouchableOpacity
        style={styles.secondaryButton}
        onPress={testConnection}
        disabled={testing}
      >
        {testing ? (
          <ActivityIndicator color="#E53935" />
        ) : (
          <Text style={styles.secondaryButtonText}>Test connection</Text>
        )}
      </TouchableOpacity>
      {testResult && (
        <Text
          style={[
            styles.testResult,
            testResult.startsWith('Failed') && styles.testResultError,
          ]}
        >
          {testResult}
        </Text>
      )}

      {/* Backend update */}
      <TouchableOpacity
        style={styles.secondaryButton}
        onPress={() => router.push('/admin/update')}
        accessibilityRole="button"
      >
        <Text style={styles.secondaryButtonText}>Backend update…</Text>
      </TouchableOpacity>

      {/* Audio */}
      <Text style={[styles.sectionHeader, { marginTop: 24 }]}>Audio</Text>

      <Text style={styles.label}>Recording quality</Text>
      <View style={styles.qualityGroup}>
        {QUALITY_OPTIONS.map((opt) => (
          <TouchableOpacity
            key={opt.value}
            style={[
              styles.qualityOption,
              audioQuality === opt.value && styles.qualityOptionActive,
            ]}
            onPress={() => setAudioQuality(opt.value)}
            accessibilityRole="radio"
            accessibilityState={{ checked: audioQuality === opt.value }}
          >
            <Text
              style={[
                styles.qualityOptionText,
                audioQuality === opt.value && styles.qualityOptionTextActive,
              ]}
            >
              {opt.label}
            </Text>
          </TouchableOpacity>
        ))}
      </View>
      <Text style={styles.hint}>
        All options record at 16 kHz mono AAC — the difference is only
        bitrate.  Medium (32 kbps) is recommended.
      </Text>

      {/* Defaults */}
      <Text style={[styles.sectionHeader, { marginTop: 24 }]}>Defaults</Text>

      <Text style={styles.label}>Default participant count</Text>
      <TextInput
        style={styles.input}
        value={defaultParticipants}
        onChangeText={setDefaultParticipants}
        keyboardType="number-pad"
        returnKeyType="done"
        placeholder="2"
      />
      <Text style={styles.hint}>
        Pre-fills the participant count field on the Record screen.
        Providing the count improves speaker diarisation.
      </Text>

      {/* Device ID (read-only, for reference) */}
      <Text style={[styles.sectionHeader, { marginTop: 24 }]}>Device</Text>
      <Text style={styles.label}>Device ID</Text>
      <Text style={styles.deviceId} selectable>
        {deviceId || '(will be assigned on save)'}
      </Text>

      {/* Save */}
      <TouchableOpacity
        style={[styles.saveButton, saving && styles.saveButtonBusy]}
        onPress={save}
        disabled={saving}
        accessibilityRole="button"
      >
        {saving ? (
          <ActivityIndicator color="#fff" />
        ) : (
          <Text style={styles.saveButtonText}>Save settings</Text>
        )}
      </TouchableOpacity>
    </ScrollView>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  container: {
    padding: 20,
    paddingBottom: 40,
    backgroundColor: '#fff',
  },
  sectionHeader: {
    fontSize: 11,
    fontWeight: '700',
    color: '#aaa',
    textTransform: 'uppercase',
    letterSpacing: 1,
    marginBottom: 12,
  },
  label: {
    fontSize: 14,
    fontWeight: '500',
    color: '#333',
    marginBottom: 6,
    marginTop: 16,
  },
  input: {
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 10,
    fontSize: 15,
    color: '#111',
    backgroundColor: '#fafafa',
  },
  hint: {
    fontSize: 12,
    color: '#bbb',
    marginTop: 6,
    lineHeight: 17,
  },
  secondaryButton: {
    marginTop: 12,
    borderWidth: 1,
    borderColor: '#E53935',
    borderRadius: 8,
    padding: 10,
    alignItems: 'center',
  },
  secondaryButtonText: {
    color: '#E53935',
    fontWeight: '600',
    fontSize: 14,
  },
  testResult: {
    marginTop: 8,
    fontSize: 13,
    color: '#43A047',
  },
  testResultError: {
    color: '#E53935',
  },
  qualityGroup: {
    flexDirection: 'row',
    gap: 8,
    flexWrap: 'wrap',
  },
  qualityOption: {
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    backgroundColor: '#fafafa',
  },
  qualityOptionActive: {
    borderColor: '#E53935',
    backgroundColor: '#FFF3F3',
  },
  qualityOptionText: {
    fontSize: 13,
    color: '#555',
  },
  qualityOptionTextActive: {
    color: '#E53935',
    fontWeight: '600',
  },
  deviceId: {
    fontFamily: 'monospace',
    fontSize: 12,
    color: '#888',
    marginTop: 4,
  },
  saveButton: {
    marginTop: 32,
    backgroundColor: '#E53935',
    borderRadius: 10,
    padding: 14,
    alignItems: 'center',
  },
  saveButtonBusy: {
    opacity: 0.6,
  },
  saveButtonText: {
    color: '#fff',
    fontSize: 16,
    fontWeight: '700',
  },
});
