/**
 * Recordings store — lightweight client-side cache of recording list data.
 *
 * This is intentionally not a full sync engine.  It:
 *   - Holds the last fetched list of recordings (offline-first fallback).
 *   - Merges optimistic local state (a recording that was just created shows
 *     up immediately with status "uploading" before the server confirms it).
 *   - Surfaces per-recording upload progress from the upload queue.
 *
 * Persistence: the list is cached in AsyncStorage so the Library screen is
 * not blank on first render when offline.
 */

import { create } from 'zustand';
import AsyncStorage from '@react-native-async-storage/async-storage';
import type { Recording } from '../types';

const CACHE_KEY = 'scribe_recordings_cache';

interface RecordingsState {
  recordings: Recording[];
  /** Per-recording upload progress: recordingId → 0..1 */
  uploadProgress: Record<string, number>;
  hydrated: boolean;

  hydrate: () => Promise<void>;
  setRecordings: (recordings: Recording[]) => Promise<void>;
  upsertRecording: (recording: Recording) => void;
  setUploadProgress: (recordingId: string, progress: number) => void;
  getRecording: (id: string) => Recording | undefined;
}

export const useRecordingsStore = create<RecordingsState>((set, get) => ({
  recordings: [],
  uploadProgress: {},
  hydrated: false,

  hydrate: async () => {
    try {
      const stored = await AsyncStorage.getItem(CACHE_KEY);
      if (stored) {
        set({ recordings: JSON.parse(stored), hydrated: true });
      } else {
        set({ hydrated: true });
      }
    } catch {
      set({ hydrated: true });
    }
  },

  setRecordings: async (recordings: Recording[]) => {
    set({ recordings });
    try {
      await AsyncStorage.setItem(CACHE_KEY, JSON.stringify(recordings));
    } catch {
      // Cache write failure is non-fatal
    }
  },

  upsertRecording: (recording: Recording) => {
    const existing = get().recordings;
    const idx = existing.findIndex((r) => r.id === recording.id);
    const next =
      idx >= 0
        ? existing.map((r) => (r.id === recording.id ? recording : r))
        : [recording, ...existing];
    get().setRecordings(next);
  },

  setUploadProgress: (recordingId: string, progress: number) => {
    set((s) => ({
      uploadProgress: { ...s.uploadProgress, [recordingId]: progress },
    }));
  },

  getRecording: (id: string) => get().recordings.find((r) => r.id === id),
}));
