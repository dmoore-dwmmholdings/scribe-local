#!/usr/bin/env python3
"""Splice one short turn by a new voice into an existing fixture.

The participant floor exists to fold away slivers — a stretch too short to be
anybody, left behind when a turn is split finely enough to catch a handover.
Every fixture here has speakers who talk for a quarter of the recording, so
none of them says anything about the person the floor must *not* fold: someone
who speaks once, briefly, and is still a participant.

That case decides the floor. Raising it from 3 s to 5 s removes a phantom
speaker from a perturbed six-voice recording (7 back to 6) and costs this one
their entire existence — 5 speakers to 4, and their sentence handed to whoever
spoke next. A phantom cluster is a cosmetic fault; a person missing from the
transcript is not.

    add-brief-speaker.py <src-fixture-dir> <out-dir> [voice] [seconds-in]
"""
import json
import os
import subprocess
import sys
import wave

import numpy as np

LINE = "Sorry to interrupt, but I think that deadline is going to be tight for us."


def read_wav(path):
    with wave.open(path, "rb") as w:
        assert w.getnchannels() == 1 and w.getsampwidth() == 2
        return w.getframerate(), np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16)


def main():
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    src, out = sys.argv[1], sys.argv[2]
    voice = sys.argv[3] if len(sys.argv) > 3 else "Tessa"
    after_turn = int(sys.argv[4]) if len(sys.argv) > 4 else 7

    truth = json.load(open(os.path.join(src, "truth.json")))
    if any(t["speaker"] == voice for t in truth["turns"]):
        raise SystemExit(f"{voice!r} already speaks in {src}; pick a voice that does not")
    sr, base = read_wav(os.path.join(src, "conversation.wav"))

    os.makedirs(out, exist_ok=True)
    aiff = os.path.join(out, "_brief.aiff")
    wav = os.path.join(out, "_brief.wav")
    subprocess.run(["say", "-v", voice, "-o", aiff, LINE], check=True)
    subprocess.run(
        ["ffmpeg", "-y", "-i", aiff, "-ar", str(sr), "-ac", "1", wav],
        check=True, capture_output=True,
    )
    _, brief = read_wav(wav)

    # Into the gap after `after_turn`, so it interrupts nobody.
    at_ms = truth["turns"][after_turn]["end_ms"] + 150
    at = int(at_ms * sr / 1000)
    mixed = np.concatenate([base[:at], brief, base[at:]])
    dur_ms = int(len(brief) * 1000 / sr)

    turns = []
    for t in truth["turns"]:
        shift = dur_ms if t["start_ms"] >= at_ms else 0
        turns.append({**t, "start_ms": t["start_ms"] + shift, "end_ms": t["end_ms"] + shift})
    turns.append({"speaker": voice, "start_ms": at_ms, "end_ms": at_ms + dur_ms, "text": LINE})
    turns.sort(key=lambda t: t["start_ms"])

    with wave.open(os.path.join(out, "conversation.wav"), "wb") as o:
        o.setnchannels(1)
        o.setsampwidth(2)
        o.setframerate(sr)
        o.writeframes(mixed.tobytes())
    json.dump(
        {"turns": turns, "duration_ms": truth["duration_ms"] + dur_ms, "overlap_ms": truth.get("overlap_ms", 0)},
        open(os.path.join(out, "truth.json"), "w"), indent=2,
    )
    for tmp in (aiff, wav):
        os.remove(tmp)
    speakers = len({t["speaker"] for t in turns})
    print(
        f"{out}/conversation.wav  {(truth['duration_ms'] + dur_ms)/1000:.1f}s  "
        f"{speakers} speakers  {voice} speaks {dur_ms/1000:.1f}s once"
    )


main()
