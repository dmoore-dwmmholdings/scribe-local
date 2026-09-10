"""Make a clean fixture sound like it was recorded in a room.

    python3 scripts/degrade-audio.py <in.wav> <out.wav> [--snr DB] [--reverb S]
                                     [--far SPEAKER=GAIN ...] --truth <truth.json>

Every number the diarization benchmark reports comes from voices synthesised
straight to a file: no room, no microphone, no background, every speaker at
exactly the same level. Real recordings have all four, and they are what speaker
detection actually has to cope with. This applies them to a fixture so the same
conversation can be scored clean and dirty.

  --snr      additive pink noise at this signal-to-noise ratio in dB.
             20 is a quiet office, 10 a busy one, 5 unpleasant.
  --reverb   exponential-decay reverberation with this RT60 in seconds.
             0.3 is a small meeting room, 0.8 a hard-surfaced one.
  --far      scale one speaker's turns, for somebody sitting away from the mic.
             Needs --truth to know which stretches are theirs.
  --phone    one speaker comes through a telephone: band-limited to roughly
             300-3400 Hz, lightly compressed, with a little codec noise. A
             hybrid meeting has one of these in it almost by definition, and it
             is a large spectral change to a voice.
  --moving   one speaker drifts nearer and further as the recording goes on, the
             way somebody does who leans back, turns to a whiteboard, or walks.
             Their level and their reverberation change together, because both
             follow the distance. Needs --truth.

Reverb and level differences matter more than noise here: they change a voice's
spectrum, which is what a speaker embedding is measuring, where broadband noise
mostly just buries it.
"""

import argparse
import json
import wave

import numpy as np

SR = 16_000


def read_wav(path):
    with wave.open(path, "rb") as w:
        assert w.getnchannels() == 1 and w.getsampwidth() == 2
        sr = w.getframerate()
        data = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16)
    return data.astype(np.float64) / 32768.0, sr


def write_wav(path, x, sr):
    peak = np.max(np.abs(x))
    if peak > 0.99:  # leave headroom rather than clip
        x = x * (0.99 / peak)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        w.writeframes((x * 32767).astype(np.int16).tobytes())


def pink_noise(n, rng):
    """Pink noise: 1/f, which is far closer to room and office noise than white."""
    white = rng.standard_normal(n)
    spectrum = np.fft.rfft(white)
    freqs = np.fft.rfftfreq(n, 1 / SR)
    freqs[0] = freqs[1] if len(freqs) > 1 else 1.0
    spectrum /= np.sqrt(freqs)
    out = np.fft.irfft(spectrum, n)
    return out / (np.std(out) or 1.0)


def reverberate(x, rt60, rng):
    """Convolve with a synthetic room response: exponentially decaying noise.

    Crude next to a measured impulse response, but it does the thing that
    matters — smears each sound over the ones after it, so a speaker's tail runs
    into whoever follows them.
    """
    n = int(rt60 * SR)
    t = np.arange(n) / SR
    ir = rng.standard_normal(n) * np.exp(-6.9 * t / rt60)
    ir[0] += 1.0  # keep the direct sound
    ir /= np.sqrt(np.sum(ir**2))
    return np.convolve(x, ir)[: len(x)]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--snr", type=float)
    ap.add_argument("--reverb", type=float)
    ap.add_argument("--far", action="append", default=[])
    ap.add_argument("--moving", action="append", default=[])
    ap.add_argument("--phone", action="append", default=[])
    ap.add_argument("--truth")
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()

    rng = np.random.default_rng(args.seed)
    x, sr = read_wav(args.src)
    assert sr == SR, f"expected {SR} Hz, got {sr}"
    applied = []

    # Level differences first: they are a property of where somebody sat, so
    # they happen before the room and the microphone get hold of the sound.
    if args.far:
        if not args.truth:
            raise SystemExit("--far needs --truth to know whose turns to scale")
        truth = json.load(open(args.truth))
        gains = dict(kv.split("=") for kv in args.far)
        for turn in truth["turns"]:
            gain = gains.get(turn["speaker"])
            if gain is None:
                continue
            a = int(turn["start_ms"] * SR / 1000)
            b = min(int(turn["end_ms"] * SR / 1000), len(x))
            x[a:b] *= float(gain)
        applied.append("far=" + ",".join(f"{k}x{v}" for k, v in gains.items()))

    # Somebody who moves about. Distance changes two things together — how loud
    # they are and how much of the room you hear with them — so a drifting gain
    # alone would not be the test. The question is whether their voice still
    # clusters as one person once both have changed across the recording.
    if args.moving:
        if not args.truth:
            raise SystemExit("--moving needs --truth to know whose turns to move")
        truth = json.load(open(args.truth))
        wet = reverberate(x, 0.6, np.random.default_rng(args.seed + 1))
        for name in args.moving:
            theirs = [t for t in truth["turns"] if t["speaker"] == name]
            for i, turn in enumerate(theirs):
                # Nearest at the start, furthest in the middle, back by the end.
                phase = i / max(1, len(theirs) - 1)
                distance = np.sin(np.pi * phase)
                gain = 1.0 - 0.65 * distance
                a = int(turn["start_ms"] * SR / 1000)
                b = min(int(turn["end_ms"] * SR / 1000), len(x))
                x[a:b] = x[a:b] * gain * (1 - distance) + wet[a:b] * gain * distance
        applied.append("moving=" + ",".join(args.moving))

    # Somebody on the phone. The band limit is the point: a voice with its low
    # end and its top removed is spectrally a different voice, which is exactly
    # what a speaker embedding measures.
    if args.phone:
        if not args.truth:
            raise SystemExit("--phone needs --truth to know whose turns to band-limit")
        truth = json.load(open(args.truth))
        for name in args.phone:
            for turn in truth["turns"]:
                if turn["speaker"] != name:
                    continue
                a = int(turn["start_ms"] * SR / 1000)
                b = min(int(turn["end_ms"] * SR / 1000), len(x))
                seg = x[a:b]
                if seg.size < 64:
                    continue
                # Band-pass by zeroing everything outside the telephone band.
                spec = np.fft.rfft(seg)
                freqs = np.fft.rfftfreq(seg.size, 1 / SR)
                spec[(freqs < 300) | (freqs > 3400)] = 0
                seg = np.fft.irfft(spec, seg.size)
                # A little compression and codec hiss, as a phone line has.
                peak = np.max(np.abs(seg)) or 1.0
                seg = np.tanh(seg / peak * 1.8) * peak * 0.8
                seg += rng.standard_normal(seg.size) * peak * 0.01
                x[a:b] = seg
        applied.append("phone=" + ",".join(args.phone))

    if args.reverb:
        x = reverberate(x, args.reverb, rng)
        applied.append(f"reverb RT60={args.reverb}s")

    if args.snr is not None:
        speech = x[np.abs(x) > 0.01]
        rms = np.sqrt(np.mean(speech**2)) if speech.size else np.sqrt(np.mean(x**2))
        noise = pink_noise(len(x), rng) * rms / (10 ** (args.snr / 20))
        x = x + noise
        applied.append(f"pink noise SNR={args.snr}dB")

    write_wav(args.dst, x, SR)
    print(f"{args.dst}  {len(x)/SR:.1f}s  [{'; '.join(applied) or 'unchanged'}]")


if __name__ == "__main__":
    main()
