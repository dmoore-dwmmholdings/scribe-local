/**
 * Speakers — the tags that carry between recordings.
 *
 * Everyone you have ever named lives here. A speaker with a voiceprint is
 * matched during diarization, so recordings uploaded later come back already
 * labelled; a name-only speaker is just a label on the recordings it was
 * applied to. Renaming updates every recording at once, because transcripts
 * resolve names through the speaker id rather than storing a copy.
 */

import { useCallback, useEffect, useState } from 'react';
import {
  ActivityIndicator,
  Alert,
  Modal,
  RefreshControl,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  TouchableOpacity,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { api } from '../src/api/client';
import type { EnrolledSpeaker } from '../src/types';
import { Screen, ScreenHeader } from '../src/components/ui';
import { colors, mono, radius } from '../src/theme';
import { log } from '../src/util/logger';

export default function SpeakersScreen() {
  const [speakers, setSpeakers] = useState<EnrolledSpeaker[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<EnrolledSpeaker | null>(null);
  const [draftName, setDraftName] = useState('');
  const [busy, setBusy] = useState(false);

  const load = useCallback(async (background = false) => {
    if (!background) setLoading(true);
    setError(null);
    try {
      const res = await api.listSpeakers();
      setSpeakers(res.speakers ?? []);
    } catch (err) {
      log('speakers', 'list failed', err);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const submitRename = useCallback(async () => {
    const target = renaming;
    const name = draftName.trim();
    if (!target || !name || name === target.display_name) {
      setRenaming(null);
      return;
    }
    setBusy(true);
    try {
      const updated = await api.renameSpeaker(target.id, name);
      setSpeakers((list) => list.map((s) => (s.id === updated.id ? updated : s)));
      setRenaming(null);
    } catch (err) {
      Alert.alert('Could not rename', err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, [renaming, draftName]);

  const confirmDelete = useCallback(
    (speaker: EnrolledSpeaker) => {
      const where =
        speaker.recording_count > 0
          ? ` ${speaker.recording_count} recording${speaker.recording_count === 1 ? '' : 's'} will go back to "Speaker N".`
          : '';
      Alert.alert(
        `Forget ${speaker.display_name}?`,
        `The voiceprint is deleted, so this voice will not be recognised again.${where}`,
        [
          { text: 'Cancel', style: 'cancel' },
          {
            text: 'Forget',
            style: 'destructive',
            onPress: async () => {
              try {
                await api.deleteSpeaker(speaker.id);
                setSpeakers((list) => list.filter((s) => s.id !== speaker.id));
              } catch (err) {
                Alert.alert('Could not delete', err instanceof Error ? err.message : String(err));
              }
            },
          },
        ],
      );
    },
    [],
  );

  const enrolled = speakers.filter((s) => s.has_voiceprint).length;

  return (
    <Screen>
      <ScreenHeader title="Speakers" />
      <ScrollView
        contentContainerStyle={styles.content}
        refreshControl={
          <RefreshControl
            refreshing={refreshing}
            onRefresh={() => {
              setRefreshing(true);
              load(true);
            }}
            tintColor={colors.accent}
          />
        }
      >
        <Text style={styles.intro}>
          Names you give a speaker are stored here. The ones with a voiceprint are matched
          automatically in recordings you upload later.
        </Text>

        {loading ? (
          <ActivityIndicator color={colors.accent} style={styles.loading} />
        ) : error ? (
          <View style={styles.errorBox}>
            <Text style={styles.errorText}>{error}</Text>
            <TouchableOpacity style={styles.retry} onPress={() => load()}>
              <Text style={styles.retryText}>Retry</Text>
            </TouchableOpacity>
          </View>
        ) : speakers.length === 0 ? (
          <Text style={styles.empty}>
            No speakers yet. Open a recording, long-press a transcript line and tag the speaker to
            add one.
          </Text>
        ) : (
          <>
            <Text style={styles.count}>
              {speakers.length} SPEAKER{speakers.length === 1 ? '' : 'S'} · {enrolled} WITH A VOICEPRINT
            </Text>
            {speakers.map((s) => (
              <View key={s.id} style={styles.row}>
                <View style={[styles.avatar, s.has_voiceprint && styles.avatarEnrolled]}>
                  <Ionicons
                    name={s.has_voiceprint ? 'mic' : 'person-outline'}
                    size={16}
                    color={s.has_voiceprint ? colors.accent : colors.textMuted}
                  />
                </View>
                <View style={styles.rowText}>
                  <Text style={styles.name}>{s.display_name}</Text>
                  <Text style={styles.meta}>
                    {s.has_voiceprint ? 'Voice enrolled' : 'Name only'}
                    {' · '}
                    {s.recording_count} recording{s.recording_count === 1 ? '' : 's'}
                  </Text>
                </View>
                <TouchableOpacity
                  style={styles.iconBtn}
                  onPress={() => {
                    setDraftName(s.display_name);
                    setRenaming(s);
                  }}
                  accessibilityLabel={`Rename ${s.display_name}`}
                  hitSlop={8}
                >
                  <Ionicons name="pencil" size={16} color={colors.accent} />
                </TouchableOpacity>
                <TouchableOpacity
                  style={styles.iconBtn}
                  onPress={() => confirmDelete(s)}
                  accessibilityLabel={`Forget ${s.display_name}`}
                  hitSlop={8}
                >
                  <Ionicons name="trash-outline" size={16} color={colors.accentDeep} />
                </TouchableOpacity>
              </View>
            ))}
          </>
        )}
      </ScrollView>

      <Modal
        visible={renaming != null}
        transparent
        animationType="fade"
        onRequestClose={() => setRenaming(null)}
      >
        <View style={styles.overlay}>
          <View style={styles.card}>
            <Text style={styles.cardTitle}>Rename speaker</Text>
            <Text style={styles.cardHint}>
              The new name replaces the old one in every recording they appear in.
            </Text>
            <TextInput
              style={styles.input}
              value={draftName}
              onChangeText={setDraftName}
              autoFocus
              returnKeyType="done"
              onSubmitEditing={submitRename}
              placeholderTextColor={colors.textDim}
            />
            <View style={styles.cardActions}>
              <TouchableOpacity style={styles.cancel} onPress={() => setRenaming(null)}>
                <Text style={styles.cancelText}>Cancel</Text>
              </TouchableOpacity>
              <TouchableOpacity
                style={[styles.save, (!draftName.trim() || busy) && styles.dimmed]}
                onPress={submitRename}
                disabled={!draftName.trim() || busy}
              >
                {busy ? (
                  <ActivityIndicator size="small" color={colors.white} />
                ) : (
                  <Text style={styles.saveText}>Save</Text>
                )}
              </TouchableOpacity>
            </View>
          </View>
        </View>
      </Modal>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: 18, paddingBottom: 40, paddingTop: 6 },
  intro: {
    fontSize: 13,
    lineHeight: 19,
    color: colors.textMuted,
    marginBottom: 16,
  },
  count: {
    fontSize: 10,
    fontWeight: '700',
    letterSpacing: 1.4,
    fontFamily: mono,
    color: colors.textDim,
    marginBottom: 10,
  },
  loading: { paddingVertical: 30 },
  empty: {
    fontSize: 13,
    lineHeight: 19,
    color: colors.textMuted,
    paddingVertical: 24,
    textAlign: 'center',
  },
  errorBox: { alignItems: 'center', gap: 12, paddingVertical: 24 },
  errorText: { color: colors.accentDeep, fontSize: 13, textAlign: 'center' },
  retry: {
    backgroundColor: colors.accent,
    borderRadius: 10,
    paddingHorizontal: 18,
    paddingVertical: 8,
  },
  retryText: { color: colors.white, fontWeight: '600' },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 11,
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.inner,
    padding: 12,
    marginBottom: 8,
  },
  avatar: {
    width: 34,
    height: 34,
    borderRadius: 17,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: colors.chipInactiveBg,
  },
  avatarEnrolled: { backgroundColor: colors.accentFaint },
  rowText: { flex: 1 },
  name: { fontSize: 15, fontWeight: '600', color: colors.textPrimary },
  meta: { fontSize: 11, color: colors.textDim, marginTop: 2 },
  iconBtn: { paddingHorizontal: 6, paddingVertical: 4 },
  overlay: {
    flex: 1,
    backgroundColor: colors.scrim,
    justifyContent: 'center',
    padding: 24,
  },
  card: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.card,
    padding: 18,
  },
  cardTitle: { fontSize: 17, fontWeight: '700', color: colors.textPrimary },
  cardHint: { fontSize: 12, lineHeight: 17, color: colors.textMuted, marginTop: 4 },
  input: {
    backgroundColor: colors.bg,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: radius.inner,
    paddingHorizontal: 12,
    paddingVertical: 10,
    color: colors.textPrimary,
    fontSize: 15,
    marginTop: 14,
  },
  cardActions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    alignItems: 'center',
    gap: 10,
    marginTop: 16,
  },
  cancel: { paddingHorizontal: 14, paddingVertical: 9 },
  cancelText: { fontSize: 13, fontWeight: '600', color: colors.textMuted },
  save: {
    backgroundColor: colors.accent,
    borderRadius: radius.inner,
    paddingHorizontal: 20,
    paddingVertical: 9,
    minWidth: 76,
    alignItems: 'center',
  },
  saveText: { fontSize: 13, fontWeight: '700', color: colors.white },
  dimmed: { opacity: 0.5 },
});
