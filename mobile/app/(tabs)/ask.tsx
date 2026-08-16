/**
 * Screen 5: Ask — a conversation with the corpus.
 *
 * Start state: an ember-orb hero, framing copy, and "TRY ASKING" suggestion
 * cards.  After that it is a thread: questions as warm bubbles, answers led by
 * a mini ember avatar with inline tappable citation superscripts, and a SOURCES
 * list whose rows carry ember thumbnails and deep-link to the cited moment.
 *
 * Every turn is kept and sent back as `history`, so follow-ups resolve against
 * what was already asked. Each turn owns its own text and citations — the
 * screen never renders the live input as a past message.
 */

import { useState, useCallback, useRef, useEffect } from 'react';
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
import type { AskTurn, Citation } from '../../src/types';
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
// Thread model
// ---------------------------------------------------------------------------

/** One message in the conversation. */
interface ChatTurn {
  id: string;
  role: 'user' | 'assistant';
  text: string;
  /** Sources for an assistant turn; each turn keeps its own. */
  citations?: Citation[];
  /** A request that failed — shown in place, and never sent back as history. */
  failed?: boolean;
}

let turnSeq = 0;
const nextTurnId = () => `t${++turnSeq}`;

// ---------------------------------------------------------------------------

function SourceRow({
  citation,
  label,
  highlighted,
  onPress,
}: {
  citation: Citation;
  label: number;
  highlighted: boolean;
  onPress: () => void;
}) {
  return (
    <TouchableOpacity
      style={[styles.source, highlighted && styles.sourceHighlighted]}
      onPress={onPress}
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

/** One assistant turn: the answer, its inline citation markers, its sources. */
function AnswerTurn({
  turn,
  highlighted,
  onFocusCitation,
  onOpenCitation,
}: {
  turn: ChatTurn;
  highlighted: string | null;
  onFocusCitation: (key: string) => void;
  onOpenCitation: (c: Citation) => void;
}) {
  const citations = turn.citations ?? [];
  const segments = parseAnswer(turn.text, citations.length);

  // Show the sources the answer actually cited, in citation order. An answer
  // that cites nothing (chit-chat, arithmetic, "I did not find that") gets no
  // SOURCES block — listing every retrieved chunk under it implies the answer
  // rests on them.
  const citedOrder: number[] = [];
  for (const seg of segments) {
    if (seg.type === 'marker' && !citedOrder.includes(seg.n)) citedOrder.push(seg.n);
  }
  const sourcesToShow = citedOrder.map((n) => ({ n, citation: citations[n - 1] }));

  return (
    <View style={styles.answerRow}>
      <View style={styles.answerAvatar}>
        <EmberOrb size={30} variant="mini" seed={6.4} dir={-1} />
      </View>
      <View style={styles.answerBody}>
        <Text style={[styles.answerText, turn.failed && styles.answerFailed]}>
          {segments.map((seg, i) =>
            seg.type === 'text' ? (
              <Text key={i}>{seg.text}</Text>
            ) : (
              <Text
                key={i}
                style={styles.marker}
                onPress={() => onFocusCitation(`${turn.id}:${seg.n}`)}
                accessibilityRole="link"
                accessibilityLabel={`Citation ${seg.n}`}
              >
                {seg.n}
              </Text>
            ),
          )}
        </Text>

        {sourcesToShow.length > 0 && (
          <View style={styles.sourcesSection}>
            <Text style={styles.sourcesHeader}>SOURCES</Text>
            {sourcesToShow.map(({ n, citation }) => (
              <SourceRow
                key={n}
                citation={citation}
                label={n}
                highlighted={highlighted === `${turn.id}:${n}`}
                onPress={() => onOpenCitation(citation)}
              />
            ))}
          </View>
        )}
      </View>
    </View>
  );
}

// ---------------------------------------------------------------------------

export default function AskScreen() {
  const router = useRouter();
  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [question, setQuestion] = useState('');
  const [loading, setLoading] = useState(false);
  /** Briefly lit citation, keyed `${turnId}:${n}` so turns do not collide. */
  const [highlighted, setHighlighted] = useState<string | null>(null);
  const scrollRef = useRef<ScrollView>(null);

  const send = useCallback(
    async (raw: string) => {
      const q = raw.trim();
      if (!q || loading) return;
      Keyboard.dismiss();

      // Snapshot the thread BEFORE this question — that is the history the
      // server needs to resolve follow-ups. Failed turns are left out; they are
      // client-side errors, not something the assistant said.
      const history: AskTurn[] = turns
        .filter((t) => !t.failed)
        .map((t) => ({ role: t.role, content: t.text }));

      setTurns((prev) => [...prev, { id: nextTurnId(), role: 'user', text: q }]);
      setQuestion('');
      setLoading(true);
      try {
        const res = await api.ask({ question: q, history, top_k: 6 });
        setTurns((prev) => [
          ...prev,
          {
            id: nextTurnId(),
            role: 'assistant',
            text: res.answer,
            citations: res.citations ?? [],
          },
        ]);
      } catch (err) {
        setTurns((prev) => [
          ...prev,
          {
            id: nextTurnId(),
            role: 'assistant',
            text: err instanceof Error ? err.message : 'Request failed',
            failed: true,
          },
        ]);
      } finally {
        setLoading(false);
      }
    },
    [turns, loading],
  );

  const submit = useCallback(() => send(question), [send, question]);

  const newChat = useCallback(() => {
    setTurns([]);
    setQuestion('');
    setHighlighted(null);
  }, []);

  const openCitation = useCallback(
    (c: Citation) => {
      router.push({
        pathname: '/recordings/[id]',
        params: { id: c.recording_id, seekMs: String(c.start_ms ?? 0) },
      });
    },
    [router],
  );

  const focusCitation = useCallback((key: string) => {
    setHighlighted(key);
    setTimeout(() => setHighlighted((cur) => (cur === key ? null : cur)), 1500);
  }, []);

  // Keep the newest turn in view as the thread grows.
  useEffect(() => {
    if (turns.length === 0) return;
    const t = setTimeout(() => scrollRef.current?.scrollToEnd({ animated: true }), 60);
    return () => clearTimeout(t);
  }, [turns, loading]);

  const showStart = turns.length === 0 && !loading;

  return (
    <Screen>
      <KeyboardAvoidingView
        style={styles.flex}
        behavior={Platform.OS === 'ios' ? 'padding' : undefined}
        keyboardVerticalOffset={Platform.OS === 'ios' ? 90 : 0}
      >
        {turns.length > 0 && (
          <View style={styles.threadBar}>
            <Text style={styles.threadLabel}>CONVERSATION</Text>
            <TouchableOpacity onPress={newChat} accessibilityLabel="Start a new conversation">
              <Text style={styles.newChat}>New chat</Text>
            </TouchableOpacity>
          </View>
        )}

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
                Ask anything about your recordings, then keep talking — follow-ups remember what
                you already asked. Claims from a transcript link back to the moment in the audio.
              </Text>
            </View>
          )}

          {showStart && (
            <View style={styles.suggestions}>
              <Text style={styles.suggestLabel}>TRY ASKING</Text>
              {SUGGESTIONS.map((s) => (
                <TouchableOpacity key={s} style={styles.suggestCard} onPress={() => send(s)}>
                  <Ionicons name="sparkles" size={15} color={colors.accent} />
                  <Text style={styles.suggestText}>{s}</Text>
                  <Text style={styles.suggestChevron}>›</Text>
                </TouchableOpacity>
              ))}
            </View>
          )}

          {/* The thread */}
          {turns.map((turn) =>
            turn.role === 'user' ? (
              <View key={turn.id} style={styles.questionRow}>
                <View style={styles.questionBubble}>
                  {/* The turn's own text — never the live input, which would
                      rewrite this bubble as the next question is typed. */}
                  <Text style={styles.questionText}>{turn.text}</Text>
                </View>
              </View>
            ) : (
              <AnswerTurn
                key={turn.id}
                turn={turn}
                highlighted={highlighted}
                onFocusCitation={focusCitation}
                onOpenCitation={openCitation}
              />
            ),
          )}

          {loading && (
            <View style={turns.length > 0 ? styles.loadingInline : styles.loading}>
              <EmberOrb size={turns.length > 0 ? 30 : 64} variant={turns.length > 0 ? 'mini' : 'small'} seed={4.1} />
              <Text style={styles.loadingText}>Thinking…</Text>
            </View>
          )}
        </ScrollView>

        {/* Input bar */}
        <View style={styles.inputBar}>
          <TextInput
            style={styles.input}
            placeholder={turns.length > 0 ? 'Ask a follow-up…' : 'Ask a question…'}
            placeholderTextColor={colors.textDim}
            value={question}
            onChangeText={setQuestion}
            onSubmitEditing={submit}
            returnKeyType="send"
            multiline
            maxLength={2000}
            editable={!loading}
          />
          <TouchableOpacity
            style={[styles.sendButton, (loading || !question.trim()) && styles.sendDisabled]}
            onPress={submit}
            disabled={loading || !question.trim()}
            accessibilityLabel="Send"
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
  loadingInline: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    marginTop: 14,
  },
  loadingText: {
    fontSize: 13,
    color: colors.textMuted,
  },
  threadBar: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 18,
    paddingTop: 6,
    paddingBottom: 8,
  },
  threadLabel: {
    fontSize: 10,
    fontWeight: '700',
    letterSpacing: 1.5,
    color: colors.textMuted,
    fontFamily: mono,
  },
  newChat: {
    fontSize: 12,
    fontWeight: '600',
    color: colors.accent,
  },
  answerFailed: {
    color: colors.accentDeep,
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
