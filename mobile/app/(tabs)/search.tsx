/**
 * Screen 4: Search
 *
 * Global hybrid search across all recordings.  Results deep-link to the
 * specific moment in the Recording Detail screen.
 *
 * The server handles both keyword (tsvector) and semantic (pgvector) search,
 * fused with RRF (design §9 "Hybrid search").
 */

import { useState, useCallback } from 'react';
import {
  View,
  Text,
  StyleSheet,
  TextInput,
  FlatList,
  TouchableOpacity,
  ActivityIndicator,
  Keyboard,
} from 'react-native';
import { useRouter } from 'expo-router';
import { api } from '../../src/api/client';
import type { SearchHit } from '../../src/types';

// ---------------------------------------------------------------------------

function formatMs(ms: number | null): string {
  if (ms == null) return '';
  const totalSecs = Math.floor(ms / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

// ---------------------------------------------------------------------------

export default function SearchScreen() {
  const router = useRouter();
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<SearchHit[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [searched, setSearched] = useState(false);

  const runSearch = useCallback(async () => {
    const q = query.trim();
    if (!q) return;
    Keyboard.dismiss();
    setLoading(true);
    setError(null);
    setSearched(true);

    try {
      const res = await api.search({ q, limit: 40 });
      setResults(res.hits);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Search failed');
      setResults([]);
    } finally {
      setLoading(false);
    }
  }, [query]);

  return (
    <View style={styles.container}>
      {/* Search bar */}
      <View style={styles.searchBar}>
        <TextInput
          style={styles.searchInput}
          placeholder="Search across all meetings…"
          value={query}
          onChangeText={setQuery}
          onSubmitEditing={runSearch}
          returnKeyType="search"
          autoCapitalize="none"
          autoCorrect={false}
        />
        <TouchableOpacity
          style={styles.searchButton}
          onPress={runSearch}
          disabled={loading}
          accessibilityLabel="Search"
        >
          <Text style={styles.searchButtonText}>Go</Text>
        </TouchableOpacity>
      </View>

      {/* Content */}
      {loading && (
        <View style={styles.centered}>
          <ActivityIndicator color="#E53935" />
        </View>
      )}

      {!loading && error && (
        <View style={styles.centered}>
          <Text style={styles.errorText}>{error}</Text>
        </View>
      )}

      {!loading && !error && searched && results.length === 0 && (
        <View style={styles.centered}>
          <Text style={styles.emptyText}>No results found.</Text>
        </View>
      )}

      {!loading && results.length > 0 && (
        <FlatList
          data={results}
          keyExtractor={(_, i) => String(i)}
          contentContainerStyle={styles.list}
          renderItem={({ item }) => (
            <TouchableOpacity
              style={styles.hit}
              onPress={() =>
                router.push({
                  pathname: '/recordings/[id]',
                  params: {
                    id: item.recording_id,
                    seekMs: String(item.start_ms ?? 0),
                  },
                })
              }
              accessibilityRole="button"
            >
              <Text style={styles.hitRecording} numberOfLines={1}>
                {item.recording_title ?? 'Untitled'}{' '}
                {item.start_ms != null ? `· ${formatMs(item.start_ms)}` : ''}
              </Text>
              <Text style={styles.hitText} numberOfLines={3}>
                {item.text}
              </Text>
              <Text style={styles.hitScore}>
                relevance {(item.score * 100).toFixed(0)}%
              </Text>
            </TouchableOpacity>
          )}
        />
      )}
    </View>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#f5f5f5',
  },
  searchBar: {
    flexDirection: 'row',
    padding: 12,
    backgroundColor: '#fff',
    borderBottomWidth: 1,
    borderBottomColor: '#eee',
    gap: 8,
  },
  searchInput: {
    flex: 1,
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    fontSize: 16,
    backgroundColor: '#fafafa',
  },
  searchButton: {
    backgroundColor: '#E53935',
    borderRadius: 8,
    paddingHorizontal: 16,
    paddingVertical: 8,
    justifyContent: 'center',
  },
  searchButtonText: {
    color: '#fff',
    fontWeight: '600',
    fontSize: 14,
  },
  centered: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 32,
  },
  errorText: {
    color: '#E53935',
    textAlign: 'center',
    fontSize: 14,
  },
  emptyText: {
    color: '#888',
    fontSize: 14,
  },
  list: {
    padding: 12,
    gap: 10,
  },
  hit: {
    backgroundColor: '#fff',
    borderRadius: 10,
    padding: 14,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.05,
    shadowRadius: 3,
    elevation: 1,
  },
  hitRecording: {
    fontSize: 13,
    fontWeight: '600',
    color: '#1E88E5',
    marginBottom: 4,
  },
  hitText: {
    fontSize: 14,
    color: '#333',
    lineHeight: 20,
    marginBottom: 6,
  },
  hitScore: {
    fontSize: 11,
    color: '#bbb',
  },
});
