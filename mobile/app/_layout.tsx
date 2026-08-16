import '../src/util/polyfills'; // crypto.getRandomValues + randomUUID — must be first
import { installCrashLogging, log } from '../src/util/logger';
installCrashLogging(); // install global JS error handler before anything renders
import { useEffect } from 'react';
import { Stack } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import { useSettingsStore } from '../src/state/settingsStore';
import { useRecordingsStore } from '../src/state/recordingsStore';
import { uploadQueue } from '../src/recording/uploadQueue';
import { colors } from '../src/theme';
import { ErrorBoundary } from '../src/components/ErrorBoundary';

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
  }, [hydrateSettings, hydrateRecordings]);

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
      </Stack>
    </ErrorBoundary>
  );
}
