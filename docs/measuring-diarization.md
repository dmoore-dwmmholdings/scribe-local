# Measuring transcription and speaker detection

`crates/scribe-asr/examples/diarize_check.rs` scores the real diarizer against a
conversation whose speakers are known, and reports what fraction of speech was
given to the right person. It is an example rather than a test because it needs
the ONNX models and a WAV file.

There are two harnesses. `diarize_check` (in `scribe-asr`) scores the diarizer
on its own and needs only the two diarization models. `transcript_check` (in
`scribe-pipeline`) scores what a reader actually sees — real ASR word timings,
real diarization, and the merge stage's own labelling — and needs an ASR
checkpoint as well.

## Get the models

The diarization pair is about 107 MB and is all `diarize_check` needs:

```bash
mkdir -p models/diarization && cd models/diarization
curl -L -o segmentation.onnx \
  https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.onnx
curl -L -o embedding.onnx \
  https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_large.onnx
```

## Build a conversation with known ground truth

`scripts/make-diar-fixture.py` speaks a scripted conversation through several
macOS `say` voices, concatenates the turns with a fixed pause, and writes both
the WAV and a `truth.json` recording exactly who spoke when.

```bash
python3 scripts/make-diar-fixture.py /tmp/diar        # short
python3 scripts/make-diar-fixture.py /tmp/diar-long 136  # over ten minutes
```

Synthesised voices are not people — they are cleaner and more separable than a
real room, so a score here is an upper bound, not a prediction. What it is good
for is comparing one change against another on identical audio.

Add Parakeet for `transcript_check` (about 670 MB):

```bash
mkdir -p models/asr/parakeet-tdt-0.6b-v3 && cd models/asr/parakeet-tdt-0.6b-v3
R=https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main
for f in encoder.int8.onnx decoder.int8.onnx joiner.int8.onnx tokens.txt; do
  curl -L -o "$f" "$R/$f"
done
```

## Run it

```bash
cargo build --release -p scribe-asr --example diarize_check
DYLD_LIBRARY_PATH="$PWD/target/release" \
  ./target/release/examples/diarize_check models /tmp/diar/conversation.wav /tmp/diar/truth.json
```

```bash
cargo build --release -p scribe-pipeline --example transcript_check
DYLD_LIBRARY_PATH="$PWD/target/release" \
  ./target/release/examples/transcript_check models /tmp/diar/conversation.wav /tmp/diar/truth.json
```

Pass a fourth argument to `diarize_check` to state the speaker count instead of
discovering it.
Set `DIARIZE_CHECK_TURNS=1` to print every turn found against every turn spoken,
which is how you see *where* it went wrong rather than only how far.
`TRANSCRIPT_CHECK_LINES=1` prints the finished transcript with speaker names.
`SCRIBE_DIARIZE_TIMING=1` splits diarization into segmentation and embedding;
`SCRIBE_ASR_THREADS` and `SCRIBE_ASR_DEVICE` override the thread count and the
execution provider.

The `DYLD_LIBRARY_PATH` is needed because the sherpa-onnx dylib is emitted next
to the binary without an rpath entry pointing at itself.

## Where it stands

Scoring the share of speech given to the right person, on fixtures built by
`make-diar-fixture.py`:

| fixture | mode | speakers found | correct |
|---|---|---|---|
| 3 voices, 55 s | discover | 3 of 3 | 99.6% |
| 3 voices, 55 s | stated 3 | 3 of 3 | 99.6% |
| 5 voices, 69 s | discover | 5 of 5 | 99.7% |
| 5 voices, 69 s | stated 5 | 5 of 5 | 99.7% |
| 4 voices, 2.5 min | discover | 4 of 4 | 99.7% |
| 4 voices, 11.2 min | discover | 4 of 4 | 99.7% |

The last row matters on its own: past ten minutes diarization windows the audio
and each window is clustered without knowing anything about the others, so the
speaker sets have to be stitched back together by voice. That path is only
exercised by a recording long enough to need it.

Before turns were split at pauses, the three-voice fixture scored 69.7%: every
time Karen spoke and Samantha followed, the segmentation model ran the two
together into a single turn and gave it to Samantha.

A wrong count is honoured rather than overridden — say 4 on the five-voice
fixture and four speakers come back, at 88.9%. The stated number is the
caller's to get right.

## End to end

`transcript_check`, discovering the speaker count, on a 15-core M-series machine:

| fixture | transcribe | diarize | total | WER | right speaker | utterances |
|---|---|---|---|---|---|---|
| 4 voices, 2.5 min | 18.2x | 9.2x | 6.1x | 4.2% | 100.0% | 29 of 30 turns |
| 4 voices, 11.1 min | 20.4x | 7.3x | 5.4x | 0.7% | 100.0% | 136 of 136 turns |

Multiples are of real time, so 5.4x means an hour of audio in about eleven
minutes. Two things worth reading off that table:

**Diarization is the bottleneck, not transcription.** It runs at roughly a third
the speed of ASR, and the two are separate pipeline stages, so effort spent
making transcription faster is effort spent on the cheaper half.

**Within diarization, segmentation is nearly all of it** — `SCRIBE_DIARIZE_TIMING`
puts it at 13.5 s against 1.2 s of embedding on the 2.5-minute fixture, or 91%
against 8%. Embedding every piece separately, which is what makes the speaker
labelling work, costs almost nothing. sherpa-onnx exposes speaker diarization
only as one object, so the segmentation model cannot currently be run without
the embedding and clustering it does internally and this code then discards.

The merge stage — assigning each word a speaker and smoothing the strays — does
not register at millisecond resolution.

Note that `smoothed` reads 0 on both fixtures: the stray-word smoothing never
fires on audio this clean, so it remains unmeasured. It is written to be inert
when diarization and ASR agree, and that is all these numbers show.

## Dirty audio

`scripts/degrade-audio.py` puts a fixture through a room: reverberation, pink
noise at a chosen signal-to-noise ratio, and a per-speaker gain for somebody
sitting away from the microphone.

```bash
python3 scripts/degrade-audio.py in.wav out.wav \
    --reverb 0.4 --snr 15 --far Karen=0.3 --truth truth.json
```

Reverb and level differences matter more than noise: they change a voice's
spectrum, which is what a speaker embedding measures, where broadband noise
mostly buries it. Everything above this section was measured on audio with none
of the three, and it flattered the transcriber badly — the diarizer much less.

Word error rate on the "dirty" preset above (RT60 0.4 s, 15 dB SNR, one speaker
at a third of the level):

| fixture | before | after |
|---|---|---|
| 4 voices, 2.5 min, clean | 4.2% | 1.5% |
| 4 voices, 2.5 min, dirty | 57.4% | 5.1% |
| 4 voices, 11.1 min, clean | 0.6% | 1.0% |
| 4 voices, 11.1 min, dirty | 19.1% | 5.1% |

"Before" is a 150-second decode window cut at a fixed offset; "after" is 30
seconds cut at the quietest nearby moment. The eleven-minute clean case is 8
edits worse out of 2060 words, which is the price of moving boundaries around,
and is paid back four times over everywhere else.

Speaker attribution barely moves under degradation: 99.9% of words on the right
person on the dirty eleven-minute fixture, against 100.0% clean. The splitting
and clustering work described above is not what these conditions break.

## Overlapping speech

`make-diar-fixture.py` takes an overlap in milliseconds as its third argument,
making every third turn start that far inside the one before it. On the
two-and-a-half-minute fixture:

| overlap | time with two voices | WER | right speaker |
|---|---|---|---|
| none | 0.0 s | 1.5% | 100.0% |
| 600 ms | 5.4 s | 7.8% | 100.0% |
| 1500 ms | 13.5 s | 26.6% | 100.0% |

Overlap is an ASR problem, not a diarization one. Attribution does not move;
the words do, because a single channel carrying two voices is a single channel
carrying two voices and the transcriber can only have one of them. Words spoken
over somebody are scored right if they land on either speaker, and separately
reported.

## The smoothing that never fired

The merge stage used to repair brief speaker flickers — a word mid-sentence
handed to whoever spoke next, because a diarization turn boundary and an ASR
word boundary disagreed by a few hundred milliseconds. It was written from a
plausible failure and never measured.

Across every condition here — clean, pink noise at 20 dB and 10 dB, reverb, a
speaker at a third of the level, whole sentences overlapping, and short
backchannels spoken into the middle of somebody's turn — it fired zero times.
Not "rarely": the runs it looked for never existed. Splitting turns at pauses
removed the fragmentation that produced them, and a backchannel over an open
microphone is usually not transcribed at all, so there is no stray word to move.

It is now a count rather than a repair, logged at debug level. If a real
recording produces these, the log says so, which is the evidence a repair would
need and more than the original ever had.

## Names across recordings

`enroll_check` is the only harness that spans two recordings, which is the only
setting where enrollment means anything: within one recording a voice has to be
told apart from the others present and is compared against itself through the
same microphone, and across recordings it has to be recognised through a
different one.

```bash
cargo run --release -p scribe-pipeline --example enroll_check -- \
    models A.wav A-truth.json B.wav B-truth.json Karen -Samantha
```

Voiceprints are taken from the diarized voices of A, which is what enrolling
from a recording does. A bare name is withheld from enrollment — that person is
in both meetings and must come back unrecognised, which is the false-positive
test. A name prefixed with `-` is present in B's audio but hidden from the
matcher, so anyone who resembles them competes for their identity unopposed.

Two meetings of the same four people, saying nothing in common:

| B's condition | recognised | misidentified | false positives |
|---|---|---|---|
| same room as A | 3 of 3 | 0 | 0 |
| different room, noise, one speaker quiet | 3 of 3 | 0 | 0 |
| Samantha enrolled but absent from B | 2 of 2 | 0 | 0 |

The similarities behind that are the interesting part:

| | own voiceprint | nearest other |
|---|---|---|
| same room | 0.987 – 0.990 | 0.599 |
| different room | 0.561 – 0.689 | 0.303 |

A voice recognised through the same microphone sits near 0.99. The same voice
through a different room sits near 0.56. And a *different person* in a good room
scored **0.599** against somebody else's voiceprint — a higher number than the
true match in the bad room. That is the measurement that decides the design: no
cutoff on similarity separates those two cases, and neither does requiring the
match to stand clear of the rest of the enrolled library, because both pass
both.

What separates them is the rest of the recording. Recognition is not one
question asked repeatedly; it is one microphone in one room. Against two voices
recognised at 0.99, a third at 0.599 is not the same kind of event. Against two
at 0.59 and 0.69, a third at 0.561 plainly is. So a match must also be within
three quarters of the median match this recording has already produced. With
nothing accepted yet there is no standard to hold it to, and the floor and the
separation are all there is.

Before that rule, case three above returned Karen named as Samantha.

## Why diarization costs what it does

Diarization runs at roughly a third the speed of transcription, and inside it
segmentation is 91% of the cost against embedding's 8%. Two obvious ways to
reclaim that have been measured and neither works.

**Skipping sherpa's internal clustering.** `OfflineSpeakerDiarization` runs
pyannote segmentation, extracts a speaker embedding for every segment, and
clusters them. This code uses only the segment boundaries: it discards the
speaker labels, splits the segments at their own pauses, and embeds and clusters
the pieces itself. So the internal embedding and clustering are pure waste — but
sherpa-onnx exposes diarization only as one object, with no way to run the
segmentation model alone. That would need an upstream API that does not exist.

**Replacing pyannote with a voice activity detector.** pyannote answers two
questions — where speech is, and where the speaker changes within it — and this
code only uses the first. It is also unreliable at the second: two similar
voices either side of a short pause come back as one turn, which is why turns
are split at pauses at all. Silero VAD answers the question that gets used, from
a model a tenth the size. Measured on the same audio:

| fixture | pyannote | Silero VAD |
|---|---|---|
| 4 voices, clean | 14.3 s, 99.9% | 1.5 s, 98.7% |
| 4 voices, reverb only | —, 99.6% | —, 99.0% |
| 4 voices, 14 dB SNR | —, 99.6% | —, **59.9%** |
| 4 voices, reverb + noise + one quiet | —, 99.6% | —, **30.3%** |

Eight to ten times faster and correct on clean audio, and it falls apart the
moment there is noise. The turn counts say why: on the last row the VAD emitted
6 segments where the conversation has 30 turns. With a noise floor above its
speech threshold it stops *closing* segments, so each one runs across several
speakers, embeds to a blend of them, and clustering collapses. A trained
segmentation model does not have that failure because it is not deciding on
energy.

**Quantizing the segmentation model.** The obvious version of "cheaper
segmentation": pyannote is published int8 as well, at a quarter of the size, and
the loader already accepts it.

| fixture | fp32 | int8 |
|---|---|---|
| clean | 14.3 s, 99.9% | 14.7 s, 99.5% |
| reverb only | 15.0 s, 99.6% | 9.9 s, **62.1%** |
| 14 dB SNR | 14.8 s, 99.6% | 15.1 s, 99.2% |
| reverb + noise + one quiet | 18.4 s, 99.6% | 10.5 s, **63.6%** (5 speakers, not 4) |

Not faster where it is accurate, and where it is faster it is faster because it
has stopped finding the segments. Both failed experiments on this page break in
the same place — reverberation — which is worth knowing on its own: a room is
harder for segmentation than noise is, and any cheaper substitute should be
tested against one before anything else.

fp32 is preferred at load, and an install carrying only the int8 model gets a
warning saying what it costs.

**Running windows in parallel.** A recording past ten minutes is diarized one
window at a time, strictly in sequence, and ONNX Runtime stops scaling well
before this machine's core count — 4 threads to 8 buys 9%. Two windows at once
should therefore be nearly free.

Measured the cheap way, by running two diarizations of the same fixture as
separate processes rather than writing the concurrency first:

| | wall clock |
|---|---|
| one process, 8 threads | 14.1 s |
| two processes in sequence | 28.7 s |
| two processes at once, 8 threads each | 22.9 s |
| two processes at once, 4 threads each | 23.4 s |

1.25x, and that flatters it — the two processes also load their models
concurrently, which in-process windows would not. The thread pool is already
using most of the machine, so a second window mostly contends with the first.
Against that: a model set per worker is about 110 MB of resident memory, and
concurrency inside a stage that must not crash is not free to maintain. Not
worth building, so it was not built.

The same 1.25x is available today without any code, for anyone with a *backlog*
rather than one long recording: `[worker].concurrency = 2` runs two recordings
at once. It does nothing for a single recording.

So the cost is real work, and a speed win would have to come from a
*genuinely* better segmentation model rather than a smaller copy of this one,
a cheaper substitute for it, or more of the same machine.

## Whisper's word timings

sherpa returns no token timestamps for its Whisper models. The transcriber
notices and spreads each decode window's words evenly across it, because the
alternative — leaving every word at `[0, 0]` — would give the merge stage no
overlap to work with and collapse the recording into one unseekable block.

`SCRIBE_ASR_MODEL` selects the checkpoint, so both can be run over the same
audio. `SCRIBE_ASR_TIMING_PATH=1` prints which timing path each decode took.

| fixture | model | words timed into silence | right speaker |
|---|---|---|---|
| 4 voices, regular turns | Parakeet | 0.4% | 100.0% |
| | Whisper | 7.0% | 100.0% |
| 4 voices, 11 min | Parakeet | 1.4% | 100.0% |
| | Whisper | 7.0% | 100.0% |
| 4 voices, turns 0.4 s to 12 s | Parakeet | 3.4% | 96.4% |
| | Whisper | 8.3% | 96.3% |

"Timed into silence" is the share of words whose midpoint lands where nobody
was speaking — a direct check on whether the timings are real rather than
plausible.

**Speaker labelling does not care.** A speaker turn is seconds long and the
timing error is a fraction of one, so a misplaced word still overlaps the right
turn. The two models score the same on attribution on every fixture, including
one built with turns ranging from 0.4 s to 12 s specifically because regular
turns flatter a model that is guessing at timing.

**Anything word-level does care.** Roughly one word in fourteen is placed where
no one is talking, so the playback highlighter lights the wrong word and tapping
a word seeks to the wrong moment. `deploy/server.toml` ships Whisper and now
says this next to the setting.

## The one who only says "yes"

`make-diar-fixture.py` builds turns of roughly equal length in rotation, which
is not how anybody talks. A fixture with turns from 0.4 s to 12 s — one-word
answers next to monologues — is the only one here that scores below 100%, and
what it fails at is worth knowing.

One participant's entire contribution was "Yes.", "That tracks." and "I can take
that.": 2.5 seconds across three turns. He does not appear in the transcript at
all. The recording comes back with three speakers instead of four, and his three
turns are credited to whoever was nearest.

A cluster has to hold `MIN_SPEAKER_SPEECH_MS` of speech to stand as a
participant rather than be folded into the voice it most resembles, and 2.5 s is
under it. Two ways out were tried:

**Lowering the floor.** Swept from 1 s to 5 s across seven fixtures. Every value
from 1.5 s upward gives an identical answer on all of them; 1 s produces phantom
participants on four of the seven and drops attribution several points. The
floor is a plateau, not a tuned edge — and it does not recover him anyway,
because his three turns never cluster into one 2.5 s cluster. They arrive as
separate slivers of about a second each, and no floor above 1 s keeps those.

**Letting a cluster stand if it resembles nobody**, on the reasoning that a
fragment of somebody present looks like them and a quiet stranger does not. This
is wrong for a reason worth writing down: a one-second embedding is *unreliable*,
not *distinctive*. "Resembles nobody" and "too brief to measure" are the same
reading, so every sliver stands. The same recording came back with twelve
speakers and attribution fell from 95.5% to 92.4%.

So this is a limit of the embedding model on sub-second speech rather than a
threshold that wants adjusting. Somebody who only ever interjects will be folded
into a neighbour, and the transcript will under-count the room. Naming them by
hand still works, and a name given that way is not affected by any of this.

## Execution provider

`SCRIBE_ASR_DEVICE=coreml` is slower than the CPU provider on Apple Silicon —
23.0 s against 15.2 s on the 2.5-minute fixture, with identical accuracy. These
models do not map onto the neural engine well enough to pay for the conversion.
`cpu` is the right default here; the setting is there for CUDA.

## Threads

`SCRIBE_ASR_THREADS` overrides the ONNX thread count, which is how the default
was chosen. On a 15-core machine, the 2.5-minute fixture:

| threads | wall clock |
|---|---|
| 1 | 56.6 s |
| 2 | 36.1 s |
| 4 | 21.7 s |
| 8 | 19.8 s |
| 12 | 18.1 s |

Accuracy is identical at every setting. Returns fall off sharply after 4 and
have nearly stopped by 8, which is why `[asr].num_threads` auto-detects but caps
there — past it the scheduling costs more than the parallelism buys, and the
worker has a database and an HTTP server to run as well. The old hardcoded 2
against the current default of 8 is the 36.1 s row against the 19.8 s one.

## A warning about fixtures

The generator never speaks the same line twice, and that is not cosmetic. An
earlier version cycled a short list of sentences, so one voice said identical
words repeatedly; identical text through one voice synthesises to identical
audio, embeds to a cosine of 1.0, and drags the whole merge sequence upward. It
scored 91.2% and reported seven speakers where there were four. The same code on
a fixture with no repeats scores 99.7% and finds four. The bug was in the
measurement, and it looked exactly like a bug in the diarizer.

Again: synthesised voices are cleaner and more separable than a real room. Use
these numbers to compare changes, not to predict field accuracy.
