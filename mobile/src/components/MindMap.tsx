/**
 * MindMap — a branching visual of a recording's summary (Phase 2.1).
 *
 * v1 derives the tree from the existing summary structure (no extra backend
 * call): the title is the root, and Overview / Topics / Action items / Decisions
 * become colored branches with their items as leaves. Rendered as an indented
 * tree with colored connector rails — readable on a phone, and a faithful
 * stand-in for Plaud's mind map (whose export is itself a markdown outline).
 *
 * A later pass can swap `buildMindMap` for an LLM-generated deeper tree without
 * touching the renderer.
 */

import { useMemo } from 'react';
import { View, Text, StyleSheet, Modal, ScrollView, TouchableOpacity, Share } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import type { Summary } from '../types';
import { colors, mono, radius, speakerColors } from '../theme';

// ---------------------------------------------------------------------------

interface Branch {
  label: string;
  color: string;
  leaves: string[];
}

interface MindMap {
  root: string;
  branches: Branch[];
}

/** Build a mind map from a summary + a fallback title. */
export function buildMindMap(summary: Summary | null, fallbackTitle: string): MindMap {
  const root = summary?.title?.trim() || fallbackTitle || 'Recording';
  const branches: Branch[] = [];
  const push = (label: string, leaves: (string | null | undefined)[]) => {
    const clean = leaves.map((l) => (l ?? '').trim()).filter(Boolean);
    if (clean.length) {
      branches.push({ label, color: speakerColors[branches.length % speakerColors.length], leaves: clean });
    }
  };

  if (summary?.summary?.trim()) push('Overview', [summary.summary]);
  push('Topics', summary?.topics ?? []);
  push('Action items', summary?.action_items ?? []);
  push('Decisions', summary?.decisions ?? []);

  return { root, branches };
}

/** Render the mind map as a markdown outline (for sharing/export). */
function toMarkdown(map: MindMap): string {
  const lines = [`# ${map.root}`, ''];
  for (const b of map.branches) {
    lines.push(`## ${b.label}`);
    for (const leaf of b.leaves) lines.push(`- ${leaf}`);
    lines.push('');
  }
  return `${lines.join('\n').trim()}\n`;
}

// ---------------------------------------------------------------------------

/** Full-screen mind-map view for a recording's summary. */
export function MindMapModal({
  visible,
  summary,
  title,
  onDismiss,
}: {
  visible: boolean;
  summary: Summary | null;
  title: string;
  onDismiss: () => void;
}) {
  const map = useMemo(() => buildMindMap(summary, title), [summary, title]);
  const empty = map.branches.length === 0;

  const share = () => {
    Share.share({ message: toMarkdown(map), title: `${map.root}.md` }).catch(() => {});
  };

  return (
    <Modal visible={visible} animationType="slide" onRequestClose={onDismiss} presentationStyle="pageSheet">
      <View style={styles.screen}>
        <View style={styles.header}>
          <Text style={styles.headerTitle}>Mind map</Text>
          <View style={styles.headerActions}>
            {!empty && (
              <TouchableOpacity onPress={share} accessibilityLabel="Share mind map" style={styles.headerBtn}>
                <Ionicons name="share-outline" size={20} color={colors.accent} />
              </TouchableOpacity>
            )}
            <TouchableOpacity onPress={onDismiss} accessibilityLabel="Close" style={styles.headerBtn}>
              <Ionicons name="close" size={22} color={colors.textMuted} />
            </TouchableOpacity>
          </View>
        </View>

        {empty ? (
          <View style={styles.emptyWrap}>
            <Ionicons name="git-branch-outline" size={30} color={colors.textDim} />
            <Text style={styles.emptyText}>
              No summary yet — generate one and its topics, action items and decisions become the
              mind map.
            </Text>
          </View>
        ) : (
          <ScrollView contentContainerStyle={styles.content}>
            {/* Root */}
            <View style={styles.rootNode}>
              <Text style={styles.rootText}>{map.root}</Text>
            </View>

            {map.branches.map((b) => (
              <View key={b.label} style={styles.branch}>
                <View style={styles.branchHeader}>
                  <View style={[styles.branchDot, { backgroundColor: b.color }]} />
                  <Text style={styles.branchLabel}>{b.label}</Text>
                </View>
                <View style={[styles.leaves, { borderLeftColor: b.color }]}>
                  {b.leaves.map((leaf, i) => (
                    <View key={i} style={styles.leafRow}>
                      <View style={[styles.leafDot, { backgroundColor: b.color }]} />
                      <Text style={styles.leafText}>{leaf}</Text>
                    </View>
                  ))}
                </View>
              </View>
            ))}
          </ScrollView>
        )}
      </View>
    </Modal>
  );
}

// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: colors.bg,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 18,
    paddingTop: 16,
    paddingBottom: 12,
  },
  headerTitle: {
    fontSize: 18,
    fontWeight: '700',
    color: colors.textPrimary,
  },
  headerActions: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  headerBtn: {
    padding: 4,
  },
  emptyWrap: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: 40,
    gap: 14,
  },
  emptyText: {
    fontSize: 14,
    lineHeight: 20,
    color: colors.textMuted,
    textAlign: 'center',
  },
  content: {
    paddingHorizontal: 20,
    paddingBottom: 48,
  },
  rootNode: {
    alignSelf: 'flex-start',
    backgroundColor: colors.accentSoft,
    borderWidth: 1,
    borderColor: colors.accent,
    borderRadius: radius.card,
    paddingHorizontal: 16,
    paddingVertical: 12,
    marginBottom: 8,
    maxWidth: '100%',
  },
  rootText: {
    fontSize: 16,
    fontWeight: '700',
    color: colors.textPrimary,
  },
  branch: {
    marginTop: 14,
  },
  branchHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  branchDot: {
    width: 11,
    height: 11,
    borderRadius: 6,
  },
  branchLabel: {
    fontSize: 13,
    fontWeight: '700',
    color: colors.textPrimary,
    fontFamily: mono,
    letterSpacing: 0.5,
  },
  leaves: {
    marginLeft: 5,
    paddingLeft: 16,
    borderLeftWidth: 2,
    marginTop: 6,
    gap: 8,
  },
  leafRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 9,
  },
  leafDot: {
    width: 6,
    height: 6,
    borderRadius: 3,
    marginTop: 7,
    opacity: 0.7,
  },
  leafText: {
    flex: 1,
    fontSize: 13.5,
    lineHeight: 20,
    color: colors.textLight,
  },
});
