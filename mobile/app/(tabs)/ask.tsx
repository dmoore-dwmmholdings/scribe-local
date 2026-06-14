/**
 * Screen 5: Ask
 *
 * Natural-language question over the corpus (RAG).
 * The server embeds the question, retrieves top-k transcript chunks from
 * pgvector, feeds them to the LLM, and returns an answer with citations
 * (design §9 "Ask (RAG)").
 *
 * Citations deep-link to the specific moment in the Recording Detail screen.
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
import { useRouter } from 'expo-router';
import { api } from '../../src/api/client';
import type { AskResponse, Citation } from '../../src/types';

// ---------------------------------------------------------------------------
// Answer parsing
// ---------------------------------------------------------------------------

/**
 * Split an answer string into plain-text and citation-marker segments.
 * A marker `[N]` is only treated as a citation when N maps to a real citation
 * index (1-based) — stray brackets like "[note]" or "[99]" stay as text.
 */
type AnswerSegment =
  | { type: 'text'; text: string }
  | { type: 'marker'; n: number };

function parseAnswer(answer: string, citationCount: number): AnswerSegment[] {
  const segments: AnswerSegment[] = [];
  const re = /\[(\d+)\]/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = re.exec(answer)) !== null) {
    const n = Number(match[1]);
    // Only treat as a marker if it references a real citation.
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

// ---------------------------------------------------------------------------

function formatMs(ms: number | null): string {
  if (ms == null) return '';
  const totalSecs = Math.floor(ms / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

// ---------------------------------------------------------------------------

function CitationCard({
  citation,
  label,
  highlighted,
  onPress,
  onLayout,
}: {
  citation: Citation;
  /** 1-based number shown in the [N] prefix. */
  label: number;
  highlighted: boolean;
  onPress: () => void;
  onLayout: (y: number) => void;
}) {
  return (
    <TouchableOpacity
      style={[styles.citation, highlighted && styles.citationHighlighted]}
      onPress={onPress}
      onLayout={(e) => onLayout(e.nativeEvent.layout.y)}
      accessibilityRole="link"
    >
      <Text style={styles.citationLabel}>
        [{label}] {citation.recording_title ?? 'Untitled'}
        {citation.start_ms != null ? ` · ${formatMs(citation.start_ms)}` : ''}
      </Text>
      <Text style={styles.citationSnippet} numberOfLines={3}>
        {citation.snippet}
      </Text>
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
  // Y offset of the Sources section + each card within the scroll view, so we
  // can scroll a tapped [N] marker into view.
  const sectionYRef = useRef(0);
  const cardYRef = useRef<Record<number, number>>({});

  const ask = useCallback(async () => {
    const q = question.trim();
    if (!q) return;
    Keyboard.dismiss();
    setLoading(true);
    setError(null);
    setResponse(null);
    setHighlighted(null);
    cardYRef.current = {};

    try {
      const res = await api.ask({ question: q, top_k: 6 });
      setResponse(res);
      setTimeout(() => scrollRef.current?.scrollToEnd({ animated: true }), 100);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Request failed');
    } finally {
      setLoading(false);
    }
  }, [question]);

  const openCitation = useCallback(
    (c: Citation) => {
      router.push({
        pathname: '/recordings/[id]',
        params: {
          id: c.recording_id,
          seekMs: String(c.start_ms ?? 0),
        },
      });
    },
    [router],
  );

  // Tapping a [N] marker: scroll its source card into view and flash a
  // highlight so the user can see which source backs that claim.
  const focusCitation = useCallback((n: number) => {
    setHighlighted(n);
    const cardY = cardYRef.current[n];
    if (cardY != null) {
      scrollRef.current?.scrollTo({
        y: Math.max(sectionYRef.current + cardY - 12, 0),
        animated: true,
      });
    }
    setTimeout(() => {
      setHighlighted((cur) => (cur === n ? null : cur));
    }, 1500);
  }, []);

  // Citations actually referenced by a [N] marker, in order of first mention.
  const citations = response?.citations ?? [];
  const segments = response ? parseAnswer(response.answer, citations.length) : [];
  const citedOrder: number[] = [];
  for (const seg of segments) {
    if (seg.type === 'marker' && !citedOrder.includes(seg.n)) {
      citedOrder.push(seg.n);
    }
  }
  // Fall back to all citations if the model didn't use any brackets.
  const sourcesToShow =
    citedOrder.length > 0
      ? citedOrder.map((n) => ({ n, citation: citations[n - 1] }))
      : citations.map((citation, i) => ({ n: i + 1, citation }));

  return (
    <KeyboardAvoidingView
      style={styles.container}
      behavior={Platform.OS === 'ios' ? 'padding' : undefined}
      keyboardVerticalOffset={Platform.OS === 'ios' ? 90 : 0}
    >
      <ScrollView
        ref={scrollRef}
        contentContainerStyle={styles.scroll}
        keyboardShouldPersistTaps="handled"
      >
        <Text style={styles.intro}>
          Ask a question over all your recorded meetings. The answer is generated
          locally by your server's LLM.
        </Text>

        {/* Answer with inline, tappable [N] citation markers */}
        {response && (
          <View style={styles.answerCard}>
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
                    {` [${seg.n}] `}
                  </Text>
                ),
              )}
            </Text>
          </View>
        )}

        {/* Sources — only the cited ones, in [N] order (or all as fallback) */}
        {response && sourcesToShow.length > 0 && (
          <View
            style={styles.citationsSection}
            onLayout={(e) => {
              sectionYRef.current = e.nativeEvent.layout.y;
            }}
          >
            <Text style={styles.citationsHeader}>Sources</Text>
            {sourcesToShow.map(({ n, citation }) => (
              <CitationCard
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
          placeholder="Ask anything about your meetings…"
          value={question}
          onChangeText={setQuestion}
          onSubmitEditing={ask}
          returnKeyType="send"
          multiline
          maxLength={500}
          editable={!loading}
        />
        {loading ? (
          <ActivityIndicator color="#E53935" style={styles.sendButton} />
        ) : (
          <TouchableOpacity
            style={styles.sendButton}
            onPress={ask}
            disabled={loading || !question.trim()}
            accessibilityLabel="Ask"
          >
            <Text style={styles.sendText}>Ask</Text>
          </TouchableOpacity>
        )}
      </View>
    </KeyboardAvoidingView>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#f5f5f5',
  },
  scroll: {
    padding: 16,
    paddingBottom: 8,
  },
  intro: {
    fontSize: 13,
    color: '#888',
    marginBottom: 16,
    lineHeight: 20,
  },
  answerCard: {
    backgroundColor: '#fff',
    borderRadius: 12,
    padding: 16,
    marginBottom: 16,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.05,
    shadowRadius: 4,
    elevation: 2,
  },
  answerText: {
    fontSize: 15,
    color: '#111',
    lineHeight: 24,
  },
  marker: {
    fontFamily: Platform.OS === 'ios' ? 'Menlo' : 'monospace',
    fontSize: 12,
    fontWeight: '700',
    color: '#fff',
    backgroundColor: '#1E88E5',
    borderRadius: 4,
    overflow: 'hidden',
  },
  citationsSection: {
    marginBottom: 16,
  },
  citationsHeader: {
    fontSize: 13,
    fontWeight: '600',
    color: '#555',
    marginBottom: 8,
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  citation: {
    backgroundColor: '#fff',
    borderRadius: 8,
    padding: 12,
    marginBottom: 8,
    borderLeftWidth: 3,
    borderLeftColor: '#1E88E5',
  },
  citationHighlighted: {
    backgroundColor: '#E3F2FD',
    borderLeftWidth: 4,
  },
  citationLabel: {
    fontSize: 12,
    fontWeight: '600',
    color: '#1E88E5',
    marginBottom: 4,
  },
  citationSnippet: {
    fontSize: 13,
    color: '#555',
    lineHeight: 18,
  },
  errorCard: {
    backgroundColor: '#FFF3F3',
    borderRadius: 8,
    padding: 12,
  },
  errorText: {
    color: '#E53935',
    fontSize: 13,
  },
  inputBar: {
    flexDirection: 'row',
    padding: 12,
    backgroundColor: '#fff',
    borderTopWidth: 1,
    borderTopColor: '#eee',
    gap: 8,
    alignItems: 'flex-end',
  },
  input: {
    flex: 1,
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    fontSize: 15,
    maxHeight: 100,
    backgroundColor: '#fafafa',
  },
  sendButton: {
    backgroundColor: '#E53935',
    borderRadius: 8,
    paddingHorizontal: 16,
    paddingVertical: 8,
    justifyContent: 'center',
    alignItems: 'center',
    minWidth: 48,
    minHeight: 36,
  },
  sendText: {
    color: '#fff',
    fontWeight: '600',
    fontSize: 14,
  },
});
