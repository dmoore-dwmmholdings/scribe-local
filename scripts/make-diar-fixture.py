"""Speak a scripted conversation through several macOS voices, with ground truth.

Writes `conversation.wav` (16 kHz mono) and `truth.json` recording exactly who
spoke when, for `cargo run --example diarize_check`. See docs/measuring-diarization.md.

    python3 scripts/make-diar-fixture.py <output-dir>

Synthesised voices are not people: cleaner and more separable than a real room,
so treat a score from this as an upper bound and a way to compare two changes on
identical audio, not as a prediction of field accuracy.
"""
import json, os, subprocess, sys, wave

LINES = [
    ("Samantha", "Good morning everyone, thanks for joining the quarterly planning call today."),
    ("Daniel",   "Morning. I have the revenue numbers ready whenever you want to walk through them."),
    ("Karen",    "Before we start, could someone remind me where we landed on the hiring freeze?"),
    ("Samantha", "We agreed to pause new requisitions until the end of the second quarter."),
    ("Daniel",   "That is right, and it already shows up in the forecast I circulated last week."),
    ("Karen",    "Understood. Then my only concern is whether support can absorb the extra volume."),
    ("Samantha", "Let us take that offline and come back with a staffing proposal next Tuesday."),
    ("Daniel",   "Works for me. I will pull the ticket backlog and share it before the meeting."),
    ("Karen",    "Sounds good, I will bring the customer satisfaction trend for the same period."),
    ("Samantha", "Excellent. Anything else anyone wants to raise before we close out this call?"),
    ("Daniel",   "Nothing further from finance, we are in reasonable shape heading into the quarter."),
    ("Karen",    "Same here, nothing urgent from the support side beyond what we already covered."),
]

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
    turns.append({"speaker": voice, "start_ms": cursor_ms, "end_ms": cursor_ms + dur_ms})
    frames.append(data)
    cursor_ms += dur_ms
    frames.append(gap_frames)
    cursor_ms += GAP_MS

with wave.open("conversation.wav", "wb") as out:
    out.setnchannels(1); out.setsampwidth(2); out.setframerate(16000)
    out.writeframes(b"".join(frames))

json.dump({"turns": turns, "duration_ms": cursor_ms}, open("truth.json", "w"), indent=2)
print(f"{os.getcwd()}/conversation.wav  {cursor_ms/1000:.1f}s  {len(set(v for v,_ in LINES))} speakers  {len(turns)} turns")
