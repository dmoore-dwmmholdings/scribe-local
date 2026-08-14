/**
 * Kiln design system — the warm-dark, kiln-fire palette the mobile app is
 * built on.  Ported 1:1 from the Claude Design "Scribe · Kiln — 03" handoff:
 * warm near-black backgrounds, a kiln-fire orange accent, neutral-grotesk body
 * type with monospaced technical accents, and a living ember vortex as the
 * through-line (see src/components/EmberOrb).
 *
 * Dark-only by design (the user asked for dark mode exclusively).
 */

import { Platform } from 'react-native';

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

export const colors = {
  /** Page background — warm near-black. */
  bg: '#100c0b',
  /** Raised surfaces: cards, inputs, controls. */
  surface: '#1a1513',
  /** Bottom tab bar / footer chrome. */
  surfaceAlt: '#150f0d',
  /** Device-bezel black behind a sheet (rarely used in-app). */
  ink: '#070605',

  /** Kiln-fire orange — the primary accent. */
  accent: '#ff7a3c',
  /** Deep ember used for the accent gradient's far stop. */
  accentDeep: '#e8512e',
  /** Calmer amber used for paused / warning affordances. */
  amber: '#e2a85f',
  /** Healthy / ready green (RDY chip, connected dot). */
  statusReady: '#93b072',

  /** Primary text — warm off-white. */
  textPrimary: '#f0e5d8',
  /** Body copy inside answer/transcript blocks. */
  textBody: '#e7dcc5',
  /** Lighter secondary text (suggestions, light labels). */
  textLight: '#d8cabb',
  /** Muted secondary text — warm taupe. */
  textMuted: '#9c8a7c',
  /** Dim tertiary text — timestamps, inactive tab labels. */
  textDim: '#6a594d',
  /** Search-snippet body. */
  textSnippet: '#b9a99a',
  /** Figure captions / hints. */
  caption: '#8c7d70',

  white: '#ffffff',

  // Borders ---------------------------------------------------------------
  border: 'rgba(255,235,220,0.07)',
  borderInput: 'rgba(255,235,220,0.09)',
  borderButton: 'rgba(255,235,220,0.15)',
  borderDivider: 'rgba(255,235,220,0.06)',
  sectionDivider: 'rgba(255,255,255,0.09)',

  // Accent washes ---------------------------------------------------------
  /** Soft accent fill behind pills / active rows. */
  accentSoft: 'rgba(255,122,60,0.12)',
  /** Faint accent tile behind ember thumbnails. */
  accentFaint: 'rgba(255,122,60,0.07)',
  /** Active filter-chip fill + border. */
  chipActiveBg: 'rgba(255,122,60,0.15)',
  chipActiveBorder: 'rgba(255,122,60,0.3)',
  /** Inactive filter-chip fill. */
  chipInactiveBg: 'rgba(255,255,255,0.05)',
  /** Search highlight (<mark>) background + text. */
  markBg: 'rgba(255,122,60,0.28)',
  markText: '#ffd9bf',

  // Toggles ---------------------------------------------------------------
  toggleOff: 'rgba(255,235,220,0.12)',
  toggleKnobOff: '#d8cabb',

  /** Handle/scrim for modals. */
  scrim: 'rgba(0,0,0,0.6)',
} as const;

/** The accent gradient (135°, orange → deep ember). */
export const accentGradient = ['#ff7a3c', '#e8512e'] as const;

// ---------------------------------------------------------------------------
// Speaker color-rails (transcript / library dots).  Orange leads, matching the
// design's Dawson=orange, Priya=blue, Marco=green, +purple/amber/teal.
// ---------------------------------------------------------------------------

export const speakerColors = [
  '#ff7a3c', // 0
  '#6f9fd8', // 1
  '#93b072', // 2
  '#c98bd0', // 3
  '#e2a85f', // 4
  '#6fd0c0', // 5
] as const;

export function speakerColor(localIdx: number | null | undefined): string {
  if (localIdx == null) return colors.textMuted;
  return speakerColors[localIdx % speakerColors.length];
}

// ---------------------------------------------------------------------------
// Recording-status accents.  The design surfaces these as mono chips:
// RDY (green), PROC (orange).  We keep failed on deep ember.
// ---------------------------------------------------------------------------

export const statusColor: Record<string, string> = {
  uploading: '#ff7a3c',
  processing: '#ff7a3c',
  ready: '#93b072',
  failed: '#e8512e',
};

/** Short mono badge label per status, as in the Library design. */
export const statusBadge: Record<string, string> = {
  uploading: 'UP',
  processing: 'PROC',
  ready: 'RDY',
  failed: 'FAIL',
};

// ---------------------------------------------------------------------------
// Typography
// ---------------------------------------------------------------------------

/** Monospaced family used for technical accents (timers, labels, addresses). */
export const mono = Platform.select({ ios: 'Menlo', default: 'monospace' });

export const radius = {
  card: 16,
  inner: 14,
  input: 13,
  thumb: 12,
  thumbSm: 8,
  pill: 20,
  chip: 999,
} as const;

// ---------------------------------------------------------------------------
// Ember vortex palette (gold core → tangerine → coral → ember-pink).  Shared
// by the orb and the playback waveform so the whole app shares one fire.
// ---------------------------------------------------------------------------

export const emberPalette: ReadonlyArray<readonly [number, number, number]> = [
  [255, 214, 140],
  [255, 176, 80],
  [255, 120, 54],
  [255, 72, 64],
  [255, 98, 150],
];
