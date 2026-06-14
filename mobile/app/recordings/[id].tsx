/**
 * Screen 3: Recording Detail
 *
 * Speaker-labelled transcript, tap-a-line to play that moment via the audio
 * endpoint with HTTP Range seeking, LLM summary + action items, in-recording
 * search, and inline speaker-naming (calls the name endpoint).
 *
 * Design §11: "speaker-labelled transcript (tap a line → play that moment via
 * the audio endpoint with range/seek), LLM summary + action items, in-
 * recording search, and inline speaker-naming (calls the name endpoint)."
 *
 * Audio playback:
 *   expo-audio's createAudioPlayer() is used with the recording's audio URL
 *   (GET /recordings/{id}/audio).  The server supports HTTP Range requests,
 *   which the player uses automatically for seeking.  Bearer auth is sent via
 *   the `headers` field of the AudioSource object.
 *
 *   Seeking to a moment: tapping a transcript line calls
 *   player.seekTo(startMs / 1000) to jump directly to that utterance.
 */

import { useEffect, useState, useCallback, useRef } from 'react';
import {
  View,
  Text,
  StyleSheet,
  ScrollView,
  TouchableOpacity,
  TextInput,
  Modal,
  ActivityIndicator,
  Alert,
} from 'react-native';
import { useLocalSearchParams, useNavigation } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';
import {
  createAudioPlayer,
  setAudioModeAsync,
  useAudioPlayerStatus,
  type AudioPlayer,
} from 'expo-audio';
import { api } from '../../src/api/client';
import type {
  RecordingDetailResponse,
  Utterance,
  RecordingSpeaker,
} from '../../src/types';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatMs(ms: number): string {
  const totalSecs = Math.floor(ms / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

function speakerLabel(
  localIdx: number | null,
  speakers: RecordingSpeaker[],
): string {
  if (localIdx == null) return 'Unknown';
  const sp = speakers.find((s) => s.local_idx === localIdx);
  return sp?.display_name ?? `Speaker ${localIdx}`;
}

// A small palette for speaker colours
const SPEAKER_COLORS = [
  '#1E88E5',
  '#43A047',
  '#FB8C00',
  '#8E24AA',
  '#E53935',
  '#00ACC1',
];

function speakerColor(localIdx: number | null): string {
  if (localIdx == null) return '#888';
  return SPEAKER_COLORS[localIdx % SPEAKER_COLORS.length];
}

// ---------------------------------------------------------------------------
// Player status hook wrapper
// ---------------------------------------------------------------------------

function PlayerStatus({ player, onStatus }: {
  player: AudioPlayer;
  onStatus: (posMs: number, playing: boolean) => void;
}) {
  const status = useAudioPlayerStatus(player);
  useEffect(() => {
    onStatus(
      Math.round((status.currentTime ?? 0) * 1000),
      status.playing ?? false,
    );
  }, [status, onStatus]);
  return null;
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

function UtteranceRow({
  utterance,
  speakers,
  isActive,
  matchesSearch,
  onPress,
  onLongPress,
}: {
  utterance: Utterance;
  speakers: RecordingSpeaker[];
  isActive: boolean;
  matchesSearch: boolean;
  onPress: () => void;
  onLongPress: () => void;
}) {
  const label = utterance.speaker_name ?? speakerLabel(utterance.local_idx, speakers);
  const color = speakerColor(utterance.local_idx);

  return (
    <TouchableOpacity
      style={[
        styles.utterance,
        isActive && styles.utteranceActive,
        matchesSearch && styles.utteranceMatch,
      ]}
      onPress={onPress}
      onLongPress={onLongPress}
      accessibilityRole="button"
      accessibilityLabel={`${label}: ${utterance.text}`}
    >
      <View style={styles.utteranceHeader}>
        <Text style={[styles.speakerName, { color }]}>{label}</Text>
        <Text style={styles.utteranceTime}>{formatMs(utterance.start_ms)}</Text>
      </View>
      <Text style={styles.utteranceText}>{utterance.text}</Text>
    </TouchableOpacity>
  );
}

// ---------------------------------------------------------------------------
// Speaker name modal
// ---------------------------------------------------------------------------

function NameSpeakerModal({
  visible,
  currentName,
  onSave,
  onDismiss,
}: {
  visible: boolean;
  currentName: string;
  onSave: (name: string, enroll: boolean) => void;
  onDismiss: () => void;
}) {
  const [name, setName] = useState(currentName);
  const [enroll, setEnroll] = useState(true);

  useEffect(() => {
    if (visible) setName(currentName);
  }, [visible, currentName]);

  return (
    <Modal visible={visible} transparent animationType="fade" onRequestClose={onDismiss}>
      <View style={styles.modalOverlay}>
        <View style={styles.modalCard}>
          <Text style={styles.modalTitle}>Name speaker</Text>
          <TextInput
            style={styles.modalInput}
            value={name}
            onChangeText={setName}
            placeholder="e.g. Dawson"
            autoFocus
            returnKeyType="done"
          />
          <TouchableOpacity
            style={styles.modalEnroll}
            onPress={() => setEnroll((e) => !e)}
          >
            <View style={[styles.checkbox, enroll && styles.checkboxChecked]} />
            <Text style={styles.enrollText}>
              Enroll voice (recognise in future recordings)
            </Text>
          </TouchableOpacity>
          <View style={styles.modalActions}>
            <TouchableOpacity style={styles.modalCancel} onPress={onDismiss}>
              <Text style={styles.modalCancelText}>Cancel</Text>
            </TouchableOpacity>
            <TouchableOpacity
              style={styles.modalSave}
              onPress={() => onSave(name.trim(), enroll)}
              disabled={!name.trim()}
            >
              <Text style={styles.modalSaveText}>Save</Text>
            </TouchableOpacity>
          </View>
        </View>
      </View>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Main screen
// ---------------------------------------------------------------------------

export default function RecordingDetailScreen() {
  const { id, seekMs } = useLocalSearchParams<{ id: string; seekMs?: string }>();
  const navigation = useNavigation();

  const [detail, setDetail] = useState<RecordingDetailResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Audio playback — expo-audio v0.4 direct player
  const playerRef = useRef<AudioPlayer | null>(null);
  const [positionMs, setPositionMs] = useState(0);
  const [playing, setPlaying] = useState(false);

  // In-recording search
  const [searchQuery, setSearchQuery] = useState('');

  // Speaker naming
  const [namingUtterance, setNamingUtterance] = useState<Utterance | null>(null);

  // -------------------------------------------------------------------------
  // Load detail
  // -------------------------------------------------------------------------

  const loadDetail = useCallback(
    async (background = false) => {
      if (!id) return;
      if (!background) setLoading(true);
      setError(null);
      try {
        const res = await api.getRecording(id);
        setDetail(res);
        navigation.setOptions({
          title: res.recording.title ?? 'Recording',
        });
      } catch (err) {
        // On a background refresh, keep the stale screen rather than wiping it
        if (!background) {
          setError(err instanceof Error ? err.message : 'Failed to load recording');
        }
      } finally {
        if (!background) setLoading(false);
      }
    },
    [id, navigation],
  );

  useEffect(() => {
    loadDetail();
  }, [loadDetail]);

  // While the recording is still uploading/processing, poll until it reaches a
  // terminal status (ready/failed).  Background refreshes don't show the
  // full-screen spinner.
  const status = detail?.recording.status;
  useEffect(() => {
    if (!id) return;
    if (status !== 'uploading' && status !== 'processing') return;

    const interval = setInterval(() => {
      loadDetail(true);
    }, 4000);

    return () => clearInterval(interval);
  }, [id, status, loadDetail]);

  // -------------------------------------------------------------------------
  // Audio
  // -------------------------------------------------------------------------

  const handleStatus = useCallback((posMs: number, isPlaying: boolean) => {
    setPositionMs(posMs);
    setPlaying(isPlaying);
  }, []);

  useEffect(() => {
    if (!id) return;

    const initAudio = async () => {
      try {
        await setAudioModeAsync({
          allowsRecording: false,
          playsInSilentMode: true,
          shouldPlayInBackground: false,
        });

        const player = createAudioPlayer({
          uri: api.audioUrl(id),
          headers: { Authorization: api.authHeader() },
        });

        playerRef.current = player;

        // If we arrived from search/ask with a seek target, jump there
        if (seekMs) {
          await player.seekTo(Number(seekMs) / 1000);
          player.play();
        }
      } catch {
        // Audio load failure is non-fatal — transcript still works
      }
    };

    initAudio();

    return () => {
      playerRef.current?.remove();
      playerRef.current = null;
    };
  }, [id, seekMs]);

  const seekTo = useCallback(async (ms: number) => {
    const player = playerRef.current;
    if (!player) return;
    try {
      await player.seekTo(ms / 1000);
      player.play();
    } catch {
      // Ignore seek errors
    }
  }, []);

  const togglePlayback = useCallback(() => {
    const player = playerRef.current;
    if (!player) return;
    if (playing) {
      player.pause();
    } else {
      player.play();
    }
  }, [playing]);

  // -------------------------------------------------------------------------
  // Speaker naming
  // -------------------------------------------------------------------------

  const handleNameSpeaker = useCallback(
    async (name: string, enroll: boolean) => {
      if (!namingUtterance || !id) return;
      const localIdx = namingUtterance.local_idx;
      if (localIdx == null) return;

      setNamingUtterance(null);
      try {
        await api.nameSpeaker(id, localIdx, { name, enroll });
        // Reload to pick up new display names
        await loadDetail();
      } catch (err) {
        Alert.alert('Error', `Could not name speaker: ${String(err)}`);
      }
    },
    [namingUtterance, id, loadDetail],
  );

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------

  if (loading) {
    return (
      <View style={styles.centered}>
        <ActivityIndicator color="#E53935" />
      </View>
    );
  }

  if (error || !detail) {
    return (
      <View style={styles.centered}>
        <Text style={styles.errorText}>{error ?? 'No data'}</Text>
        <TouchableOpacity style={styles.retryButton} onPress={() => loadDetail()}>
          <Text style={styles.retryButtonText}>Retry</Text>
        </TouchableOpacity>
      </View>
    );
  }

  const { recording, speakers, utterances, summary } = detail;

  const filteredUtterances = searchQuery.trim()
    ? utterances.filter((u) =>
        u.text.toLowerCase().includes(searchQuery.toLowerCase()),
      )
    : utterances;

  return (
    <>
      {/* Attach the status listener to the player without a visible element */}
      {playerRef.current && (
        <PlayerStatus player={playerRef.current} onStatus={handleStatus} />
      )}

      <ScrollView style={styles.container} contentContainerStyle={styles.content}>
        {/* Status badge */}
        {status !== 'ready' && (
          <View style={styles.statusBanner}>
            <Text style={styles.statusBannerText}>
              {status === 'uploading'
                ? 'Recording — transcribing live; speaker labels are assigned when you stop.'
                : status === 'processing'
                  ? 'Finalizing transcript and speaker labels…'
                  : 'Processing failed. Check server logs.'}
            </Text>
          </View>
        )}

        {/* Mini playback bar */}
        {status === 'ready' && (
          <View style={styles.playbackBar}>
            <TouchableOpacity
              style={styles.playButton}
              onPress={togglePlayback}
              accessibilityLabel={playing ? 'Pause' : 'Play'}
            >
              <Ionicons name={playing ? 'pause' : 'play'} size={18} color="#fff" />
            </TouchableOpacity>
            <Text style={styles.position}>{formatMs(positionMs)}</Text>
            {recording.duration_ms != null && (
              <Text style={styles.duration}>
                / {formatMs(recording.duration_ms)}
              </Text>
            )}
          </View>
        )}

        {/* Summary */}
        {summary && (
          <View style={styles.summaryCard}>
            {summary.title && (
              <Text style={styles.summaryTitle}>{summary.title}</Text>
            )}
            {summary.summary && (
              <Text style={styles.summaryText}>{summary.summary}</Text>
            )}
            {summary.action_items && summary.action_items.length > 0 && (
              <>
                <Text style={styles.summarySubheader}>Action items</Text>
                {summary.action_items.map((item, i) => (
                  <Text key={i} style={styles.summaryListItem}>
                    • {item}
                  </Text>
                ))}
              </>
            )}
            {summary.decisions && summary.decisions.length > 0 && (
              <>
                <Text style={styles.summarySubheader}>Decisions</Text>
                {summary.decisions.map((d, i) => (
                  <Text key={i} style={styles.summaryListItem}>
                    • {d}
                  </Text>
                ))}
              </>
            )}
          </View>
        )}

        {/* In-recording search */}
        {utterances.length > 0 && (
          <TextInput
            style={styles.searchInput}
            placeholder="Search transcript…"
            value={searchQuery}
            onChangeText={setSearchQuery}
            clearButtonMode="while-editing"
            returnKeyType="search"
          />
        )}

        {/* Transcript */}
        {filteredUtterances.length === 0 && utterances.length === 0 && status === 'ready' && (
          <Text style={styles.emptyTranscript}>
            No transcript available yet. The server may still be processing.
          </Text>
        )}

        {filteredUtterances.map((u) => (
          <UtteranceRow
            key={u.id}
            utterance={u}
            speakers={speakers}
            isActive={positionMs >= u.start_ms && positionMs < u.end_ms}
            matchesSearch={
              searchQuery.trim() !== '' &&
              u.text.toLowerCase().includes(searchQuery.toLowerCase())
            }
            onPress={() => seekTo(u.start_ms)}
            onLongPress={() => setNamingUtterance(u)}
          />
        ))}

        {searchQuery.trim() && filteredUtterances.length === 0 && (
          <Text style={styles.emptySearch}>
            No transcript lines match "{searchQuery}".
          </Text>
        )}
      </ScrollView>

      {/* Speaker naming modal */}
      <NameSpeakerModal
        visible={namingUtterance != null}
        currentName={
          namingUtterance
            ? (speakerLabel(namingUtterance.local_idx, speakers))
            : ''
        }
        onSave={handleNameSpeaker}
        onDismiss={() => setNamingUtterance(null)}
      />
    </>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#f5f5f5',
  },
  content: {
    padding: 12,
    paddingBottom: 40,
  },
  centered: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 32,
  },
  errorText: {
    color: '#E53935',
    fontSize: 14,
    textAlign: 'center',
    marginBottom: 16,
  },
  retryButton: {
    backgroundColor: '#E53935',
    borderRadius: 8,
    paddingHorizontal: 20,
    paddingVertical: 8,
  },
  retryButtonText: {
    color: '#fff',
    fontWeight: '600',
  },
  statusBanner: {
    backgroundColor: '#FFF8E1',
    borderRadius: 8,
    padding: 12,
    marginBottom: 12,
  },
  statusBannerText: {
    fontSize: 13,
    color: '#F57F17',
  },
  playbackBar: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#fff',
    borderRadius: 10,
    padding: 10,
    marginBottom: 12,
    gap: 8,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.05,
    shadowRadius: 3,
    elevation: 1,
  },
  playButton: {
    width: 36,
    height: 36,
    borderRadius: 18,
    backgroundColor: '#E53935',
    alignItems: 'center',
    justifyContent: 'center',
  },
  position: {
    fontSize: 14,
    fontVariant: ['tabular-nums'],
    color: '#333',
  },
  duration: {
    fontSize: 14,
    color: '#aaa',
    fontVariant: ['tabular-nums'],
  },
  summaryCard: {
    backgroundColor: '#fff',
    borderRadius: 12,
    padding: 16,
    marginBottom: 12,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.05,
    shadowRadius: 4,
    elevation: 1,
  },
  summaryTitle: {
    fontSize: 17,
    fontWeight: '700',
    color: '#111',
    marginBottom: 8,
  },
  summaryText: {
    fontSize: 14,
    color: '#444',
    lineHeight: 22,
    marginBottom: 10,
  },
  summarySubheader: {
    fontSize: 12,
    fontWeight: '700',
    color: '#888',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
    marginTop: 10,
    marginBottom: 4,
  },
  summaryListItem: {
    fontSize: 14,
    color: '#333',
    lineHeight: 20,
    paddingLeft: 4,
  },
  searchInput: {
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    fontSize: 14,
    backgroundColor: '#fff',
    marginBottom: 10,
  },
  utterance: {
    backgroundColor: '#fff',
    borderRadius: 8,
    padding: 12,
    marginBottom: 6,
  },
  utteranceActive: {
    borderLeftWidth: 3,
    borderLeftColor: '#E53935',
  },
  utteranceMatch: {
    backgroundColor: '#FFFDE7',
  },
  utteranceHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    marginBottom: 4,
  },
  speakerName: {
    fontSize: 12,
    fontWeight: '700',
  },
  utteranceTime: {
    fontSize: 12,
    color: '#aaa',
    fontVariant: ['tabular-nums'],
  },
  utteranceText: {
    fontSize: 14,
    color: '#222',
    lineHeight: 20,
  },
  emptyTranscript: {
    fontSize: 14,
    color: '#aaa',
    textAlign: 'center',
    marginTop: 32,
  },
  emptySearch: {
    fontSize: 14,
    color: '#aaa',
    textAlign: 'center',
    marginTop: 16,
  },
  // Modal
  modalOverlay: {
    flex: 1,
    backgroundColor: 'rgba(0,0,0,0.4)',
    alignItems: 'center',
    justifyContent: 'center',
  },
  modalCard: {
    width: '80%',
    backgroundColor: '#fff',
    borderRadius: 16,
    padding: 20,
  },
  modalTitle: {
    fontSize: 16,
    fontWeight: '700',
    color: '#111',
    marginBottom: 12,
  },
  modalInput: {
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    fontSize: 15,
    marginBottom: 12,
  },
  modalEnroll: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    marginBottom: 16,
  },
  checkbox: {
    width: 18,
    height: 18,
    borderWidth: 2,
    borderColor: '#ccc',
    borderRadius: 4,
  },
  checkboxChecked: {
    backgroundColor: '#E53935',
    borderColor: '#E53935',
  },
  enrollText: {
    fontSize: 13,
    color: '#444',
    flex: 1,
  },
  modalActions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: 8,
  },
  modalCancel: {
    paddingHorizontal: 16,
    paddingVertical: 8,
  },
  modalCancelText: {
    color: '#888',
    fontSize: 14,
  },
  modalSave: {
    backgroundColor: '#E53935',
    borderRadius: 8,
    paddingHorizontal: 16,
    paddingVertical: 8,
  },
  modalSaveText: {
    color: '#fff',
    fontWeight: '600',
    fontSize: 14,
  },
});
