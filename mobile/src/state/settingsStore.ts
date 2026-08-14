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
const SECURE_KEY_UPDATE_TOKEN = 'scribe_update_token';

const DEFAULT_SETTINGS: AppSettings = {
  baseUrl: '',
  deviceKey: '',
  deviceId: '',
  audioQuality: 'medium',
  defaultParticipants: 2,
  updateToken: '',
  reduceMotion: false,
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
      const [stored, deviceKey, updateToken] = await Promise.all([
        AsyncStorage.getItem(STORAGE_KEY),
        SecureStore.getItemAsync(SECURE_KEY_DEVICE_KEY),
        SecureStore.getItemAsync(SECURE_KEY_UPDATE_TOKEN),
      ]);

      const parsed: Partial<AppSettings> = stored ? JSON.parse(stored) : {};
      set({
        ...DEFAULT_SETTINGS,
        ...parsed,
        deviceKey: deviceKey ?? '',
        updateToken: updateToken ?? '',
        hydrated: true,
      });
    } catch {
      set({ hydrated: true });
    }
  },

  update: async (patch: Partial<AppSettings>) => {
    const current = get();
    const next: AppSettings = { ...current, ...patch };

    // Persist deviceKey and updateToken securely; store the rest as plain JSON
    const { deviceKey, updateToken, ...rest } = next;

    await Promise.all([
      AsyncStorage.setItem(
        STORAGE_KEY,
        JSON.stringify(rest),
      ),
      SecureStore.setItemAsync(SECURE_KEY_DEVICE_KEY, deviceKey),
      SecureStore.setItemAsync(SECURE_KEY_UPDATE_TOKEN, updateToken),
    ]);

    set(next);
  },
}));
