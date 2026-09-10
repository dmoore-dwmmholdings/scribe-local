#!/usr/bin/env bash
# scripts/e2e-check.sh — run the real pipeline over a file and show what it made.
#
# The component harnesses (diarize_check, transcript_check, enroll_check) each
# measure one model against known answers. This runs the thing a user actually
# gets: transcode, diarize, transcribe, merge, embed, summarize, through the CLI,
# against a real Postgres, and prints the transcript that came out with the
# speaker each line was given.
#
# Use it to check an install end to end, or to see what the pipeline makes of
# your own recording — which is worth more than any synthetic fixture.
#
#   ./scripts/e2e-check.sh <audio-file> [more-audio-files...]
#
# Files are ingested in order into the same database, so enrolled voices carry
# from one to the next. To enrol first:
#
#   scribe --config <cfg> enroll --name Alice --audio alice-sample.wav
#
# Requires: a Postgres with pgvector reachable at SCRIBE_E2E_DB, models under
# ./models, ffmpeg, and a release build of the CLI. Nothing here touches a real
# deployment — it uses its own database and its own blob directory.

set -euo pipefail

DB_URL="${SCRIBE_E2E_DB:-postgres://scribe:scribe@127.0.0.1:5434/scribe_e2e}"
MODELS="${SCRIBE_MODELS:-$PWD/models}"
WORK="${SCRIBE_E2E_WORK:-$(mktemp -d)}"
BIN="${SCRIBE_BIN:-$PWD/target/release/scribe}"

if [ $# -lt 1 ]; then
  sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
fi

[ -x "$BIN" ] || { echo "no scribe binary at $BIN — cargo build --release -p scribe-cli"; exit 1; }
[ -d "$MODELS/diarization" ] || { echo "no models under $MODELS — see docs/measuring-diarization.md"; exit 1; }

# The sherpa dylib is emitted beside the binary without an rpath to itself.
export DYLD_LIBRARY_PATH="$(dirname "$BIN"):${DYLD_LIBRARY_PATH:-}"
export LD_LIBRARY_PATH="$(dirname "$BIN"):${LD_LIBRARY_PATH:-}"

mkdir -p "$WORK/blobs"
CFG="$WORK/config.toml"
cat > "$CFG" <<TOML
[database]
url = "$DB_URL"
max_connections = 5

[storage]
blobs = "$WORK/blobs"

[asr]
model = "${SCRIBE_ASR_MODEL:-parakeet-tdt-0.6b-v3}"
diarization = true
device = "${SCRIBE_ASR_DEVICE:-cpu}"

[worker]
stages = ["all"]
concurrency = 1
models_dir = "$MODELS"

# Deliberately unreachable unless you point it somewhere: the LLM stages are
# meant to degrade rather than fail, and this checks that they do.
[llm]
base_url = "${SCRIBE_LLM_URL:-http://127.0.0.1:1/v1}"
TOML

echo "work dir   $WORK"
echo "database   $DB_URL"
"$BIN" --config "$CFG" migrate 2>&1 | tail -1

for FILE in "$@"; do
  echo
  echo "=== $FILE ==="
  START=$(date +%s)
  ID=$("$BIN" --config "$CFG" ingest --inline "$FILE" 2>"$WORK/log" | tail -1)
  ELAPSED=$(( $(date +%s) - START ))

  grep -E "matched enrolled speaker" "$WORK/log" | sed 's/\x1b\[[0-9;]*m//g' \
    | sed 's/.*local_idx=/  recognised: local_idx=/' || true
  grep -E "WARN" "$WORK/log" | sed 's/\x1b\[[0-9;]*m//g' | sed 's/^/  /' || true

  "$BIN" --config "$CFG" transcript "$ID" 2>/dev/null | sed 's/^/  /'
  echo "  -- ${ELAPSED}s"
done
