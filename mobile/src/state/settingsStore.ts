/**
 * Settings store — persists user configuration to SecureStore / AsyncStorage.
 *
 * Sensitive values (deviceKey) go to expo-secure-store; non-sensitive
 * settings go to @react-native-async-storage/async-storage.
 *
 * The store is a Zustand store that hydrates from storage on first access.
 */

import { create } from 'zustand';
import * as SecureStore from 'expo-secure-store';
import AsyncStorage from '@react-native-async-storage/async-storage';
import type { AppSettings } from '../types';

const STORAGE_KEY = 'scribe_settings';
const SECURE_KEY_DEVICE_KEY = 'scribe_device_key';

const DEFAULT_SETTINGS: AppSettings = {
  baseUrl: '',
  deviceKey: '',
  deviceId: '',
  audioQuality: 'medium',
  defaultParticipants: 2,
};

interface SettingsState extends AppSettings {
  hydrated: boolean;
  hydrate: () => Promise<void>;
  update: (patch: Partial<AppSettings>) => Promise<void>;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  ...DEFAULT_SETTINGS,
  hydrated: false,

  hydrate: async () => {
    try {
      const [stored, deviceKey] = await Promise.all([
        AsyncStorage.getItem(STORAGE_KEY),
        SecureStore.getItemAsync(SECURE_KEY_DEVICE_KEY),
      ]);

      const parsed: Partial<AppSettings> = stored ? JSON.parse(stored) : {};
      set({
        ...DEFAULT_SETTINGS,
        ...parsed,
        deviceKey: deviceKey ?? '',
        hydrated: true,
      });
    } catch {
      set({ hydrated: true });
    }
  },

  update: async (patch: Partial<AppSettings>) => {
    const current = get();
    const next: AppSettings = { ...current, ...patch };

    // Persist deviceKey securely; store the rest as plain JSON
    const { deviceKey, ...rest } = next;

    await Promise.all([
      AsyncStorage.setItem(
        STORAGE_KEY,
        JSON.stringify(rest),
      ),
      SecureStore.setItemAsync(SECURE_KEY_DEVICE_KEY, deviceKey),
    ]);

    set(next);
  },
}));
