/**
 * Screen 4: Search — Kiln full-text + semantic search across all transcripts.
 *
 * Query field, filter chips, and matched snippets with the query term
 * highlighted; results deep-link to the moment in the Recording Detail screen.
 * The server fuses keyword (tsvector) and semantic (pgvector) search with RRF.
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
import { Ionicons } from '@expo/vector-icons';
import { useRouter } from 'expo-router';
import { api } from '../../src/api/client';
import type { SearchHit } from '../../src/types';
import { Screen, ScreenTitle } from '../../src/components/ui';
import { colors, mono, radius } from '../../src/theme';

// ---------------------------------------------------------------------------

const FILTERS = ['All', 'Transcript', 'Speakers', 'This week'] as const;
type Filter = (typeof FILTERS)[number];

function formatMs(ms: number | null): string {
  if (ms == null) return '';
  const totalSecs = Math.floor(ms / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

/** Split a snippet around case-insensitive matches of the query for highlighting. */
function highlight(text: string, q: string) {
  const term = q.trim();
  if (!term) return [{ text, hit: false }];
  const out: { text: string; hit: boolean }[] = [];
  const lower = text.toLowerCase();
  const needle = term.toLowerCase();
  let i = 0;
  while (i < text.length) {
    const idx = lower.indexOf(needle, i);
    if (idx === -1) {
      out.push({ text: text.slice(i), hit: false });
      break;
    }
    if (idx > i) out.push({ text: text.slice(i, idx), hit: false });
    out.push({ text: text.slice(idx, idx + needle.length), hit: true });
    i = idx + needle.length;
  }
  return out;
}

// ---------------------------------------------------------------------------

export default function SearchScreen() {
  const router = useRouter();
  const [query, setQuery] = useState('');
  const [submitted, setSubmitted] = useState('');
  const [filter, setFilter] = useState<Filter>('All');
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
    setSubmitted(q);

    try {
      // "This week" narrows to the last 7 days; the API supports a `from` bound.
      const from =
        filter === 'This week'
          ? new Date(Date.now() - 7 * 86400_000).toISOString()
          : undefined;
      const res = await api.search({ q, from, limit: 40 });
      setResults(res.hits);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Search failed');
      setResults([]);
    } finally {
      setLoading(false);
    }
  }, [query, filter]);

  return (
    <Screen>
      <ScreenTitle title="Search" />

      {/* Search field */}
      <View style={styles.searchWrap}>
        <View style={styles.searchBar}>
          <Ionicons name="search" size={17} color={colors.textMuted} />
          <TextInput
            style={styles.searchInput}
            placeholder="Search across all meetings…"
            placeholderTextColor={colors.textDim}
            value={query}
            onChangeText={setQuery}
            onSubmitEditing={runSearch}
            returnKeyType="search"
            autoCapitalize="none"
            autoCorrect={false}
          />
          {query.length > 0 && (
            <TouchableOpacity
              style={styles.clearBtn}
              onPress={() => setQuery('')}
              accessibilityLabel="Clear search"
            >
              <Ionicons name="close" size={12} color={colors.textMuted} />
            </TouchableOpacity>
          )}
        </View>
      </View>

      {/* Filter chips */}
      <View style={styles.chips}>
        {FILTERS.map((f) => {
          const active = filter === f;
          return (
            <TouchableOpacity
              key={f}
              onPress={() => setFilter(f)}
              style={[styles.chip, active ? styles.chipActive : styles.chipInactive]}
            >
              <Text style={[styles.chipText, active && styles.chipTextActive]}>{f}</Text>
            </TouchableOpacity>
          );
        })}
      </View>

      {/* Content */}
      {loading && (
        <View style={styles.centered}>
          <ActivityIndicator color={colors.accent} />
        </View>
      )}

      {!loading && error && (
        <View style={styles.centered}>
          <Text style={styles.errorText}>{error}</Text>
        </View>
      )}

      {!loading && !error && searched && results.length === 0 && (
        <View style={styles.centered}>
          <View style={styles.emptyGlow}>
            <Ionicons name="search" size={28} color={colors.accent} style={{ opacity: 0.85 }} />
          </View>
          <Text style={styles.emptyTitle}>No matches found</Text>
          <Text style={styles.emptyHint}>
            Nothing in your recordings mentions “{submitted}”. Try a different term or broaden your filters.
          </Text>
        </View>
      )}

      {!loading && results.length > 0 && (
        <FlatList
          data={results}
          keyExtractor={(_, i) => String(i)}
          contentContainerStyle={styles.list}
          ListHeaderComponent={
            <Text style={styles.resultsCount}>{results.length} RESULTS</Text>
          }
          renderItem={({ item }) => (
            <TouchableOpacity
              style={styles.hit}
              onPress={() =>
                router.push({
                  pathname: '/recordings/[id]',
                  params: { id: item.recording_id, seekMs: String(item.start_ms ?? 0) },
                })
              }
              accessibilityRole="button"
            >
              <View style={styles.hitHeader}>
                <Text style={styles.hitTitle} numberOfLines={1}>
                  {item.recording_title ?? 'Untitled'}
                </Text>
                {item.start_ms != null && (
                  <Text style={styles.hitDate}>{formatMs(item.start_ms)}</Text>
                )}
              </View>
              <Text style={styles.hitSnippet} numberOfLines={3}>
                {highlight(item.text, submitted).map((seg, i) =>
                  seg.hit ? (
                    <Text key={i} style={styles.mark}>
                      {seg.text}
                    </Text>
                  ) : (
                    <Text key={i}>{seg.text}</Text>
                  ),
                )}
              </Text>
              <View style={styles.hitFooter}>
                {item.start_ms != null && (
                  <Text style={styles.timePill}>▶ {formatMs(item.start_ms)}</Text>
                )}
                <Text style={styles.relevance}>{(item.score * 100).toFixed(0)}% match</Text>
              </View>
            </TouchableOpacity>
          )}
        />
      )}
    </Screen>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  searchWrap: {
    paddingHorizontal: 18,
  },
  searchBar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 9,
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.borderInput,
    borderRadius: radius.input,
    paddingHorizontal: 13,
    paddingVertical: 11,
  },
  searchInput: {
    flex: 1,
    fontSize: 14,
    color: colors.textPrimary,
    padding: 0,
  },
  clearBtn: {
    width: 18,
    height: 18,
    borderRadius: 9,
    backgroundColor: colors.toggleOff,
    alignItems: 'center',
    justifyContent: 'center',
  },
  chips: {
    flexDirection: 'row',
    gap: 7,
    paddingHorizontal: 18,
    paddingTop: 12,
    paddingBottom: 4,
  },
  chip: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: radius.pill,
    borderWidth: 1,
  },
  chipActive: {
    backgroundColor: colors.chipActiveBg,
    borderColor: colors.chipActiveBorder,
  },
  chipInactive: {
    backgroundColor: colors.chipInactiveBg,
    borderColor: 'transparent',
  },
  chipText: {
    fontSize: 11,
    fontWeight: '600',
    color: colors.textMuted,
  },
  chipTextActive: {
    color: colors.accent,
  },
  centered: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 38,
    gap: 15,
  },
  emptyGlow: {
    width: 72,
    height: 72,
    borderRadius: 36,
    backgroundColor: colors.accentFaint,
    alignItems: 'center',
    justifyContent: 'center',
  },
  emptyTitle: {
    fontSize: 17,
    fontWeight: '700',
    color: colors.textPrimary,
  },
  emptyHint: {
    fontSize: 13,
    lineHeight: 20,
    color: colors.textMuted,
    textAlign: 'center',
  },
  errorText: {
    color: colors.accentDeep,
    textAlign: 'center',
    fontSize: 14,
  },
  list: {
    paddingHorizontal: 18,
    paddingTop: 10,
    paddingBottom: 24,
    gap: 10,
  },
  resultsCount: {
    fontSize: 11,
    fontWeight: '600',
    color: colors.textMuted,
    fontFamily: mono,
    letterSpacing: 0.5,
    marginBottom: 10,
  },
  hit: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radius.inner,
    padding: 13,
  },
  hitHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 8,
  },
  hitTitle: {
    flex: 1,
    fontSize: 14,
    fontWeight: '600',
    color: colors.textPrimary,
  },
  hitDate: {
    fontSize: 10,
    color: colors.textMuted,
    fontFamily: mono,
  },
  hitSnippet: {
    marginTop: 7,
    fontSize: 12.5,
    lineHeight: 19,
    color: colors.textSnippet,
  },
  mark: {
    backgroundColor: colors.markBg,
    color: colors.markText,
  },
  hitFooter: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    marginTop: 9,
  },
  timePill: {
    fontSize: 11,
    fontWeight: '600',
    color: colors.accent,
    fontFamily: mono,
    backgroundColor: colors.accentSoft,
    paddingHorizontal: 9,
    paddingVertical: 4,
    borderRadius: 7,
    overflow: 'hidden',
  },
  relevance: {
    fontSize: 11,
    fontWeight: '600',
    color: colors.textDim,
  },
});
