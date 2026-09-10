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

## The whole pipeline, on your own audio

The harnesses above each measure one model against known answers. `e2e-check.sh`
runs what a user actually gets — transcode, diarize, transcribe, merge, embed,
summarize, through the CLI against a real Postgres — and prints the transcript
that came out with the speaker each line was given.

```bash
cargo build --release -p scribe-cli
SCRIBE_E2E_DB=postgres://scribe:scribe@127.0.0.1:5434/scribe_e2e \
  ./scripts/e2e-check.sh recording.wav
```

Several files are ingested into the same database in order, so a voice enrolled
before them carries across:

```bash
scribe --config <cfg> enroll --name Alice --audio alice-sample.wav
./scripts/e2e-check.sh monday.wav tuesday.wav
```

It creates its own database and blob directory and points the LLM at an
unreachable address on purpose, so the summarize and transcript-correction
stages are exercised in their degraded form rather than skipped.

`scribe transcript [id]` prints any recording's transcript with speaker names on
its own, which is the answer to "did this work?" on a server with no database
client and no phone.

**Run this on a recording of real people.** Everything else on this page is
synthesised voices, which are cleaner and more separable than a room full of
humans, and several conclusions here have already had to be revised once a
fixture stopped flattering the code.

### What it confirms about the harnesses

Run over the fixtures, the real pipeline reproduces the component harnesses
exactly — the same speaker count, the same utterance boundaries, the same
attribution — and enrollment recognises the same voices across a different room
at the same similarities (0.58, 0.60, 0.64 through the database against 0.56 to
0.69 in-process), so the pgvector round-trip is faithful. The numbers on this
page are about the shipped pipeline, not only about the models.

## The constants, and which of them were guessed

Most of the numbers in this code were chosen by argument and then left. Going
back over them:

| | swept | outcome |
|---|---|---|
| cut threshold (0.8) | yes, twice | plateau 0.70–0.85; 0.90 breaks two fixtures |
| participant floor (3 s + 1%) | yes, twice | a duration alone is length-blind; see below |
| enrolment floor (0.5) | yes | 0.5–0.6 identical; 0.65 loses a bad room |
| match consistency (0.75) | yes | cannot be loosened — the numbers collide |
| silence for a split (250 ms) | yes, twice | "180–250 identical" was a clean-audio result; see below |
| silence ratio (0.15) | yes, and the sweep lied | see "the sweep that found nothing" |
| **per-window clustering (0.8)** | **yes** | **was 0.5, and 0.5 was worse** |

The last one is worth its own note. It sets how readily the segmentation model's
own clustering joins two stretches, and it looks inert, because those speaker
labels are thrown away — every piece is renumbered and clustered again across
the whole recording. It is not inert: how sherpa clusters changes the *segments*
it emits, and those are kept.

Moving it from 0.5 to 0.8 takes a six-voice degraded recording from 94.4% to
95.7% and one with somebody moving about from 99.4% to 99.7%, with nothing worse
anywhere. Higher means it merges less, which is what this code wants for the
same reason it discovers each window's voices without a target count: finer
segments are repairable downstream and merged ones are not. 0.9 is a shade
better again but costs a recording with a television playing in it, and the
response is not smooth — 0.6 returns eight speakers where 0.5 and 0.7 both
return six — so 0.8 sits in the middle of the stable region rather than on an
edge.

## The sweep that found nothing

The table above used to say the silence ratio was inert: 0.05 to 0.25 made no
difference to any fixture, so the constant was left alone. That was true, and
the conclusion drawn from it was wrong.

It was inert because every fixture it was swept on had near-silence between the
turns. The threshold is a fraction of the turn's *mean* energy, which quietly
assumes the gaps are near-silent — and when they are, any fraction in that range
sits far above the floor and far below the speech, so nothing moves.

In a real room the assumption fails outright. Pink noise at 15 dB SNR puts the
floor at 0.178 of the speech level. The threshold is 0.15. No frame is ever
below it, so no turn is ever split, so two people who spoke one after the other
are embedded as a single stretch and cluster as a single person.

That is what a six-voice reverberant fixture had been doing all along: Karen and
Moira, adjacent in the rotation, merged on every one of the four occasions they
spoke in sequence. Five speakers returned instead of six, 83.2% of words to the
right person. The clean recording of the same conversation split them correctly
every time, which is why nothing upstream looked broken.

The fix is to stop assuming the floor and measure it — the quietest frame in the
turn — then put the threshold a fixed fraction of the way from there to the
speech, in dB. Clean audio is untouched by construction: its quietest frame is
near zero, so the adaptive threshold lands below the mean-relative one and the
old behaviour wins.

| snr / reverb | measured floor | assumed floor |
|---|---|---|
| 25 dB / 0.2 | **6 spk, 99.8%** | 6 spk, 98.1% |
| 20 dB / 0.3 | **6 spk, 99.6%** | 6 spk, 91.7% |
| 15 dB / 0.4 | **6 spk, 97.4%** | 5 spk, 83.2% |
| 10 dB / 0.5 | **6 spk, 87.2%** | 6 spk, 81.2% |
| 5 dB / 0.6 | **6 spk, 82.2%** | 6 spk, 73.9% |

Better at every noise level, by 1.7 to 14.2 points, and across every other
fixture in the suite nothing moved by more than 0.1 of a point.

The lesson is about the sweep, not the constant. A parameter that governs how
the code copes with noise cannot be sized on recordings that have none; the
sweep will report it inert and it will be inert, on that material. Two of the
rows in the table above were swept the same way and deserve the same suspicion.

### Re-sweeping the other one, and why it could not move

The same table said the minimum silence for a split was flat from 180 to 250 ms.
Swept again on noisy material it is not flat anywhere, and the value is
quantized: `min_run` counts 20 ms frames, so 175 and 190 are 8 and 9 frames and
behave nothing alike.

Shortening it helps for the same reason the noise-floor fix helps. A handover
has a pause in it, but in a reverberant room the tail of the outgoing speaker
eats the front of that pause, so the *quiet* part is shorter than the pause
actually was. Requiring 250 ms of quiet misses the handover; requiring 160 finds
it.

It could not be shortened, because a shorter split also cuts turns in places
that leave slivers behind, and a sliver is worse than no cut at all: too short to
embed well, so the embedding lands it on whoever it happens to resemble, and
enough of them drag real speakers together. At 160 ms an eleven-minute degraded
recording came back as **one speaker at 18.1%**.

So the two changes only work together — refuse any cut that would leave a piece
under 400 ms, and then the split can go looking for shorter pauses:

| | 250 ms, slivers allowed | 160 ms, slivers refused |
|---|---|---|
| 4 voices, clean | 99.8% | **99.9%** |
| 4 voices, reverb+noise | 99.4% | **99.5%** |
| 4 voices, one moving | 99.7% | **99.8%** |
| 6 voices, clean | **99.9%** | 99.8% |
| 6 voices, 15 dB | 97.4% | 97.4% |
| 6 voices, 10 dB | 6 spk, 87.2% | **6 spk, 95.7%** |
| 6 voices, 5 dB | 6 spk, 82.2% | **6 spk, 86.8%** |
| 11 min, clean | 99.6% | **99.7%** |
| 11 min, degraded | 99.5% | 99.5% |

On its own, at the old 250 ms split, the minimum piece length changes nothing on
any fixture. It is worth having only because of what it unlocks.

The working band is now 140–170 ms: at 180 a noisy recording starts inventing a
seventh speaker, and at 120 the long one loses a speaker again. Before the
sliver rule the band did not exist — everything below 180 collapsed.

Twice now the same shape: a constant swept on clean audio, reported inert, and
holding a real loss on anything noisier. The remaining rows in that table were
swept the same way.

## The mover who becomes three people

A six-voice recording where one person walks about a reverberant room comes back
with **eight speakers at 91.3%**. Moira is split three ways — 14.5 s, 9.2 s and
5.2 s — and both extra pieces clear the participant floor comfortably, so
nothing folds them.

Told there are six, the same recording scores 99.6%. The embeddings separate
everyone perfectly; her own variation across the recording is simply wider than
the gap between her and the others, and no cut of the merge sequence puts those
three pieces together without joining somebody else too.

**What would fix it is a merge, and there is no merge rule here.** The cohesion
step-back added earlier only ever *increases* the speaker count — it exists for
two people sharing a cluster. This is the mirror image, and the signal for it is
too weak to act on: in this recording the two over-split clusters sit at 0.5966
against their own internal spreads of 0.5911 and 0.6107, and in a
correctly-counted six-voice recording the closest pair of substantial clusters
sits at 0.5068. One case against one case, a margin of nine hundredths, and the
failure mode of getting it wrong is two real people merged into one — which is
worse than an extra row in the speaker list. Not built.

Measured on every run without gating:

```
  note  6 voices, one moving, reverb    8 spk, 91.3%   (wants 6; was 8 spk, 91.3%)
```

## A different segmentation model is better at some of this

The segmentation model had never been swapped, only the embedding — and it is
92% of diarization time and the thing that decides where turns begin. Rev.ai's
reverb-diarization models are drop-in replacements for pyannote 3.0.

**v2 is not worth considering**: 374 MB against 6 MB, 3.7x slower, and worse
(95.2% where pyannote gets 97.4%).

**v1 is 9 MB, exactly as fast, and better at most of what is still wrong here**
— with its own per-window clustering re-tuned to 0.6, since that constant is
calibrated to a model's segment characteristics:

| fixture | pyannote 3.0 | reverb v1 |
|---|---|---|
| 6 voices, fast conversation | 97.0% | **99.7%** |
| 6 voices, fast + reverb | 6 spk, 92.8% | **6 spk, 96.6%** |
| 6 voices, reverb+noise | 97.4% | **99.1%** |
| 8 voices, reverb+noise | 97.7% | **99.3%** |
| 6 voices, moving + reverb | 8 spk, 91.3% | **7 spk, 94.1%** |
| 4 voices, one moving | **99.8%** | 95.6% |
| everything else | — | within 0.5 either way |

Five conditions better, one worse, and the one that is worse is a single fixture
rather than a class: three other moving-speaker recordings — six voices with a
different mover, the same with reverb, and an eleven-minute one — are equal or
better under v1.

**The default is unchanged all the same.** Switching costs every installation a
new model download, it fails a committed check (`4 voices, one moving` has a
99.0 floor and v1 scores 95.6), and the gains are on conditions this page
already handles at 92–98% rather than on anything broken. Swapping is a
one-file change for anyone whose rooms are reverberant or whose meetings are
fast:

```bash
curl -L -o models/diarization/segmentation.onnx \
  https://huggingface.co/csukuangfj/sherpa-onnx-reverb-diarization-v1/resolve/main/model.onnx
```

with `[asr]`-side clustering set to 0.6 (`SCRIBE_CLUSTER_THRESHOLD`) to get the
numbers above.

## Four other embedding models, none of them better

Most of what is left on this page comes back to the embedding: the eight-voice
count, the fast-conversation floor, the impostor who is named in a bad room.
A better embedding would move all three at once, so it is worth knowing whether
one is available.

Four candidates from the sherpa model zoo, dropped in against the current
ERes2Net with nothing else changed:

| fixture | ERes2Net (current) | TitaNet-small | ERes2NetV2 |
|---|---|---|---|
| 4 voices, one moving | **4 spk, 99.8%** | 5 spk, 92.6% | 4 spk, 99.8% |
| 6 voices, reverb+noise | **6 spk, 97.4%** | 6 spk, 97.4% | 5 spk, 81.0% |
| 8 voices, reverb+noise | **8 spk, 97.7%** | 9 spk, 90.6% | 7 spk, 85.1% |
| 6 voices, fast conversation | 6 spk, 92.8% | **6 spk, 93.3%** | 5 spk, 77.9% |
| 4 voices, 11 min, degraded | **4 spk, 99.5%** | 5 spk, 97.8% | 4 spk, 99.2% |
| brief 5th speaker | 5 spk, 99.6% | 5 spk, 99.6% | 5 spk, 99.6% |

The current model wins or ties everywhere except fast conversation, where
TitaNet-small is 0.5 of a point ahead.

**Both alternatives were given a re-tune before being dismissed**, since the cut
threshold is calibrated to a model's similarity scale and a drop-in comparison
is not a fair one. It does not rescue either. ERes2NetV2 at a cut of 0.85 reaches
97.6% on six voices — a shade better than the current model — but eight voices
stays at 7 speakers and 85.1% at *every* value swept. TitaNet-small returns five
speakers for four on the moving-speaker fixture at every value, which is the same
failure this page already records for TitaNet-large: it is a property of that
family, not of the tuning.

**The two English-trained models are not usable.** `wespeaker_en_voxceleb_CAM++`
and `3dspeaker_campplus_sv_en_voxceleb` are both 512-dimensional, and
`speakers.embedding` is `vector(192)` — changing it means a migration and
re-enrolling every person on file, because a voiceprint cannot be converted from
one model's space to another. They also scored badly as drop-ins (2 speakers at
42.7%, 8 at 48.2%), but that is not evidence about the models: their cosine
distributions are different and every constant here is calibrated to the current
one. They were not re-tuned, because the schema rules them out regardless.

Worth noting what this kills as an idea: the current model is trained on Mandarin
(`zh-cn`) and every voice in every fixture is English, which looks like an
obvious mismatch to fix. It is not one. The English-trained models are worse or
unusable and the newer version of the *same* model is worse.

Reproduce by pointing `diarize_check` at a directory holding a different
`diarization/embedding.onnx` — the model is a file, not a code path.

## Naming people when the database is not small

Every enrolment number on this page was measured against three or four enrolled
people. What a real installation accumulates is a year of meetings, and the risk
enrolment carries — naming somebody after a stranger who happens to sound like
them — is a function of how many strangers are on file.

`SCRIBE_ENROLL_EXTRA` enrols extra recordings whose speakers are not in the room,
which is how a roster is built without pretending they attended:

```bash
SCRIBE_ENROLL_EXTRA="r1.wav:r1.json,r2.wav:r2.json" enroll_check models A.wav A.json B.wav B.json
```

**In an ordinary room the roster does not matter.** Four people recognised from a
database of 4, 8, 12, 16, 20 and 24, every time, with nothing misidentified and
no false positive — and the same when any one of the four is withheld from
enrolment so they are present but unknown. The margin is wide: true matches sit
at 0.744–0.870 and the nearest impostor across all 24 is 0.624, which is Rishi
against Aman, two Indian-English male voices.

**In a bad room it does.** At reverb 0.6 and 0.8, with Rishi present but never
enrolled, a roster of 4 names nobody and a roster of 24 names him **Aman** at
0.610. The mechanism is exact: the floor (0.5) admits it, `SEPARATION` compares
against the *median* of the rest of the library and a large roster of
dissimilar people pushes that median down, so separation gets easier as the
roster grows. Only `MATCH_CONSISTENCY` refuses it, and at 0.75 × 0.796 = 0.597
against a similarity of 0.610 it does not.

**Tightening it is measured and not taken.** Raising `MATCH_CONSISTENCY` to 0.80
removes that false name. Across eight configurations — two bad rooms, each of
the four participants withheld in turn, roster of 24:

| room / withheld | 0.75 | 0.80 |
|---|---|---|
| 0.6 / Samantha | 3 recognised, 0 wrong | **2 recognised**, 0 wrong |
| 0.6 / Rishi | 3 recognised, **1 wrong** | 3 recognised, 0 wrong |
| 0.8 / Rishi | 3 recognised, **1 wrong** | **2 recognised**, 0 wrong |
| the other five | identical | identical |

Two wrong names removed and two right ones lost. Both wrong names are the same
pair — Rishi mistaken for Aman — while the losses fall on two different people.
That is tightening a general rule to fix one pair of similar voices, at the cost
of two others, and the sweep does not support it. 0.75 stays.

What is worth keeping is the shape of the risk: **the guard against a wrong name
weakens as the roster grows, and the floor is not what is holding it.** The
regression suite now enrols twenty-four people against a four-person meeting and
requires all four names and no false ones.

Both constants are sweepable now — `SCRIBE_MATCH_CONSISTENCY` and
`SCRIBE_ENROLL_SEPARATION`.

## Skipping the silence is not worth it

Segmentation is 92% of diarization time and it runs over the whole recording,
silence included. A meeting with real dead air in it should therefore be most of
a free win: find the speech, diarize only that.

Measured against the best case a perfect detector could give — a fixture that is
half silence, with the silence cut out using the ground truth, so no detector
error is included at all:

| | duration | segmentation |
|---|---|---|
| with 50% silence | 373 s | 17.8 s |
| silence removed | 200 s | 14.9 s |

Removing 46% of the audio buys 16% of the time. Segmentation over silence is far
cheaper than over speech — the model runs but produces almost nothing for the
rest of the pipeline to do — so the saving is nothing like proportional. A real
detector would be imperfect and cost something itself, and would risk clipping
the starts of turns, which is where handovers live. Rejected.

## The phantom that is not worth the cure

A four-voice meeting with realistic dead air — gaps from 0.3 s to 9 s — comes
back with five speakers when it is degraded. The fifth is 3.7 s in total, in two
scraps minutes apart, and both scraps lie inside Karen's turns: 98% and 96% of
them are her. It is not a person, it is two pieces of one.

The same thing happens if thirty seconds of room tone is appended to the
six-voice fixture, where the phantom is two scraps of Moira.

**It looks easy to fix, and the shape is quite distinct.** A real participant who
speaks briefly speaks *once, continuously*:

| | fragments | span | speech ÷ span |
|---|---|---|---|
| phantom (dead air) | 2.1 s at 90 s, 1.6 s at 362 s | 274 s | 0.013 |
| phantom (room tone) | 2.6 s at 83 s, 1.6 s at 171 s | 89 s | 0.047 |
| real brief speaker | 1.4 s + 3.5 s, abutting | 4.9 s | **1.0** |

And the phantoms sit right on top of their host: the dead-air one matches Karen
at 0.6568 where Karen's own worst internal pair is 0.6794, and the room-tone one
matches Moira at 0.6311 where Moira's own worst pair is 0.6123 — closer to her
than she is to herself.

**It is still not being fixed, and the reason is arithmetic.** Across sixteen
fixtures — four to eight voices, clean, reverberant, noisy, moving, phone, fast,
eleven-minute, half-silent, and one with a brief real participant — exactly one
returns a speaker too many. Against that, every rule above would fold a real
person who interjects twice rather than once, and there is no fixture on this
page that could tell me how often that happens. A phantom cluster is cosmetic;
a person missing from a transcript is not, and that asymmetry decided the
participant floor two sections ago for the same reason.

Building a discriminator on one positive example is what the cohesion threshold
had twelve measurements to avoid. So the pattern is recorded here, the
diagnostic that measures it is kept (`SCRIBE_DIARIZE_COHESION=1` now also prints
each cluster's nearest neighbour and that neighbour's own spread), and the
condition is measured on every run without gating on it:

```
  note  the same, reverb+noise    5 spk, 97.8%   (wants 4; was 5 spk, 97.8%)
```

A `note` rather than a `check`, because a permanently red check is useless as a
gate and leaving the condition out altogether is how a number goes unwatched.
What it did last time is written beside it, so a change shows up.

## What the participant floor is actually protecting

Appending thirty seconds of room tone to the end of a recording — the sound of
a meeting that has finished but is still recording, which is most of them —
turns a six-voice fixture into a seven-voice one.

The room tone is not the speaker. It produces no segments at all: pyannote
ignores it correctly. What happens is that the longer file shifts the
segmentation windows slightly, the segment boundaries move a little, and a
1.6-second sliver at a Karen-to-Moira handover that used to fold into a real
speaker instead survives as a cluster of its own.

So this is not a silence bug, and it is worth being precise about that. Four
other fixtures — four voices, six clean, fast conversation, eight voices — are
unaffected by the same thirty seconds, two of them improving slightly. It is one
recording sitting near a fold-or-not boundary, tipped by a trivial change to its
input. The useful conclusion is about how much weight these numbers carry: on a
recording near that boundary the speaker *count* can flip on a change that has
nothing to do with the speech.

**The fix that works, and why it is not taken.** The sliver holds 4.2 s against a
3 s participant floor. Raising the floor to 5 s folds it away and the recording
returns to six speakers at 99.2%, better than the 97.4% it started at, with the
six-voice, fast-conversation, eight-voice and single-voice fixtures all
unchanged. It looks free.

It is not free, and nothing on this page could see the cost, because every
speaker in every fixture talks for a fifth of the recording. Spliced a fifth
person into a four-voice meeting who says one sentence and nothing else — 4.7
seconds out of 153:

| | floor 3 s | floor 5 s |
|---|---|---|
| clean | **5 spk, 99.9%** | 4 spk, 96.6% |
| reverb+noise | **5 spk, 99.6%** | 4 spk, 96.2% |

At 5 s that person does not exist. Their sentence is handed to whoever spoke
next, and nothing in the output suggests anyone is missing. A phantom cluster is
cosmetic and a person missing from a transcript is not, so the floor stays at
3 s and the phantom stays with it.

`scripts/add-brief-speaker.py` builds that fixture and two checks in the
regression suite hold the floor down. Raising it to 5 s fails both.

## Two ways to reach a handover shorter than the split threshold, both dead

Fast conversation scores 92.8% because a gap under 160 ms produces no cut, so
both speakers land in one fragment and half a turn gets the wrong name. Two
things looked like they should fix it.

**sherpa's own minimum silence.** `min_duration_off` defaults to 0.5 s and this
code had never set it — a 500 ms floor on splitting a segment, against gaps of
80–250 ms. It looked decisive. Swept from 0.05 to 0.5 it changes nothing at all:
the fast-conversation, six-voice and eleven-minute fixtures come back identical
to the decimal at every value.

The reason is that these segments are re-split at silence afterwards and
sherpa's own boundaries within a turn are thrown away, so its post-processing
has nothing left to decide. The values are live — `min_duration_on` at 5 s
collapses a six-voice recording to four speakers at 53.3% — so this is a result
and not another disconnected knob. Both are now set explicitly rather than
inherited.

**Splitting a fragment and comparing its halves.** If a fragment secretly holds
two people, its two halves should not match, and a second pass could find the
handovers the silence split cannot reach. Measured by cutting each fragment at
its quietest interior point:

| | n | min | median | max |
|---|---|---|---|---|
| fragment = one speaker | 38 | 0.108 | 0.495 | 0.787 |
| fragment spans a handover | 6 | 0.360 | 0.500 | 0.684 |

The medians are the same, and the single-speaker range is the wider of the two —
one person's halves differ *more* than the worst handover does. Half of a short
fragment is about a second of audio and a one-second embedding is too noisy to
say who is speaking.

This is the same shape as the merge-sequence result: the information is not
present at the granularity being asked. `SCRIBE_DIARIZE_HALVES=1` prints the
table, so the question can be put again to a better embedding model, which is
the thing that would have to change.

So 92.8% stands as the floor for fast conversation, and the three levers that
could move it — a shorter split threshold, sherpa's minimum silence, a
second-pass split — are all measured and all rejected.

## How long the pauses are, which nobody had varied

Every fixture on this page put exactly 350 ms between one turn and the next.
That is an assumption about conversation, it was never varied, and everything in
"look for shorter pauses" was tuned against it — a split threshold measured on
one gap length and nothing else.

`SCRIBE_FIXTURE_GAP=150-1400` draws each gap from a range instead (seeded, so a
fixture built twice is the same fixture). The result is reassuring in the
direction that matters and useful in the other:

| | uniform 350 ms | gaps 150–1400 ms | gaps 80–250 ms |
|---|---|---|---|
| 6 voices, clean | 99.8% | 99.9% | 97.0% |
| 6 voices, reverb+noise | 97.4% | **99.2%** | 92.8% |
| 4 voices, clean | 99.9% | 99.9% | — |
| 4 voices, reverb+noise | 99.5% | **99.8%** | — |

Realistic varied pauses are *easier* than the fixture, not harder — longer gaps
make a handover simple to find, and 350 ms sits near the difficult end of the
range. So the existing numbers were not flattered by this, which is the thing
worth knowing.

Fast conversation is where it gets harder. With gaps of 80–250 ms most handovers
fall below the 160 ms split threshold and have to be caught by the segmentation
model or not at all: 92.8% in a reverberant room, the lowest score on this page
that still returns the right number of speakers.

**Chasing it does not pay.** Lowering the split threshold to 120 ms buys 1.3
points there and 2.9 at 5 dB SNR, and costs a whole speaker on eight voices at
12 dB (94.4% to 82.0%). 160 stays.

Worth noting for its own sake: 120 ms is *usable* now, where earlier in this
work it returned 3 speakers instead of 4 on the eleven-minute recording. The
cohesion step-back is what changed — it undoes the over-merging that
over-splitting causes, so the two compose. The band the split threshold can
safely sit in is wider than it was; 160 is still the best point in it.

## The thread cap that was never measured

Both speech models got at most 8 ONNX threads, however large the machine, on the
reasoning that these graphs are not wide enough to keep more busy. That was
never measured. Measured, it is wrong:

| threads | 8 | 10 | 11 | 12 | 13 | 14 | 15 |
|---|---|---|---|---|---|---|---|
| transcribe | 26.3x | 28.8x | 30.5x | 31.2x | **33.6x** | 32.5x | 25.5x |
| diarize | 10.4x | 11.7x | 12.0x | 12.4x | **12.6x** | 11.7x | 9.7x |
| total | 7.4x | 8.3x | 8.6x | 8.9x | **9.2x** | 8.6x | 7.0x |

The whole pipeline is 1.24x faster with 13 threads than with 8, on a 15-core
machine, at identical accuracy — all fifteen regression checks return the same
numbers to the decimal.

What is true is the collapse at the top. At the core count and past it the
threads contend: 15 threads is slower than 8, and 20 runs segmentation at 0.42x
of its 8-thread speed. So the default takes the machine's cores less an eighth,
capped at 16 — 13 here — rather than all of them.

Where diarization time actually goes, on an eleven-minute recording:

| | time | share |
|---|---|---|
| segmentation | 60.8 s | 92% |
| embedding | 6.0 s | 9% |
| clustering | negligible | — |

Which is why the thread count is the whole story for speed, and why the levers
tried against embedding and clustering never moved anything.

**How the wrong belief survived.** `transcript_check` read no thread override —
it printed `threads 8` whatever it was given, so every row of a thread sweep was
the same run at the default. The sweep looked flat because it was one
measurement repeated. `diarize_check` did honour the override, which is why the
two harnesses disagreed and the disagreement is what exposed it.

That is the third time on this page a measurement has been the thing at fault
rather than the code: a sweep on material that could not show the effect, a
check that reported ok for a run that never happened, and now a knob that was
not connected.

## The cluster that is too loose

Eight voices is where counting fails first. Two of them join at a similarity
that looks exactly like one person's own spread, so the merge sequence has no
step where the mistake happens and `choose_cut` sails past it. Merging down
through 9 → 8 → 7 → 6 clusters costs 0.4609, 0.4494, 0.4334, 0.4259 — four
merges within 0.035 of each other, one of which put two different people
together and three of which rejoined one person's own pieces.

The cluster gives itself away afterwards. Every other cluster holds one voice
and keeps its fragments close together; this one holds two, and has a pair that
is not close at all:

| | speech | fragments | mean similarity | least similar pair |
|---|---|---|---|---|
| the joined cluster | 58.2 s | 15 | 0.6162 | **0.3295** |
| every other cluster | 26–32 s | 6–10 | 0.66–0.78 | 0.4956–0.6775 |

So: after cutting, if the loosest cluster is far looser than the median cluster,
the cut stopped a merge too late — step back one and look again, up to four
times. A stated count is left alone, being better evidence than this.

Measured against the median rather than an absolute level, because the absolute
level says nothing. The failing recording's loosest pair was 0.3295 and a
correctly-counted eleven-minute recording's was 0.3481 — indistinguishable. As a
share of the median cluster they are 0.52 and 0.82.

| | discovered before | discovered now |
|---|---|---|
| 8 voices, clean | 8 spk, 99.8% | 8 spk, 99.8% |
| 8 voices, 20 dB | 8 spk, 99.8% | 8 spk, 99.8% |
| 8 voices, 18 dB | 7 spk, 85.2% | **8 spk, 97.7%** |
| 8 voices, 15 dB | 7 spk, 86.3% | **8 spk, 98.8%** |
| 8 voices, 12 dB | 7 spk, 82.0% | **8 spk, 94.4%** |
| 8 voices, 10 dB | 6 spk, 74.7% | **7 spk, 87.1%** |
| 8 voices, 8 dB | 6 spk, 74.8% | **7 spk, 78.6%** |

Every fixture that was already counted correctly is byte-identical — one to
eight voices, clean, reverberant, noisy, moving, phone, and the eleven-minute
windowed pair.

**The threshold is on a narrow band, and that is worth saying plainly.** Twelve
measured recordings separate cleanly — the five miscounted ones sit at 0.447 to
0.715 of their median, the seven correct ones at 0.752 to 0.954 — so the
threshold has to fall between 0.715 and 0.752, and 0.73 is what it is. Sweeping
confirms the shape: 0.70 stops fixing the 12 dB case, 0.78 starts splitting a
six-voice recording that was already right. That is a band about 0.04 wide,
fitted to twelve measurements, and a wider or more realistic set could move it.

0.73 sits at the low edge on purpose. Too low and the step-back does nothing,
which is where this started; too high and it invents speakers. The quiet failure
is the better one to have.

## Cleaning the audio first makes everything worse

The obvious next move after the noise-floor fix is to remove the noise instead
of coping with it. sherpa-onnx ships an offline speech enhancer (GTCRN, 523 KB)
and it runs at 55x real time, cheap next to diarization at 10x. It was measured
and it is firmly rejected.

| | as recorded | denoised |
|---|---|---|
| 6 voices, 15 dB | **6 spk, 97.4%** | 5 spk, 80.7% |
| 6 voices, 10 dB | **6 spk, 87.2%** | 5 spk, 78.6% |
| 6 voices, 5 dB | **6 spk, 82.2%** | 4 spk, 67.5% |
| 4 voices, 15 dB — word error rate | **2.7%** | 13.5% |
| 6 voices, 10 dB — word error rate | **2.4%** | 12.2% |
| 6 voices, 5 dB — word error rate | **8.9%** | 28.4% |

Worse on both counts at every noise level, and by a lot: five times the word
error rate on the mildest case, and a whole speaker lost on all three.

It is not an interaction with the adaptive threshold above — denoised audio has
no floor left, so that path correctly stops binding, and the score is identical
with it disabled. It is the enhancement itself. Told there are six speakers, so
that counting cannot be the problem, denoised audio still scores 88.4% against
97.4% for the same recording untouched. GTCRN is trained to make speech sound
clean to a person, and what it removes takes some of what makes a voice that
person's with it.

The transcription result says something useful on its own: Parakeet at 10 dB SNR
transcribes at 2.4% word error rate. Additive noise is not what limits this
pipeline, and there is nothing there for an enhancer to win back.

Reproduce with `examples/denoise.rs`, which is kept for exactly that purpose:

```bash
cargo build --release -p scribe-asr --example denoise
target/release/examples/denoise gtcrn_simple.onnx in.wav out.wav
```

## The fixtures are synthetic, and that has bitten four times

`say` voices are not people, and the ways they differ from people have produced
wrong conclusions repeatedly — twice by making the code look broken, twice by
flattering it:

- **Repeated sentences** let the clusterer key on content rather than voice.
- **Uniform turn lengths** hid a duration-blind participant floor.
- **Character voices** (Bells, Zarvox, Boing) barely transcribe at all: 28.8%
  word error rate, which read as the ASR model being broken.
- **Formant voices** (Fred, Kathy, Ralph) transcribe fine but are *too easy to
  tell apart*. A six-voice fixture including Fred scored 95.7% in a reverberant
  room; the same six with a modern voice in his place scored 83.2%, and that gap
  is what led to the noise-floor bug above.

`make-diar-fixture.py` now refuses both families by name, because macOS lists
all three generations identically and there is no way to tell from `say -v ?`
which is which. Use the recorded voices only: Daniel, Samantha, Rishi, Karen,
Moira, Tessa, Aman, Tara.

None of this substitutes for a recording of real people, which is still the
largest untested gap in every number in this document.

## Guarding the numbers

Every figure below was established by running a harness and reading the number.
Nothing guarded them, so a change costing ten points would have been noticed
eventually or not at all.

```bash
cargo build --release -p scribe-asr --example diarize_check
./scripts/diar-regression.sh          # about two minutes
./scripts/diar-regression.sh --full   # adds the eleven-minute windowed case
```

It builds its own fixtures and exits non-zero if any check falls short. Eleven
of them: speaker counts and accuracy from one voice up to eight, clean and
degraded, with somebody moving about and somebody on a phone; word error rate
and speaker attribution on the finished transcript; and a name given in one
meeting still recognised in another recorded in a different room, with nobody
else wrongly named. The transcription and enrolment checks are skipped with a
note when no ASR checkpoint is installed, since they need one and the
diarization checks do not.

The floors sit a little under what is measured today, so ordinary variation does
not cry wolf and a real regression cannot hide. Each harness takes its own
assertions if you want to check a fixture of your own:
`DIARIZE_EXPECT_SPEAKERS`, `DIARIZE_MIN_CORRECT`, `TRANSCRIPT_MAX_WER`,
`TRANSCRIPT_MIN_CORRECT`, `ENROLL_EXPECT_RECOGNISED`, `ENROLL_FORBID_FALSE`.

Checked against a known-bad setting — the cut threshold pushed to 0.95, which
earlier sweeps showed breaks two fixtures — it reports three failures with the
speaker counts and the shortfalls, and exits 1.

## Languages other than English

The default checkpoint, Parakeet TDT 0.6b v3, claims twenty-five European
languages, and nothing here had ever spoken one. It works, with no configuration
of any kind — there is no language setting and none is needed.

| | word error rate | speakers | attribution |
|---|---|---|---|
| French, 2 voices, 36 s | 2.8% | 2 of 2 | 100% |
| German, 1 voice, 29 s | **0.0%** | 1 of 1 | — |

Diarization does not care about language at all, which is expected — it measures
voices rather than words — and is worth having measured rather than assumed.

The English fixtures were already testing more than English, incidentally. The
default voices are British, American, Indian and Australian, and the larger sets
add Irish and South African.

### A warning that cost an hour

The first German fixture scored 43.2%, which reads as the language being badly
supported. It is not: it was built from macOS *character* voices — Eddy, Rocko,
Grandma and the rest, the stylised ones — and those are not speech. The same
script through Anna, the natural German voice, transcribes at 0.0%.

`make-diar-fixture.py` now refuses a voice macOS lists with a parenthesised
name, and says why. That is the third time on this branch a fixture has produced
a confident wrong conclusion — after one that repeated its lines and one whose
turns were all the same length.

## Small and degenerate recordings

Not every recording is a meeting.

| | speakers | correct |
|---|---|---|
| one voice, 40 s (a dictation) | 1 of 1 | 99.9% |
| two voices, 49 s | 2 of 2 | 99.8% |
| one voice, 2 s | 1 of 1 | 98.5% |
| twenty seconds of silence | 0 | — |

The single-voice case matters more than it looks. The rule that reads the
speaker count off a recording's own merge sequence has to be able to answer
"one", and a rule that always cuts somewhere never can — and separately, it was
the case that showed the transcript being grouped wrongly. An utterance used to
end only at a change of speaker or a second and a half of silence, and nobody
dictating pauses that long mid-thought, so forty seconds of one voice arrived as
a single utterance of a hundred and twenty-four words. Forty minutes would have
arrived as one of several thousand.

That matters because an utterance is the unit everything downstream works in: a
line in the transcript, what playback scrolls to and highlights, what a mark
anchors to, and what "edit this line" edits. An utterance now also ends at the
end of a sentence once it has run twelve seconds, or at thirty regardless.
Conversation is untouched — the multi-speaker fixtures return exactly the
utterance counts they did before, 30 of 30 and 136 of 136 — because turns that
short never reach either limit. Silence produces no
speakers and no turns rather than an error, and all four run through the whole
pipeline to `ready` — the silent one with an empty transcript and no summary,
which is the right answer to a recording of nothing.

## Where it stands

Scoring the share of speech given to the right person, on fixtures built by
`make-diar-fixture.py`:

| fixture | mode | speakers found | correct |
|---|---|---|---|
| 3 voices, 55 s | discover | 3 of 3 | 99.6% |
| 3 voices, 55 s | stated 3 | 3 of 3 | 99.6% |
| 5 voices, 69 s | discover | 5 of 5 | 99.8% |
| 5 voices, 69 s | stated 5 | 5 of 5 | 99.8% |
| 4 voices, 2.5 min | discover | 4 of 4 | 99.8% |
| 4 voices, 2.5 min (second script) | discover | 4 of 4 | 99.7% |
| 4 voices, 11.1 min | discover | 4 of 4 | 99.6% |

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

| fixture | transcribe | diarize | total | WER | right speaker |
|---|---|---|---|---|---|
| 4 voices, 2.5 min | 34.5x | 12.6x | 9.2x | 1.5% | 100.0% |
| 4 voices, 2.5 min, dirty | 32.9x | 11.9x | 8.7x | 2.7% | 100.0% |
| 4 voices, 11.1 min | 30.6x | 11.2x | 8.2x | 1.2% | 100.0% |
| 4 voices, 11.1 min, dirty | 32.3x | 11.5x | 8.5x | 5.0% | 100.0% |
| 4 voices, one moving | 33.4x | 12.6x | 9.2x | 1.3% | 100.0% |

Multiples are of real time, so 7.2x means an hour of audio in about eight and a
half minutes. These are current: the earlier numbers on this page were taken
before the decode window shrank and before the embedding model changed, and both
moved them. Two things worth reading off that table:

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
window at a time, strictly in sequence, and ONNX Runtime was thought to stop
scaling well before this machine's core count — 4 threads to 8 buys 9%. Two
windows at once should therefore be nearly free.

(That premise was wrong; see "the thread cap that was never measured" below.
Scaling continues to about two threads short of the core count, which leaves
even less idle capacity for a second window than this assumed. The conclusion
below — that parallel windows are not worth it — is unaffected, and if anything
better supported.)

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

## Hotwords do nothing on the shipped model

`[asr].hotwords_file` biases recognition toward names and terms you expect. It
is exactly the right tool for what speech recognition gets wrong here — a
fixture of eight lines full of Irish names, product names and infrastructure
jargon transcribes at 16.7% word error rate, with *Siobhan* as "Shivon",
*Kubernetes* as "Cuba Needs", *Eoghan* as "Ian" and *rota* as "rotor".

Pointing it at a list containing all of those changes the word error rate by
nothing. Not a little — 16.7% before and after, at boosts of 2.0 and 4.0, the
same seventeen edits.

Hotwords are matched against the tokens the model predicts, so on a sub-word
model they have to be tokenized the way it was trained, which needs the
vocabulary file. Setting `modeling_unit = "bpe"` without one stops the
recognizer building at all. sherpa says so if you look for it — *"Some hotwords
failed to encode and were skipped"* — but the configuration was accepted and the
log said biasing was enabled.

`[asr].bpe_vocab` now carries that file, and the worker warns loudly when
hotwords are set without it. The checkpoint `scribe models pull` installs does
not publish one, so on a default install hotwords are inert and now say so.

### What does work: the LLM correction pass

`[llm].correct_transcript` has the model read the finished transcript, with the
known speaker names as hints, and fix misheard proper nouns after the fact. On
the same fixture that hotwords could not touch, it takes the word error rate
from **16.7% to 1.0%** — every one of Shivon, Cuba Needs, rotor, Efa, Nayam and
Ian back to the name that was said.

`scripts/stub-llm.py` is how that was measured without a model, and how the
stage's safety claims were checked. It answers both the Ollama and OpenAI
request shapes and applies a fixed glossary read out of the prompt, which
exercises everything except the model's judgement.

```bash
python3 scripts/stub-llm.py 8799 &
# [llm] base_url = "http://127.0.0.1:8799", provider = "ollama", summarize_model = "stub"
```

It also has three modes for misbehaving: returning summaries instead of
corrections, replacing every line with "ok", and replying with something that is
not JSON. The merge stage claims none of those can corrupt a transcript, and
with all three the transcript comes back exactly as the recogniser produced it.
That claim now has a test rather than a comment.

Both LLM stages are written to degrade rather than fail when no model is
reachable, which `e2e-check.sh` checks by pointing them at a closed port. The
stub checks the other direction.

### Where the speaker labels were being lost

The summary is where telling voices apart is supposed to pay off — "Karen agreed
to send the note" rather than "it was agreed". Logging what each stage actually
sent showed the labels reaching the model for a short recording and not for a
long one.

A transcript over the prompt budget is condensed first, in parts, and the parts
do carry speaker labels — the prompt asks for them to be kept. But what reaches
the final summary pass is then the *notes*, not the transcript, and whether the
names survived is entirely down to whether the model honoured that instruction.
Nothing checked, and nothing could tell afterwards.

The final prompt now names the participants itself, in a line of its own that
does not pass through the condensation:

```
The people speaking are: Daniel, Samantha, Rishi, Speaker 3. Attribute
decisions and action items to them by name where the transcript supports it.
```

Anonymous speakers are included on purpose: "Speaker 3" still says a fourth
person was present and which lines were theirs.

### And the same gap in search

Following the labels the rest of the way turned up a second place they stopped.
Retrieval chunks carry the speaker index, but a search hit did not carry the
name and a retrieval excerpt reached the model as:

```
[1] (Q3 planning, 03:42)
Nobody has looked at that dashboard since the person who built it moved teams.
```

So "who mentioned the dashboard?" could not be answered from a transcript that
records precisely that, and a model told not to invent speakers would correctly
decline. The excerpt now reads `[1] (Q3 planning, 03:42 — Karen)`, search hits
and citations carry the name, and the system prompt says the names are there to
be used.

The `speaker` filter on `/search` was wrong in the same spirit. It matched on
the recording — *any* chunk of any recording that speaker appeared in, including
every word somebody else said — so filtering by Karen returned the meetings
Karen attended rather than the things Karen said. A chunk records the speaker it
came from, so the join belongs there. The test that covers it inserts a line
from a second speaker matching the same query, because the first version of that
test passed against both the old code and the new one.

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

A duration on its own turned out not to be enough in the other direction
either. An eleven-minute degraded recording came back with a fifth speaker
holding four and a half seconds — over the three-second floor, and 0.7% of the
speech. The same four and a half seconds would be a fifth of a forty-second
exchange. A cluster now has to clear both the floor and one percent of the
recording's speech, which fixes the long case and leaves every short one alone;
raising the absolute floor to five seconds fixes it too, and costs anybody who
says three to five seconds in a recording short enough for that to be a real
share of it.

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

## People move

`degrade-audio.py --moving <name>` takes one speaker nearer and further across
the recording, the way somebody does who leans back, turns to a whiteboard or
walks about. Level and reverberation change together, because both follow
distance.

This used to be the worst failure measured here, and it is now fixed. It is kept
because how it was fixed is the point.

| fixture | TitaNet, discovering | ERes2Net, discovering |
|---|---|---|
| 4 voices, 2.5 min, Daniel moves | 6 speakers, 89.9% | **4 speakers, 99.8%** |
| 4 voices, 11 min, Samantha moves | 5 speakers, 95.8% | **4 speakers, 99.7%** |
| 4 voices, second meeting, Daniel moves | 6 speakers, 83.3% | **4 speakers, 99.6%** |

Three things were tried against it first, and all three were the wrong place to
look.

**Levelling every piece to a common loudness before embedding.** Distance
changes how loud somebody is, so removing loudness should remove the difference.
It changes the numbers on every fixture by nothing at all: the embedding models
are already invariant to gain. What was left was reverberation, a real spectral
change.

**Reading the merge sequence more cleverly.** The cut landed on a fall of 1.03x
and left six speakers, with a fall of 2.27x three merges later that is plainly
where four people become three. Cutting at the steepest fall — the standard
elbow — is much worse on all fourteen fixtures it was tried against: similarity
approaches zero as the last unrelated clusters are forced together, so the
sharpest ratio is nearly always among the final merges, and five voices come
back as one. The rule in place compares each merge against the family already
accepted rather than the one before it, which anchors it to what "same voice"
looked like earlier in this recording. It stays.

**Stating the participant count.** Which worked on one recording and not on
another, because the count settles a recording to *N* voices by joining the
closest pair repeatedly, and whether that reunites somebody who moved depends on
whether their own two distances are closer together than the two most similar
different people present.

It was the embedding. A model that holds a voice together across a change of
room makes all three unnecessary. Worth remembering the next time something
looks like a clustering problem: three plausible repairs to the clustering, and
the fault was upstream of all of them.

The comparison below still holds for a *television*, which no embedding model
will fix, since it is not a mistake:

|  | what went wrong | does stating the count help? |
|---|---|---|
| a speaker moves | one person became several | not needed now — the model holds them together |
| a television is on | something that is not a person became one | **no**, and it makes it worse |

## Somebody on the phone

A hybrid meeting has one participant coming through a telephone: band-limited to
roughly 300–3400 Hz, compressed, with a little line noise. That is a large
change to a voice, and it is the most common asymmetric condition there is —
one person heard through a different channel from everybody else.

```bash
python3 scripts/degrade-audio.py in.wav out.wav --phone Karen --truth truth.json
```

**Diarization is fine, and slightly better than fine.**

| fixture | speakers | correct |
|---|---|---|
| 4 voices, clean | 4 of 4 | 99.8% |
| 4 voices, one on the phone | 4 of 4 | 99.9% |
| 4 voices, one on the phone, in a room | 4 of 4 | 99.4% |

Band-limiting makes that voice *more* distinct from the others, so separating
them gets marginally easier. Word error rate barely moves either, 1.5% to 1.9%.

**Recognising them across recordings is where it breaks.** Somebody enrolled in
the room and dialling in next week scores 0.774 against their own voiceprint,
which is recognised. Add any reverberation to the room the others are sitting
in and it falls to about 0.57–0.60, and they lose their name — not to the floor
or to the separation rule, both of which they clear, but to the requirement that
a match resemble the other matches this recording is producing. The people in
the room are matching at 0.85–0.90.

**And that requirement cannot be loosened.** The obvious fix is to lower the
ratio: the three dial-in matches that fail sit at 0.655 to 0.673 of their
recording's median match, and the false positive this protects against — a
different person resembling somebody enrolled — sits at 0.616. There looks to be
room between them.

There is not. In the same recording where the dial-in Daniel scores 0.566
against his own voiceprint, Karen, who is not enrolled at all, scores 0.566
against Samantha's. Identical numbers, identical ratios, one right and one
wrong. Any threshold admitting the first admits the second.

So a dial-in participant in a reverberant room may need naming by hand. The
transcript is right about who spoke; it is the name that does not carry over.

## Rooms have things in them that are not people

`scripts/add-room-noise.py` mixes non-speech events into a fixture — keystrokes,
a door slam, a chair scrape, hold music, paper shuffling — placed in the pauses
where a real room puts most of them.

```bash
python3 scripts/add-room-noise.py in.wav out.wav --truth truth.json --level 1.2
```

None of it becomes a speaker. Fifteen events at half the level of the speech,
and again at 1.2 times it, leave the count at four of four and attribution at
99.7% against 99.9% clean. The segmentation model is deciding what speech *is*,
not where the energy is, and a door slam is not close. This is also the clearest
illustration of why the cheap substitutes for it failed.

Background **speech** is a different matter, and the finding is worth stating
plainly because it is the one condition here that produces a wrong answer a user
will notice.

A television left on, at a third of the level of the room, audible only in the
pauses between turns:

| | speakers found | truth |
|---|---|---|
| discovering the count | 4 | 3 |
| told there are 3 people | 3, but the wrong 3 | 3 |

Discovering, the television is a speaker — 36% of the speech in the recording,
more than any person in the room, because it talks steadily through every lull.
The three real participants are still separated correctly at 99.7%; the
transcript simply gains a fourth attendee reading the weather.

Telling it there are three people does not fix this and makes it worse. The
count constrains how many clusters there are, not which of them are people, and
the television is the *most* acoustically distinct voice present — it is a
different speaker at a different level. So it survives, and two real
participants who sound more like each other are merged instead.

There is no signal in a single mixed channel that separates "person in this
meeting" from "voice in this room". A quiet participant and a distant television
both speak at a lower level, both mostly in the gaps, and both are somebody
else's voice. Diarization is doing its job correctly here — it separated a
voice, and a voice is what it was.

So the remedy is a correction rather than a fix. `DELETE
/recordings/{id}/speakers/{local_idx}` removes a diarized voice and every line
attributed to it, then rebuilds the summary and the search index without them —
in the app, long-press one of its lines and choose "Not a participant". Clearing
the *name* was the only option before and is not enough: the lines stay in the
transcript, and from there in the summary, so the meeting gets summarised partly
from the weather forecast.

On the fixture above that takes the recording from four speakers to three and
removes exactly the three television lines. The audio is untouched, so a
reprocess brings the voice back.

## The embedding model

`scribe models pull` installs 3D-Speaker's ERes2Net. It used to install NVIDIA's
TitaNet-large. The swap is the largest single accuracy change measured here, and
it is worth recording what it did and did not fix, since the two are not the
same shape.

Point `diarize_check` at a directory holding a different `embedding.onnx` to
compare — the model is a file, not a code path.

| fixture | TitaNet | ERes2Net |
|---|---|---|
| 4 voices, one moving | 6 spk, 89.9% | **4 spk, 99.8%** |
| 4 voices, 11 min, one moving | 5 spk, 95.8% | **4 spk, 99.7%** |
| 4 voices, second meeting, one moving | 6 spk, 83.3% | **4 spk, 99.6%** |
| 6 voices, reverb + noise | 5 spk, 82.5% | **6 spk, 94.4%** |
| 8 voices, reverb + noise | 7 spk, 86.9% | **8 spk, 89.4%** |
| 8 voices, clean | **8 spk, 99.9%** | 7 spk, 88.6% |
| everything else (16 fixtures) | — | within 1.5 points either way |

**What it fixed.** A speaker who moves about the room. That failure had been
documented across three commits as not recoverable — not by a better cut rule,
and only sometimes by stating the participant count. It is an embedding problem,
and a better embedding solves it.

**What it cost.** One fixture: eight voices, clean audio, where two women merge
and seven speakers come back instead of eight. That is a *counting* failure, not
a discrimination one — told there are eight, the same model returns eight
speakers at 99.9%, so the embeddings separate them perfectly well.

This is a property of that particular set of eight voices, not a ceiling on
eight. A fixture built from eight of the modern recorded voices — Daniel,
Samantha, Rishi, Karen, Moira, Tessa, Aman, Tara — returns 8 speakers at 99.9%
unprompted. Which eight matters more than how many.

The merge sequence shows why no rule recovers it. Merging down to eight clusters
goes 0.58, 0.56, 0.56, 0.53, 0.45, 0.38, 0.34 — every one of those rejoining one
person's own pieces — and then joins two different women at 0.31, with the
merges either side at 0.34 and 0.30. One speaker's own variation is as wide as
the gap between two different people. Sweeping the cut threshold from 0.70 to
0.90 changes nothing about this fixture at any value, while 0.70 to 0.85 leave
every other fixture identical and 0.90 breaks two of them.

There is no signal *in the merge sequence* to find. That much still holds, and
it is why no cut rule recovers this. What it does not follow is that nothing
recovers it — the signal is in the clusters the cut produces, not in the
sequence that produced them. See "the cluster that is too loose" below.

**And enrollment, which is where the difference is largest.** The same person
heard through a different room:

| | own voiceprint | nearest other |
|---|---|---|
| TitaNet | 0.56 – 0.69 | 0.60 |
| ERes2Net | **0.84 – 0.87** | 0.61 |

With TitaNet a true match across rooms sits a hair above the 0.5 floor and a
stranger sits at 0.60, which is to say the false match scores higher than the
true one and only the surrounding context separates them. With ERes2Net the gap
is not close. It also recovers an enrolled speaker who moves, who scored 0.499
against his own voiceprint before — under the floor, and refused — and scores
0.949 now.

### How far a bad room can go

Enrolling from a clean recording and then recognising the same people through
progressively worse ones, with a fourth person present who was never enrolled:

| meeting B | own voiceprint | nearest other | result |
|---|---|---|---|
| same room as A | 0.98 | 0.61 | 3 of 3, no false positives |
| different room | 0.84 – 0.87 | 0.51 | 3 of 3, no false positives |
| RT60 0.9 s, 8 dB SNR | 0.72 – 0.78 | 0.33 | 3 of 3, no false positives |
| RT60 1.4 s, 4 dB SNR | 0.61 – 0.67 | — | 3 of 3, no false positives |

The last row is the useful one, and it settles a question the floor keeps
raising. A floor of 0.5 looks far too low, since a stranger resembling somebody
scores about 0.61 — above it. Raising it to 0.7 so it does that job directly
loses every real speaker in the bad room, and 0.65 loses two of three.

A stranger in a good room and a friend in a bad one land on the same number.
That is why what decides a match is relative — how far it stands clear of the
rest of the library, and whether it is the kind of match this recording is
producing — and why the floor stays low and out of the way. Everything from 0.5
to 0.6 behaves identically on every fixture.

### Enrolling one person twice

The obvious mistake — tagging somebody "Dan" in one meeting and "Daniel" in
another — has a consequence worse than two names. It stops that person being
recognised **at all**.

A match has to stand clear of the rest of the library, and a voice cannot stand
clear of itself. With both entries scoring alike, neither wins and the speaker
comes back unnamed. Measured, one sample split in two and enrolled under two
names sits at 0.83 apart, and recognition on the next recording goes from three
of three to none. Nothing in the transcript says why.

Enrolment now says so at the time, naming the speaker whose voice it resembles
and pointing at `scribe speaker merge`. It does not refuse — the caller asked,
and people do sound alike — and the API returns `already_enrolled_as` so a client
can offer to use the existing name instead.

The remedy works: merging the two identities takes recognition back from none
to matched.

### A poor first sample used to be permanent

A voiceprint was written once and never changed. Enrolling the same person again
under a second name makes them unrecognisable, as above, and enrolling under the
same name did nothing at all — so a first sample that was short or noisy could
not be improved by any route.

It costs more than it looks. Enrolling Daniel from 1.2 seconds of quiet, noisy
audio against enrolling him properly:

| meeting B | good sample | poor sample |
|---|---|---|
| same room | 0.936 | 0.693 |
| different room | 0.823 | 0.687 |
| RT60 0.9 s, 8 dB | 0.697 | 0.670 |
| RT60 1.4 s, 4 dB | 0.620 | 0.611 |

He is still recognised in all eight cases, so nothing looks broken. But a
stranger who resembles somebody scores about 0.61, and that is the number these
have to stay clear of. The good voiceprint keeps a margin of 0.33 in a clean
room; the poor one keeps 0.08, and in a bad room keeps none.

`scribe enroll --replace` gives an existing speaker a new voice sample, and
enrolling a name that is already taken now refuses and says so rather than
quietly creating the duplicate that breaks recognition. The API takes
`replace_voiceprint` for the same purpose, and in the app it is a long press on
an already-enrolled speaker in the tag sheet — "hold to re-learn".

Replacing the poor voiceprint takes recognition in a different room from 0.687
to 0.824 through the CLI, and to 0.877 through the app, where the replacement
comes from a diarized voice built out of a minute of that person across a whole
meeting rather than from a clip. That is usually the better sample, which is why
the affordance belongs where a recording and a speaker are both to hand.

### If you already have enrolled speakers

Voiceprints are not comparable between models: a vector from one means nothing
to the other. `pull` never overwrites a model file that is already there, so an
existing install keeps whatever it enrolled against and nothing breaks by
itself. Swapping deliberately means re-enrolling everybody — delete the
`embedding.onnx`, pull, and enroll again.

### Models that are not drop-in

Both of the above produce 192 dimensions, which `speakers.embedding vector(192)`
commits to. CAM++ and the wespeaker ResNets produce 512 and would need a
migration. They also scored far worse here (28% to 43% of speech to the right
speaker), which is enough to stop looking at them regardless.

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
