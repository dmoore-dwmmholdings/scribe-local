/**
 * Tag a diarized speaker — the sheet behind "Speaker 0" in the transcript.
 *
 * The point of this screen is reuse: the same person turns up in many
 * recordings, so the sheet leads with the people already tagged before and only
 * offers a text field underneath. Picking someone from the library sends their
 * `speaker_id`, which keeps one identity instead of creating a new "Dawson"
 * every time.
 *
 * "Recognise this voice later" enrolls the voiceprint. That is the setting that
 * makes the tag carry forward on its own: the server matches it during
 * diarization, so recordings uploaded afterwards come back already labelled.
 */

import { useCallback, useEffect, useState } from 'react';
import {
  ActivityIndicator,
  Modal,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  TouchableOpacity,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { api } from '../api/client';
import type { EnrolledSpeaker, NameSpeakerRequest } from '../types';
import { colors, mono, radius } from '../theme';
import { log } from '../util/logger';

export function SpeakerTagSheet({
  visible,
  currentLabel,
  currentSpeakerId,
  saving,
  onTag,
  onUntag,
  onDismiss,
}: {
  visible: boolean;
  /** What the transcript shows today — "Speaker 0" or an assigned name. */
  currentLabel: string;
  /** The enrolled speaker currently attached, if any. */
  currentSpeakerId: string | null;
  saving: boolean;
  onTag: (req: NameSpeakerRequest) => void;
  onUntag: () => void;
  onDismiss: () => void;
}) {
  const [speakers, setSpeakers] = useState<EnrolledSpeaker[]>([]);
  const [loading, setLoading] = useState(false);
  const [name, setName] = useState('');
  const [enroll, setEnroll] = useState(true);

  // Reload the library each time the sheet opens — it changes as other
  // recordings get tagged.
  useEffect(() => {
    if (!visible) return;
    setName('');
    setEnroll(true);
    setLoading(true);
    api
      .listSpeakers()
      .then((res) => setSpeakers(res.speakers ?? []))
      .catch((err) => {
        // An older backend has no /speakers; the name field still works.
        log('speakers', 'list failed', err);
        setSpeakers([]);
      })
      .finally(() => setLoading(false));
  }, [visible]);

  const submitName = useCallback(() => {
    const trimmed = name.trim();
    if (!trimmed) return;
    onTag({ name: trimmed, enroll });
  }, [name, enroll, onTag]);

  const matches = name.trim()
    ? speakers.filter((s) => s.display_name.toLowerCase().includes(name.trim().toLowerCase()))
    : speakers;

  return (
    <Modal visible={visible} transparent animationType="fade" onRequestClose={onDismiss}>
      <View style={styles.overlay}>
        <View style={styles.card}>
          <Text style={styles.title}>Tag speaker</Text>
          <Text style={styles.subtitle}>Currently {currentLabel}</Text>

          {/* Known people first — tagging is mostly re-tagging. */}
          <Text style={styles.sectionLabel}>KNOWN SPEAKERS</Text>
          {loading ? (
            <ActivityIndicator color={colors.accent} style={styles.listLoading} />
          ) : speakers.length === 0 ? (
            <Text style={styles.empty}>
              No one tagged yet. Name this speaker below and they will show up here for your next
              recording.
            </Text>
          ) : (
            <ScrollView style={styles.list} keyboardShouldPersistTaps="handled">
              {matches.map((s) => {
                const active = s.id === currentSpeakerId;
                return (
                  <TouchableOpacity
                    key={s.id}
                    style={[styles.row, active && styles.rowActive]}
                    onPress={() => onTag({ speaker_id: s.id, enroll })}
                    disabled={saving}
                    accessibilityRole="button"
                    accessibilityState={{ selected: active }}
                    accessibilityLabel={`Tag as ${s.display_name}`}
                  >
                    <View style={styles.rowText}>
                      <Text style={styles.rowName}>{s.display_name}</Text>
                      <Text style={styles.rowMeta}>
                        {s.has_voiceprint ? 'Voice enrolled' : 'Name only'}
                        {s.recording_count > 0
                          ? ` · ${s.recording_count} recording${s.recording_count === 1 ? '' : 's'}`
                          : ''}
                      </Text>
                    </View>
                    {active ? (
                      <Ionicons name="checkmark-circle" size={18} color={colors.accent} />
                    ) : s.has_voiceprint ? (
                      <Ionicons name="mic" size={15} color={colors.textDim} />
                    ) : null}
                  </TouchableOpacity>
                );
              })}
              {matches.length === 0 && (
                <Text style={styles.empty}>No one matches “{name.trim()}”.</Text>
              )}
            </ScrollView>
          )}

          {/* New identity */}
          <Text style={styles.sectionLabel}>SOMEONE NEW</Text>
          <View style={styles.newRow}>
            <TextInput
              style={styles.input}
              value={name}
              onChangeText={setName}
              placeholder="e.g. Dawson"
              placeholderTextColor={colors.textDim}
              returnKeyType="done"
              onSubmitEditing={submitName}
            />
            <TouchableOpacity
              style={[styles.addButton, (!name.trim() || saving) && styles.dimmed]}
              onPress={submitName}
              disabled={!name.trim() || saving}
              accessibilityLabel="Add speaker"
            >
              {saving ? (
                <ActivityIndicator size="small" color={colors.white} />
              ) : (
                <Ionicons name="checkmark" size={17} color={colors.white} />
              )}
            </TouchableOpacity>
          </View>

          <TouchableOpacity
            style={styles.enrollRow}
            onPress={() => setEnroll((e) => !e)}
            accessibilityRole="checkbox"
            accessibilityState={{ checked: enroll }}
          >
            <View style={[styles.checkbox, enroll && styles.checkboxChecked]}>
              {enroll && <Ionicons name="checkmark" size={12} color={colors.white} />}
            </View>
            <Text style={styles.enrollText}>
              Recognise this voice later — recordings you upload after this get labelled
              automatically.
            </Text>
          </TouchableOpacity>

          <View style={styles.actions}>
            {currentSpeakerId ? (
              <TouchableOpacity style={styles.removeButton} onPress={onUntag} disabled={saving}>
                <Text style={styles.removeText}>Remove name</Text>
              </TouchableOpacity>
            ) : null}
            <TouchableOpacity style={styles.cancelButton} onPress={onDismiss}>
              <Text style={styles.cancelText}>Cancel</Text>
            </TouchableOpacity>
          </View>
        </View>
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
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
  title: {
    fontSize: 17,
    fontWeight: '700',
    color: colors.textPrimary,
  },
  subtitle: {
    fontSize: 12,
    color: colors.textMuted,
    marginTop: 3,
  },
  sectionLabel: {
    fontSize: 10,
    fontWeight: '700',
    letterSpacing: 1.6,
    color: colors.accent,
    fontFamily: mono,
    marginTop: 16,
    marginBottom: 7,
  },
  listLoading: {
    paddingVertical: 14,
  },
  list: {
    maxHeight: 210,
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingVertical: 10,
    paddingHorizontal: 11,
    borderRadius: radius.inner,
    borderWidth: 1,
    borderColor: colors.border,
    marginBottom: 6,
  },
  rowActive: {
    backgroundColor: colors.accentSoft,
    borderColor: colors.chipActiveBorder,
  },
  rowText: {
    flex: 1,
  },
  rowName: {
    fontSize: 14,
    fontWeight: '600',
    color: colors.textPrimary,
  },
  rowMeta: {
    fontSize: 11,
    color: colors.textDim,
    marginTop: 2,
  },
  empty: {
    fontSize: 12,
    lineHeight: 17,
    color: colors.textMuted,
    paddingVertical: 6,
  },
  newRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  input: {
    flex: 1,
    backgroundColor: colors.bg,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: radius.inner,
    paddingHorizontal: 12,
    paddingVertical: 10,
    color: colors.textPrimary,
    fontSize: 14,
  },
  addButton: {
    width: 40,
    height: 40,
    borderRadius: radius.inner,
    backgroundColor: colors.accent,
    alignItems: 'center',
    justifyContent: 'center',
  },
  dimmed: {
    opacity: 0.5,
  },
  enrollRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 9,
    marginTop: 14,
  },
  checkbox: {
    width: 18,
    height: 18,
    borderRadius: 5,
    borderWidth: 1,
    borderColor: colors.borderButton,
    alignItems: 'center',
    justifyContent: 'center',
    marginTop: 1,
  },
  checkboxChecked: {
    backgroundColor: colors.accent,
    borderColor: colors.accent,
  },
  enrollText: {
    flex: 1,
    fontSize: 12,
    lineHeight: 17,
    color: colors.textMuted,
  },
  actions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    alignItems: 'center',
    gap: 8,
    marginTop: 18,
  },
  removeButton: {
    paddingHorizontal: 14,
    paddingVertical: 9,
    borderRadius: radius.inner,
    borderWidth: 1,
    borderColor: colors.borderButton,
  },
  removeText: {
    fontSize: 13,
    fontWeight: '600',
    color: colors.accentDeep,
  },
  cancelButton: {
    paddingHorizontal: 14,
    paddingVertical: 9,
  },
  cancelText: {
    fontSize: 13,
    fontWeight: '600',
    color: colors.textMuted,
  },
});
