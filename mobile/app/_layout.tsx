import { useEffect } from 'react';
import { Stack } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import { useSettingsStore } from '../src/state/settingsStore';
import { useRecordingsStore } from '../src/state/recordingsStore';
import { uploadQueue } from '../src/recording/uploadQueue';

export default function RootLayout() {
  const hydrateSettings = useSettingsStore((s) => s.hydrate);
  const hydrateRecordings = useRecordingsStore((s) => s.hydrate);

  useEffect(() => {
    // Hydrate persisted state and restore the upload queue on startup
    Promise.all([
      hydrateSettings(),
      hydrateRecordings(),
      uploadQueue.hydrate(),
    ]).catch(console.warn);
  }, [hydrateSettings, hydrateRecordings]);

  return (
    <>
      <StatusBar style="auto" />
      <Stack>
        <Stack.Screen name="(tabs)" options={{ headerShown: false }} />
        <Stack.Screen
          name="recordings/[id]"
          options={{ title: 'Recording', headerBackTitle: 'Back' }}
        />
        <Stack.Screen
          name="admin/update"
          options={{ title: 'Backend Update', headerBackTitle: 'Settings' }}
        />
      </Stack>
    </>
  );
}
