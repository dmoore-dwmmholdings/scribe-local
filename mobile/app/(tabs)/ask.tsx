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
  Keyboard,
} from 'react-native';
import { useRouter } from 'expo-router';
import { api } from '../../src/api/client';
import type { AskResponse, Citation } from '../../src/types';

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
  index,
  onPress,
}: {
  citation: Citation;
  index: number;
  onPress: () => void;
}) {
  return (
    <TouchableOpacity style={styles.citation} onPress={onPress} accessibilityRole="link">
      <Text style={styles.citationLabel}>
        [{index + 1}] {citation.recording_title ?? 'Untitled'}
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
  const scrollRef = useRef<ScrollView>(null);

  const ask = useCallback(async () => {
    const q = question.trim();
    if (!q) return;
    Keyboard.dismiss();
    setLoading(true);
    setError(null);
    setResponse(null);

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

  return (
    <View style={styles.container}>
      <ScrollView
        ref={scrollRef}
        contentContainerStyle={styles.scroll}
        keyboardShouldPersistTaps="handled"
      >
        <Text style={styles.intro}>
          Ask a question over all your recorded meetings. The answer is generated
          locally by your server's LLM.
        </Text>

        {/* Answer */}
        {response && (
          <View style={styles.answerCard}>
            <Text style={styles.answerText}>{response.answer}</Text>
          </View>
        )}

        {/* Citations */}
        {response && response.citations.length > 0 && (
          <View style={styles.citationsSection}>
            <Text style={styles.citationsHeader}>Sources</Text>
            {response.citations.map((c, i) => (
              <CitationCard
                key={i}
                citation={c}
                index={i}
                onPress={() =>
                  router.push({
                    pathname: '/recordings/[id]',
                    params: {
                      id: c.recording_id,
                      seekMs: String(c.start_ms ?? 0),
                    },
                  })
                }
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
    </View>
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
