/**
 * Screen 2: Library — Kiln list of recordings.
 *
 * Each row carries a miniature of the recording's ember vortex (varied per
 * recording so no two look alike), a mono date · duration line, speaker dots,
 * and a status chip (RDY / PROC / FAIL).  Offline-first: cached rows show
 * immediately while the list refetches in the background.
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  View,
  Text,
  FlatList,
  StyleSheet,
  TouchableOpacity,
  RefreshControl,
  ActivityIndicator,
  Alert,
  ScrollView,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useRouter, useFocusEffect } from 'expo-router';
import * as DocumentPicker from 'expo-document-picker';
import { useRecordingsStore } from '../../src/state/recordingsStore';
import { useSettingsStore } from '../../src/state/settingsStore';
import { api } from '../../src/api/client';
import type { Recording } from '../../src/types';
import { EmberOrb, seedFromString } from '../../src/components/EmberOrb';
import { Screen, ScreenTitle, MonoCount, StatusChip } from '../../src/components/ui';
import { colors, mono, radius, speakerColors } from '../../src/theme';
import { log } from '../../src/util/logger';

// ---------------------------------------------------------------------------

function formatDate(iso: string): string {
  return new Date(iso)
    .toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
    .toUpperCase();
}

function formatDuration(ms: number | null): string {
  if (ms == null) return '';
  const totalSecs = Math.floor(ms / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

/** Best-effort Content-Type from a file extension (ffmpeg keys off the ext). */
function mimeForExt(ext: string): string {
  switch (ext) {
    case 'mp3':
      return 'audio/mpeg';
    case 'm4a':
    case 'mp4':
      return 'audio/mp4';
    case 'aac':
      return 'audio/aac';
    case 'wav':
      return 'audio/wav';
    case 'ogg':
    case 'oga':
      return 'audio/ogg';
    case 'flac':
      return 'audio/flac';
    case 'webm':
      return 'audio/webm';
    case 'mov':
      return 'video/quicktime';
    case 'mkv':
      return 'video/x-matroska';
    case 'm4v':
      return 'video/x-m4v';
    default:
      return 'application/octet-stream';
  }
}

// ---------------------------------------------------------------------------

function RecordingRow({
  recording,
  index,
  onPress,
}: {
  recording: Recording;
  index: number;
  onPress: () => void;
}) {
  const dotCount = Math.min(recording.participants_expected ?? 0, 3);
  const extra = (recording.participants_expected ?? 0) - dotCount;

  return (
    <TouchableOpacity style={styles.row} onPress={onPress} accessibilityRole="button">
      <View style={styles.thumb}>
        <EmberOrb size={54} variant="mini" seed={seedFromString(recording.id)} dir={index % 2 ? -1 : 1} />
      </View>

      <View style={styles.rowBody}>
        <Text style={styles.rowTitle} numberOfLines={1}>
          {recording.title ?? 'Untitled recording'}
        </Text>
        <Text style={styles.rowMeta}>
          {formatDate(recording.created_at)}
          {recording.duration_ms != null ? ` · ${formatDuration(recording.duration_ms)}` : ''}
        </Text>
        {dotCount > 0 && (
          <View style={styles.dots}>
            {Array.from({ length: dotCount }).map((_, i) => (
              <View key={i} style={[styles.dot, { backgroundColor: speakerColors[i % speakerColors.length] }]} />
            ))}
            {extra > 0 && <Text style={styles.dotsExtra}>+{extra}</Text>}
          </View>
        )}
      </View>

      <StatusChip status={recording.status} />
    </TouchableOpacity>
  );
}

// ---------------------------------------------------------------------------

function isPending(r: Recording): boolean {
  return r.status === 'uploading' || r.status === 'processing';
}

/** A single tag-filter pill. */
function FilterChip({
  label,
  active,
  onPress,
}: {
  label: string;
  active: boolean;
  onPress: () => void;
}) {
  return (
    <TouchableOpacity
      style={[styles.chip, active ? styles.chipActive : styles.chipInactive]}
      onPress={onPress}
      accessibilityRole="button"
      accessibilityState={{ selected: active }}
    >
      <Text style={[styles.chipText, active && styles.chipTextActive]} numberOfLines={1}>
        {label}
      </Text>
    </TouchableOpacity>
  );
}

export default function LibraryScreen() {
  const router = useRouter();
  const { baseUrl, deviceId } = useSettingsStore();
  const { recordings, hydrated, refresh } = useRecordingsStore();
  const [importing, setImporting] = useState(false);
  const [selectedTag, setSelectedTag] = useState<string | null>(null);

  // Tag filter is derived from the cached recordings (offline-friendly).
  const allTags = useMemo(() => {
    const s = new Set<string>();
    recordings.forEach((r) => (r.tags ?? []).forEach((t) => s.add(t)));
    return Array.from(s).sort();
  }, [recordings]);
  // Guard against a stale selection that no longer exists after a refresh.
  const effectiveTag = selectedTag && allTags.includes(selectedTag) ? selectedTag : null;
  const visible = useMemo(
    () => (effectiveTag ? recordings.filter((r) => (r.tags ?? []).includes(effectiveTag)) : recordings),
    [recordings, effectiveTag],
  );

  const fetchRecordings = useCallback(async () => {
    if (!baseUrl) return;
    await refresh();
  }, [baseUrl, refresh]);

  // Import an existing audio file: pick -> create -> upload as segment 0 ->
  // complete, then open the new recording (it transcribes like any capture).
  const handleImport = useCallback(async () => {
    if (!baseUrl) {
      Alert.alert('Not configured', 'Set your Scribe server URL in Settings first.');
      return;
    }
    try {
      // Video containers too: meeting recordings from Zoom/Teams/Meet arrive as
      // .mp4/.mov, and the server's transcode stage pulls the audio track out
      // with ffmpeg just the same.
      const picked = await DocumentPicker.getDocumentAsync({
        type: ['audio/*', 'video/*'],
        copyToCacheDirectory: true,
        multiple: false,
      });
      if (picked.canceled) return;
      const asset = picked.assets[0];
      if (!asset) return;
      const name = asset.name ?? 'Imported audio';
      const dot = name.lastIndexOf('.');
      const ext = (dot > 0 ? name.slice(dot + 1) : 'm4a').toLowerCase();
      const title = dot > 0 ? name.slice(0, dot) : name;
      const contentType = asset.mimeType || mimeForExt(ext);

      setImporting(true);
      log('import', `importing "${name}" (${ext}, ${asset.size ?? '?'} bytes)`);
      const created = await api.createRecording({
        title,
        audio_format: ext,
        device_id: deviceId || undefined,
      });
      await api.importAudioSegment({ recordingId: created.id, fileUri: asset.uri, ext, contentType });
      await api.completeRecording(created.id);
      log('import', `imported -> ${created.id}`);
      await refresh();
      router.push(`/recordings/${created.id}`);
    } catch (err) {
      log('import', 'failed', err);
      Alert.alert('Import failed', err instanceof Error ? err.message : String(err));
    } finally {
      setImporting(false);
    }
  }, [baseUrl, deviceId, refresh, router]);

  useFocusEffect(
    useCallback(() => {
      fetchRecordings();
    }, [fetchRecordings]),
  );

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
      <Screen>
        <View style={styles.centered}>
          <ActivityIndicator color={colors.accent} />
        </View>
      </Screen>
    );
  }

  if (recordings.length === 0) {
    return (
      <Screen>
        <ScreenTitle title="Library" />
        <View style={styles.centered}>
          <View style={styles.emptyGlow}>
            <Ionicons name="mic-outline" size={34} color={colors.accent} />
          </View>
          <Text style={styles.emptyTitle}>No recordings yet</Text>
          <Text style={styles.emptyHint}>
            {baseUrl
              ? 'Your captured meetings will appear here — transcribed, diarized and fully searchable. Tap Record to start your first.'
              : 'Configure the server URL in Settings, then tap Record to capture your first meeting.'}
          </Text>
          {baseUrl && (
            <TouchableOpacity style={styles.cta} onPress={() => router.replace('/')}>
              <View style={styles.ctaDot} />
              <Text style={styles.ctaText}>Start recording</Text>
            </TouchableOpacity>
          )}
          {baseUrl && (
            <TouchableOpacity onPress={handleImport} disabled={importing} style={styles.importLink}>
              {importing ? (
                <ActivityIndicator size="small" color={colors.accent} />
              ) : (
                <Text style={styles.importLinkText}>or import an audio or video file</Text>
              )}
            </TouchableOpacity>
          )}
        </View>
      </Screen>
    );
  }

  return (
    <Screen>
      <ScreenTitle
        title="Library"
        right={
          <View style={styles.headerRight}>
            <TouchableOpacity
              onPress={handleImport}
              disabled={importing}
              accessibilityLabel="Import an audio or video file"
              style={styles.importBtn}
            >
              {importing ? (
                <ActivityIndicator size="small" color={colors.accent} />
              ) : (
                <Ionicons name="add-circle-outline" size={22} color={colors.accent} />
              )}
            </TouchableOpacity>
            <MonoCount>{recordings.length} REC</MonoCount>
          </View>
        }
      />
      {allTags.length > 0 && (
        <ScrollView
          horizontal
          showsHorizontalScrollIndicator={false}
          style={styles.filterScroll}
          contentContainerStyle={styles.filterRow}
        >
          <FilterChip label="All" active={!effectiveTag} onPress={() => setSelectedTag(null)} />
          {allTags.map((t) => (
            <FilterChip
              key={t}
              label={t}
              active={effectiveTag === t}
              onPress={() => setSelectedTag(t)}
            />
          ))}
        </ScrollView>
      )}
      <FlatList
        data={visible}
        keyExtractor={(r) => r.id}
        contentContainerStyle={styles.list}
        refreshControl={
          <RefreshControl refreshing={false} onRefresh={fetchRecordings} tintColor={colors.accent} />
        }
        renderItem={({ item, index }) => (
          <RecordingRow
            recording={item}
            index={index}
            onPress={() => router.push(`/recordings/${item.id}`)}
          />
        )}
        ListEmptyComponent={
          effectiveTag ? (
            <Text style={styles.filterEmpty}>No recordings tagged “{effectiveTag}”.</Text>
          ) : null
        }
      />
    </Screen>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  centered: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 34,
    gap: 16,
  },
  emptyGlow: {
    width: 84,
    height: 84,
    borderRadius: 42,
    backgroundColor: colors.accentFaint,
    alignItems: 'center',
    justifyContent: 'center',
  },
  emptyTitle: {
    fontSize: 17,
    fontWeight: '700',
    color: colors.textPrimary,
  },
  emptyHint: {
    fontSize: 13,
    lineHeight: 20,
    color: colors.textMuted,
    textAlign: 'center',
  },
  cta: {
    marginTop: 6,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    backgroundColor: colors.accent,
    paddingHorizontal: 18,
    paddingVertical: 11,
    borderRadius: 22,
  },
  ctaDot: {
    width: 9,
    height: 9,
    borderRadius: 5,
    backgroundColor: colors.white,
  },
  ctaText: {
    color: colors.white,
    fontSize: 13,
    fontWeight: '600',
  },
  importLink: {
    marginTop: 4,
    paddingVertical: 8,
    paddingHorizontal: 12,
    minHeight: 20,
    justifyContent: 'center',
  },
  importLinkText: {
    color: colors.accent,
    fontSize: 13,
    fontWeight: '500',
  },
  headerRight: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
  },
  importBtn: {
    padding: 2,
  },
  filterScroll: {
    flexGrow: 0,
    maxHeight: 44,
  },
  filterRow: {
    paddingHorizontal: 18,
    paddingVertical: 6,
    gap: 8,
    alignItems: 'center',
  },
  chip: {
    paddingHorizontal: 13,
    paddingVertical: 6,
    borderRadius: 14,
    borderWidth: 1,
    maxWidth: 180,
  },
  chipActive: {
    backgroundColor: colors.accentSoft,
    borderColor: colors.chipActiveBorder,
  },
  chipInactive: {
    backgroundColor: colors.surface,
    borderColor: colors.borderInput,
  },
  chipText: {
    fontSize: 12.5,
    fontWeight: '500',
    color: colors.textMuted,
  },
  chipTextActive: {
    color: colors.accent,
    fontWeight: '600',
  },
  filterEmpty: {
    fontSize: 14,
    color: colors.textDim,
    textAlign: 'center',
    marginTop: 32,
  },
  list: {
    paddingHorizontal: 18,
    paddingTop: 4,
    paddingBottom: 24,
    gap: 11,
  },
  row: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.card,
    padding: 13,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 13,
  },
  thumb: {
    width: 54,
    height: 54,
    borderRadius: radius.thumb,
    overflow: 'hidden',
    backgroundColor: colors.accentFaint,
  },
  rowBody: {
    flex: 1,
    minWidth: 0,
  },
  rowTitle: {
    fontSize: 15,
    fontWeight: '600',
    color: colors.textPrimary,
  },
  rowMeta: {
    fontSize: 11,
    fontWeight: '500',
    color: colors.textMuted,
    fontFamily: mono,
    marginTop: 4,
  },
  dots: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    marginTop: 8,
  },
  dot: {
    width: 9,
    height: 9,
    borderRadius: 5,
  },
  dotsExtra: {
    fontSize: 10,
    color: colors.textMuted,
    marginLeft: 2,
  },
});
