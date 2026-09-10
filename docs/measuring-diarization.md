# Measuring speaker detection

`crates/scribe-asr/examples/diarize_check.rs` scores the real diarizer against a
conversation whose speakers are known, and reports what fraction of speech was
given to the right person. It is an example rather than a test because it needs
the ONNX models and a WAV file.

## Get the models

Only the diarization pair is needed — about 107 MB, against ~640 MB for an ASR
checkpoint the harness never calls:

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

## Run it

```bash
cargo build --release -p scribe-asr --example diarize_check
DYLD_LIBRARY_PATH="$PWD/target/release" \
  ./target/release/examples/diarize_check models /tmp/diar/conversation.wav /tmp/diar/truth.json
```

Pass a fourth argument to state the speaker count instead of discovering it.
Set `DIARIZE_CHECK_TURNS=1` to print every turn found against every turn spoken,
which is how you see *where* it went wrong rather than only how far.

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
