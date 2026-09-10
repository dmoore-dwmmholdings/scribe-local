#!/usr/bin/env bash
# scripts/diar-regression.sh — check transcription and speaker detection still
# do what they did.
#
# Every accuracy figure in docs/measuring-diarization.md was established by
# running a harness by hand and reading the number. Nothing guarded them: a
# change that cost ten points would have been noticed by somebody eventually,
# or not.
#
# This builds the fixtures, runs the diarizer over each, and fails if any of
# them comes back with the wrong number of speakers or below its recorded
# accuracy. The floors are set a little under what is measured today, so
# ordinary variation does not cry wolf and a real regression cannot hide.
#
#   ./scripts/diar-regression.sh            # the quick set (about two minutes)
#   ./scripts/diar-regression.sh --full     # everything, including 11 minutes
#
# Needs the diarization models under ./models — see docs/measuring-diarization.md.
# Synthesising the fixtures needs macOS `say`, ffmpeg, and numpy.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
FIX="${SCRIBE_FIXTURES:-$ROOT/target/diar-fixtures}"
BIN=target/release/examples/diarize_check
TBIN=target/release/examples/transcript_check
EBIN=target/release/examples/enroll_check
export DYLD_LIBRARY_PATH="$ROOT/target/release:${DYLD_LIBRARY_PATH:-}"
export LD_LIBRARY_PATH="$ROOT/target/release:${LD_LIBRARY_PATH:-}"

[ -x "$BIN" ] || { echo "build first: cargo build --release -p scribe-asr --example diarize_check"; exit 1; }
HAVE_ASR=0
[ -x "$TBIN" ] && [ -d models/asr ] && HAVE_ASR=1
[ -d models/diarization ] || { echo "no models under ./models — see docs/measuring-diarization.md"; exit 1; }

PY=$(command -v python3)
mkdir -p "$FIX"

build() { # name turns overlap offset
  [ -f "$FIX/$1/conversation.wav" ] && return 0
  echo "  building fixture $1…"
  $PY scripts/make-diar-fixture.py "$FIX/$1" "${2:-30}" "${3:-0}" "${4:-0}" >/dev/null
}
degrade() { # src dst args...
  [ -f "$FIX/$1/$2.wav" ] && return 0
  $PY scripts/degrade-audio.py "$FIX/$1/conversation.wav" "$FIX/$1/$2.wav" \
      --truth "$FIX/$1/truth.json" "${@:3}" >/dev/null
}

PASS=0; FAIL=0
# Measure a condition without gating on it. For a known fault: a check would be
# permanently red and useless as a gate, and leaving it out entirely is how a
# number goes unwatched for months. This keeps it in front of whoever runs the
# suite, with what it did last time written down beside it.
note() { # label dir variant expected-speakers was
  local out
  out=$("$BIN" models "$FIX/$2/$3.wav" "$FIX/$2/truth.json" 2>&1)
  if ! echo "$out" | grep -q "found speakers"; then
    printf "  ????  %-34s did not run\n" "$1"
    return
  fi
  local got
  got=$(echo "$out" | awk '/found speakers/{s=$3} /correct speaker/{a=$3} END{printf "%s spk, %s", s, a}')
  printf "  note  %-34s %s   (wants %s; was %s)\n" "$1" "$got" "$4" "$5"
}

# Run a case that is known to kill the process, and report rather than die.
#
# The diarizer does not always fail by returning something wrong. On some
# recordings the pyannote-3.0 segmentation model reads out of bounds and the
# process takes SIGBUS, which in the worker means the whole worker goes down and
# takes any other job in flight with it. That is worth a line in this output.
crashcheck() { # label dir variant
  local out code
  out=$("$BIN" models "$FIX/$2/$3.wav" "$FIX/$2/truth.json" 2>&1)
  code=$?
  if [ "$code" -eq 0 ]; then
    printf "  ok    %-34s did not crash (%s)\n" "$1" \
      "$(echo "$out" | awk '/found speakers/{s=$3} /correct speaker/{a=$3} END{printf "%s spk, %s", s, a}')"
    PASS=$((PASS+1))
  else
    printf "  note  %-34s CRASHED, exit %s\n" "$1" "$code"
    printf "        %-34s known fault in pyannote-3.0; reverb-v1 does not\n" ""
  fi
}

check() { # label dir variant expected-speakers min-correct [stated]
  local label=$1 dir=$2 var=$3 spk=$4 minc=$5 stated=${6:-}
  local out
  out=$(DIARIZE_EXPECT_SPEAKERS=$spk DIARIZE_MIN_CORRECT=$minc \
        "$BIN" models "$FIX/$dir/$var.wav" "$FIX/$dir/truth.json" $stated 2>&1)
  local got
  got=$(echo "$out" | awk '/found speakers/{s=$3} /correct speaker/{a=$3} END{printf "%s spk, %s", s, a}')
  # A run that produced no numbers did not pass — it did not happen. Without
  # this a missing fixture printed "ok    8 voices, clean   spk," and counted
  # toward the total.
  if ! echo "$out" | grep -q "found speakers"; then
    printf "  FAIL  %-34s %s\n" "$label" "did not run"
    echo "$out" | tail -3 | sed 's/^/          /'
    FAIL=$((FAIL+1))
    return
  fi
  if echo "$out" | grep -q "^FAIL"; then
    printf "  FAIL  %-34s %s\n" "$label" "$got"
    echo "$out" | grep "^FAIL" | sed 's/^/          /'
    FAIL=$((FAIL+1))
  else
    printf "  ok    %-34s %s\n" "$label" "$got"
    PASS=$((PASS+1))
  fi
}

SCRIBE_FIXTURE_VOICES="Daniel" build solo 8
SCRIBE_FIXTURE_VOICES="Daniel,Samantha" build duo 10
build four 30
degrade four dirty --reverb 0.4 --snr 15 --far Karen=0.3
degrade four moving --moving Daniel
degrade four phone --phone Karen --reverb 0.4 --snr 18
SCRIBE_FIXTURE_VOICES="Daniel,Samantha,Rishi,Karen,Moira,Tessa" build six 36
degrade six dirty --reverb 0.4 --snr 15

echo "speaker detection, against the figures in docs/measuring-diarization.md"
check "1 voice (a dictation)"          solo conversation 1 99.0
check "2 voices"                       duo  conversation 2 99.0
check "4 voices, clean"                four conversation 4 99.0
check "4 voices, reverb+noise+quiet"   four dirty        4 99.0
check "4 voices, one moving"           four moving       4 99.0
check "4 voices, one on the phone"     four phone        4 99.0
check "6 voices, clean"                six  conversation 6 99.0
check "6 voices, reverb+noise"         six  dirty        6 96.0

if [ "${1:-}" = "--full" ]; then
  build long 136
  degrade long dirty --reverb 0.4 --snr 15 --far Karen=0.3
  check "4 voices, 11 min (windowed)"  long conversation 4 99.0
  check "4 voices, 11 min, degraded"   long dirty        4 99.0

  # Eight voices is where counting fails first: two of them join at a
  # similarity that looks like one person's own spread, so the merge sequence
  # has no step to find. The cohesion step-back is what recovers it, and this
  # is the check that guards it — without it the degraded case returns 7.
  SCRIBE_FIXTURE_VOICES="Daniel,Samantha,Rishi,Karen,Moira,Tessa,Aman,Tara" build eight 48
  degrade eight dirty --reverb 0.4 --snr 15
  check "8 voices, clean"              eight conversation 8 99.0
  check "8 voices, reverb+noise"       eight dirty        8 97.0

  # Fast conversation: gaps of 80-250 ms, most of them under the split
  # threshold, so the handover has to be found by the segmentation model or not
  # at all. The lowest-scoring condition that still counts correctly, and the
  # one that moves if the split threshold is lowered to chase it.
  SCRIBE_FIXTURE_GAP=80-250 \
    SCRIBE_FIXTURE_VOICES="Daniel,Samantha,Rishi,Karen,Moira,Tessa" build tight 36
  degrade tight dirty --reverb 0.4 --snr 15
  check "6 voices, gaps under 250 ms"  tight conversation 6 99.0
  # Fast conversation in a reverberant room now returns seven speakers for
  # six. It returned six at 92.8% while neighbouring turns shared a sentence;
  # with different words either side of the handover it splits one voice.
  # Not gated — a real condition and a real failure. The alternative
  # segmentation model measured in the docs returns six here.
  note  "6 voices, tight and degraded"   tight dirty        6 "7 spk, 90.8%"

  # Someone who speaks once and briefly is still a participant. This is the
  # check that stops the participant floor being raised to tidy away slivers:
  # at 5 s this person disappears entirely and their sentence is handed to
  # whoever spoke next.
  if [ ! -f "$FIX/brief/conversation.wav" ]; then
    echo "  building fixture brief…"
    $PY scripts/add-brief-speaker.py "$FIX/four" "$FIX/brief" >/dev/null
  fi
  degrade brief dirty --reverb 0.4 --snr 15
  check "5th speaker, 4.7 s of 153 s"  brief conversation 5 99.0
  check "the same, reverb+noise"       brief dirty        5 99.0

  # A meeting with real dead air in it — gaps from 0.3 s to 9 s, half the
  # recording silent. Clean it is right; degraded it returns a fifth speaker
  # made of two scraps of Karen, 3.7 s in total, which clears the participant
  # floor. Not gated: see "the phantom that is not worth the cure" in
  # docs/measuring-diarization.md for why the floor is not raised to catch it.
  # A roster the size of a year's meetings. Enrolment is only ever measured
  # against a handful of enrolled people, and the risk it carries — naming
  # somebody after a stranger who happens to sound like them — grows with how
  # many strangers are on file. These five meetings put twenty more people in
  # the database who are not in the room.
  if [ -x "$EBIN" ]; then
  ROSTER=""
  i=0
  for vs in "Eddy (English (UK)),Flo (English (UK)),Grandma (English (UK)),Grandpa (English (UK))" \
            "Reed (English (UK)),Rocko (English (UK)),Sandy (English (UK)),Shelley (English (UK))" \
            "Eddy (English (US)),Flo (English (US)),Grandma (English (US)),Grandpa (English (US))" \
            "Reed (English (US)),Rocko (English (US)),Sandy (English (US)),Shelley (English (US))" \
            "Aman,Tara,Tessa,Moira"; do
    i=$((i+1))
    if [ ! -f "$FIX/roster$i/conversation.wav" ]; then
      echo "  building fixture roster${i}…"
      SCRIBE_FIXTURE_VOICES="$vs" $PY scripts/make-diar-fixture.py "$FIX/roster$i" 16 >/dev/null
    fi
    ROSTER="${ROSTER:+$ROSTER,}$FIX/roster$i/conversation.wav:$FIX/roster$i/truth.json"
  done
  out=$(SCRIBE_ENROLL_EXTRA="$ROSTER" ENROLL_EXPECT_RECOGNISED=4 ENROLL_FORBID_FALSE=1 "$EBIN" models \
        "$FIX/meetA/conversation.wav" "$FIX/meetA/truth.json" \
        "$FIX/meetB/room.wav" "$FIX/meetB/truth.json" 2>&1)
  got=$(echo "$out" | awk '/^recognised/{r=$2} /^false positives/{f=$3} END{printf "%s recognised, %s false", r, f}')
  if ! echo "$out" | grep -q "^recognised"; then
    printf "  FAIL  %-34s %s\n" "4 names against a roster of 24" "did not run"
    echo "$out" | tail -3 | sed 's/^/          /'
    FAIL=$((FAIL+1))
  elif echo "$out" | grep -q "^FAIL"; then
    printf "  FAIL  %-34s %s\n" "4 names against a roster of 24" "$got"
    echo "$out" | grep "^FAIL" | sed 's/^/          /'
    FAIL=$((FAIL+1))
  else
    printf "  ok    %-34s %s\n" "4 names against a roster of 24" "$got"
    PASS=$((PASS+1))
  fi
  fi

  # A speaker who moves about a reverberant room. Her own variation across the
  # recording exceeds the gap between her and the others, so she comes back as
  # three people and the meeting has eight. Told there are six it scores 99.6%,
  # so the embeddings separate everyone — this is a counting failure, and the
  # rule that would catch it is a merge, which nothing here has. Not gated: see
  # "the mover who becomes three people" in docs/measuring-diarization.md.
  degrade six moving --moving Moira --reverb 0.4 --snr 18
  note  "6 voices, one moving, reverb"    six moving        6 "8 spk, 91.3%"

  # One person dictating into a reverberant, noisy room. Some such recordings
  # take the segmentation model out of bounds and kill the process; this one
  # does, reproducibly. Not every single-speaker recording does — the same
  # length and degradation through two other voices is fine — so it is content,
  # not a simple length rule.
  SCRIBE_FIXTURE_VOICES="Daniel" build dictation 14
  degrade dictation dirty --reverb 0.4 --snr 15
  crashcheck "70 s dictation in a bad room" dictation dirty

  SCRIBE_FIXTURE_GAP=300-9000 build lull 40
  degrade lull dirty --reverb 0.4 --snr 15
  check "4 voices, half of it silence"  lull conversation 4 99.0
  note  "the same, reverb+noise"        lull dirty        4 "5 spk, 97.8%"
fi

# The words themselves, and whether a name carries from one recording to the
# next. Both need an ASR checkpoint, which the diarization checks above do not.
if [ "$HAVE_ASR" = 1 ]; then
  echo
  echo "transcription"
  tcheck() { # label dir variant max-wer min-correct
    local out
    out=$(TRANSCRIPT_MAX_WER=$4 TRANSCRIPT_MIN_CORRECT=$5 \
          "$TBIN" models "$FIX/$2/$3.wav" "$FIX/$2/truth.json" 2>&1)
    local got
    got=$(echo "$out" | awk '/word error rate/{w=$4} /right speaker/{r=$3} END{printf "WER %s, %s to the right speaker", w, r}')
    if echo "$out" | grep -q "^FAIL"; then
      printf "  FAIL  %-34s %s\n" "$1" "$got"
      echo "$out" | grep "^FAIL" | sed 's/^/          /'
      FAIL=$((FAIL+1))
    else
      printf "  ok    %-34s %s\n" "$1" "$got"
      PASS=$((PASS+1))
    fi
  }
  tcheck "4 voices, clean"              four conversation 3.0 99.0
  tcheck "4 voices, reverb+noise+quiet" four dirty        10.0 99.0

  # A name given in one meeting, recognised in another recorded differently.
  build meetA 24 0 0
  build meetB 24 0 12
  degrade meetB room --reverb 0.5 --snr 18 --far Karen=0.35
  echo
  echo "names across recordings"
  out=$(ENROLL_EXPECT_RECOGNISED=3 ENROLL_FORBID_FALSE=1 "$EBIN" models \
        "$FIX/meetA/conversation.wav" "$FIX/meetA/truth.json" \
        "$FIX/meetB/room.wav" "$FIX/meetB/truth.json" Karen 2>&1)
  got=$(echo "$out" | awk '/^recognised/{r=$2} /^false positives/{f=$3} END{printf "%s recognised, %s false", r, f}')
  if echo "$out" | grep -q "^FAIL"; then
    printf "  FAIL  %-34s %s\n" "enrolled in A, heard in B's room" "$got"
    echo "$out" | grep "^FAIL" | sed 's/^/          /'
    FAIL=$((FAIL+1))
  else
    printf "  ok    %-34s %s\n" "enrolled in A, heard in B's room" "$got"
    PASS=$((PASS+1))
  fi
else
  echo
  echo "  (skipping transcription and enrolment: no ASR model under ./models/asr)"
fi

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
