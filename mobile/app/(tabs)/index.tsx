/**
 * Screen 1: Record
 *
 * Big record/stop button, live elapsed timer, segment+upload indicator,
 * optional title input, and expected-participant-count field.
 *
 * Design §11: "big record button, live timer, segment/upload indicator,
 * a field for expected participant count (feeds diarization), optional title."
 *
 * Background-recording caveats:
 *   iOS:     Recording continues when the app is backgrounded (UIBackgroundModes
 *            audio is set).  Do NOT start a recording while already backgrounded.
 *   Android: The foreground service (FOREGROUND_SERVICE_MICROPHONE) keeps
 *            recording alive.  Same restriction: start in foreground only.
 *            See design §11, §16 and app.json.
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
} from 'react-native';
import { RecordingSession } from '../../src/recording/recordingSession';
import { useSettingsStore } from '../../src/state/settingsStore';
import type { SessionEvent, SessionState } from '../../src/recording/recordingSession';

// ---------------------------------------------------------------------------

function formatElapsed(ms: number): string {
  const totalSecs = Math.floor(ms / 1000);
  const h = Math.floor(totalSecs / 3600);
  const m = Math.floor((totalSecs % 3600) / 60);
  const s = totalSecs % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${pad(h)}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
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

  const sessionRef = useRef<RecordingSession | null>(null);

  // -------------------------------------------------------------------------

  useEffect(() => {
    return () => {
      // Clean up on unmount (e.g. navigating away mid-recording)
      if (sessionRef.current?.state === 'recording') {
        sessionRef.current.stop().catch(console.warn);
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

    try {
      await session.start({
        title: title.trim() || undefined,
        participantsExpected: parseInt(participants, 10) || undefined,
      });
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

  // -------------------------------------------------------------------------

  const isRecording = sessionState === 'recording';
  const isBusy = sessionState === 'starting' || sessionState === 'stopping';

  return (
    <ScrollView
      contentContainerStyle={styles.container}
      keyboardShouldPersistTaps="handled"
    >
      {/* Timer */}
      <Text style={styles.timer}>{formatElapsed(elapsedMs)}</Text>

      {/* Segment / upload indicator */}
      {isRecording && (
        <Text style={styles.segmentIndicator}>
          {uploadedCount}/{segmentCount} segments uploaded
        </Text>
      )}

      {/* Session state label */}
      {isBusy && (
        <Text style={styles.statusLabel}>
          {sessionState === 'starting' ? 'Starting…' : 'Stopping…'}
        </Text>
      )}
      {sessionState === 'done' && (
        <Text style={styles.statusLabel}>Recording saved — processing on server</Text>
      )}
      {sessionState === 'error' && (
        <Text style={[styles.statusLabel, styles.errorLabel]}>Error</Text>
      )}

      {/* Big record/stop button */}
      <TouchableOpacity
        style={[
          styles.recordButton,
          isRecording && styles.recordButtonActive,
          isBusy && styles.recordButtonBusy,
        ]}
        onPress={isRecording ? stopRecording : startRecording}
        disabled={isBusy}
        accessibilityLabel={isRecording ? 'Stop recording' : 'Start recording'}
        accessibilityRole="button"
      >
        <View
          style={[
            styles.recordButtonInner,
            isRecording && styles.stopShape,
          ]}
        />
      </TouchableOpacity>
      <Text style={styles.recordButtonLabel}>
        {isRecording ? 'Tap to stop' : 'Tap to record'}
      </Text>

      {/* Fields — only editable when not recording */}
      {!isRecording && (
        <View style={styles.fields}>
          <Text style={styles.fieldLabel}>Title (optional)</Text>
          <TextInput
            style={styles.input}
            placeholder="e.g. Sprint planning"
            value={title}
            onChangeText={setTitle}
            returnKeyType="done"
            editable={!isBusy}
          />

          <Text style={styles.fieldLabel}>Expected participants</Text>
          <TextInput
            style={styles.input}
            placeholder="2"
            value={participants}
            onChangeText={setParticipants}
            keyboardType="number-pad"
            returnKeyType="done"
            editable={!isBusy}
          />
          <Text style={styles.hint}>
            Providing the participant count improves speaker diarisation accuracy.
          </Text>
        </View>
      )}

      {/* iOS / Android background-recording notice */}
      {isRecording && (
        <Text style={styles.backgroundNotice}>
          {Platform.OS === 'ios'
            ? 'Recording continues in background. Incoming calls will briefly pause capture.'
            : 'Recording is kept alive by a foreground service. Do not force-kill the app.'}
        </Text>
      )}
    </ScrollView>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  container: {
    flexGrow: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 24,
    backgroundColor: '#fff',
  },
  timer: {
    fontSize: 56,
    fontVariant: ['tabular-nums'],
    fontWeight: '200',
    color: '#111',
    marginBottom: 8,
  },
  segmentIndicator: {
    fontSize: 13,
    color: '#777',
    marginBottom: 24,
  },
  statusLabel: {
    fontSize: 14,
    color: '#555',
    marginBottom: 16,
  },
  errorLabel: {
    color: '#E53935',
  },
  recordButton: {
    width: 96,
    height: 96,
    borderRadius: 48,
    backgroundColor: '#E53935',
    alignItems: 'center',
    justifyContent: 'center',
    marginBottom: 12,
    shadowColor: '#E53935',
    shadowOffset: { width: 0, height: 4 },
    shadowOpacity: 0.4,
    shadowRadius: 8,
    elevation: 6,
  },
  recordButtonActive: {
    backgroundColor: '#B71C1C',
  },
  recordButtonBusy: {
    opacity: 0.5,
  },
  recordButtonInner: {
    width: 36,
    height: 36,
    borderRadius: 18,
    backgroundColor: '#fff',
  },
  stopShape: {
    borderRadius: 6,
  },
  recordButtonLabel: {
    fontSize: 13,
    color: '#888',
    marginBottom: 32,
  },
  fields: {
    width: '100%',
    maxWidth: 400,
  },
  fieldLabel: {
    fontSize: 13,
    color: '#555',
    marginBottom: 4,
    marginTop: 16,
  },
  input: {
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 10,
    fontSize: 16,
    color: '#111',
  },
  hint: {
    fontSize: 12,
    color: '#aaa',
    marginTop: 6,
  },
  backgroundNotice: {
    fontSize: 12,
    color: '#aaa',
    textAlign: 'center',
    maxWidth: 300,
    marginTop: 24,
    lineHeight: 18,
  },
});
