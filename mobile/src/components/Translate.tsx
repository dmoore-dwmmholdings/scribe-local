/**
 * TranslateModal — on-demand translation of a recording's summary (Phase 2.3).
 *
 * Self-contained: pick a target language, call POST /recordings/{id}/translate
 * (the server runs the summary through the LLM), then show the translated text
 * with a share action. Nothing is stored server-side; the result lives for the
 * modal's lifetime. A genuine leapfrog — neither Otter nor Plaud translates.
 */

import { useEffect, useState } from 'react';
import {
  View,
  Text,
  StyleSheet,
  Modal,
  ScrollView,
  TouchableOpacity,
  ActivityIndicator,
  Share,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { api } from '../api/client';
import { log } from '../util/logger';
import { colors, radius } from '../theme';

const LANGUAGES = [
  'Spanish',
  'French',
  'German',
  'Italian',
  'Portuguese',
  'Dutch',
  'Japanese',
  'Chinese (Simplified)',
  'Korean',
  'Hindi',
  'Arabic',
  'Russian',
  'English',
];

export function TranslateModal({
  visible,
  recordingId,
  onDismiss,
}: {
  visible: boolean;
  recordingId: string;
  onDismiss: () => void;
}) {
  const [lang, setLang] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Reset to the picker whenever the modal is reopened.
  useEffect(() => {
    if (!visible) {
      setLang(null);
      setLoading(false);
      setText(null);
      setError(null);
    }
  }, [visible]);

  const translate = async (target: string) => {
    setLang(target);
    setLoading(true);
    setError(null);
    setText(null);
    try {
      const res = await api.translate(recordingId, target);
      setText(res.text);
    } catch (err) {
      log('translate', 'failed', err);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  const backToPicker = () => {
    setLang(null);
    setText(null);
    setError(null);
  };

  const share = () => {
    if (text) Share.share({ message: text, title: `Summary (${lang})` }).catch(() => {});
  };

  const showingResult = loading || text != null || error != null;

  return (
    <Modal visible={visible} animationType="slide" onRequestClose={onDismiss} presentationStyle="pageSheet">
      <View style={styles.screen}>
        <View style={styles.header}>
          {showingResult ? (
            <TouchableOpacity onPress={backToPicker} accessibilityLabel="Back" style={styles.headerBtn}>
              <Ionicons name="chevron-back" size={22} color={colors.accent} />
            </TouchableOpacity>
          ) : (
            <View style={styles.headerBtn} />
          )}
          <Text style={styles.headerTitle} numberOfLines={1}>
            {showingResult && lang ? `Translation · ${lang}` : 'Translate summary'}
          </Text>
          <View style={styles.headerActions}>
            {text != null && (
              <TouchableOpacity onPress={share} accessibilityLabel="Share translation" style={styles.headerBtn}>
                <Ionicons name="share-outline" size={20} color={colors.accent} />
              </TouchableOpacity>
            )}
            <TouchableOpacity onPress={onDismiss} accessibilityLabel="Close" style={styles.headerBtn}>
              <Ionicons name="close" size={22} color={colors.textMuted} />
            </TouchableOpacity>
          </View>
        </View>

        {!showingResult && (
          <ScrollView contentContainerStyle={styles.pickerContent}>
            <Text style={styles.hint}>Translate the summary into:</Text>
            <View style={styles.langWrap}>
              {LANGUAGES.map((l) => (
                <TouchableOpacity
                  key={l}
                  style={styles.langChip}
                  onPress={() => translate(l)}
                  accessibilityRole="button"
                >
                  <Text style={styles.langChipText}>{l}</Text>
                </TouchableOpacity>
              ))}
            </View>
          </ScrollView>
        )}

        {loading && (
          <View style={styles.centered}>
            <ActivityIndicator color={colors.accent} />
            <Text style={styles.centeredText}>Translating to {lang}…</Text>
          </View>
        )}

        {!loading && error != null && (
          <View style={styles.centered}>
            <Ionicons name="alert-circle-outline" size={28} color={colors.accentDeep} />
            <Text style={styles.errorText}>{error}</Text>
            <TouchableOpacity style={styles.retryBtn} onPress={() => lang && translate(lang)}>
              <Text style={styles.retryText}>Retry</Text>
            </TouchableOpacity>
          </View>
        )}

        {!loading && text != null && (
          <ScrollView contentContainerStyle={styles.resultContent}>
            <Text style={styles.resultText}>{text}</Text>
          </ScrollView>
        )}
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: colors.bg,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 14,
    paddingTop: 16,
    paddingBottom: 12,
    gap: 8,
  },
  headerTitle: {
    flex: 1,
    fontSize: 17,
    fontWeight: '700',
    color: colors.textPrimary,
    textAlign: 'center',
  },
  headerActions: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  headerBtn: {
    minWidth: 30,
    padding: 4,
    alignItems: 'center',
  },
  pickerContent: {
    paddingHorizontal: 20,
    paddingBottom: 40,
  },
  hint: {
    fontSize: 13,
    color: colors.textMuted,
    marginBottom: 14,
  },
  langWrap: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 10,
  },
  langChip: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: 12,
    paddingHorizontal: 14,
    paddingVertical: 10,
  },
  langChipText: {
    fontSize: 14,
    color: colors.textPrimary,
    fontWeight: '500',
  },
  centered: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 36,
    gap: 14,
  },
  centeredText: {
    fontSize: 13.5,
    color: colors.textMuted,
  },
  errorText: {
    fontSize: 13.5,
    color: colors.accentDeep,
    textAlign: 'center',
    lineHeight: 19,
  },
  retryBtn: {
    backgroundColor: colors.accent,
    borderRadius: 10,
    paddingHorizontal: 20,
    paddingVertical: 9,
  },
  retryText: {
    color: colors.white,
    fontWeight: '600',
  },
  resultContent: {
    paddingHorizontal: 20,
    paddingBottom: 48,
  },
  resultText: {
    fontSize: 15,
    lineHeight: 23,
    color: colors.textLight,
  },
});
