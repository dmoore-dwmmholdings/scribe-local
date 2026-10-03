#!/bin/bash
# Run the UI flow tests in the simulator against a local Scribe server.
#   ios/run-ui-tests.sh [test-name] [screenshot-dir]
# Needs a server on 127.0.0.1:8443 (see docs/install-iphone.md for the app,
# and deploy/local.toml for running a server on this Mac).
set -euo pipefail
cd "$(dirname "$0")"
TEST=${1:-}
OUT=${2:-$PWD/build/screens}
SIM=${SIM:-"iPhone 17 Pro"}
mkdir -p "$OUT"
rm -f "$OUT"/*.png
xcodegen -q
UDID=$(xcrun simctl list devices available | grep -F "$SIM (" | head -1 | grep -oE '[0-9A-F-]{36}')
xcrun simctl boot "$UDID" 2>/dev/null || true
xcrun simctl privacy "$UDID" grant microphone com.dwmmholdings.scribenative 2>/dev/null || true
ONLY=()
[ -n "$TEST" ] && ONLY=(-only-testing:"ScribeUITests/AppFlowTests/$TEST")
TEST_RUNNER_SCREENSHOT_DIR="$OUT" \
TEST_RUNNER_SCRIBE_TEST_BASE_URL="${SCRIBE_TEST_BASE_URL:-http://127.0.0.1:8443}" \
TEST_RUNNER_SCRIBE_TEST_KEY="${SCRIBE_TEST_KEY:-}" \
TEST_RUNNER_SCRIBE_TEST_AUDIO_FILE="${SCRIBE_TEST_AUDIO_FILE:-}" \
xcodebuild test -project Scribe.xcodeproj -scheme Scribe \
  -destination "platform=iOS Simulator,id=$UDID" -derivedDataPath build/sim \
  ${ONLY[@]+"${ONLY[@]}"} 2>&1 | grep -E 'Test Case|error:|XCTAssert|failed|passed|\*\* TEST' || true
ls "$OUT"
