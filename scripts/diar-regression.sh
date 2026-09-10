#!/usr/bin/env bash
# scripts/diar-regression.sh — check speaker detection still does what it did.
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
export DYLD_LIBRARY_PATH="$ROOT/target/release:${DYLD_LIBRARY_PATH:-}"
export LD_LIBRARY_PATH="$ROOT/target/release:${LD_LIBRARY_PATH:-}"

[ -x "$BIN" ] || { echo "build first: cargo build --release -p scribe-asr --example diarize_check"; exit 1; }
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
check() { # label dir variant expected-speakers min-correct [stated]
  local label=$1 dir=$2 var=$3 spk=$4 minc=$5 stated=${6:-}
  local out
  out=$(DIARIZE_EXPECT_SPEAKERS=$spk DIARIZE_MIN_CORRECT=$minc \
        "$BIN" models "$FIX/$dir/$var.wav" "$FIX/$dir/truth.json" $stated 2>&1)
  local got
  got=$(echo "$out" | awk '/found speakers/{s=$3} /correct speaker/{a=$3} END{printf "%s spk, %s", s, a}')
  if echo "$out" | grep -q "^FAIL"; then
    printf "  FAIL  %-34s %s\n" "$label" "$got"
    echo "$out" | grep "^FAIL" | sed 's/^/          /'
    FAIL=$((FAIL+1))
  else
    printf "  ok    %-34s %s\n" "$label" "$got"
    PASS=$((PASS+1))
  fi
}

build four 30
degrade four dirty --reverb 0.4 --snr 15 --far Karen=0.3
degrade four moving --moving Daniel
degrade four phone --phone Karen --reverb 0.4 --snr 18
SCRIBE_FIXTURE_VOICES="Daniel,Samantha,Rishi,Karen,Moira,Fred" build six 36
degrade six dirty --reverb 0.4 --snr 15

echo "speaker detection, against the figures in docs/measuring-diarization.md"
check "4 voices, clean"                four conversation 4 99.0
check "4 voices, reverb+noise+quiet"   four dirty        4 99.0
check "4 voices, one moving"           four moving       4 99.0
check "4 voices, one on the phone"     four phone        4 99.0
check "6 voices, clean"                six  conversation 6 99.0
check "6 voices, reverb+noise"         six  dirty        6 90.0

if [ "${1:-}" = "--full" ]; then
  build long 136
  degrade long dirty --reverb 0.4 --snr 15 --far Karen=0.3
  check "4 voices, 11 min (windowed)"  long conversation 4 99.0
  check "4 voices, 11 min, degraded"   long dirty        4 99.0
fi

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
