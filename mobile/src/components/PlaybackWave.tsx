/**
 * PlaybackWave — the Recording-Detail playback scrubber, in the kiln-fire
 * spirit of the EmberOrb.  Declarative Skia (no offscreen surface / snapshots):
 * a faint waveform silhouette (played portion glows warm), kiln-fire embers
 * streaming through the envelope via an Atlas, and a soft sweeping playhead at
 * the live playback position.
 *
 * Ember motion is stateless (clock-driven drift) so there's no cross-frame
 * mutable state to manage; brightness/size lifts across the played portion.
 * `progress` (0..1) and `playing` come from the live audio player.
 */

import { useMemo } from 'react';
import {
  Atlas,
  Canvas,
  Circle,
  Group,
  Path,
  RadialGradient,
  rect,
  Skia,
  useClock,
  useRSXformBuffer,
  useTexture,
  vec,
  type SkRect,
} from '@shopify/react-native-skia';
import { useDerivedValue, useSharedValue } from 'react-native-reanimated';
import { colors, emberPalette } from '../theme';

const TILE = 14;

interface PlaybackWaveProps {
  width: number;
  height?: number;
  /** 0..1 playback fraction. */
  progress?: number;
  playing?: boolean;
}

export function PlaybackWave({ width, height = 30, progress = 0, playing = false }: PlaybackWaveProps) {
  const w = Math.max(1, Math.round(width));
  const h = height;
  const mid = h / 2;

  const progressSV = useSharedValue(progress);
  progressSV.value = Math.max(0, Math.min(1, progress));
  const playingSV = useSharedValue(playing);
  playingSV.value = playing;

  const clock = useClock();

  // Bar silhouette envelope + cached paths.
  const { base, dimPath, hotPath, n } = useMemo(() => {
    const count = Math.max(24, Math.round(w / 4));
    const b: number[] = [];
    for (let i = 0; i < count; i++) {
      const s = Math.sin(i * 0.55 + 1.2) * 0.5 + 0.5;
      const s2 = Math.sin(i * 1.73 + 2.7) * 0.5 + 0.5;
      b.push(0.24 + 0.76 * (0.55 * s + 0.45 * s2));
    }
    const gap = 2;
    const bw = (w - gap * (count - 1)) / count;
    const dim = Skia.Path.Make();
    const hot = Skia.Path.Make();
    for (let i = 0; i < count; i++) {
      const bh = Math.max(1.5, b[i] * h * 0.7);
      const r = rect(i * (bw + gap), mid - bh / 2, Math.max(0.5, bw), bh);
      dim.addRRect(Skia.RRectXY(r, 1, 1));
      hot.addRRect(Skia.RRectXY(r, 1, 1));
    }
    return { base: b, dimPath: dim, hotPath: hot, n: count };
  }, [w, h, mid]);

  // Stateless ember seeds.
  const pcount = Math.max(36, Math.round(w / 2.6));
  const { frac0, off, speed, sprites } = useMemo(() => {
    const f: number[] = [];
    const o: number[] = [];
    const v: number[] = [];
    const sp: SkRect[] = [];
    for (let i = 0; i < pcount; i++) {
      f.push(Math.random());
      o.push(Math.random() * 2 - 1);
      v.push(0.02 + Math.random() * 0.03);
      const pal = (Math.random() * emberPalette.length) | 0;
      sp.push(rect(pal * TILE, 0, TILE, TILE));
    }
    return { frac0: f, off: o, speed: v, sprites: sp };
  }, [pcount]);

  const sheet = useTexture(
    <Group>
      {emberPalette.map((c, i) => {
        const center = vec(i * TILE + TILE / 2, TILE / 2);
        return (
          <Circle key={i} c={center} r={TILE / 2}>
            <RadialGradient
              c={center}
              r={TILE / 2}
              colors={[`rgba(${c[0]},${c[1]},${c[2]},0.95)`, `rgba(${c[0]},${c[1]},${c[2]},0)`]}
            />
          </Circle>
        );
      })}
    </Group>,
    { width: TILE * emberPalette.length, height: TILE },
  );

  const transforms = useRSXformBuffer(pcount, (val, i) => {
    'worklet';
    const t = clock.value / 1000;
    const prog = progressSV.value;
    const moving = playingSV.value ? 1 : 0.5;

    let x = (frac0[i] + speed[i] * t * moving) % 1;
    if (x < 0) x += 1;

    const f = x * (n - 1);
    const i0 = Math.floor(f);
    const i1 = i0 + 1 < n ? i0 + 1 : n - 1;
    const envAt = base[i0] + (base[i1] - base[i0]) * (f - i0);
    const band = envAt * h * 0.46;

    const px = x * w;
    const py = mid + off[i] * band;

    const played = x <= prog ? 1 : 0.4;
    const prox = Math.max(0, 1 - Math.abs(x - prog) * 5);
    const sc = (played * (0.7 + 0.5 * prox)) * (3 / TILE);

    const anchor = sc * (TILE / 2);
    val.set(sc, 0, px - anchor, py - anchor);
  });

  const playedClip = useDerivedValue(() => rect(0, 0, progressSV.value * w, h));
  const headX = useDerivedValue(() => progressSV.value * w);
  const headPath = useDerivedValue(() => {
    const p = Skia.Path.Make();
    p.addRect(rect(progressSV.value * w - 0.75, 1, 1.5, h - 2));
    return p;
  });

  return (
    <Canvas style={{ width: w, height: h }}>
      <Path path={dimPath} color="rgba(255,235,220,0.07)" />
      <Group clip={playedClip}>
        <Path path={hotPath} color="rgba(255,150,80,0.45)" />
      </Group>
      <Group blendMode="plus">
        <Atlas image={sheet} sprites={sprites} transforms={transforms} />
      </Group>
      <Circle cx={headX} cy={mid} r={6} color="rgba(255,180,100,0.35)" />
      <Path path={headPath} color={colors.accent} />
    </Canvas>
  );
}
