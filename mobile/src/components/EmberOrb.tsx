/**
 * EmberOrb — the living kiln-fire vortex that threads through the whole app.
 *
 * Rendered with @shopify/react-native-skia's declarative <Atlas> (GPU-batched
 * sprites) driven by a Reanimated clock — the stable path, with no offscreen
 * surface, no per-frame makeImageSnapshot, and no manual image disposal (those
 * caused use-after-free / OOM crashes during long sessions).
 *
 * The motion is the design's `support.js` flow field: each ember is advected by
 * a tangential swirl + turbulent noise + slight outward drift, and recycles
 * into the core at the rim.  The flow field's coherent inward phases make the
 * cloud occasionally ball up into a tight glow ("simulation vibe").  Persistent
 * ember positions live in a plain-number shared value (no native objects), and
 * each ember leaves a short size-tapered trail (a few ghost sprites along its
 * recent path) for the silky look.  A hot radial core glow breathes with a
 * simulated speech envelope.
 *
 * Variants: "big" (~220px hero), "small" (~58–74px), "mini" (~30–54px).
 */

import { useEffect, useMemo } from 'react';
import {
  Atlas,
  Canvas,
  Circle,
  Group,
  rect,
  RadialGradient,
  useClock,
  useRSXformBuffer,
  useTexture,
  vec,
  type SkRect,
} from '@shopify/react-native-skia';
import { useDerivedValue, useSharedValue } from 'react-native-reanimated';
import { emberPalette } from '../theme';
import { useSettingsStore } from '../state/settingsStore';
import { log } from '../util/logger';

type Variant = 'big' | 'small' | 'mini';

/** Stable [0,8) seed from a string id, so an id's ember always looks the same. */
export function seedFromString(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return (h % 800) / 100;
}

interface EmberOrbProps {
  /** Square edge length in logical px. */
  size: number;
  variant?: Variant;
  /** Varies the flow field so instances look distinct. */
  seed?: number;
  /** Swirl direction: 1 or -1. */
  dir?: 1 | -1;
}

const TILE = 22; // sprite-sheet cell (one soft dot per palette color)

// Head count, trail length, and head dot size (logical px) per variant.
// Many small, low-alpha sprites → additive density builds the glow + silky
// filaments (close to the design's fade-buffer look) while staying on the
// stable Atlas renderer.
const SPEC: Record<Variant, { count: number; trail: number; dot: number }> = {
  big: { count: 500, trail: 9, dot: 4 },
  small: { count: 170, trail: 6, dot: 3.4 },
  mini: { count: 76, trail: 4, dot: 3 },
};

/** Static, non-animated fallback (diagnostic isolation / reduce-motion). */
function StaticEmberOrb({ size }: { size: number }) {
  const cx = size / 2;
  const cy = size / 2;
  const colors = useMemo(
    () => ['rgba(255,176,80,0.85)', 'rgba(255,120,54,0.5)', 'rgba(255,72,64,0)'],
    [],
  );
  return (
    <Canvas style={{ width: size, height: size }}>
      <Circle c={vec(cx, cy)} r={size * 0.42}>
        <RadialGradient c={vec(cx, cy)} r={size * 0.42} colors={colors} positions={[0, 0.45, 1]} />
      </Circle>
    </Canvas>
  );
}

export function EmberOrb(props: EmberOrbProps) {
  const reduceMotion = useSettingsStore((s) => s.reduceMotion);
  if (reduceMotion) return <StaticEmberOrb size={props.size} />;
  return <AnimatedEmberOrb {...props} />;
}

function AnimatedEmberOrb({ size, variant = 'big', seed, dir = 1 }: EmberOrbProps) {
  const { count, trail: K, dot } = SPEC[variant];
  const cx = size / 2;
  const cy = size / 2;
  const R = size * 0.36;
  const baseScale = dot / TILE;
  const fseed = seed ?? 1.1;
  const total = count * K;

  const clock = useClock();
  const env = useSharedValue(0.4);
  // Persistent ember-path state: plain number arrays only (worklet-safe).
  const state = useSharedValue<{ hx: number[]; hy: number[] } | null>(null);

  // Per-ember palette assignment → one Atlas sprite rect per (ember, trail-k).
  const sprites = useMemo(() => {
    const arr: SkRect[] = [];
    for (let p = 0; p < count; p++) {
      const pal = (Math.random() * emberPalette.length) | 0;
      const r = rect(pal * TILE, 0, TILE, TILE);
      for (let k = 0; k < K; k++) arr.push(r);
    }
    return arr;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [count, K]);

  // Soft radial dot per palette color → the additive ember sprite sheet.
  const sheet = useTexture(
    <Group>
      {emberPalette.map((c, i) => {
        const center = vec(i * TILE + TILE / 2, TILE / 2);
        return (
          <Circle key={i} c={center} r={TILE / 2}>
            <RadialGradient
              c={center}
              r={TILE / 2}
              colors={[`rgba(${c[0]},${c[1]},${c[2]},0.4)`, `rgba(${c[0]},${c[1]},${c[2]},0)`]}
            />
          </Circle>
        );
      })}
    </Group>,
    { width: TILE * emberPalette.length, height: TILE },
  );

  const transforms = useRSXformBuffer(total, (val, idx) => {
    'worklet';
    // Lazily seed the ember field once (plain arrays on the UI runtime).
    let st = state.value;
    if (!st) {
      const hx: number[] = [];
      const hy: number[] = [];
      for (let p = 0; p < count; p++) {
        const a = Math.random() * Math.PI * 2;
        const rr = Math.sqrt(Math.random()) * R;
        const px = cx + Math.cos(a) * rr;
        const py = cy + Math.sin(a) * rr;
        for (let k = 0; k < K; k++) {
          hx.push(px);
          hy.push(py);
        }
      }
      st = { hx, hy };
      state.value = st;
    }

    const t = clock.value / 1000;
    const p = (idx / K) | 0;
    const k = idx % K;
    const base = p * K;
    const hx = st.hx;
    const hy = st.hy;

    // Advance each ember once per frame (at its trail head, k === 0).
    if (k === 0) {
      if (idx === 0) {
        let target =
          0.36 +
          0.15 * Math.sin(t * 1.25 + fseed) +
          0.09 * Math.sin(t * 2.3 + fseed * 1.3) +
          0.05 * Math.sin(t * 4.3 + fseed);
        if (target < 0.04) target = 0.04;
        if (target > 1) target = 1;
        env.value += (target - env.value) * 0.07;
      }
      const e = env.value;
      // Calm swirl speed (the design's value) — fast enough to ball up, slow
      // enough that the trail reads as a smooth comet, not a string of beads.
      const spd = R * (0.007 + 0.017 * e);

      let px = hx[base];
      let py = hy[base];
      const dx = px - cx;
      const dy = py - cy;
      const r = Math.sqrt(dx * dx + dy * dy) || 0.0001;
      const tx = -dy / r;
      const ty = dx / r;
      const fl =
        (Math.sin(px * 0.019 + t * 0.4) +
          Math.sin(py * 0.021 - t * 0.33) +
          Math.sin((px + py) * 0.013 + t * 0.27) +
          Math.sin((px - py) * 0.016 - t * 0.21)) *
        0.9;
      const ang = fl * Math.PI + fseed * 1.7;
      const vx = tx * 0.96 * dir + Math.cos(ang) * 0.38 + (dx / r) * 0.08;
      const vy = ty * 0.96 * dir + Math.sin(ang) * 0.38 + (dy / r) * 0.08;
      const vl = Math.sqrt(vx * vx + vy * vy) || 1;
      px += (vx / vl) * spd;
      py += (vy / vl) * spd;

      const recycled = r > R * 0.99 || Math.random() < 0.0045;
      if (recycled) {
        const a2 = Math.random() * Math.PI * 2;
        const rr2 = Math.random() * R * 0.18;
        px = cx + Math.cos(a2) * rr2;
        py = cy + Math.sin(a2) * rr2;
      }

      // Shift the trail and push the new head; collapse the trail on recycle.
      if (recycled) {
        for (let j = 0; j < K; j++) {
          hx[base + j] = px;
          hy[base + j] = py;
        }
      } else {
        for (let j = K - 1; j >= 1; j--) {
          hx[base + j] = hx[base + j - 1];
          hy[base + j] = hy[base + j - 1];
        }
        hx[base] = px;
        hy[base] = py;
      }
    }

    const e = env.value;
    const sx = hx[base + k];
    const sy = hy[base + k];
    const dxh = hx[base] - cx;
    const dyh = hy[base] - cy;
    const rn = (Math.sqrt(dxh * dxh + dyh * dyh) || 0) / R;
    const feather = rn > 0.62 ? Math.max(0, 1 - (rn - 0.62) / 0.38) : 1;
    const taper = 1 - (k / K) * 0.55; // head full, tail tapers (comet)
    const sc = baseScale * (0.6 + 0.5 * e) * taper * feather;

    const anchor = sc * (TILE / 2);
    val.set(sc, 0, sx - anchor, sy - anchor);
  });

  const glowOpacity = useDerivedValue(() => 0.2 + 0.32 * env.value);
  const glowColors = useMemo(() => ['rgba(255,206,128,0.22)', 'rgba(255,206,128,0)'], []);

  // Breadcrumb the hero orb's lifecycle (it's the long-running one on Record).
  useEffect(() => {
    if (variant === 'big') log('orb', `big orb mount size=${size} count=${count} trail=${K}`);
    return () => {
      if (variant === 'big') log('orb', 'big orb unmount');
    };
  }, [variant, size, count, K]);

  return (
    <Canvas style={{ width: size, height: size }}>
      <Circle c={vec(cx, cy)} r={R * 0.62} opacity={glowOpacity}>
        <RadialGradient c={vec(cx, cy)} r={R * 0.62} colors={glowColors} />
      </Circle>
      <Group blendMode="plus">
        <Atlas image={sheet} sprites={sprites} transforms={transforms} />
      </Group>
    </Canvas>
  );
}
