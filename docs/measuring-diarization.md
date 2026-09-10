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
python3 scripts/make-diar-fixture.py /tmp/diar
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

Measured on the two fixtures `make-diar-fixture.py` builds, scoring the share of
speech given to the right person:

| fixture | mode | speakers found | correct |
|---|---|---|---|
| 3 voices, 55 s | discover | 3 of 3 | 99.6% |
| 3 voices, 55 s | stated 3 | 3 of 3 | 99.6% |
| 5 voices, 69 s | discover | 5 of 5 | 99.7% |
| 5 voices, 69 s | stated 5 | 5 of 5 | 99.7% |

Before turns were split at pauses the three-voice fixture scored 69.7%: every
time Karen spoke and Samantha followed, the segmentation model ran the two
together into a single turn and attributed it to Samantha.

A wrong count is honoured rather than overridden — say 4 on the five-voice
fixture and four speakers come back, at 88.9%. The stated number is the
caller's to get right.

Again: these are synthesised voices, cleaner and more separable than a real
room. Use the numbers to compare changes, not to predict field accuracy.
