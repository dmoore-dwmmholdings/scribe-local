"""Speak a scripted conversation through several macOS voices, with ground truth.

Writes `conversation.wav` (16 kHz mono) and `truth.json` recording exactly who
spoke when, for `cargo run --example diarize_check`. See docs/measuring-diarization.md.

    python3 scripts/make-diar-fixture.py <output-dir> [turns] [overlap-ms] [line-offset]

With no turn count it builds a short conversation. Pass a larger number for a
recording over ten minutes, which is where diarization switches to windowing the
audio and stitching the speaker sets back together.

An overlap in milliseconds makes every third turn start that far inside the one
before it, so two people are talking at once. Real conversation does this
constantly and a fixture built by concatenation can never show it.

A line offset starts the same voices on different sentences, for a second
recording of the same people saying something else — which is what testing
whether a name sticks across recordings needs.

Synthesised voices are not people: cleaner and more separable than a real room,
so treat a score from this as an upper bound and a way to compare two changes on
identical audio, not as a prediction of field accuracy.
"""
import json, os, re, subprocess, sys, wave

import numpy as np

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

# Four by default. `SCRIBE_FIXTURE_VOICES` takes a comma-separated list, for a
# larger meeting — a room of eight is ordinary and is a different problem from a
# room of four, since every extra voice is another chance to confuse two.
# Use the *natural* voices only. macOS also ships character voices — Eddy,
# Rocko, Grandma, Bells and the rest — and they are not speech: the same German
# script transcribes at 0.0% word error rate through Anna and 82.5% through two
# of those, which reads as the language being unsupported when it is not.
#
# The defaults span accents deliberately: British, American, Indian, Australian,
# and with more voices Irish and South African.
VOICES = os.environ.get(
    "SCRIBE_FIXTURE_VOICES", "Daniel,Samantha,Rishi,Karen"
).split(",")

# macOS ships three generations of voice under one list. The modern ones are
# recorded speech; the rest are synthesised, and a fixture built from them
# measures the fixture.
#
# Two ways they hurt, in opposite directions. The character voices — Bells,
# Zarvox, Boing — barely transcribe at all: a German script through them scored
# 43% word error rate and read as the language being unsupported. The older
# formant voices — Fred, Kathy, Ralph, Junior, Albert — do transcribe, and are
# *too easy to tell apart*: a six-speaker fixture including Fred scored 95.7% in
# a reverberant room where the same six modern voices scored 83.2%. One
# understates the code and the other flatters it.
#
# Neither is detectable from `say -v ?`, which lists them exactly like the rest.
_SYNTHETIC_VOICES = {
    # Character voices.
    "bad news", "bahh", "bells", "boing", "bubbles", "cellos", "deranged",
    "good news", "hysterical", "jester", "organ", "pipe organ", "princess",
    "superstar", "trinoids", "whisper", "wobble", "zarvox",
    # Formant-synthesis voices, from before the recorded ones.
    "agnes", "albert", "bruce", "fred", "junior", "kathy", "ralph", "vicki",
}

_SYNTHETIC_VOICE = (
    "a synthesised voice rather than a recorded one. Those either barely "
    "transcribe or are unrealistically easy to tell apart, and either way the "
    "numbers describe the fixture"
)
# Round robin through the voices, and never repeat a line.
def check_voices():
    """Refuse a voice that would measure the fixture rather than the code.

    Only the explicit denylist decides that. An earlier version also refused any
    voice macOS listed with a parenthesised name, on the theory that those were
    the character voices. They are not: the character voices have plain names
    (Bells, Zarvox), and what the parentheses actually mark is the Siri-era
    recorded voices — "Aman (English (India))" — which are exactly the ones
    worth using. That heuristic threw away two of the eight usable voices and
    reported them as not existing.
    """
    listed = subprocess.run(["say", "-v", "?"], capture_output=True, text=True).stdout
    known = set()
    for line in listed.splitlines():
        # "Name  lang_TAG  # sample" — the tag is what separates name from rest.
        m = re.match(r"^(.*?)\s+([a-z]{2}(?:_[A-Z]{2})?)\s*#", line)
        if m:
            name = m.group(1).strip()
            known.add(name)
            # `say -v Aman` works even though the list says "Aman (English
            # (India))", so accept what `say` accepts.
            known.add(name.split(" (")[0].strip())
    for v in VOICES:
        if v not in known:
            raise SystemExit(f"no such voice: {v!r} (try: say -v '?')")
        if v.lower() in _SYNTHETIC_VOICES:
            raise SystemExit(f"{v!r} is {_SYNTHETIC_VOICE}")


def build_lines(turns, offset=0):
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
        (
            VOICES[i % len(VOICES)],
            SENTENCES[(offset + i // len(VOICES)) % len(SENTENCES)],
        )
        for i in range(turns)
    ]


check_voices()
LINES = build_lines(
    int(sys.argv[2]) if len(sys.argv) > 2 else 30,
    int(sys.argv[4]) if len(sys.argv) > 4 else 0,
)

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

# Lay the turns onto one timeline, tracking exact boundaries.
#
# Turns are mixed rather than concatenated, so a turn can be told to start
# before the previous one finishes. Real conversation does this constantly —
# interruptions, and backchannels spoken straight over whoever has the floor —
# and it is the one condition a fixture built by concatenation can never show.
GAP_MS = 350
OVERLAP_MS = int(sys.argv[3]) if len(sys.argv) > 3 else 0
# How often an overlap happens, when one is asked for. Every turn overlapping
# the last is not a conversation, it is a crowd.
OVERLAP_EVERY = 3

clips = []
cursor_ms = 0
for i, (voice, wav) in enumerate(parts):
    with wave.open(wav, "rb") as w:
        assert w.getframerate() == 16000 and w.getnchannels() == 1
        data = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16)
        dur_ms = int(len(data) * 1000 / 16000)

    # Step back into the previous turn instead of leaving a gap after it.
    if OVERLAP_MS and i > 0 and i % OVERLAP_EVERY == 0:
        start_ms = max(0, cursor_ms - GAP_MS - OVERLAP_MS)
    else:
        start_ms = cursor_ms

    turns.append({"speaker": voice, "start_ms": start_ms, "end_ms": start_ms + dur_ms,
                  "text": LINES[i][1]})
    clips.append((start_ms, data))
    cursor_ms = max(cursor_ms, start_ms + dur_ms) + GAP_MS

total = np.zeros(int((cursor_ms + 1000) * 16000 / 1000), dtype=np.float64)
for start_ms, data in clips:
    a = int(start_ms * 16000 / 1000)
    total[a:a + len(data)] += data.astype(np.float64)
peak = np.max(np.abs(total))
if peak > 32000:  # two voices at once can sum past full scale
    total *= 32000 / peak
mixed = total.astype(np.int16)

# Time actually spent with two people talking at once.
overlap_ms = 0
for a in range(len(turns)):
    for b in range(a + 1, len(turns)):
        lo = max(turns[a]["start_ms"], turns[b]["start_ms"])
        hi = min(turns[a]["end_ms"], turns[b]["end_ms"])
        overlap_ms += max(0, hi - lo)

with wave.open("conversation.wav", "wb") as out:
    out.setnchannels(1); out.setsampwidth(2); out.setframerate(16000)
    out.writeframes(mixed.tobytes())

json.dump({"turns": turns, "duration_ms": cursor_ms, "overlap_ms": overlap_ms},
          open("truth.json", "w"), indent=2)
print(f"{os.getcwd()}/conversation.wav  {cursor_ms/1000:.1f}s  "
      f"{len(set(v for v,_ in LINES))} speakers  {len(turns)} turns  "
      f"{overlap_ms/1000:.1f}s overlapped")
