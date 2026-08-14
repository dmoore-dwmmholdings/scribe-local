/**
 * Screen 5: Ask — Kiln RAG over the corpus.
 *
 * Start state: an ember-orb hero, framing copy, and "TRY ASKING" suggestion
 * cards.  Answer state: the question as a warm bubble, the answer led by a mini
 * ember avatar with inline tappable citation superscripts, and a SOURCES list
 * whose rows carry ember thumbnails and deep-link to the cited moment.
 *
 * The server embeds the question, retrieves top-k chunks from pgvector, and
 * returns an answer with citations.
 */

import { useState, useCallback, useRef } from 'react';
import {
  View,
  Text,
  StyleSheet,
  TextInput,
  ScrollView,
  TouchableOpacity,
  ActivityIndicator,
  KeyboardAvoidingView,
  Platform,
  Keyboard,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useRouter } from 'expo-router';
import { api } from '../../src/api/client';
import type { AskResponse, Citation } from '../../src/types';
import { EmberOrb, seedFromString } from '../../src/components/EmberOrb';
import { Screen } from '../../src/components/ui';
import { colors, mono, radius } from '../../src/theme';

// ---------------------------------------------------------------------------
// Answer parsing
// ---------------------------------------------------------------------------

type AnswerSegment = { type: 'text'; text: string } | { type: 'marker'; n: number };

function parseAnswer(answer: string, citationCount: number): AnswerSegment[] {
  const segments: AnswerSegment[] = [];
  const re = /\[(\d+)\]/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = re.exec(answer)) !== null) {
    const n = Number(match[1]);
    if (n < 1 || n > citationCount) continue;
    if (match.index > lastIndex) {
      segments.push({ type: 'text', text: answer.slice(lastIndex, match.index) });
    }
    segments.push({ type: 'marker', n });
    lastIndex = re.lastIndex;
  }
  if (lastIndex < answer.length) {
    segments.push({ type: 'text', text: answer.slice(lastIndex) });
  }
  return segments;
}

function formatMs(ms: number | null): string {
  if (ms == null) return '';
  const totalSecs = Math.floor(ms / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

const SUGGESTIONS = [
  'What did we decide about the record screen?',
  'List every action item from this week',
  'Summarize my most recent meeting',
];

// ---------------------------------------------------------------------------

function SourceRow({
  citation,
  label,
  highlighted,
  onPress,
  onLayout,
}: {
  citation: Citation;
  label: number;
  highlighted: boolean;
  onPress: () => void;
  onLayout: (y: number) => void;
}) {
  return (
    <TouchableOpacity
      style={[styles.source, highlighted && styles.sourceHighlighted]}
      onPress={onPress}
      onLayout={(e) => onLayout(e.nativeEvent.layout.y)}
      accessibilityRole="link"
    >
      <View style={styles.sourceThumb}>
        <EmberOrb size={30} variant="mini" seed={seedFromString(citation.recording_id + label)} dir={label % 2 ? -1 : 1} />
      </View>
      <View style={styles.sourceBody}>
        <Text style={styles.sourceTitle} numberOfLines={1}>
          [{label}] {citation.recording_title ?? 'Untitled'}
        </Text>
        <Text style={styles.sourceMeta} numberOfLines={2}>
          {citation.start_ms != null ? `${formatMs(citation.start_ms)} · ` : ''}
          {citation.snippet}
        </Text>
      </View>
      <Text style={styles.sourcePlay}>▶</Text>
    </TouchableOpacity>
  );
}

// ---------------------------------------------------------------------------

export default function AskScreen() {
  const router = useRouter();
  const [question, setQuestion] = useState('');
  const [response, setResponse] = useState<AskResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [highlighted, setHighlighted] = useState<number | null>(null);
  const scrollRef = useRef<ScrollView>(null);
  const sectionYRef = useRef(0);
  const cardYRef = useRef<Record<number, number>>({});

  const ask = useCallback(
    async (q: string) => {
      const query = q.trim();
      if (!query) return;
      Keyboard.dismiss();
      setLoading(true);
      setError(null);
      setResponse(null);
      setHighlighted(null);
      cardYRef.current = {};

      try {
        const res = await api.ask({ question: query, top_k: 6 });
        setResponse(res);
        setTimeout(() => scrollRef.current?.scrollToEnd({ animated: true }), 100);
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Request failed');
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  const submit = useCallback(() => ask(question), [ask, question]);

  const runSuggestion = useCallback(
    (s: string) => {
      setQuestion(s);
      ask(s);
    },
    [ask],
  );

  const openCitation = useCallback(
    (c: Citation) => {
      router.push({
        pathname: '/recordings/[id]',
        params: { id: c.recording_id, seekMs: String(c.start_ms ?? 0) },
      });
    },
    [router],
  );

  const focusCitation = useCallback((n: number) => {
    setHighlighted(n);
    const cardY = cardYRef.current[n];
    if (cardY != null) {
      scrollRef.current?.scrollTo({ y: Math.max(sectionYRef.current + cardY - 12, 0), animated: true });
    }
    setTimeout(() => setHighlighted((cur) => (cur === n ? null : cur)), 1500);
  }, []);

  const citations = response?.citations ?? [];
  const segments = response ? parseAnswer(response.answer, citations.length) : [];
  const citedOrder: number[] = [];
  for (const seg of segments) {
    if (seg.type === 'marker' && !citedOrder.includes(seg.n)) citedOrder.push(seg.n);
  }
  const sourcesToShow =
    citedOrder.length > 0
      ? citedOrder.map((n) => ({ n, citation: citations[n - 1] }))
      : citations.map((citation, i) => ({ n: i + 1, citation }));

  const showStart = !response && !loading && !error;

  return (
    <Screen>
      <KeyboardAvoidingView
        style={styles.flex}
        behavior={Platform.OS === 'ios' ? 'padding' : undefined}
        keyboardVerticalOffset={Platform.OS === 'ios' ? 90 : 0}
      >
        <ScrollView
          ref={scrollRef}
          contentContainerStyle={styles.scroll}
          keyboardShouldPersistTaps="handled"
        >
          {/* Start hero + suggestions */}
          {showStart && (
            <View style={styles.hero}>
              <EmberOrb size={74} variant="small" seed={2.2} />
              <Text style={styles.heroTitle}>Ask your meetings</Text>
              <Text style={styles.heroBody}>
                Answers are drawn from all your recordings — every claim linked back to its
                moment in the audio.
              </Text>
            </View>
          )}

          {showStart && (
            <View style={styles.suggestions}>
              <Text style={styles.suggestLabel}>TRY ASKING</Text>
              {SUGGESTIONS.map((s) => (
                <TouchableOpacity key={s} style={styles.suggestCard} onPress={() => runSuggestion(s)}>
                  <Ionicons name="sparkles" size={15} color={colors.accent} />
                  <Text style={styles.suggestText}>{s}</Text>
                  <Text style={styles.suggestChevron}>›</Text>
                </TouchableOpacity>
              ))}
            </View>
          )}

          {loading && (
            <View style={styles.loading}>
              <EmberOrb size={64} variant="small" seed={4.1} />
              <Text style={styles.loadingText}>Searching your recordings…</Text>
            </View>
          )}

          {/* Answer */}
          {response && (
            <>
              <View style={styles.questionRow}>
                <View style={styles.questionBubble}>
                  <Text style={styles.questionText}>{question}</Text>
                </View>
              </View>

              <View style={styles.answerRow}>
                <View style={styles.answerAvatar}>
                  <EmberOrb size={30} variant="mini" seed={6.4} dir={-1} />
                </View>
                <View style={styles.answerBody}>
                  <Text style={styles.answerText}>
                    {segments.map((seg, i) =>
                      seg.type === 'text' ? (
                        <Text key={i}>{seg.text}</Text>
                      ) : (
                        <Text
                          key={i}
                          style={styles.marker}
                          onPress={() => focusCitation(seg.n)}
                          accessibilityRole="link"
                          accessibilityLabel={`Citation ${seg.n}`}
                        >
                          {seg.n}
                        </Text>
                      ),
                    )}
                  </Text>

                  {sourcesToShow.length > 0 && (
                    <View
                      style={styles.sourcesSection}
                      onLayout={(e) => {
                        sectionYRef.current = e.nativeEvent.layout.y;
                      }}
                    >
                      <Text style={styles.sourcesHeader}>SOURCES</Text>
                      {sourcesToShow.map(({ n, citation }) => (
                        <SourceRow
                          key={n}
                          citation={citation}
                          label={n}
                          highlighted={highlighted === n}
                          onLayout={(y) => {
                            cardYRef.current[n] = y;
                          }}
                          onPress={() => openCitation(citation)}
                        />
                      ))}
                    </View>
                  )}
                </View>
              </View>
            </>
          )}

          {error && (
            <View style={styles.errorCard}>
              <Text style={styles.errorText}>{error}</Text>
            </View>
          )}
        </ScrollView>

        {/* Input bar */}
        <View style={styles.inputBar}>
          <TextInput
            style={styles.input}
            placeholder={response ? 'Ask a follow-up…' : 'Ask a question…'}
            placeholderTextColor={colors.textDim}
            value={question}
            onChangeText={setQuestion}
            onSubmitEditing={submit}
            returnKeyType="send"
            multiline
            maxLength={500}
            editable={!loading}
          />
          <TouchableOpacity
            style={[styles.sendButton, (loading || !question.trim()) && styles.sendDisabled]}
            onPress={submit}
            disabled={loading || !question.trim()}
            accessibilityLabel="Ask"
          >
            {loading ? (
              <ActivityIndicator color={colors.white} size="small" />
            ) : (
              <Ionicons name="arrow-up" size={18} color={colors.white} />
            )}
          </TouchableOpacity>
        </View>
      </KeyboardAvoidingView>
    </Screen>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  flex: { flex: 1 },
  scroll: {
    paddingHorizontal: 18,
    paddingTop: 8,
    paddingBottom: 8,
    flexGrow: 1,
  },
  hero: {
    alignItems: 'center',
    paddingTop: 40,
    paddingHorizontal: 12,
    gap: 15,
  },
  heroTitle: {
    fontSize: 19,
    fontWeight: '700',
    color: colors.textPrimary,
    letterSpacing: -0.3,
  },
  heroBody: {
    fontSize: 13,
    lineHeight: 20,
    color: colors.textMuted,
    textAlign: 'center',
  },
  suggestions: {
    marginTop: 28,
    gap: 9,
  },
  suggestLabel: {
    fontSize: 10,
    fontWeight: '700',
    letterSpacing: 1.5,
    color: colors.textMuted,
    fontFamily: mono,
    paddingLeft: 4,
    marginBottom: 2,
  },
  suggestCard: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.input,
    padding: 13,
  },
  suggestText: {
    flex: 1,
    fontSize: 13,
    fontWeight: '500',
    color: colors.textLight,
  },
  suggestChevron: {
    color: colors.textDim,
    fontSize: 16,
  },
  loading: {
    alignItems: 'center',
    paddingTop: 56,
    gap: 14,
  },
  loadingText: {
    fontSize: 13,
    color: colors.textMuted,
  },
  questionRow: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    marginTop: 8,
  },
  questionBubble: {
    maxWidth: '82%',
    backgroundColor: colors.accentSoft,
    borderWidth: 1,
    borderColor: colors.chipActiveBorder,
    borderRadius: 16,
    borderBottomRightRadius: 5,
    paddingHorizontal: 13,
    paddingVertical: 10,
  },
  questionText: {
    fontSize: 13,
    lineHeight: 19,
    color: colors.textPrimary,
  },
  answerRow: {
    flexDirection: 'row',
    gap: 10,
    marginTop: 14,
  },
  answerAvatar: {
    width: 30,
    height: 30,
  },
  answerBody: {
    flex: 1,
    minWidth: 0,
  },
  answerText: {
    fontSize: 13.5,
    lineHeight: 22,
    color: colors.textBody,
  },
  marker: {
    fontSize: 10,
    lineHeight: 14,
    fontWeight: '700',
    color: colors.accent,
    fontFamily: mono,
  },
  sourcesSection: {
    marginTop: 14,
  },
  sourcesHeader: {
    fontSize: 10,
    fontWeight: '700',
    letterSpacing: 1.5,
    color: colors.textMuted,
    fontFamily: mono,
    marginBottom: 8,
  },
  source: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: 11,
    padding: 8,
    marginBottom: 7,
  },
  sourceHighlighted: {
    borderColor: colors.chipActiveBorder,
    backgroundColor: colors.accentSoft,
  },
  sourceThumb: {
    width: 30,
    height: 30,
    borderRadius: radius.thumbSm,
    overflow: 'hidden',
    backgroundColor: colors.accentFaint,
  },
  sourceBody: {
    flex: 1,
    minWidth: 0,
  },
  sourceTitle: {
    fontSize: 12.5,
    fontWeight: '600',
    color: colors.textPrimary,
  },
  sourceMeta: {
    fontSize: 10,
    color: colors.textMuted,
    fontFamily: mono,
    marginTop: 2,
    lineHeight: 14,
  },
  sourcePlay: {
    fontSize: 13,
    color: colors.accent,
  },
  errorCard: {
    backgroundColor: 'rgba(232,81,46,0.1)',
    borderRadius: radius.inner,
    padding: 13,
    marginTop: 16,
  },
  errorText: {
    color: colors.accentDeep,
    fontSize: 13,
  },
  inputBar: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 9,
    paddingHorizontal: 16,
    paddingTop: 8,
    paddingBottom: 12,
    borderTopWidth: 1,
    borderTopColor: colors.border,
    backgroundColor: colors.bg,
  },
  input: {
    flex: 1,
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: 22,
    paddingHorizontal: 16,
    paddingVertical: 11,
    fontSize: 13,
    color: colors.textPrimary,
    maxHeight: 110,
  },
  sendButton: {
    width: 42,
    height: 42,
    borderRadius: 21,
    backgroundColor: colors.accent,
    alignItems: 'center',
    justifyContent: 'center',
  },
  sendDisabled: {
    opacity: 0.45,
  },
});
