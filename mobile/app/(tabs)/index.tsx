/**
 * Screen 1: Record  — Kiln recording moment.
 *
 * Two layouts, matching the design:
 *   - Immersive (live-transcript OFF / idle): the big ember orb is the hero,
 *     a large mono timer beneath it, and a Pause · Stop · Mark control cluster.
 *   - Live transcription ON: a compact small-orb header + timer + LIVE chip,
 *     the streaming transcript filling the screen (dim history → bright newest
 *     with a blinking caret), then the toggle and the control cluster.
 *
 * Pause/Resume and Mark are wired to the recording session.  While recording,
 * segments are transcribed server-side and this screen polls for the
 * accumulating transcript; provisional utterances are joined into flowing text
 * so a sentence split across a segment boundary reads as one (speaker labels
 * and final sentence segmentation arrive once processing completes).
 */

import { useState, useEffect, useRef, useCallback } from 'react';
import {
  View,
  Text,
  StyleSheet,
  TextInput,
  TouchableOpacity,
  Platform,
  Alert,
  ScrollView,
  Animated,
} from 'react-native';
import { RecordingSession } from '../../src/recording/recordingSession';
import { useSettingsStore } from '../../src/state/settingsStore';
import type { SessionEvent, SessionState } from '../../src/recording/recordingSession';
import { api } from '../../src/api/client';
import type { Utterance } from '../../src/types';
import { EmberOrb } from '../../src/components/EmberOrb';
import { onLiveActivityAction } from '../../modules/scribe-live-activity';
import { Screen } from '../../src/components/ui';
import { colors, mono } from '../../src/theme';

// ---------------------------------------------------------------------------

function formatElapsed(ms: number): string {
  const totalSecs = Math.floor(ms / 1000);
  const h = Math.floor(totalSecs / 3600);
  const m = Math.floor((totalSecs % 3600) / 60);
  const s = totalSecs % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${pad(h)}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

/** Join provisional utterances into one flowing string (merges segment splits). */
function flowingText(utterances: Utterance[]): string {
  return utterances
    .map((u) => u.text.trim())
    .filter(Boolean)
    .join(' ')
    .replace(/\s+/g, ' ')
    .trim();
}

// ---------------------------------------------------------------------------

export default function RecordScreen() {
  const { defaultParticipants, baseUrl } = useSettingsStore();

  const [sessionState, setSessionState] = useState<SessionState>('idle');
  const [elapsedMs, setElapsedMs] = useState(0);
  const [segmentCount, setSegmentCount] = useState(0);
  const [uploadedCount, setUploadedCount] = useState(0);
  const [title, setTitle] = useState('');
  const [participants, setParticipants] = useState(String(defaultParticipants));
  const [liveTranscript, setLiveTranscript] = useState(true);
  const [recordingId, setRecordingId] = useState<string | null>(null);
  const [liveUtterances, setLiveUtterances] = useState<Utterance[]>([]);
  const [markCount, setMarkCount] = useState(0);

  const sessionRef = useRef<RecordingSession | null>(null);
  const transcriptRef = useRef<ScrollView>(null);

  // -------------------------------------------------------------------------

  useEffect(() => {
    return () => {
      const s = sessionRef.current;
      if (s && (s.state === 'recording' || s.state === 'paused')) {
        s.stop().catch(console.warn);
      }
    };
  }, []);

  const handleEvent = useCallback((event: SessionEvent) => {
    if (event.type === 'state') setSessionState(event.state);
    if (event.type === 'tick') setElapsedMs(event.elapsedMs);
    if (event.type === 'segment') {
      if (!event.uploaded) setSegmentCount((c) => c + 1);
      else setUploadedCount((c) => c + 1);
    }
    if (event.type === 'error') {
      Alert.alert('Recording error', event.error.message);
    }
  }, []);

  const startRecording = useCallback(async () => {
    if (!baseUrl) {
      Alert.alert(
        'Not configured',
        'Please set your Scribe server URL in Settings before recording.',
      );
      return;
    }

    const session = new RecordingSession();
    session.on(handleEvent);
    sessionRef.current = session;

    setElapsedMs(0);
    setSegmentCount(0);
    setUploadedCount(0);
    setLiveUtterances([]);
    setMarkCount(0);
    setRecordingId(null);

    try {
      await session.start({
        title: title.trim() || undefined,
        participantsExpected: parseInt(participants, 10) || undefined,
      });
      setRecordingId(session.recordingId);
    } catch (err) {
      Alert.alert('Could not start recording', String(err));
    }
  }, [baseUrl, title, participants, handleEvent]);

  const stopRecording = useCallback(async () => {
    if (!sessionRef.current) return;
    try {
      await sessionRef.current.stop();
    } catch (err) {
      Alert.alert('Could not stop recording', String(err));
    }
  }, []);

  const togglePause = useCallback(async () => {
    const s = sessionRef.current;
    if (!s) return;
    try {
      if (s.state === 'recording') await s.pause();
      else if (s.state === 'paused') await s.resume();
    } catch (err) {
      Alert.alert('Pause failed', String(err));
    }
  }, []);

  // Pause/Resume and Stop pressed on the Live Activity (Lock Screen or Dynamic
  // Island). iOS performs the LiveActivityIntent in this process, so it can
  // drive the same session the on-screen buttons do — no separate code path,
  // and no risk of the two disagreeing about state.
  useEffect(() => {
    return onLiveActivityAction((action) => {
      if (action === 'stop') void stopRecording();
      else if (action === 'togglePause') void togglePause();
    });
  }, [stopRecording, togglePause]);

  const doMark = useCallback(() => {
    const at = sessionRef.current?.mark();
    if (at != null) setMarkCount((c) => c + 1);
  }, []);

  // -------------------------------------------------------------------------

  const isRecording = sessionState === 'recording';
  const isPaused = sessionState === 'paused';
  const isActive = isRecording || isPaused;
  const isBusy = sessionState === 'starting' || sessionState === 'stopping';
  const liveMode = isActive && liveTranscript;

  // Live transcript polling.
  useEffect(() => {
    if (!isActive || !recordingId || !liveTranscript) return;
    let cancelled = false;
    const poll = async () => {
      try {
        const res = await api.getRecording(recordingId);
        if (!cancelled) setLiveUtterances(res.utterances ?? []);
      } catch {
        // transient — keep last transcript
      }
    };
    poll();
    const interval = setInterval(poll, 2500);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [isActive, recordingId, liveTranscript]);

  const participantsNum = parseInt(participants, 10) || 0;
  const liveText = flowingText(liveUtterances);

  // Keep the streaming transcript scrolled to the newest words.
  useEffect(() => {
    if (liveMode) transcriptRef.current?.scrollToEnd({ animated: true });
  }, [liveText, liveMode]);

  // -------------------------------------------------------------------------
  // Shared control cluster (Pause · Stop · Mark / Stop · Resume · Mark / Record)
  // -------------------------------------------------------------------------

  const renderCluster = () => {
    if (!isActive) {
      return (
        <View style={styles.clusterCenter}>
          <View style={styles.slot}>
            <TouchableOpacity
              style={[styles.recordButton, isBusy && styles.dimmed]}
              onPress={startRecording}
              disabled={isBusy}
              accessibilityLabel="Start recording"
            >
              <View style={styles.recordDot} />
            </TouchableOpacity>
          </View>
          <Text style={[styles.controlLabel, styles.controlLabelActive]}>
            {isBusy ? '…' : 'RECORD'}
          </Text>
        </View>
      );
    }
    return (
      <>
        {isPaused ? (
          <ClusterButton label="STOP" onPress={stopRecording} disabled={isBusy}>
            <View style={styles.stopSmallSquare} />
          </ClusterButton>
        ) : (
          <ClusterButton label="PAUSE" onPress={togglePause}>
            <View style={styles.pauseIcon}>
              <View style={styles.pauseBar} />
              <View style={styles.pauseBar} />
            </View>
          </ClusterButton>
        )}

        <View style={styles.clusterCenter}>
          <View style={styles.slot}>
            {isPaused ? (
              <TouchableOpacity
                style={styles.resumeButton}
                onPress={togglePause}
                accessibilityLabel="Resume recording"
              >
                <View style={styles.playTriangle} />
              </TouchableOpacity>
            ) : (
              <TouchableOpacity
                style={styles.stopButton}
                onPress={stopRecording}
                disabled={isBusy}
                accessibilityLabel="Stop recording"
              >
                <View style={styles.stopSquare} />
              </TouchableOpacity>
            )}
          </View>
          <Text style={[styles.controlLabel, styles.controlLabelActive]}>
            {isPaused ? 'RESUME' : 'STOP'}
          </Text>
        </View>

        <ClusterButton label="MARK" onPress={doMark}>
          <View style={styles.markIcon} />
        </ClusterButton>
      </>
    );
  };

  // -------------------------------------------------------------------------
  // Live-transcription layout
  // -------------------------------------------------------------------------

  if (liveMode) {
    const words = liveText ? liveText.split(' ') : [];
    const TAIL = 16;
    const head = words.slice(0, Math.max(0, words.length - TAIL)).join(' ');
    const tail = words.slice(Math.max(0, words.length - TAIL)).join(' ');

    return (
      <Screen>
        <View style={styles.liveRoot}>
          {/* Compact header */}
          <View style={styles.liveHeader}>
            <View style={styles.liveOrb}>
              <EmberOrb size={58} variant="small" seed={2.2} />
            </View>
            <View style={styles.liveHeaderMid}>
              <Text style={[styles.liveTimer, isPaused && styles.timerPaused]}>
                {formatElapsed(elapsedMs)}
              </Text>
              <Text style={styles.liveSub} numberOfLines={1}>
                {isPaused ? 'Paused' : 'Listening'} · {title.trim() || 'Untitled'}
              </Text>
            </View>
            <View style={[styles.liveChip, isPaused && styles.liveChipPaused]}>
              {!isPaused && <View style={styles.liveChipDot} />}
              <Text style={[styles.liveChipText, isPaused && { color: colors.amber }]}>
                {isPaused ? 'PAUSED' : 'LIVE'}
              </Text>
            </View>
          </View>

          {/* Streaming transcript */}
          <View style={styles.transcriptWrap}>
            <ScrollView
              ref={transcriptRef}
              contentContainerStyle={styles.transcriptContent}
              showsVerticalScrollIndicator={false}
            >
              {liveText ? (
                <Text style={styles.transcriptParagraph}>
                  {head ? <Text style={styles.transcriptDim}>{head} </Text> : null}
                  <Text style={styles.transcriptBright}>{tail}</Text>
                  {!isPaused && <BlinkingCaret />}
                </Text>
              ) : (
                <Text style={styles.liveWaiting}>
                  Listening… the transcript appears a few seconds after the first segment uploads.
                </Text>
              )}
            </ScrollView>
            <View style={styles.transcriptFade} pointerEvents="none" />
          </View>

          {/* Toggle ON */}
          <View style={styles.liveToggleRow}>
            <Text style={[styles.toggleLabel, styles.toggleLabelOn]}>Live transcript</Text>
            <Toggle value onChange={() => setLiveTranscript(false)} />
          </View>

          {/* Cluster */}
          <View style={[styles.cluster, styles.liveCluster]}>{renderCluster()}</View>
        </View>
      </Screen>
    );
  }

  // -------------------------------------------------------------------------
  // Immersive layout (orb hero)
  // -------------------------------------------------------------------------

  const subtitle = isActive
    ? `${title.trim() || 'Untitled'}${participantsNum ? ` · ~${participantsNum} speakers` : ''}`
    : baseUrl
      ? 'Ready when you are'
      : 'Configure your server in Settings';

  return (
    <Screen>
      <ScrollView contentContainerStyle={styles.container} keyboardShouldPersistTaps="handled">
        {/* Mono status header */}
        <View style={styles.recHeader}>
          {isRecording ? (
            <Text style={styles.recHeaderActive}>
              <Text style={styles.recDot}>● </Text>REC · 16kHz AAC
            </Text>
          ) : isPaused ? (
            <Text style={[styles.recHeaderActive, { color: colors.amber }]}>❚❚ PAUSED</Text>
          ) : isBusy ? (
            <Text style={styles.recHeaderMuted}>
              {sessionState === 'starting' ? '… STARTING' : '… STOPPING'}
            </Text>
          ) : sessionState === 'done' ? (
            <Text style={styles.recHeaderActive}>✓ SAVED · PROCESSING</Text>
          ) : sessionState === 'error' ? (
            <Text style={[styles.recHeaderMuted, { color: colors.accentDeep }]}>! ERROR</Text>
          ) : (
            <Text style={styles.recHeaderMuted}>READY · 16kHz AAC</Text>
          )}
          <View style={styles.recHeaderRight}>
            {markCount > 0 && <Text style={styles.markChip}>⚑ {markCount}</Text>}
            {isActive && (
              <Text style={styles.recHeaderMuted}>
                SEG {uploadedCount}/{segmentCount}
              </Text>
            )}
          </View>
        </View>

        {/* Ember orb hero */}
        <View style={[styles.orbWrap, isPaused && styles.orbPaused]}>
          <EmberOrb size={220} variant="big" seed={2.2} />
        </View>

        {/* Timer + subtitle */}
        <Text style={[styles.timer, isPaused && styles.timerPaused]}>{formatElapsed(elapsedMs)}</Text>
        <Text style={styles.subtitle}>{subtitle}</Text>

        {/* Live-transcript toggle (active only) */}
        {isActive && (
          <View style={styles.toggleRow}>
            <Text style={[styles.toggleLabel, liveTranscript && styles.toggleLabelOn]}>
              Live transcript
            </Text>
            <Toggle value={liveTranscript} onChange={() => setLiveTranscript((v) => !v)} />
          </View>
        )}

        {/* Control cluster */}
        <View style={styles.cluster}>{renderCluster()}</View>

        {/* Fields — only when not actively recording */}
        {!isActive && (
          <View style={styles.fields}>
            <Text style={styles.fieldLabel}>TITLE</Text>
            <TextInput
              style={styles.input}
              placeholder="e.g. Sprint planning"
              placeholderTextColor={colors.textDim}
              value={title}
              onChangeText={setTitle}
              returnKeyType="done"
              editable={!isBusy}
            />

            <Text style={styles.fieldLabel}>EXPECTED SPEAKERS · OPTIONAL HINT</Text>
            <TextInput
              style={styles.input}
              placeholder="2"
              placeholderTextColor={colors.textDim}
              value={participants}
              onChangeText={setParticipants}
              keyboardType="number-pad"
              returnKeyType="done"
              editable={!isBusy}
            />
            <Text style={styles.hint}>
              Just a hint to help diarisation — Scribe still adapts if more or fewer people
              actually speak.
            </Text>
          </View>
        )}

        {/* Background-recording notice */}
        {isActive && (
          <Text style={styles.backgroundNotice}>
            {Platform.OS === 'ios'
              ? 'Recording continues in the background. Incoming calls briefly pause capture.'
              : 'Recording is kept alive by a foreground service. Do not force-kill the app.'}
          </Text>
        )}
      </ScrollView>
    </Screen>
  );
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

function BlinkingCaret() {
  const op = useRef(new Animated.Value(1)).current;
  useEffect(() => {
    const loop = Animated.loop(
      Animated.sequence([
        Animated.timing(op, { toValue: 0.15, duration: 480, useNativeDriver: false }),
        Animated.timing(op, { toValue: 1, duration: 480, useNativeDriver: false }),
      ]),
    );
    loop.start();
    return () => loop.stop();
  }, [op]);
  return <Animated.Text style={[styles.caret, { opacity: op }]}>▌</Animated.Text>;
}

function Toggle({ value, onChange }: { value: boolean; onChange: () => void }) {
  return (
    <TouchableOpacity
      style={[styles.toggle, value ? styles.toggleOn : styles.toggleOff]}
      onPress={onChange}
      accessibilityRole="switch"
      accessibilityState={{ checked: value }}
    >
      <View style={[styles.toggleKnob, value ? styles.toggleKnobOn : styles.toggleKnobOffPos]} />
    </TouchableOpacity>
  );
}

function ClusterButton({
  label,
  children,
  onPress,
  disabled,
}: {
  label: string;
  children: React.ReactNode;
  onPress: () => void;
  disabled?: boolean;
}) {
  return (
    <View style={styles.clusterCenter}>
      <View style={styles.slot}>
        <TouchableOpacity
          style={[styles.sideButton, disabled && styles.dimmed]}
          onPress={onPress}
          disabled={disabled}
          accessibilityLabel={label}
        >
          {children}
        </TouchableOpacity>
      </View>
      <Text style={styles.controlLabel}>{label}</Text>
    </View>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  container: {
    flexGrow: 1,
    alignItems: 'center',
    paddingHorizontal: 26,
    paddingTop: 18,
    paddingBottom: 40,
  },
  // Immersive header -------------------------------------------------------
  recHeader: {
    width: '100%',
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginBottom: 8,
  },
  recHeaderRight: { flexDirection: 'row', alignItems: 'center', gap: 10 },
  recHeaderActive: {
    fontSize: 11,
    fontWeight: '600',
    color: colors.accent,
    fontFamily: mono,
    letterSpacing: 0.5,
  },
  recDot: { color: colors.accent },
  recHeaderMuted: {
    fontSize: 11,
    fontWeight: '600',
    color: colors.textMuted,
    fontFamily: mono,
    letterSpacing: 0.5,
  },
  markChip: { fontSize: 11, fontWeight: '700', color: colors.accent, fontFamily: mono },
  orbWrap: { marginTop: 24, width: 220, height: 220 },
  orbPaused: { opacity: 0.45 },
  timer: {
    fontSize: 58,
    fontWeight: '300',
    lineHeight: 62,
    color: colors.textPrimary,
    fontVariant: ['tabular-nums'],
    fontFamily: mono,
    letterSpacing: -1.5,
    marginTop: 24,
  },
  timerPaused: { color: colors.textSnippet },
  subtitle: { marginTop: 10, fontSize: 12, fontWeight: '600', color: colors.textMuted, letterSpacing: 0.5 },
  // Live layout ------------------------------------------------------------
  liveRoot: { flex: 1, paddingHorizontal: 22, paddingTop: 8 },
  liveHeader: { flexDirection: 'row', alignItems: 'center', gap: 13, paddingBottom: 12 },
  liveOrb: { width: 58, height: 58 },
  liveHeaderMid: { flex: 1, minWidth: 0 },
  liveTimer: {
    fontSize: 26,
    fontWeight: '300',
    color: colors.textPrimary,
    fontVariant: ['tabular-nums'],
    fontFamily: mono,
    letterSpacing: -1,
  },
  liveSub: { marginTop: 4, fontSize: 11, fontWeight: '600', color: colors.textMuted, letterSpacing: 0.4 },
  liveChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    backgroundColor: colors.accentSoft,
    borderColor: colors.chipActiveBorder,
    borderWidth: 1,
    paddingHorizontal: 10,
    paddingVertical: 5,
    borderRadius: 20,
  },
  liveChipPaused: { backgroundColor: 'rgba(226,168,95,0.12)', borderColor: 'rgba(226,168,95,0.3)' },
  liveChipDot: { width: 7, height: 7, borderRadius: 4, backgroundColor: colors.accent },
  liveChipText: { fontSize: 10, fontWeight: '700', color: colors.accent, fontFamily: mono, letterSpacing: 0.5 },
  transcriptWrap: { flex: 1, position: 'relative' },
  transcriptContent: { paddingVertical: 8, flexGrow: 1, justifyContent: 'flex-end' },
  transcriptFade: {
    position: 'absolute',
    top: 0,
    left: 0,
    right: 0,
    height: 28,
    backgroundColor: 'transparent',
  },
  transcriptParagraph: { fontSize: 16, lineHeight: 26 },
  transcriptDim: { color: colors.textDim },
  transcriptBright: { color: colors.textPrimary },
  caret: { color: colors.accent, fontSize: 16 },
  liveWaiting: { fontSize: 13, lineHeight: 20, color: colors.textMuted, textAlign: 'center' },
  liveToggleRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 10,
    paddingVertical: 12,
    borderTopWidth: 1,
    borderTopColor: colors.border,
  },
  // Toggle ----------------------------------------------------------------
  toggleRow: { flexDirection: 'row', alignItems: 'center', gap: 10, marginTop: 24 },
  toggleLabel: { fontSize: 12, fontWeight: '600', color: colors.textMuted },
  toggleLabelOn: { color: colors.accent },
  toggle: { width: 40, height: 23, borderRadius: 12, justifyContent: 'center' },
  toggleOn: { backgroundColor: colors.accent },
  toggleOff: { backgroundColor: colors.toggleOff },
  toggleKnob: { width: 18, height: 18, borderRadius: 9, position: 'absolute' },
  toggleKnobOn: { right: 2.5, backgroundColor: colors.white },
  toggleKnobOffPos: { left: 2.5, backgroundColor: colors.toggleKnobOff },
  // Cluster ---------------------------------------------------------------
  cluster: { flexDirection: 'row', alignItems: 'flex-start', justifyContent: 'center', gap: 30, marginTop: 30 },
  liveCluster: { marginTop: 0, paddingBottom: 24 },
  clusterCenter: { alignItems: 'center', gap: 8 },
  slot: { height: 74, justifyContent: 'center', alignItems: 'center' },
  controlLabel: { fontSize: 10, fontWeight: '600', color: colors.textMuted, fontFamily: mono },
  controlLabelActive: { color: colors.accent },
  dimmed: { opacity: 0.45 },
  sideButton: {
    width: 56,
    height: 56,
    borderRadius: 28,
    borderWidth: 1,
    borderColor: colors.borderButton,
    backgroundColor: colors.surface,
    alignItems: 'center',
    justifyContent: 'center',
  },
  pauseIcon: { flexDirection: 'row', gap: 4 },
  pauseBar: { width: 5, height: 18, borderRadius: 2, backgroundColor: colors.textLight },
  markIcon: { width: 16, height: 20, borderRadius: 3, borderWidth: 2, borderColor: colors.textLight },
  stopSmallSquare: { width: 18, height: 18, borderRadius: 4, backgroundColor: colors.accent },
  stopButton: {
    width: 74,
    height: 74,
    borderRadius: 37,
    borderWidth: 2.5,
    borderColor: colors.accent,
    backgroundColor: colors.surface,
    alignItems: 'center',
    justifyContent: 'center',
    shadowColor: colors.accent,
    shadowOffset: { width: 0, height: 0 },
    shadowOpacity: 0.6,
    shadowRadius: 13,
    elevation: 8,
  },
  stopSquare: { width: 26, height: 26, borderRadius: 7, backgroundColor: colors.accent },
  resumeButton: {
    width: 74,
    height: 74,
    borderRadius: 37,
    backgroundColor: colors.accent,
    alignItems: 'center',
    justifyContent: 'center',
    shadowColor: colors.accent,
    shadowOffset: { width: 0, height: 0 },
    shadowOpacity: 0.6,
    shadowRadius: 13,
    elevation: 8,
  },
  playTriangle: {
    width: 0,
    height: 0,
    marginLeft: 5,
    borderTopWidth: 13,
    borderBottomWidth: 13,
    borderLeftWidth: 22,
    borderTopColor: 'transparent',
    borderBottomColor: 'transparent',
    borderLeftColor: colors.white,
  },
  recordButton: {
    width: 74,
    height: 74,
    borderRadius: 37,
    borderWidth: 2.5,
    borderColor: colors.accent,
    backgroundColor: colors.surface,
    alignItems: 'center',
    justifyContent: 'center',
    shadowColor: colors.accent,
    shadowOffset: { width: 0, height: 0 },
    shadowOpacity: 0.6,
    shadowRadius: 13,
    elevation: 8,
  },
  recordDot: { width: 30, height: 30, borderRadius: 15, backgroundColor: colors.accent },
  // Fields ----------------------------------------------------------------
  fields: { width: '100%', maxWidth: 460, marginTop: 32 },
  fieldLabel: {
    fontSize: 11,
    fontWeight: '700',
    color: colors.textMuted,
    fontFamily: mono,
    letterSpacing: 1.5,
    marginTop: 16,
    marginBottom: 7,
    paddingLeft: 2,
  },
  input: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: 13,
    paddingHorizontal: 14,
    paddingVertical: 12,
    fontSize: 15,
    color: colors.textPrimary,
  },
  hint: { fontSize: 12, color: colors.textDim, marginTop: 8, lineHeight: 17 },
  backgroundNotice: {
    fontSize: 12,
    color: colors.textDim,
    textAlign: 'center',
    maxWidth: 320,
    marginTop: 26,
    lineHeight: 18,
  },
});
