import '../src/util/polyfills'; // crypto.getRandomValues + randomUUID — must be first
import { installCrashLogging, log } from '../src/util/logger';
installCrashLogging(); // install global JS error handler before anything renders
import { useEffect } from 'react';
import { Alert } from 'react-native';
import * as Linking from 'expo-linking';
import { Stack } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import { useSettingsStore } from '../src/state/settingsStore';
import { parsePairingLink, describePairing } from '../src/state/pairing';
import { useRecordingsStore } from '../src/state/recordingsStore';
import { uploadQueue } from '../src/recording/uploadQueue';
import { colors } from '../src/theme';
import { ErrorBoundary } from '../src/components/ErrorBoundary';
import { endAllLiveActivities } from '../modules/scribe-live-activity';

export default function RootLayout() {
  const hydrateSettings = useSettingsStore((s) => s.hydrate);
  const hydrateRecordings = useRecordingsStore((s) => s.hydrate);

  useEffect(() => {
    log('app', 'RootLayout mounted; hydrating');
    // Hydrate persisted state and restore the upload queue on startup
    Promise.all([
      hydrateSettings(),
      hydrateRecordings(),
      uploadQueue.hydrate(),
    ])
      .then(() => log('app', 'hydrate complete'))
      .catch((e) => log('app', 'hydrate failed', e));

    // Clear Live Activities stranded by a previous run. ActivityKit activities
    // survive app termination and reboot, so a crash mid-recording otherwise
    // leaves the Lock Screen insisting a recording is still going, with no way
    // for the user to dismiss it. Nothing can legitimately be recording at
    // startup, so anything still present is stale by definition.
    void endAllLiveActivities().then((n) => {
      if (n > 0) log('app', `cleared ${n} stale live activity/activities`);
    });
  }, [hydrateSettings, hydrateRecordings]);

  // Pairing deep links (scribe://pair?url=…&key=…), handled here rather than on
  // the Settings screen so a scanned QR works whatever the app has open — and
  // works on a cold start, which is the usual case for a first pairing.
  useEffect(() => {
    const apply = async (link: string) => {
      const payload = parsePairingLink(link);
      if (!payload) return; // not a pairing link, or malformed — ignore quietly
      log('app', `pairing link for ${payload.baseUrl}`);

      // Confirm before saving. Anything that can open a URL can offer a pairing
      // link, and accepting one silently would repoint every future recording
      // at a server the user never chose.
      const accept = await new Promise<boolean>((resolve) => {
        Alert.alert('Connect to this server?', describePairing(payload), [
          { text: 'Cancel', style: 'cancel', onPress: () => resolve(false) },
          { text: 'Connect', onPress: () => resolve(true) },
        ]);
      });
      if (!accept) return;

      // Settings hydrate concurrently on startup; awaiting that first stops a
      // late hydrate from overwriting what we just paired.
      await hydrateSettings();
      await useSettingsStore.getState().update({
        baseUrl: payload.baseUrl,
        deviceKey: payload.deviceKey,
      });
      log('app', 'pairing saved');
    };

    // A cold start delivers the link here...
    void Linking.getInitialURL().then((url) => {
      if (url) void apply(url);
    });
    // ...and a warm one here.
    const sub = Linking.addEventListener('url', ({ url }) => void apply(url));
    return () => sub.remove();
  }, [hydrateSettings]);

  return (
    <ErrorBoundary>
      <StatusBar style="light" />
      <Stack
        screenOptions={{
          contentStyle: { backgroundColor: colors.bg },
          headerStyle: { backgroundColor: colors.bg },
          headerTintColor: colors.accent,
          headerTitleStyle: { color: colors.textPrimary, fontWeight: '700' },
          headerShadowVisible: false,
        }}
      >
        <Stack.Screen name="(tabs)" options={{ headerShown: false }} />
        {/* Every pushed screen renders its own Kiln header (back chevron +
            title) instead of the native stack header — one back control, in one
            place, with a hit area we control. */}
        <Stack.Screen name="recordings/[id]" options={{ headerShown: false }} />
        <Stack.Screen name="speakers" options={{ headerShown: false }} />
        <Stack.Screen name="admin/update" options={{ headerShown: false }} />
        <Stack.Screen name="admin/logs" options={{ headerShown: false }} />
        <Stack.Screen name="admin/schedule" options={{ headerShown: false }} />
      </Stack>
    </ErrorBoundary>
  );
}
