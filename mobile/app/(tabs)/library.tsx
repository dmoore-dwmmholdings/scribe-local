/**
 * Screen 2: Library
 *
 * List of recordings with status (uploading/processing/ready/failed) and
 * per-recording upload progress.  Offline-first: shows cached data
 * immediately while refetching in the background.
 *
 * Tapping a row deep-links to the Recording Detail screen.
 */

import { useCallback, useEffect } from 'react';
import {
  View,
  Text,
  FlatList,
  StyleSheet,
  TouchableOpacity,
  RefreshControl,
  ActivityIndicator,
} from 'react-native';
import { useRouter, useFocusEffect } from 'expo-router';
import { useRecordingsStore } from '../../src/state/recordingsStore';
import { useSettingsStore } from '../../src/state/settingsStore';
import type { Recording } from '../../src/types';

// ---------------------------------------------------------------------------

function formatDate(iso: string): string {
  return new Date(iso).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function formatDuration(ms: number | null): string {
  if (ms == null) return '';
  const totalSecs = Math.floor(ms / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

const STATUS_COLORS: Record<string, string> = {
  uploading: '#FB8C00',
  processing: '#1E88E5',
  ready: '#43A047',
  failed: '#E53935',
};

// ---------------------------------------------------------------------------

function RecordingRow({
  recording,
  uploadProgress,
  onPress,
}: {
  recording: Recording;
  uploadProgress: number;
  onPress: () => void;
}) {
  const color = STATUS_COLORS[recording.status] ?? '#888';

  return (
    <TouchableOpacity style={styles.row} onPress={onPress} accessibilityRole="button">
      <View style={styles.rowHeader}>
        <Text style={styles.rowTitle} numberOfLines={1}>
          {recording.title ?? 'Untitled recording'}
        </Text>
        <Text style={[styles.rowStatus, { color }]}>{recording.status}</Text>
      </View>
      <Text style={styles.rowMeta}>
        {formatDate(recording.created_at)}
        {recording.duration_ms != null
          ? `  ·  ${formatDuration(recording.duration_ms)}`
          : ''}
        {recording.participants_expected != null
          ? `  ·  ${recording.participants_expected} participants`
          : ''}
      </Text>

      {/* Upload progress bar (only visible while uploading) */}
      {recording.status === 'uploading' && uploadProgress < 1 && (
        <View style={styles.progressTrack}>
          <View
            style={[styles.progressFill, { width: `${Math.round(uploadProgress * 100)}%` }]}
          />
        </View>
      )}
    </TouchableOpacity>
  );
}

// ---------------------------------------------------------------------------

function isPending(r: Recording): boolean {
  return r.status === 'uploading' || r.status === 'processing';
}

export default function LibraryScreen() {
  const router = useRouter();
  const { baseUrl } = useSettingsStore();
  const { recordings, uploadProgress, hydrated, refresh } =
    useRecordingsStore();

  const fetchRecordings = useCallback(async () => {
    if (!baseUrl) return;
    await refresh();
  }, [baseUrl, refresh]);

  // Refresh whenever the Library screen regains focus.
  useFocusEffect(
    useCallback(() => {
      fetchRecordings();
    }, [fetchRecordings]),
  );

  // While any recording is still uploading/processing, poll the list until
  // they all reach a terminal status.  Stops polling once everything settles.
  const hasPending = recordings.some(isPending);
  useEffect(() => {
    if (!baseUrl || !hasPending) return;

    const interval = setInterval(() => {
      fetchRecordings();
    }, 5000);

    return () => clearInterval(interval);
  }, [baseUrl, hasPending, fetchRecordings]);

  if (!hydrated) {
    return (
      <View style={styles.centered}>
        <ActivityIndicator />
      </View>
    );
  }

  if (recordings.length === 0) {
    return (
      <View style={styles.centered}>
        <Text style={styles.empty}>No recordings yet.</Text>
        <Text style={styles.emptyHint}>
          {baseUrl
            ? 'Tap "Record" to capture your first meeting.'
            : 'Configure the server URL in Settings first.'}
        </Text>
      </View>
    );
  }

  return (
    <FlatList
      data={recordings}
      keyExtractor={(r) => r.id}
      contentContainerStyle={styles.list}
      refreshControl={
        <RefreshControl
          refreshing={false}
          onRefresh={fetchRecordings}
          tintColor="#E53935"
        />
      }
      renderItem={({ item }) => (
        <RecordingRow
          recording={item}
          uploadProgress={uploadProgress[item.id] ?? (item.status === 'ready' ? 1 : 0)}
          onPress={() => router.push(`/recordings/${item.id}`)}
        />
      )}
    />
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  centered: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 32,
  },
  empty: {
    fontSize: 16,
    color: '#555',
    marginBottom: 8,
  },
  emptyHint: {
    fontSize: 13,
    color: '#aaa',
    textAlign: 'center',
  },
  list: {
    padding: 16,
    gap: 12,
  },
  row: {
    backgroundColor: '#fff',
    borderRadius: 12,
    padding: 16,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.06,
    shadowRadius: 4,
    elevation: 2,
  },
  rowHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginBottom: 4,
  },
  rowTitle: {
    fontSize: 16,
    fontWeight: '600',
    color: '#111',
    flex: 1,
    marginRight: 8,
  },
  rowStatus: {
    fontSize: 12,
    fontWeight: '500',
    textTransform: 'capitalize',
  },
  rowMeta: {
    fontSize: 13,
    color: '#888',
    marginBottom: 8,
  },
  progressTrack: {
    height: 3,
    backgroundColor: '#eee',
    borderRadius: 2,
    overflow: 'hidden',
  },
  progressFill: {
    height: 3,
    backgroundColor: '#FB8C00',
    borderRadius: 2,
  },
});
