/**
 * Screen 3: Recording Detail — Kiln transcript + playback.
 *
 * A flowing-ember playback scrubber (PlaybackWave) that shares the orb's DNA,
 * an LLM summary card, and a speaker color-rail transcript.  Audio streams from
 * GET /recordings/{id}/audio with HTTP Range seeking; bearer auth is sent via
 * the AudioSource headers.
 *
 * The transcript is word-addressable: tap any word to play from it, tap the
 * line elsewhere to play from its start, long-press to edit the text or tag the
 * speaker.  While audio plays, the spoken word is highlighted and the view
 * follows along — see src/playback/karaoke.ts for the timing and the store that
 * keeps that from re-rendering the whole list.
 *
 * Moments bookmarked during recording are MARKS (⚑), shown both as jump chips
 * and on the word that was being spoken when the mark was made.  The word
 * "highlight" is reserved for the karaoke highlight.
 */

import { useEffect, useState, useCallback, useRef, useMemo, memo } from 'react';
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
  Share,
  FlatList,
} from 'react-native';
import { useLocalSearchParams, useNavigation, useRouter } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';
import { useAudioPlayer, useAudioPlayerStatus, setAudioModeAsync } from 'expo-audio';
import { api } from '../../src/api/client';
import type {
  NameSpeakerRequest,
  RecordingDetailResponse,
  Utterance,
  RecordingSpeaker,
  SummaryTemplate,
} from '../../src/types';
import { Screen } from '../../src/components/ui';
import { PlaybackWave } from '../../src/components/PlaybackWave';
import { PipelineProgressCard } from '../../src/components/PipelineProgress';
import { SpeakerTagSheet } from '../../src/components/SpeakerTagSheet';
import {
  anchorMarks,
  NOT_SPEAKING,
  useActiveWordIndex,
  useKaraoke,
  type KaraokeStore,
  type TimedToken,
} from '../../src/playback/karaoke';
import { colors, mono, radius, speakerColor } from '../../src/theme';
import { log } from '../../src/util/logger';
import { buildExport, type ExportFormat } from '../../src/util/export';
import { MindMapModal } from '../../src/components/MindMap';
import { TranslateModal } from '../../src/components/Translate';

// Playback speeds cycled by the speed button.
const PLAYBACK_RATES = [1, 1.25, 1.5, 2];

// Stable empty transcript: the karaoke timeline is memoized on this array's
// identity, so a fresh `[]` each render would rebuild it every time.
const NO_UTTERANCES: Utterance[] = [];
const NO_MARKS: number[] = [];
const NO_SPEAKERS: RecordingSpeaker[] = [];

// Shown until GET /summary-templates returns; ids must match the backend.
const FALLBACK_TEMPLATES: SummaryTemplate[] = [
  { id: 'general', label: 'General meeting' },
  { id: 'standup', label: 'Standup' },
  { id: 'interview', label: 'Interview' },
  { id: 'one_on_one', label: '1:1' },
  { id: 'lecture', label: 'Lecture / talk' },
  { id: 'sales', label: 'Sales call' },
];

// ---------------------------------------------------------------------------

function formatMs(ms: number): string {
  const totalSecs = Math.floor(ms / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

function formatDate(iso: string): string {
  return new Date(iso)
    .toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
    .toUpperCase();
}

function speakerLabel(localIdx: number | null, speakers: RecordingSpeaker[]): string {
  if (localIdx == null) return 'Unknown';
  const sp = speakers.find((s) => s.local_idx === localIdx);
  return sp?.display_name ?? `Speaker ${localIdx}`;
}

// ---------------------------------------------------------------------------

/** Kiln in-screen header: back chevron · title · date · optional share. */
function DetailHeader({
  title,
  date,
  onBack,
  onShare,
  onMenu,
}: {
  title: string;
  date?: string;
  onBack: () => void;
  onShare?: () => void;
  onMenu?: () => void;
}) {
  return (
    <View style={styles.header}>
      <TouchableOpacity onPress={onBack} accessibilityLabel="Back" style={styles.backBtn}>
        <Text style={styles.backChevron}>‹</Text>
      </TouchableOpacity>
      <Text style={styles.headerTitle} numberOfLines={1}>
        {title}
      </Text>
      {date ? <Text style={styles.headerDate}>{date}</Text> : null}
      {onShare ? (
        <TouchableOpacity onPress={onShare} accessibilityLabel="Export" style={styles.shareBtn}>
          <Ionicons name="share-outline" size={20} color={colors.accent} />
        </TouchableOpacity>
      ) : null}
      {onMenu ? (
        <TouchableOpacity onPress={onMenu} accessibilityLabel="More actions" style={styles.shareBtn}>
          <Ionicons name="ellipsis-horizontal" size={20} color={colors.accent} />
        </TouchableOpacity>
      ) : null}
    </View>
  );
}

// ---------------------------------------------------------------------------

/**
 * One transcript line.
 *
 * Memoized, and it reads the spoken-word index from the karaoke store itself
 * rather than taking it as a prop: a word only changes what *this* row looks
 * like, so a tick must not re-render the rest of the transcript.
 *
 * Every word is its own `Text` with an `onPress`, so a tap plays from that
 * word. Nested `Text` handlers win over the row's, and the row still catches
 * taps on the padding — so tapping the line anywhere else plays from its start,
 * and a long press anywhere still opens the line's actions.
 */
const UtteranceRow = memo(function UtteranceRow({
  utterance,
  speakers,
  tokens,
  markedTokens,
  store,
  matchesSearch,
  onPress,
  onWordPress,
  onLongPress,
}: {
  utterance: Utterance;
  speakers: RecordingSpeaker[];
  tokens: TimedToken[] | undefined;
  /** Indices of words a mark was placed on during recording. */
  markedTokens: number[] | undefined;
  store: KaraokeStore;
  matchesSearch: boolean;
  onPress: (utterance: Utterance) => void;
  onWordPress: (token: TimedToken) => void;
  onLongPress: (utterance: Utterance) => void;
}) {
  const wordIdx = useActiveWordIndex(store, utterance.id);
  const isActive = wordIdx !== NOT_SPEAKING;
  const label = utterance.speaker_name ?? speakerLabel(utterance.local_idx, speakers);
  const color = speakerColor(utterance.local_idx);
  const markCount = markedTokens?.length ?? 0;

  return (
    <TouchableOpacity
      style={[styles.utterance, (isActive || matchesSearch) && styles.utteranceActive]}
      onPress={() => onPress(utterance)}
      onLongPress={() => onLongPress(utterance)}
      accessibilityRole="button"
      accessibilityLabel={`${label}: ${utterance.text}`}
    >
      <View style={[styles.rail, { backgroundColor: color }]} />
      <View style={styles.utteranceBody}>
        <View style={styles.utteranceHeader}>
          <Text style={[styles.speakerName, { color }]}>{label}</Text>
          <Text style={styles.utteranceTime}>{formatMs(utterance.start_ms)}</Text>
          {markCount > 0 && (
            <Text style={styles.rowMarkChip}>
              ⚑{markCount > 1 ? ` ${markCount}` : ''}
            </Text>
          )}
        </View>
        {tokens && tokens.length > 0 ? (
          <Text style={[styles.utteranceText, isActive && styles.utteranceTextActive]}>
            {tokens.map((t, i) => {
              const marked = markedTokens?.includes(i) ?? false;
              return (
                <Text
                  key={i}
                  onPress={() => onWordPress(t)}
                  onLongPress={() => onLongPress(utterance)}
                  suppressHighlighting
                  style={[
                    i === wordIdx && styles.wordActive,
                    marked && styles.wordMarked,
                  ]}
                >
                  {i > 0 ? ' ' : ''}
                  {marked ? <Text style={styles.markGlyph}>⚑</Text> : null}
                  {t.text}
                </Text>
              );
            })}
          </Text>
        ) : (
          // No usable word timings: the line stays one block, still tappable.
          <Text style={[styles.utteranceText, isActive && styles.utteranceTextActive]}>
            {utterance.text}
          </Text>
        )}
      </View>
    </TouchableOpacity>
  );
});

// ---------------------------------------------------------------------------

/** Bottom-sheet-style picker for the export format. */
function ExportSheet({
  visible,
  onSelect,
  onDismiss,
}: {
  visible: boolean;
  onSelect: (format: ExportFormat) => void;
  onDismiss: () => void;
}) {
  const options: { label: string; sub: string; value: ExportFormat }[] = [
    { label: 'Markdown', sub: 'Summary + transcript · .md', value: 'markdown' },
    { label: 'Plain text', sub: 'Summary + transcript · .txt', value: 'text' },
    { label: 'Subtitles', sub: 'Timed captions · .srt', value: 'srt' },
  ];
  return (
    <Modal visible={visible} transparent animationType="fade" onRequestClose={onDismiss}>
      <TouchableOpacity style={styles.modalOverlay} activeOpacity={1} onPress={onDismiss}>
        <View style={styles.sheetCard}>
          <Text style={styles.modalTitle}>Export & share</Text>
          {options.map((o) => (
            <TouchableOpacity
              key={o.value}
              style={styles.sheetRow}
              onPress={() => onSelect(o.value)}
              accessibilityRole="button"
              accessibilityLabel={`Export as ${o.label}`}
            >
              <View style={styles.sheetRowText}>
                <Text style={styles.sheetRowLabel}>{o.label}</Text>
                <Text style={styles.sheetRowSub}>{o.sub}</Text>
              </View>
              <Ionicons name="share-outline" size={18} color={colors.accent} />
            </TouchableOpacity>
          ))}
          <TouchableOpacity style={styles.modalCancel} onPress={onDismiss}>
            <Text style={styles.modalCancelText}>Cancel</Text>
          </TouchableOpacity>
        </View>
      </TouchableOpacity>
    </Modal>
  );
}

// ---------------------------------------------------------------------------

/** Bottom-sheet picker for the summary template. */
function TemplateSheet({
  visible,
  templates,
  current,
  generated,
  onSelect,
  onDismiss,
}: {
  visible: boolean;
  templates: SummaryTemplate[];
  current?: string | null;
  generated?: string[];
  onSelect: (template: string) => void;
  onDismiss: () => void;
}) {
  return (
    <Modal visible={visible} transparent animationType="fade" onRequestClose={onDismiss}>
      <TouchableOpacity style={styles.modalOverlay} activeOpacity={1} onPress={onDismiss}>
        <View style={styles.sheetCard}>
          <Text style={styles.modalTitle}>Summary views</Text>
          {templates.map((t) => {
            const active = current === t.id;
            const has = generated?.includes(t.id) ?? false;
            return (
              <TouchableOpacity
                key={t.id}
                style={styles.sheetRow}
                onPress={() => onSelect(t.id)}
                accessibilityRole="button"
                accessibilityState={{ selected: active }}
                accessibilityLabel={`Summary view ${t.label}`}
              >
                <View style={styles.sheetRowText}>
                  <Text style={[styles.sheetRowLabel, active && { color: colors.accent }]}>
                    {t.label}
                  </Text>
                  <Text style={styles.sheetRowSub}>
                    {has ? (active ? 'Showing' : 'Saved · tap to view') : 'Generate'}
                  </Text>
                </View>
                {active ? (
                  <Ionicons name="checkmark" size={18} color={colors.accent} />
                ) : has ? (
                  <Ionicons name="document-text-outline" size={16} color={colors.accent} />
                ) : (
                  <Ionicons name="sparkles-outline" size={16} color={colors.textMuted} />
                )}
              </TouchableOpacity>
            );
          })}
          <TouchableOpacity style={styles.modalCancel} onPress={onDismiss}>
            <Text style={styles.modalCancelText}>Cancel</Text>
          </TouchableOpacity>
        </View>
      </TouchableOpacity>
    </Modal>
  );
}

// ---------------------------------------------------------------------------

/**
 * Long-press chooser: edit the line's text or tag its speaker.
 *
 * `canTag` is false for provisional live-transcription lines, which have no
 * diarized speaker to attach a name to yet.
 */
function UtteranceActionSheet({
  visible,
  canTag,
  onEdit,
  onRename,
  onDismiss,
}: {
  visible: boolean;
  canTag: boolean;
  onEdit: () => void;
  onRename: () => void;
  onDismiss: () => void;
}) {
  return (
    <Modal visible={visible} transparent animationType="fade" onRequestClose={onDismiss}>
      <TouchableOpacity style={styles.modalOverlay} activeOpacity={1} onPress={onDismiss}>
        <View style={styles.sheetCard}>
          <Text style={styles.modalTitle}>Edit line</Text>
          <TouchableOpacity style={styles.sheetRow} onPress={onEdit} accessibilityRole="button">
            <View style={styles.sheetRowText}>
              <Text style={styles.sheetRowLabel}>Edit text</Text>
              <Text style={styles.sheetRowSub}>Correct what was said</Text>
            </View>
            <Ionicons name="create-outline" size={18} color={colors.accent} />
          </TouchableOpacity>
          {canTag && (
            <TouchableOpacity style={styles.sheetRow} onPress={onRename} accessibilityRole="button">
              <View style={styles.sheetRowText}>
                <Text style={styles.sheetRowLabel}>Tag speaker</Text>
                <Text style={styles.sheetRowSub}>Reuse a name across recordings</Text>
              </View>
              <Ionicons name="person-outline" size={18} color={colors.accent} />
            </TouchableOpacity>
          )}
          <TouchableOpacity style={styles.modalCancel} onPress={onDismiss}>
            <Text style={styles.modalCancelText}>Cancel</Text>
          </TouchableOpacity>
        </View>
      </TouchableOpacity>
    </Modal>
  );
}

/**
 * How many people are in the recording.
 *
 * Imported audio arrives with no count, and that is the case where diarization
 * goes furthest wrong — so this is the correction, not a preference.
 */
function ParticipantsModal({
  visible,
  current,
  saving,
  onSave,
  onDismiss,
}: {
  visible: boolean;
  current: number | null;
  saving: boolean;
  onSave: (count: number) => void;
  onDismiss: () => void;
}) {
  const [value, setValue] = useState('');
  useEffect(() => {
    if (visible) setValue(current != null ? String(current) : '');
  }, [visible, current]);

  const count = Number(value);
  const valid = Number.isInteger(count) && count >= 1 && count <= 64;

  return (
    <Modal visible={visible} transparent animationType="fade" onRequestClose={onDismiss}>
      <View style={styles.modalOverlay}>
        <View style={styles.modalCard}>
          <Text style={styles.modalTitle}>Speaker count</Text>
          <Text style={styles.modalHint}>
            How many people speak in this recording? Diarization pins its clustering to this
            number instead of guessing, which is what splits one voice into many on a long
            recording.
          </Text>
          <View style={styles.countRow}>
            {[2, 3, 4, 5, 6].map((n) => (
              <TouchableOpacity
                key={n}
                style={[styles.countChip, count === n && styles.countChipActive]}
                onPress={() => setValue(String(n))}
                accessibilityLabel={`${n} speakers`}
              >
                <Text style={[styles.countChipText, count === n && styles.countChipTextActive]}>
                  {n}
                </Text>
              </TouchableOpacity>
            ))}
          </View>
          <TextInput
            style={styles.modalInputCompact}
            value={value}
            onChangeText={setValue}
            keyboardType="number-pad"
            placeholder="or type a number"
            placeholderTextColor={colors.textDim}
            returnKeyType="done"
          />
          <View style={styles.modalActions}>
            <TouchableOpacity style={styles.modalCancel} onPress={onDismiss}>
              <Text style={styles.modalCancelText}>Cancel</Text>
            </TouchableOpacity>
            <TouchableOpacity
              style={[styles.modalSave, (!valid || saving) && styles.dimmedAction]}
              onPress={() => onSave(count)}
              disabled={!valid || saving}
            >
              {saving ? (
                <ActivityIndicator color={colors.white} />
              ) : (
                <Text style={styles.modalSaveText}>Save</Text>
              )}
            </TouchableOpacity>
          </View>
        </View>
      </View>
    </Modal>
  );
}

/** Multiline editor for a single transcript line. */
function EditUtteranceModal({
  utterance,
  saving,
  onSave,
  onDismiss,
}: {
  utterance: Utterance | null;
  saving: boolean;
  onSave: (text: string) => void;
  onDismiss: () => void;
}) {
  const [text, setText] = useState('');
  useEffect(() => {
    if (utterance) setText(utterance.text);
  }, [utterance]);

  return (
    <Modal visible={utterance != null} transparent animationType="fade" onRequestClose={onDismiss}>
      <View style={styles.modalOverlay}>
        <View style={styles.modalCard}>
          <Text style={styles.modalTitle}>Edit transcript line</Text>
          <TextInput
            style={styles.editInput}
            value={text}
            onChangeText={setText}
            multiline
            autoFocus
            placeholder="Transcript text"
            placeholderTextColor={colors.textDim}
          />
          <View style={styles.modalActions}>
            <TouchableOpacity style={styles.modalCancel} onPress={onDismiss}>
              <Text style={styles.modalCancelText}>Cancel</Text>
            </TouchableOpacity>
            <TouchableOpacity
              style={styles.modalSave}
              onPress={() => onSave(text)}
              disabled={saving || !text.trim()}
            >
              {saving ? (
                <ActivityIndicator color={colors.white} />
              ) : (
                <Text style={styles.modalSaveText}>Save</Text>
              )}
            </TouchableOpacity>
          </View>
        </View>
      </View>
    </Modal>
  );
}

/** Inline tag chips with add/remove for a recording. */
function TagsEditor({
  tags,
  saving,
  onChange,
}: {
  tags: string[];
  saving: boolean;
  onChange: (next: string[]) => void;
}) {
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState('');

  const commit = () => {
    const t = draft.trim().toLowerCase();
    setDraft('');
    setAdding(false);
    if (t && !tags.includes(t)) onChange([...tags, t]);
  };

  return (
    <View style={styles.tagsCard}>
      <Text style={styles.summaryLabel}>TAGS</Text>
      <View style={styles.tagsWrap}>
        {tags.map((t) => (
          <View key={t} style={styles.tagChip}>
            <Text style={styles.tagChipText}>{t}</Text>
            <TouchableOpacity
              onPress={() => onChange(tags.filter((x) => x !== t))}
              accessibilityLabel={`Remove tag ${t}`}
              hitSlop={8}
            >
              <Ionicons name="close" size={13} color={colors.accent} />
            </TouchableOpacity>
          </View>
        ))}
        {adding ? (
          <TextInput
            style={styles.tagInput}
            value={draft}
            onChangeText={setDraft}
            onBlur={commit}
            onSubmitEditing={commit}
            autoFocus
            autoCapitalize="none"
            autoCorrect={false}
            placeholder="tag"
            placeholderTextColor={colors.textDim}
            returnKeyType="done"
          />
        ) : (
          <TouchableOpacity style={styles.tagAdd} onPress={() => setAdding(true)} disabled={saving}>
            {saving ? (
              <ActivityIndicator size="small" color={colors.accent} />
            ) : (
              <>
                <Ionicons name="add" size={14} color={colors.accent} />
                <Text style={styles.tagAddText}>Tag</Text>
              </>
            )}
          </TouchableOpacity>
        )}
      </View>
    </View>
  );
}

// ---------------------------------------------------------------------------

export default function RecordingDetailScreen() {
  const { id, seekMs } = useLocalSearchParams<{ id: string; seekMs?: string }>();
  const navigation = useNavigation();
  const router = useRouter();

  const [detail, setDetail] = useState<RecordingDetailResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Audio — expo-audio managed player + status hook (reliable play/pause/seek).
  const audioSource = useMemo(
    () => (id ? { uri: api.audioUrl(id), headers: { Authorization: api.authHeader() } } : null),
    [id],
  );
  const player = useAudioPlayer(audioSource);
  const audioStatus = useAudioPlayerStatus(player);
  const positionMs = Math.round((audioStatus?.currentTime ?? 0) * 1000);
  const statusDurationMs = Math.round((audioStatus?.duration ?? 0) * 1000);
  const playing = audioStatus?.playing ?? false;
  const audioLoaded = audioStatus?.isLoaded ?? false;
  const [waveWidth, setWaveWidth] = useState(0);

  const [searchQuery, setSearchQuery] = useState('');
  const [namingUtterance, setNamingUtterance] = useState<Utterance | null>(null);
  const [savingSpeaker, setSavingSpeaker] = useState(false);
  const [showExport, setShowExport] = useState(false);
  const [rate, setRate] = useState(1);
  const [templates, setTemplates] = useState<SummaryTemplate[]>(FALLBACK_TEMPLATES);
  const [showTemplates, setShowTemplates] = useState(false);
  const [resummarizing, setResummarizing] = useState(false);
  const [utteranceAction, setUtteranceAction] = useState<Utterance | null>(null);
  const [editingUtterance, setEditingUtterance] = useState<Utterance | null>(null);
  const [savingEdit, setSavingEdit] = useState(false);
  const [savingTags, setSavingTags] = useState(false);
  const [activeTemplate, setActiveTemplate] = useState<string | null>(null);
  const [showActions, setShowActions] = useState(false);
  const [showParticipants, setShowParticipants] = useState(false);
  const [savingParticipants, setSavingParticipants] = useState(false);
  const [showMindMap, setShowMindMap] = useState(false);
  const [showTranslate, setShowTranslate] = useState(false);

  // -------------------------------------------------------------------------
  // Karaoke highlighting + follow-along scrolling
  // -------------------------------------------------------------------------

  const utterances = detail?.utterances ?? NO_UTTERANCES;
  const { store: karaoke, tokensById, timeline } = useKaraoke({
    player,
    utterances,
    playing,
    positionMs,
    rate,
    enabled: audioLoaded,
  });

  // Marks captured during recording, tied to the word being spoken at the time.
  const marks = detail?.recording.marks ?? NO_MARKS;
  const marksByUtterance = useMemo(() => anchorMarks(timeline, marks), [timeline, marks]);

  const listRef = useRef<FlatList<Utterance>>(null);
  const [following, setFollowing] = useState(true);
  // The scroll callback fires outside React's render, so it reads the ref.
  const followingRef = useRef(true);
  const setFollow = useCallback((next: boolean) => {
    followingRef.current = next;
    setFollowing(next);
  }, []);

  // Row index by utterance id, for follow-along scrolling. Rebuilt whenever the
  // visible list changes (a search filter changes what index a line sits at).
  const rowIndexById = useRef(new Map<number, number>());

  const scrollToUtterance = useCallback((utteranceId: number | null) => {
    if (utteranceId == null) return;
    const index = rowIndexById.current.get(utteranceId);
    if (index == null) return; // filtered out of the current view
    // Park the spoken line a quarter of the way down rather than against the
    // top edge, so the lines leading up to it stay readable.
    listRef.current?.scrollToIndex({ index, animated: true, viewPosition: 0.25 });
  }, []);

  // A virtualized list cannot always scroll straight to a far-off row: it has
  // to render its way there first. Retry once the list has measured more.
  const handleScrollToIndexFailed = useCallback(
    (info: { index: number; averageItemLength: number }) => {
      listRef.current?.scrollToOffset({
        offset: info.averageItemLength * info.index,
        animated: false,
      });
      setTimeout(() => {
        listRef.current?.scrollToIndex({
          index: info.index,
          animated: true,
          viewPosition: 0.25,
        });
      }, 120);
    },
    [],
  );

  // Follow the spoken line. Driven by a store callback instead of state so a
  // new line scrolls the view without re-rendering the transcript.
  useEffect(() => {
    karaoke.onUtteranceChange = (utteranceId) => {
      if (followingRef.current) scrollToUtterance(utteranceId);
    };
    return () => {
      karaoke.onUtteranceChange = null;
    };
  }, [karaoke, scrollToUtterance]);


  // -------------------------------------------------------------------------

  const loadDetail = useCallback(
    async (background = false) => {
      if (!id) return;
      if (!background) setLoading(true);
      setError(null);
      try {
        const res = await api.getRecording(id);
        setDetail(res);
        navigation.setOptions({ title: res.recording.title ?? 'Recording' });
      } catch (err) {
        if (!background) setError(err instanceof Error ? err.message : 'Failed to load recording');
      } finally {
        if (!background) setLoading(false);
      }
    },
    [id, navigation],
  );

  useEffect(() => {
    loadDetail();
  }, [loadDetail]);

  const status = detail?.recording.status;
  useEffect(() => {
    if (!id) return;
    if (status !== 'uploading' && status !== 'processing') return;
    const interval = setInterval(() => loadDetail(true), 4000);
    return () => clearInterval(interval);
  }, [id, status, loadDetail]);

  // -------------------------------------------------------------------------

  // Configure the audio session once (play in silent mode, no background).
  useEffect(() => {
    setAudioModeAsync({
      allowsRecording: false,
      playsInSilentMode: true,
      shouldPlayInBackground: false,
    }).catch(() => {
      // non-fatal
    });
  }, []);

  useEffect(() => {
    log('audio', `player loaded=${audioLoaded} dur=${statusDurationMs}ms`);
  }, [audioLoaded, statusDurationMs]);

  // Deep-link seek (arriving from Search / Ask) once the audio has loaded.
  const didSeekRef = useRef(false);
  useEffect(() => {
    if (!seekMs || didSeekRef.current || !audioLoaded) return;
    didSeekRef.current = true;
    log('audio', `deep-link seek to ${seekMs}ms`);
    player
      .seekTo(Number(seekMs) / 1000)
      .then(() => player.play())
      .catch((e) => log('audio', 'deep-link seek failed', e));
  }, [seekMs, audioLoaded, player]);

  const seekTo = useCallback(
    (ms: number) => {
      log('audio', `seek to ${ms}ms`);
      // Jumping to a moment is a request to follow it again.
      setFollow(true);
      player
        .seekTo(ms / 1000)
        .then(() => player.play())
        .catch((e) => log('audio', 'seek failed', e));
    },
    [player, setFollow],
  );

  const togglePlayback = useCallback(() => {
    log('audio', playing ? 'pause' : 'play');
    if (playing) player.pause();
    else player.play();
  }, [player, playing]);

  const cycleRate = useCallback(() => {
    const next = PLAYBACK_RATES[(PLAYBACK_RATES.indexOf(rate) + 1) % PLAYBACK_RATES.length];
    setRate(next);
    try {
      player.setPlaybackRate(next);
    } catch (e) {
      log('audio', 'setPlaybackRate failed', e);
    }
  }, [rate, player]);

  // Re-apply the chosen rate once audio (re)loads — a fresh source resets to 1×.
  useEffect(() => {
    if (audioLoaded && rate !== 1) {
      try {
        player.setPlaybackRate(rate);
      } catch {
        // non-fatal
      }
    }
  }, [audioLoaded, rate, player]);

  // -------------------------------------------------------------------------

  const handleExport = useCallback(
    async (format: ExportFormat) => {
      setShowExport(false);
      if (!detail) return;
      try {
        const { content, filename } = buildExport(detail, format);
        log('export', `sharing ${filename} (${content.length} chars)`);
        await Share.share({ message: content, title: filename });
      } catch (err) {
        log('export', 'share failed', err);
        Alert.alert('Export failed', err instanceof Error ? err.message : String(err));
      }
    },
    [detail],
  );

  // Per-speaker talk-time, derived from utterance durations. Computed from
  // `detail` (may be null) so the hook runs unconditionally before any return.
  const talkTime = useMemo(() => {
    const utts = detail?.utterances ?? [];
    const sps = detail?.speakers ?? [];
    const map = new Map<string, { label: string; ms: number; color: string }>();
    let total = 0;
    for (const u of utts) {
      const d = Math.max(0, u.end_ms - u.start_ms);
      if (d === 0) continue;
      total += d;
      const label = u.speaker_name ?? speakerLabel(u.local_idx, sps);
      const prev = map.get(label);
      if (prev) prev.ms += d;
      else map.set(label, { label, ms: d, color: speakerColor(u.local_idx) });
    }
    return { rows: Array.from(map.values()).sort((a, b) => b.ms - a.ms), total };
  }, [detail]);

  // All generated summary views (newer backend: `summaries[]`; older: single `summary`).
  const summaries = useMemo(
    () => detail?.summaries ?? (detail?.summary ? [detail.summary] : []),
    [detail],
  );

  // Pull the authoritative template list (falls back to the constant if the
  // backend predates the endpoint).
  useEffect(() => {
    api
      .summaryTemplates()
      .then((res) => {
        if (res.templates?.length) setTemplates(res.templates);
      })
      .catch(() => {
        /* keep FALLBACK_TEMPLATES */
      });
  }, []);

  const handleResummarize = useCallback(
    async (template: string) => {
      setShowTemplates(false);
      if (!id) return;
      setResummarizing(true);
      try {
        await api.resummarize(id, template);
        // Poll until that template's summary view appears, then show it.
        const deadline = Date.now() + 90_000;
        while (Date.now() < deadline) {
          await new Promise((r) => setTimeout(r, 3000));
          const res = await api.getRecording(id);
          setDetail(res);
          const views = res.summaries ?? (res.summary ? [res.summary] : []);
          if (views.some((s) => (s.template ?? 'general') === template)) {
            setActiveTemplate(template);
            break;
          }
        }
      } catch (err) {
        Alert.alert('Could not summarize', err instanceof Error ? err.message : String(err));
      } finally {
        setResummarizing(false);
      }
    },
    [id],
  );

  // Pick a template from the sheet: switch instantly if that view already
  // exists, otherwise generate it.
  const handlePickTemplate = useCallback(
    (template: string) => {
      setShowTemplates(false);
      const views = detail?.summaries ?? (detail?.summary ? [detail.summary] : []);
      if (views.some((s) => (s.template ?? 'general') === template)) {
        setActiveTemplate(template);
      } else {
        handleResummarize(template);
      }
    },
    [detail, handleResummarize],
  );

  /**
   * Save the speaker count, then re-run the pipeline with it.
   *
   * Diarization discovering the count by itself is exactly what goes wrong on a
   * long recording — it split four voices into dozens. Stating the number pins
   * the clustering to it.
   */
  const handleSetParticipants = useCallback(
    async (count: number) => {
      if (!id) return;
      setShowParticipants(false);
      setSavingParticipants(true);
      try {
        await api.setParticipants(id, count);
        setDetail((d) =>
          d ? { ...d, recording: { ...d.recording, participants_expected: count } } : d,
        );
        Alert.alert(
          'Speaker count saved',
          `Set to ${count}. Re-run the transcript now so diarization uses it?`,
          [
            { text: 'Later', style: 'cancel' },
            {
              text: 'Re-run',
              onPress: async () => {
                try {
                  await api.reprocess(id);
                  await loadDetail();
                } catch (err) {
                  Alert.alert(
                    'Could not reprocess',
                    err instanceof Error ? err.message : String(err),
                  );
                }
              },
            },
          ],
        );
      } catch (err) {
        Alert.alert('Could not save', err instanceof Error ? err.message : String(err));
      } finally {
        setSavingParticipants(false);
      }
    },
    [id, loadDetail],
  );

  const handleReprocess = useCallback(() => {
    setShowActions(false);
    if (!id) return;
    Alert.alert(
      'Reprocess recording',
      'Re-run transcription, diarization and summary from the original audio? This replaces the current transcript.',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Reprocess',
          onPress: async () => {
            try {
              await api.reprocess(id);
              await loadDetail(); // status -> processing kicks the existing poll loop
            } catch (err) {
              Alert.alert('Could not reprocess', err instanceof Error ? err.message : String(err));
            }
          },
        },
      ],
    );
  }, [id, loadDetail]);

  /**
   * Tag the diarized speaker behind the long-pressed line, either with someone
   * already in the library (`speaker_id`) or a new name. With `enroll` set the
   * server keeps the voiceprint, which is what carries the name into recordings
   * uploaded later.
   */
  const handleTagSpeaker = useCallback(
    async (req: NameSpeakerRequest) => {
      if (!namingUtterance || !id) return;
      const localIdx = namingUtterance.local_idx;
      if (localIdx == null) return;
      setSavingSpeaker(true);
      try {
        await api.nameSpeaker(id, localIdx, req);
        setNamingUtterance(null);
        await loadDetail();
      } catch (err) {
        Alert.alert('Could not tag speaker', err instanceof Error ? err.message : String(err));
      } finally {
        setSavingSpeaker(false);
      }
    },
    [namingUtterance, id, loadDetail],
  );

  /** Drop the name from this recording's speaker; the library entry stays. */
  const handleUntagSpeaker = useCallback(async () => {
    if (!namingUtterance || !id) return;
    const localIdx = namingUtterance.local_idx;
    if (localIdx == null) return;
    setSavingSpeaker(true);
    try {
      await api.unnameSpeaker(id, localIdx);
      setNamingUtterance(null);
      await loadDetail();
    } catch (err) {
      Alert.alert('Could not remove name', err instanceof Error ? err.message : String(err));
    } finally {
      setSavingSpeaker(false);
    }
  }, [namingUtterance, id, loadDetail]);

  // Stable row callbacks — the rows are memoized, so fresh closures per render
  // would defeat the point of only re-rendering the line being spoken.
  const handleRowPress = useCallback((u: Utterance) => seekTo(u.start_ms), [seekTo]);
  const handleRowLongPress = useCallback((u: Utterance) => setUtteranceAction(u), []);
  /** Tap a word: play from that word. */
  const handleWordPress = useCallback((t: TimedToken) => seekTo(t.startMs), [seekTo]);

  const keyExtractor = useCallback((u: Utterance) => String(u.id), []);

  const renderRow = useCallback(
    ({ item }: { item: Utterance }) => (
      <UtteranceRow
        utterance={item}
        speakers={detail?.speakers ?? NO_SPEAKERS}
        tokens={tokensById.get(item.id)}
        markedTokens={marksByUtterance.get(item.id)}
        store={karaoke}
        matchesSearch={
          searchQuery.trim() !== '' &&
          item.text.toLowerCase().includes(searchQuery.toLowerCase())
        }
        onPress={handleRowPress}
        onWordPress={handleWordPress}
        onLongPress={handleRowLongPress}
      />
    ),
    [
      detail?.speakers,
      tokensById,
      marksByUtterance,
      karaoke,
      searchQuery,
      handleRowPress,
      handleWordPress,
      handleRowLongPress,
    ],
  );

  const handleEditUtterance = useCallback(
    async (text: string) => {
      const u = editingUtterance;
      setEditingUtterance(null);
      if (!u || !id) return;
      const trimmed = text.trim();
      if (!trimmed || trimmed === u.text) return;
      setSavingEdit(true);
      try {
        const updated = await api.editUtterance(id, u.id, trimmed);
        setDetail((d) =>
          d ? { ...d, utterances: d.utterances.map((x) => (x.id === updated.id ? updated : x)) } : d,
        );
      } catch (err) {
        Alert.alert('Could not save edit', err instanceof Error ? err.message : String(err));
      } finally {
        setSavingEdit(false);
      }
    },
    [editingUtterance, id],
  );

  const handleSetTags = useCallback(
    async (next: string[]) => {
      if (!id) return;
      setSavingTags(true);
      try {
        const res = await api.setTags(id, next);
        setDetail((d) => (d ? { ...d, recording: { ...d.recording, tags: res.tags } } : d));
      } catch (err) {
        Alert.alert('Could not update tags', err instanceof Error ? err.message : String(err));
      } finally {
        setSavingTags(false);
      }
    },
    [id],
  );

  // -------------------------------------------------------------------------

  if (loading) {
    return (
      <Screen>
        <DetailHeader title="Recording" onBack={() => router.back()} />
        <View style={styles.centered}>
          <ActivityIndicator color={colors.accent} />
        </View>
      </Screen>
    );
  }

  if (error || !detail) {
    return (
      <Screen>
        <DetailHeader title="Recording" onBack={() => router.back()} />
        <View style={styles.centered}>
          <Text style={styles.errorText}>{error ?? 'No data'}</Text>
          <TouchableOpacity style={styles.retryButton} onPress={() => loadDetail()}>
            <Text style={styles.retryButtonText}>Retry</Text>
          </TouchableOpacity>
        </View>
      </Screen>
    );
  }

  // `utterances` is already bound above (the karaoke timeline needs it before
  // the early returns).
  const { recording, speakers } = detail;
  const dur = recording.duration_ms ?? statusDurationMs;
  const progress = dur > 0 ? positionMs / dur : 0;
  const templateLabel = (tid?: string | null) =>
    tid ? (templates.find((t) => t.id === tid)?.label ?? null) : null;
  const generatedTemplates = summaries.map((s) => s.template ?? 'general');
  const activeSummary =
    summaries.find((s) => (s.template ?? 'general') === activeTemplate) ?? summaries[0] ?? null;

  const filteredUtterances = searchQuery.trim()
    ? utterances.filter((u) => u.text.toLowerCase().includes(searchQuery.toLowerCase()))
    : utterances;

  // Keep the id → row index map in step with what is actually listed.
  rowIndexById.current = new Map(filteredUtterances.map((u, i) => [u.id, i]));

  return (
    <Screen>
      <DetailHeader
        title={recording.title ?? 'Recording'}
        date={formatDate(recording.created_at)}
        onBack={() => router.back()}
        onShare={utterances.length > 0 || summaries.length > 0 ? () => setShowExport(true) : undefined}
        onMenu={() => setShowActions(true)}
      />

      <FlatList
        ref={listRef}
        data={filteredUtterances}
        keyExtractor={keyExtractor}
        renderItem={renderRow}
        contentContainerStyle={styles.content}
        // Dragging the transcript means the reader wants to look somewhere
        // else; stop yanking the view back to the playhead until they ask.
        onScrollBeginDrag={() => setFollow(false)}
        onScrollToIndexFailed={handleScrollToIndexFailed}
        keyboardShouldPersistTaps="handled"
        // A long meeting is a thousand lines and tens of thousands of word
        // spans. Only what is on screen may be mounted.
        initialNumToRender={12}
        maxToRenderPerBatch={8}
        windowSize={9}
        // NOT removeClippedSubviews: on Android it is known to swallow touches
        // on recycled rows, and every word here is a touch target.
        ListHeaderComponent={
          <>
        {/* Status banner. Prefer the per-stage checklist when the server sends
            one — on a long recording a single "processing…" line is
            indistinguishable from a hang. Older backends omit `progress`, so
            fall back to the plain line. */}
        {status !== 'ready' &&
          (detail?.progress && status !== 'uploading' ? (
            <PipelineProgressCard progress={detail.progress} />
          ) : (
            <View style={styles.statusBanner}>
              <Text style={styles.statusBannerText}>
                {status === 'uploading'
                  ? 'Recording — transcribing live; speaker labels are assigned when you stop.'
                  : status === 'processing'
                    ? 'Finalizing transcript and speaker labels…'
                    : 'Processing failed. Check server logs.'}
              </Text>
            </View>
          ))}

        {/* Playback bar */}
        {status === 'ready' && (
          <View style={styles.playbackBar}>
            <TouchableOpacity
              style={styles.playButton}
              onPress={togglePlayback}
              accessibilityLabel={playing ? 'Pause' : 'Play'}
            >
              <Ionicons name={playing ? 'pause' : 'play'} size={15} color={colors.white} />
            </TouchableOpacity>
            <View
              style={styles.waveWrap}
              onLayout={(e) => setWaveWidth(Math.round(e.nativeEvent.layout.width))}
            >
              {waveWidth > 0 && (
                <PlaybackWave width={waveWidth} height={30} progress={progress} playing={playing} />
              )}
            </View>
            <TouchableOpacity
              style={styles.speedBtn}
              onPress={cycleRate}
              accessibilityLabel={`Playback speed ${rate}×`}
            >
              <Text style={styles.speedText}>{rate}×</Text>
            </TouchableOpacity>
            <Text style={styles.position}>{formatMs(dur || positionMs)}</Text>
          </View>
        )}

        {/* Marks captured during recording. Called MARKS, not "highlights" —
            the transcript's own highlight is the word being spoken. */}
        {status === 'ready' && marks.length > 0 && (
          <View style={styles.marksCard}>
            <Text style={styles.summaryLabel}>MARKS</Text>
            <ScrollView
              horizontal
              showsHorizontalScrollIndicator={false}
              contentContainerStyle={styles.marksRow}
            >
              {marks.map((m, i) => (
                <TouchableOpacity
                  key={i}
                  style={styles.markChip}
                  onPress={() => seekTo(m)}
                  accessibilityLabel={`Play from mark at ${formatMs(m)}`}
                >
                  <Text style={styles.markChipGlyph}>⚑</Text>
                  <Text style={styles.markChipText}>{formatMs(m)}</Text>
                </TouchableOpacity>
              ))}
            </ScrollView>
          </View>
        )}

        {/* Tags */}
        {(status === 'ready' || (recording.tags?.length ?? 0) > 0) && (
          <TagsEditor tags={recording.tags ?? []} saving={savingTags} onChange={handleSetTags} />
        )}

        {/* Summary (active view; switch/add views via the template chip) */}
        {activeSummary && (
          <View style={styles.summaryCard}>
            <View style={styles.summaryHeaderRow}>
              <Text style={styles.summaryLabel}>SUMMARY</Text>
              <TouchableOpacity
                style={styles.templateChip}
                onPress={() => setShowTemplates(true)}
                disabled={resummarizing}
                accessibilityLabel="Switch or add a summary view"
              >
                {resummarizing ? (
                  <ActivityIndicator size="small" color={colors.accent} />
                ) : (
                  <>
                    <Ionicons name="options-outline" size={12} color={colors.accent} />
                    <Text style={styles.templateChipText}>
                      {templateLabel(activeSummary.template) ?? 'Template'}
                      {summaries.length > 1 ? ` · ${summaries.length}` : ''}
                    </Text>
                  </>
                )}
              </TouchableOpacity>
            </View>
            {activeSummary.title && <Text style={styles.summaryTitle}>{activeSummary.title}</Text>}
            {activeSummary.summary && <Text style={styles.summaryText}>{activeSummary.summary}</Text>}
            {activeSummary.action_items && activeSummary.action_items.length > 0 && (
              <>
                <Text style={styles.summarySub}>ACTION ITEMS</Text>
                {activeSummary.action_items.map((item, i) => (
                  <View key={i} style={styles.bulletRow}>
                    <View style={styles.bullet} />
                    <Text style={styles.bulletText}>{item}</Text>
                  </View>
                ))}
              </>
            )}
            {activeSummary.decisions && activeSummary.decisions.length > 0 && (
              <>
                <Text style={styles.summarySub}>DECISIONS</Text>
                {activeSummary.decisions.map((d, i) => (
                  <View key={i} style={styles.bulletRow}>
                    <View style={styles.bullet} />
                    <Text style={styles.bulletText}>{d}</Text>
                  </View>
                ))}
              </>
            )}
          </View>
        )}

        {/* Generate summary when there isn't one yet */}
        {status === 'ready' && summaries.length === 0 && (
          <TouchableOpacity
            style={styles.generateCard}
            onPress={() => setShowTemplates(true)}
            disabled={resummarizing}
            accessibilityLabel="Generate summary"
          >
            {resummarizing ? (
              <ActivityIndicator color={colors.accent} />
            ) : (
              <>
                <Ionicons name="sparkles-outline" size={16} color={colors.accent} />
                <Text style={styles.generateText}>Generate summary…</Text>
              </>
            )}
          </TouchableOpacity>
        )}

        {/* Talk-time breakdown */}
        {status === 'ready' && talkTime.rows.length >= 2 && talkTime.total > 0 && (
          <View style={styles.analyticsCard}>
            <Text style={styles.summaryLabel}>TALK TIME</Text>
            {talkTime.rows.map((r) => {
              const pct = Math.round((r.ms / talkTime.total) * 100);
              return (
                <View key={r.label} style={styles.ttRow}>
                  <Text style={[styles.ttName, { color: r.color }]} numberOfLines={1}>
                    {r.label}
                  </Text>
                  <View style={styles.ttBarTrack}>
                    <View
                      style={[styles.ttBarFill, { width: `${pct}%`, backgroundColor: r.color }]}
                    />
                  </View>
                  <Text style={styles.ttValue}>{pct}%</Text>
                </View>
              );
            })}
          </View>
        )}

        {/* In-recording search */}
        {utterances.length > 0 && (
          <View style={styles.searchBar}>
            <Ionicons name="search" size={15} color={colors.textMuted} />
            <TextInput
              style={styles.searchInput}
              placeholder="Search transcript…"
              placeholderTextColor={colors.textDim}
              value={searchQuery}
              onChangeText={setSearchQuery}
              returnKeyType="search"
            />
          </View>
        )}
          </>
        }
        ListEmptyComponent={
          searchQuery.trim() ? (
            <Text style={styles.emptySearch}>No transcript lines match “{searchQuery}”.</Text>
          ) : status === 'ready' ? (
            <Text style={styles.emptyTranscript}>
              No transcript available yet. The server may still be processing.
            </Text>
          ) : null
        }
      />

      {/* Offered only once the reader has scrolled away during playback. */}
      {playing && !following && utterances.length > 0 && (
        <TouchableOpacity
          style={styles.followPill}
          onPress={() => {
            setFollow(true);
            scrollToUtterance(karaoke.activeUtteranceId);
          }}
          accessibilityLabel="Follow playback"
        >
          <Ionicons name="arrow-down" size={13} color={colors.white} />
          <Text style={styles.followPillText}>Follow playback</Text>
        </TouchableOpacity>
      )}

      <SpeakerTagSheet
        visible={namingUtterance != null}
        currentLabel={
          namingUtterance
            ? (namingUtterance.speaker_name ??
              speakerLabel(namingUtterance.local_idx, speakers))
            : ''
        }
        currentSpeakerId={
          namingUtterance
            ? (speakers.find((s) => s.local_idx === namingUtterance.local_idx)?.speaker_id ?? null)
            : null
        }
        saving={savingSpeaker}
        onTag={handleTagSpeaker}
        onUntag={handleUntagSpeaker}
        onDismiss={() => setNamingUtterance(null)}
      />

      <ParticipantsModal
        visible={showParticipants}
        current={recording.participants_expected ?? null}
        saving={savingParticipants}
        onSave={handleSetParticipants}
        onDismiss={() => setShowParticipants(false)}
      />

      <ExportSheet
        visible={showExport}
        onSelect={handleExport}
        onDismiss={() => setShowExport(false)}
      />

      <TemplateSheet
        visible={showTemplates}
        templates={templates}
        current={activeSummary?.template ?? activeTemplate}
        generated={generatedTemplates}
        onSelect={handlePickTemplate}
        onDismiss={() => setShowTemplates(false)}
      />

      <Modal
        visible={showActions}
        transparent
        animationType="fade"
        onRequestClose={() => setShowActions(false)}
      >
        <TouchableOpacity
          style={styles.modalOverlay}
          activeOpacity={1}
          onPress={() => setShowActions(false)}
        >
          <View style={styles.sheetCard}>
            <Text style={styles.modalTitle}>Actions</Text>
            <TouchableOpacity
              style={styles.sheetRow}
              onPress={() => {
                setShowActions(false);
                setShowMindMap(true);
              }}
              accessibilityRole="button"
            >
              <View style={styles.sheetRowText}>
                <Text style={styles.sheetRowLabel}>Mind map</Text>
                <Text style={styles.sheetRowSub}>Visual outline of the summary</Text>
              </View>
              <Ionicons name="git-branch-outline" size={18} color={colors.accent} />
            </TouchableOpacity>
            {summaries.length > 0 && (
              <TouchableOpacity
                style={styles.sheetRow}
                onPress={() => {
                  setShowActions(false);
                  setShowTranslate(true);
                }}
                accessibilityRole="button"
              >
                <View style={styles.sheetRowText}>
                  <Text style={styles.sheetRowLabel}>Translate summary</Text>
                  <Text style={styles.sheetRowSub}>Into another language</Text>
                </View>
                <Ionicons name="language-outline" size={18} color={colors.accent} />
              </TouchableOpacity>
            )}
            <TouchableOpacity
              style={styles.sheetRow}
              onPress={() => {
                setShowActions(false);
                setShowParticipants(true);
              }}
              accessibilityRole="button"
            >
              <View style={styles.sheetRowText}>
                <Text style={styles.sheetRowLabel}>Speaker count</Text>
                <Text style={styles.sheetRowSub}>
                  {recording.participants_expected != null
                    ? `Set to ${recording.participants_expected} — fixes wrong speaker splits`
                    : 'Not set — tell it how many people are in the room'}
                </Text>
              </View>
              <Ionicons name="people-outline" size={18} color={colors.accent} />
            </TouchableOpacity>
            <TouchableOpacity style={styles.sheetRow} onPress={handleReprocess} accessibilityRole="button">
              <View style={styles.sheetRowText}>
                <Text style={styles.sheetRowLabel}>Reprocess transcript</Text>
                <Text style={styles.sheetRowSub}>Re-run transcription &amp; summary from the audio</Text>
              </View>
              <Ionicons name="refresh" size={18} color={colors.accent} />
            </TouchableOpacity>
            <TouchableOpacity style={styles.modalCancel} onPress={() => setShowActions(false)}>
              <Text style={styles.modalCancelText}>Cancel</Text>
            </TouchableOpacity>
          </View>
        </TouchableOpacity>
      </Modal>

      <UtteranceActionSheet
        visible={utteranceAction != null}
        canTag={utteranceAction?.local_idx != null}
        onEdit={() => {
          const u = utteranceAction;
          setUtteranceAction(null);
          setEditingUtterance(u);
        }}
        onRename={() => {
          const u = utteranceAction;
          setUtteranceAction(null);
          setNamingUtterance(u);
        }}
        onDismiss={() => setUtteranceAction(null)}
      />

      <EditUtteranceModal
        utterance={editingUtterance}
        saving={savingEdit}
        onSave={handleEditUtterance}
        onDismiss={() => setEditingUtterance(null)}
      />

      <MindMapModal
        visible={showMindMap}
        summary={activeSummary}
        title={recording.title ?? 'Recording'}
        onDismiss={() => setShowMindMap(false)}
      />

      {id ? (
        <TranslateModal
          visible={showTranslate}
          recordingId={id}
          onDismiss={() => setShowTranslate(false)}
        />
      ) : null}
    </Screen>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 18,
    paddingTop: 4,
    paddingBottom: 12,
  },
  backBtn: {
    paddingRight: 2,
  },
  backChevron: {
    fontSize: 30,
    fontWeight: '300',
    color: colors.accent,
    lineHeight: 32,
  },
  headerTitle: {
    flex: 1,
    fontSize: 17,
    fontWeight: '700',
    color: colors.textPrimary,
  },
  headerDate: {
    fontSize: 10,
    fontWeight: '600',
    color: colors.textMuted,
    fontFamily: mono,
  },
  shareBtn: {
    paddingLeft: 10,
    paddingVertical: 2,
  },
  content: {
    paddingHorizontal: 18,
    paddingBottom: 40,
  },
  centered: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 32,
    gap: 16,
  },
  errorText: {
    color: colors.accentDeep,
    fontSize: 14,
    textAlign: 'center',
  },
  retryButton: {
    backgroundColor: colors.accent,
    borderRadius: 10,
    paddingHorizontal: 20,
    paddingVertical: 9,
  },
  retryButtonText: {
    color: colors.white,
    fontWeight: '600',
  },
  statusBanner: {
    backgroundColor: 'rgba(226,168,95,0.12)',
    borderWidth: 1,
    borderColor: 'rgba(226,168,95,0.3)',
    borderRadius: radius.inner,
    padding: 12,
    marginBottom: 12,
  },
  statusBannerText: {
    fontSize: 13,
    lineHeight: 18,
    color: colors.amber,
  },
  playbackBar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 11,
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.inner,
    padding: 11,
    marginBottom: 12,
  },
  playButton: {
    width: 36,
    height: 36,
    borderRadius: 18,
    backgroundColor: colors.accent,
    alignItems: 'center',
    justifyContent: 'center',
  },
  waveWrap: {
    flex: 1,
    height: 30,
    overflow: 'hidden',
  },
  position: {
    fontSize: 11,
    fontWeight: '600',
    color: colors.textMuted,
    fontFamily: mono,
    fontVariant: ['tabular-nums'],
  },
  speedBtn: {
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: 8,
    paddingHorizontal: 8,
    paddingVertical: 4,
    minWidth: 38,
    alignItems: 'center',
  },
  speedText: {
    fontSize: 11,
    fontWeight: '700',
    color: colors.textMuted,
    fontFamily: mono,
  },
  summaryHeaderRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  templateChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    minHeight: 22,
    paddingHorizontal: 9,
    paddingVertical: 4,
    borderRadius: 7,
    backgroundColor: colors.accentSoft,
  },
  templateChipText: {
    fontSize: 11,
    fontWeight: '600',
    color: colors.accent,
  },
  generateCard: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 8,
    minHeight: 48,
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.chipActiveBorder,
    borderStyle: 'dashed',
    borderRadius: radius.inner,
    padding: 14,
    marginBottom: 12,
  },
  generateText: {
    fontSize: 14,
    fontWeight: '600',
    color: colors.accent,
  },
  analyticsCard: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.inner,
    padding: 14,
    marginBottom: 12,
  },
  ttRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    marginTop: 10,
  },
  ttName: {
    width: 84,
    fontSize: 12,
    fontWeight: '600',
  },
  ttBarTrack: {
    flex: 1,
    height: 6,
    borderRadius: 3,
    backgroundColor: colors.bg,
    overflow: 'hidden',
  },
  ttBarFill: {
    height: 6,
    borderRadius: 3,
  },
  ttValue: {
    width: 36,
    textAlign: 'right',
    fontSize: 11,
    fontWeight: '600',
    color: colors.textMuted,
    fontFamily: mono,
    fontVariant: ['tabular-nums'],
  },
  summaryCard: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.inner,
    padding: 14,
    marginBottom: 12,
  },
  summaryLabel: {
    fontSize: 10,
    fontWeight: '700',
    color: colors.accent,
    fontFamily: mono,
    letterSpacing: 2,
  },
  summaryTitle: {
    fontSize: 16,
    fontWeight: '700',
    color: colors.textPrimary,
    marginTop: 8,
  },
  summaryText: {
    fontSize: 12.5,
    lineHeight: 19,
    color: colors.textLight,
    marginTop: 8,
  },
  tagRow: {
    flexDirection: 'row',
    gap: 6,
    marginTop: 11,
  },
  tagAccent: {
    fontSize: 10,
    fontWeight: '600',
    color: colors.accent,
    fontFamily: mono,
    backgroundColor: colors.accentSoft,
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 6,
    overflow: 'hidden',
  },
  summarySub: {
    fontSize: 10,
    fontWeight: '700',
    color: colors.textMuted,
    fontFamily: mono,
    letterSpacing: 1.5,
    marginTop: 14,
    marginBottom: 6,
  },
  bulletRow: {
    flexDirection: 'row',
    gap: 8,
    marginBottom: 5,
  },
  bullet: {
    width: 5,
    height: 5,
    borderRadius: 3,
    backgroundColor: colors.accent,
    marginTop: 7,
  },
  bulletText: {
    flex: 1,
    fontSize: 13,
    lineHeight: 20,
    color: colors.textLight,
  },
  searchBar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: 11,
    paddingHorizontal: 12,
    paddingVertical: 9,
    marginBottom: 10,
  },
  searchInput: {
    flex: 1,
    fontSize: 14,
    color: colors.textPrimary,
    padding: 0,
  },
  emptyTranscript: {
    fontSize: 14,
    color: colors.textDim,
    textAlign: 'center',
    marginTop: 32,
  },
  emptySearch: {
    fontSize: 14,
    color: colors.textDim,
    textAlign: 'center',
    marginTop: 16,
  },
  utterance: {
    flexDirection: 'row',
    gap: 11,
    paddingVertical: 9,
    paddingHorizontal: 10,
    marginHorizontal: -10,
    borderRadius: 10,
    marginBottom: 2,
  },
  utteranceActive: {
    backgroundColor: colors.accentFaint,
  },
  rail: {
    width: 3,
    borderRadius: 2,
  },
  utteranceBody: {
    flex: 1,
  },
  utteranceHeader: {
    flexDirection: 'row',
    alignItems: 'baseline',
    gap: 8,
  },
  speakerName: {
    fontSize: 12,
    fontWeight: '700',
  },
  utteranceTime: {
    fontSize: 10,
    color: colors.textDim,
    fontFamily: mono,
  },
  utteranceText: {
    fontSize: 13,
    lineHeight: 20,
    color: colors.textLight,
    marginTop: 4,
  },
  utteranceTextActive: {
    color: colors.textPrimary,
  },
  /** The word being spoken — the same wash the in-transcript search uses. */
  wordActive: {
    color: colors.markText,
    backgroundColor: colors.markBg,
    fontWeight: '700',
  },
  /** A word a mark was placed on: underlined, so it reads even when unplayed. */
  wordMarked: {
    color: colors.amber,
    textDecorationLine: 'underline',
    textDecorationColor: colors.amber,
  },
  /** The ⚑ that sits immediately before a marked word. */
  markGlyph: {
    color: colors.amber,
    fontSize: 11,
  },
  /** ⚑ badge in a transcript line's header, for scanning without playback. */
  rowMarkChip: {
    fontSize: 10,
    fontWeight: '700',
    color: colors.amber,
    fontFamily: mono,
  },
  markChipGlyph: {
    fontSize: 11,
    color: colors.accent,
  },
  followPill: {
    position: 'absolute',
    alignSelf: 'center',
    bottom: 22,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    backgroundColor: colors.accent,
    borderRadius: radius.pill,
    paddingHorizontal: 14,
    paddingVertical: 9,
  },
  followPillText: {
    fontSize: 12,
    fontWeight: '700',
    color: colors.white,
  },
  // Modal
  modalOverlay: {
    flex: 1,
    backgroundColor: colors.scrim,
    alignItems: 'center',
    justifyContent: 'center',
  },
  modalCard: {
    width: '82%',
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.borderButton,
    borderRadius: radius.card,
    padding: 20,
  },
  modalTitle: {
    fontSize: 16,
    fontWeight: '700',
    color: colors.textPrimary,
    marginBottom: 14,
  },
  // Export sheet
  sheetCard: {
    position: 'absolute',
    left: 0,
    right: 0,
    bottom: 0,
    backgroundColor: colors.surface,
    borderTopWidth: 1,
    borderColor: colors.borderButton,
    borderTopLeftRadius: radius.card,
    borderTopRightRadius: radius.card,
    padding: 20,
    paddingBottom: 34,
  },
  sheetRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    paddingVertical: 13,
    borderTopWidth: 1,
    borderColor: colors.borderDivider,
  },
  sheetRowText: {
    flex: 1,
  },
  sheetRowLabel: {
    fontSize: 15,
    fontWeight: '600',
    color: colors.textPrimary,
  },
  sheetRowSub: {
    fontSize: 12,
    color: colors.textMuted,
    fontFamily: mono,
    marginTop: 2,
  },
  // Edit-utterance modal
  editInput: {
    backgroundColor: colors.bg,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: 11,
    paddingHorizontal: 13,
    paddingVertical: 11,
    fontSize: 15,
    lineHeight: 21,
    color: colors.textPrimary,
    marginBottom: 14,
    minHeight: 90,
    textAlignVertical: 'top',
  },
  // Tags editor
  tagsCard: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.inner,
    padding: 14,
    marginBottom: 12,
  },
  tagsWrap: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 8,
    marginTop: 10,
  },
  tagChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    backgroundColor: colors.accentSoft,
    borderRadius: 8,
    paddingLeft: 10,
    paddingRight: 7,
    paddingVertical: 5,
  },
  tagChipText: {
    fontSize: 12.5,
    fontWeight: '500',
    color: colors.accent,
  },
  tagInput: {
    minWidth: 80,
    backgroundColor: colors.bg,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: 8,
    paddingHorizontal: 10,
    paddingVertical: 5,
    fontSize: 12.5,
    color: colors.textPrimary,
  },
  tagAdd: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 3,
    borderWidth: 1,
    borderColor: colors.chipActiveBorder,
    borderStyle: 'dashed',
    borderRadius: 8,
    paddingHorizontal: 10,
    paddingVertical: 5,
    minHeight: 28,
  },
  tagAddText: {
    fontSize: 12.5,
    fontWeight: '500',
    color: colors.accent,
  },
  // Highlights / marks
  marksCard: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.inner,
    padding: 14,
    marginBottom: 12,
  },
  marksRow: {
    gap: 8,
    marginTop: 10,
    paddingRight: 4,
  },
  markChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    backgroundColor: colors.accentSoft,
    borderRadius: 8,
    paddingHorizontal: 10,
    paddingVertical: 6,
  },
  markChipText: {
    fontSize: 12,
    fontWeight: '600',
    color: colors.accent,
    fontFamily: mono,
    fontVariant: ['tabular-nums'],
  },
  modalHint: {
    fontSize: 12,
    lineHeight: 17,
    color: colors.textMuted,
    marginTop: 6,
  },
  countRow: {
    flexDirection: 'row',
    gap: 8,
    marginTop: 14,
  },
  countChip: {
    flex: 1,
    alignItems: 'center',
    paddingVertical: 10,
    borderRadius: radius.inner,
    borderWidth: 1,
    borderColor: colors.borderInput,
    backgroundColor: colors.bg,
  },
  countChipActive: {
    backgroundColor: colors.accentSoft,
    borderColor: colors.chipActiveBorder,
  },
  countChipText: {
    fontSize: 15,
    fontWeight: '700',
    color: colors.textMuted,
    fontFamily: mono,
  },
  countChipTextActive: {
    color: colors.accent,
  },
  modalInputCompact: {
    backgroundColor: colors.bg,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: radius.inner,
    paddingHorizontal: 12,
    paddingVertical: 9,
    fontSize: 14,
    color: colors.textPrimary,
    marginTop: 10,
    marginBottom: 14,
  },
  dimmedAction: {
    opacity: 0.5,
  },
  modalActions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: 8,
    alignItems: 'center',
  },
  modalCancel: {
    paddingHorizontal: 16,
    paddingVertical: 9,
  },
  modalCancelText: {
    color: colors.textMuted,
    fontSize: 14,
  },
  modalSave: {
    backgroundColor: colors.accent,
    borderRadius: 10,
    paddingHorizontal: 18,
    paddingVertical: 9,
  },
  modalSaveText: {
    color: colors.white,
    fontWeight: '600',
    fontSize: 14,
  },
});
