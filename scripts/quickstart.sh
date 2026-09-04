#!/usr/bin/env bash
# Set up the whole Scribe stack on this machine with one command.
#
# Builds the scribe image, starts Postgres, downloads the speech models, applies
# migrations, and runs the API and the worker. Then it prints the two things you
# need on the phone: the server URL and the device token.
#
# The only prerequisite is Docker. Nothing needs editing first.
#
# Usage:
#   ./scripts/quickstart.sh                 local trial on 127.0.0.1
#   ./scripts/quickstart.sh --tailscale     also publish on your tailnet
#   ./scripts/quickstart.sh --ollama        also run Ollama for summaries
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

use_tailscale=0
use_ollama=0
api_port=8443
while [ $# -gt 0 ]; do
    case "$1" in
    --tailscale) use_tailscale=1 ;;
    --ollama) use_ollama=1 ;;
    --api-port) api_port="$2"; shift ;;
    -h | --help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

step() { printf '\n\033[36m[%s] %s\033[0m\n' "$1" "$2"; }
ok() { printf '      \033[32m%s\033[0m\n' "$1"; }
note() { printf '      %s\n' "$1"; }

echo "== Scribe quickstart =="

# --- 1. Docker ---------------------------------------------------------------
step 1 "Checking Docker ..."
command -v docker >/dev/null || {
    echo "Docker is not installed. Install Docker Engine or Docker Desktop, then re-run." >&2
    exit 1
}
docker version --format '{{.Server.Version}}' >/dev/null 2>&1 || {
    echo "The Docker daemon is not reachable. Start it (or add yourself to the docker group), then re-run." >&2
    exit 1
}
ok "Docker $(docker version --format '{{.Server.Version}}') is ready."

# --- 2. Local settings -------------------------------------------------------
step 2 "Writing .env ..."
if [ -f .env ]; then
    ok ".env already exists - leaving it alone."
else
    cp .env.example .env
    ok "created .env from .env.example"
fi
if [ "$api_port" != "8443" ]; then
    printf 'SCRIBE_API_PORT=%s\n' "$api_port" >> .env
    note "API port set to $api_port"
fi

# --- 3. Tailscale (optional) -------------------------------------------------
public_url="http://127.0.0.1:$api_port"
if [ "$use_tailscale" = "1" ]; then
    step 3 "Publishing the API on your tailnet ..."
    if ! command -v tailscale >/dev/null; then
        echo "      warning: tailscale not found - skipping." >&2
    else
        out="$(tailscale serve --bg "http://127.0.0.1:$api_port" 2>&1 || true)"
        case "$out" in
        *"not enabled"*) echo "      warning: Tailscale Serve is off for your tailnet. Turn it on at https://login.tailscale.com/f/serve" >&2 ;;
        esac
        dns="$(tailscale status --json 2>/dev/null | sed -n 's/.*"DNSName": *"\([^"]*\)\..*/\1/p' | head -1)"
        if [ -n "$dns" ]; then
            public_url="https://$dns"
            ok "tailnet URL: $public_url"
        fi
    fi
else
    step 3 "Skipping Tailscale (re-run with --tailscale to reach this server from your phone)."
fi
# The worker and the app both resolve audio URLs against this value.
grep -v '^[[:space:]]*SCRIBE_PUBLIC_BASE_URL[[:space:]]*=' .env > .env.tmp || true
printf 'SCRIBE_PUBLIC_BASE_URL=%s\n' "$public_url" >> .env.tmp
mv .env.tmp .env

# --- 4. Build and start ------------------------------------------------------
step 4 "Building and starting the stack ..."
note "The first build compiles the ML stack and downloads ~750 MB of models."
note "Expect 15-30 minutes. Later runs take seconds."
profile_args=()
[ "$use_ollama" = "1" ] && profile_args=(--profile ollama)
docker compose "${profile_args[@]}" up -d --build

# --- 5. Ollama model (optional) ----------------------------------------------
if [ "$use_ollama" = "1" ]; then
    step 5 "Pulling the summarization model ..."
    model="$(sed -n 's/^[[:space:]]*SCRIBE_SUMMARIZE_MODEL[[:space:]]*=[[:space:]]*//p' .env | head -1)"
    docker compose exec -T ollama ollama pull "${model:-gemma3:12b}"
fi

# --- 6. Verify ---------------------------------------------------------------
step 6 "Waiting for the API ..."
healthy=0
for _ in $(seq 1 60); do
    if curl -fsS "http://127.0.0.1:$api_port/health" >/dev/null 2>&1; then
        ok "API health: $(curl -fsS "http://127.0.0.1:$api_port/health")"
        healthy=1
        break
    fi
    sleep 2
done
[ "$healthy" = "1" ] || echo "      warning: the API did not answer yet. Check: docker compose logs -f scribe-serve" >&2

# The token is minted inside the data volume on first start; read it from there
# rather than from the log, which may already have scrolled away.
token="$(docker compose exec -T scribe-serve sh -c "sed -n 's/^phone = \"\(.*\)\"/\1/p' /data/devices.toml" 2>/dev/null | tr -d '\r\n' || true)"

printf '\n\033[32m== Scribe is running ==\033[0m\n'

# Pairing link. The app registers the `scribe://` scheme and handles
# `scribe://pair`, so scanning this carries both values across in one step
# instead of asking anyone to retype a 64-character token on a phone keyboard.
#
# The token is still included: it is what authenticates the phone unless the
# server is running with auth.trust_tailscale_identity, and a link that works in
# both configurations is better than one that works in the newer one only.
pair_url="scribe://pair?url=$public_url"
[ -n "$token" ] && pair_url="$pair_url&key=$token"

if command -v qrencode >/dev/null 2>&1; then
    echo "  Scan this with the phone's camera to pair:"
    echo
    qrencode -t ANSIUTF8 -m 2 "$pair_url"
else
    # No hard dependency on qrencode: the link works however it reaches the
    # phone (AirDrop, Messages, a tap on the phone's own browser).
    echo "  Pairing link — open this on the phone to pair in one tap:"
    echo
    echo "    $pair_url"
    echo
    echo "  (install qrencode to get this as a scannable QR code here)"
fi

echo
echo "  Or enter these by hand in Settings:"
echo "    Server URL:   $public_url"
if [ -n "$token" ]; then
    echo "    Device token: $token"
else
    echo "    Device token: docker compose exec scribe-serve cat /data/devices.toml"
fi
echo
echo "  Logs:     docker compose logs -f scribe-serve scribe-worker"
echo "  Stop:     docker compose down"
echo "  Restart:  docker compose up -d"
if [ "$use_tailscale" != "1" ]; then
    echo
    echo "  The phone cannot reach 127.0.0.1. Re-run with --tailscale to publish"
    echo "  the API on your tailnet."
fi
