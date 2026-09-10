"""Mix non-speech events into a fixture: typing, doors, chairs, music, paper.

Real recordings are full of these and no synthetic conversation has any. They
matter because diarization has to decide they are not people: a door slam that
becomes "Speaker 5" is the most visible way speaker detection can fail.

    python3 scripts/add-room-noise.py <in.wav> <out.wav> [--gap-only] [--seed N]

Events are placed in the pauses between turns by default, where a real room puts
most of them, and `--gap-only` off allows them over speech as well. The truth
file is unchanged: these are not speakers and must not become any.
"""

import argparse
import json
import wave

import numpy as np

SR = 16_000


def typing(rng, secs=1.2):
    """Keystrokes: short broadband clicks with fast decay."""
    x = np.zeros(int(secs * SR))
    for _ in range(int(secs * 8)):
        at = rng.integers(0, len(x) - 400)
        click = rng.standard_normal(400) * np.exp(-np.arange(400) / 40.0)
        x[at:at + 400] += click
    return x / (np.max(np.abs(x)) or 1)


def door(rng, secs=0.6):
    """A slam: low-frequency thump with a short tail."""
    n = int(secs * SR)
    t = np.arange(n) / SR
    thump = np.sin(2 * np.pi * 70 * t) * np.exp(-t * 12)
    return (thump + rng.standard_normal(n) * 0.25 * np.exp(-t * 20)) / 1.25


def chair(rng, secs=0.9):
    """A scrape: filtered noise that rises and falls."""
    n = int(secs * SR)
    env = np.sin(np.pi * np.arange(n) / n) ** 2
    raw = rng.standard_normal(n)
    # Cheap low-pass so it is a scrape rather than a hiss.
    k = 64
    raw = np.convolve(raw, np.ones(k) / k, mode="same")
    return (raw * env) / (np.max(np.abs(raw * env)) or 1)


def music(rng, secs=3.0):
    """A few seconds of a held chord, the shape of hold music or a laptop."""
    n = int(secs * SR)
    t = np.arange(n) / SR
    x = sum(np.sin(2 * np.pi * f * t) for f in (220.0, 277.2, 329.6))
    env = np.minimum(1.0, np.minimum(t * 4, (secs - t) * 4))
    return (x / 3) * env


def paper(rng, secs=0.8):
    """Paper shuffling: bursty high-frequency noise."""
    n = int(secs * SR)
    x = rng.standard_normal(n) * (rng.random(n) > 0.7)
    return np.convolve(x, [1, -0.9], mode="same") / 3


EVENTS = [typing, door, chair, music, paper]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--truth", required=True)
    ap.add_argument("--over-speech", action="store_true")
    ap.add_argument("--level", type=float, default=0.5,
                    help="event loudness relative to speech RMS")
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()

    rng = np.random.default_rng(args.seed)
    with wave.open(args.src, "rb") as w:
        assert w.getframerate() == SR and w.getnchannels() == 1
        x = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64) / 32768

    speech = x[np.abs(x) > 0.01]
    rms = np.sqrt(np.mean(speech**2)) if speech.size else 0.05

    turns = json.load(open(args.truth))["turns"]
    # The pauses between turns, which is where a room puts most of its noise.
    gaps = []
    for a, b in zip(turns, turns[1:]):
        if b["start_ms"] - a["end_ms"] > 200:
            gaps.append((a["end_ms"], b["start_ms"]))

    placed = []
    for i, make in enumerate(EVENTS * 3):
        ev = make(rng) * rms / 0.3 * args.level
        if args.over_speech or not gaps:
            at_ms = int(rng.integers(0, max(1, len(x) // 16 - len(ev) // 16)))
        else:
            lo, hi = gaps[i % len(gaps)]
            at_ms = int(lo + (hi - lo) * 0.1)
        a = int(at_ms * SR / 1000)
        b = min(a + len(ev), len(x))
        x[a:b] += ev[: b - a]
        placed.append((make.__name__, at_ms / 1000))

    peak = np.max(np.abs(x))
    if peak > 0.99:
        x *= 0.99 / peak
    with wave.open(args.dst, "wb") as o:
        o.setnchannels(1); o.setsampwidth(2); o.setframerate(SR)
        o.writeframes((x * 32767).astype(np.int16).tobytes())

    print(f"{args.dst}  {len(placed)} events: " +
          ", ".join(f"{n}@{s:.0f}s" for n, s in placed[:6]) + " ...")


if __name__ == "__main__":
    main()
