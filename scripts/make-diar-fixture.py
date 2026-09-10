"""Speak a scripted conversation through several macOS voices, with ground truth.

Writes `conversation.wav` (16 kHz mono) and `truth.json` recording exactly who
spoke when, for `cargo run --example diarize_check`. See docs/measuring-diarization.md.

    python3 scripts/make-diar-fixture.py <output-dir> [turns]

With no turn count it builds a short conversation. Pass a larger number for a
recording over ten minutes, which is where diarization switches to windowing the
audio and stitching the speaker sets back together.

Synthesised voices are not people: cleaner and more separable than a real room,
so treat a score from this as an upper bound and a way to compare two changes on
identical audio, not as a prediction of field accuracy.
"""
import json, os, subprocess, sys, wave

# A bank of distinct lines. Nothing is spoken twice: identical text through the
# same voice synthesises to identical audio, which embeds to a cosine of 1.0 and
# makes clustering look far easier than it is.
SENTENCES = [
    "I think the important thing is that we agree on sequencing before anyone starts building.",
    "Let me pull up the figures from last quarter so we are all looking at the same page.",
    "That does not match what I remember, but I am very happy to be corrected on the detail.",
    "We should write this down somewhere more permanent than the recording of a meeting.",
    "My worry is that the timeline assumes nobody takes any holiday between now and launch.",
    "Could we come back to that once we have heard from the platform side of the house?",
    "I think we are overcomplicating a decision that could be settled in about five minutes.",
    "Customer feedback has been remarkably consistent on this point for three months running.",
    "I will take that away and report back at the start of next week with a firm proposal.",
    "There is a dependency on the migration landing first, which is not in our control.",
    "If we go that route we will need roughly double the testing before anything ships.",
    "Sorry, could you repeat the last part of that? You cut out for a second on my end.",
    "The staging environment has been unreliable all week and it is slowing everybody down.",
    "Nobody has looked at that dashboard since the person who built it moved teams.",
    "I would rather ship something small in February than something perfect in June.",
    "We tried almost exactly this two years ago and it failed for reasons worth revisiting.",
    "Are we confident the numbers behind that chart are being refreshed automatically?",
    "It might help to write down what we are explicitly choosing not to do this quarter.",
    "The support queue doubled after the pricing change and has not come back down since.",
    "I am not blocking, I just want my concern recorded somewhere before we move on.",
    "Whoever picks this up will need access to the billing system, which takes a week.",
    "Let us assume the worst case on latency and see whether the design still holds up.",
    "That was my fault, I sent the announcement before the feature flag was fully rolled.",
    "There is an argument for doing nothing here and revisiting after the busy season.",
    "The contract renews in March, so anything we promise has to land well before that.",
    "I spoke to two customers this morning and neither had noticed the change at all.",
    "Can somebody confirm whether the old endpoint is actually switched off or just hidden?",
    "We keep discussing this in passing and never quite deciding, which is the real cost.",
    "My preference would be to split it, ship the read path now and the write path later.",
    "Honestly the documentation is worse than having none, because people trust it.",
    "The migration script has been sitting in review for eleven days without a comment.",
    "I would like us to agree what success looks like before we pick any of the options.",
    "Two of the three integrations broke silently and we only found out from a customer.",
    "There is no owner for that service, which is why nothing has been done about it.",
    "Can we get the retention numbers split by plan rather than reported in aggregate?",
    "My reading of the logs is that the retry storm made the outage considerably worse.",
    "It would be worth asking the field team before we commit to that particular date.",
    "We have three different definitions of an active user and all of them are in use.",
    "The onboarding flow drops about forty percent of people at the verification step.",
    "I would like to revisit the decision about the queue once the load testing is done.",
    "Nobody wants to be the person who says this, but the estimate was never realistic.",
    "Let us book thirty minutes on Thursday and work through the edge cases properly.",
    "The last time we shipped on a Friday it cost us the whole of the weekend after it.",
    "I have written it up in the document but I do not think anybody has opened it yet.",
    "That assumes the vendor honours the timeline in their proposal, which is optimistic.",
]

VOICES = ["Daniel", "Samantha", "Rishi", "Karen"]
# Round robin through the voices, and never repeat a line.
def build_lines(turns):
    """Round-robin the voices, and never speak the same line twice.

    Identical text through one voice synthesises to identical audio, which
    embeds to a cosine of 1.0 and makes clustering look far easier than it is —
    a fixture that repeated its lines scored 91% where the same code scores
    99.7% on one that does not. Each voice works through the whole bank, so a
    (voice, line) pair is used at most once.
    """
    if turns > len(VOICES) * len(SENTENCES):
        raise SystemExit(
            f"at most {len(VOICES) * len(SENTENCES)} turns without repeating a line; "
            "add more sentences to the bank"
        )
    return [
        (VOICES[i % len(VOICES)], SENTENCES[(i // len(VOICES)) % len(SENTENCES)])
        for i in range(turns)
    ]


LINES = build_lines(int(sys.argv[2]) if len(sys.argv) > 2 else 30)

out_dir = sys.argv[1] if len(sys.argv) > 1 else "."
os.makedirs(out_dir, exist_ok=True)
os.chdir(out_dir)

turns = []
parts = []
for i, (voice, text) in enumerate(LINES):
    aiff = f"line{i}.aiff"
    wav = f"line{i}.wav"
    subprocess.run(["say", "-v", voice, "-o", aiff, text], check=True)
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", aiff,
                    "-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le", wav], check=True)
    parts.append((voice, wav))

# Concatenate with a short pause between turns, tracking exact boundaries.
GAP_MS = 350
frames = []
cursor_ms = 0
gap_frames = b"\x00\x00" * int(16000 * GAP_MS / 1000)
for voice, wav in parts:
    with wave.open(wav, "rb") as w:
        assert w.getframerate() == 16000 and w.getnchannels() == 1
        data = w.readframes(w.getnframes())
        dur_ms = int(w.getnframes() * 1000 / 16000)
    turns.append({"speaker": voice, "start_ms": cursor_ms, "end_ms": cursor_ms + dur_ms,
                  "text": LINES[len(turns)][1]})
    frames.append(data)
    cursor_ms += dur_ms
    frames.append(gap_frames)
    cursor_ms += GAP_MS

with wave.open("conversation.wav", "wb") as out:
    out.setnchannels(1); out.setsampwidth(2); out.setframerate(16000)
    out.writeframes(b"".join(frames))

json.dump({"turns": turns, "duration_ms": cursor_ms}, open("truth.json", "w"), indent=2)
print(f"{os.getcwd()}/conversation.wav  {cursor_ms/1000:.1f}s  {len(set(v for v,_ in LINES))} speakers  {len(turns)} turns")
