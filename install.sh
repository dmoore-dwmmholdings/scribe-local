#!/usr/bin/env bash
# Scribe — one-command install.
#
#   curl -fsSL https://raw.githubusercontent.com/dmoore-dwmmholdings/scribe-local/master/install.sh | bash
#
# Windows (Git Bash / MSYS): downloads the prebuilt server bundle from the latest
# release, provisions a current ffmpeg, generates its own secrets, starts
# Postgres, applies migrations, downloads the speech models, publishes the API on
# your tailnet, and starts the API and the worker.
#
# Linux / macOS: there is no prebuilt bundle, so it clones the repo and brings up
# the container stack, which ends in the same place.
#
# Nothing has to be edited first and nothing is prompted for, so it is safe to
# pipe into bash. Re-running is safe too: existing secrets, models, and the
# database are kept.
#
# Options (with a pipe, pass them after `bash -s --`):
#   --dir PATH      install location (default: ~/scribe)
#   --model NAME    parakeet-tdt-0.6b-v3 (default) or whisper-large-v3-turbo
#   --service       install as always-on Windows services (needs Administrator)
#   --no-tailscale  skip publishing on the tailnet
#   --no-start      set everything up, but do not start the API or the worker
#   --docker        force the container install even on Windows
set -euo pipefail

REPO="dmoore-dwmmholdings/scribe-local"
FFMPEG_URL="https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
# ffmpeg builds older than this cannot read the MP4 'ipcm' audio and 'chnl' boxes
# that current phones write; those recordings fail in the transcode stage.
FFMPEG_MIN_YEAR=2025

INSTALL_DIR=""
ASR_MODEL="parakeet-tdt-0.6b-v3"
USE_TAILSCALE=1
DO_START=1
AS_SERVICE=0
FORCE_DOCKER=0
API_PORT=8443
DB_PORT=5433
API_PORT_PINNED=0
DB_PORT_PINNED=0

while [ $# -gt 0 ]; do
    case "$1" in
    --dir) INSTALL_DIR="$2"; shift ;;
    --model) ASR_MODEL="$2"; shift ;;
    --service) AS_SERVICE=1 ;;
    --no-tailscale) USE_TAILSCALE=0 ;;
    --no-start) DO_START=0 ;;
    --docker) FORCE_DOCKER=1 ;;
    --api-port) API_PORT="$2"; API_PORT_PINNED=1; shift ;;
    --db-port) DB_PORT="$2"; DB_PORT_PINNED=1; shift ;;
    -h | --help) sed -n '2,30p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

step() { printf '\n\033[36m==> %s\033[0m\n' "$1"; }
ok() { printf '    \033[32m%s\033[0m\n' "$1"; }
note() { printf '    %s\n' "$1"; }
die() { printf '\n\033[31merror: %s\033[0m\n' "$1" >&2; exit 1; }

# A listening socket on the port would otherwise surface as a bare OS error
# 10048 deep in serve.log, long after the installer claimed success.
is_port_taken() {
    command -v netstat >/dev/null 2>&1 || return 1
    netstat -ano 2>/dev/null | grep -qE "[:.]$1[[:space:]]+.*LISTEN"
}

# A busy port must not stop a one-command install: a developer Postgres on 5433
# is common, and so is a second Scribe. Walk up to the next free port instead.
pick_free_port() {
    local port="$1" limit=$((  $1 + 20 ))
    while [ "$port" -lt "$limit" ]; do
        is_port_taken "$port" || { echo "$port"; return; }
        port=$((port + 1))
    done
    echo "$1"
}

case "$(uname -s)" in
MINGW* | MSYS* | CYGWIN*) PLATFORM=windows ;;
Darwin) PLATFORM=macos ;;
*) PLATFORM=linux ;;
esac

echo "== Scribe installer =="

# ---------------------------------------------------------------------------
# Docker is required either way: natively it runs Postgres, in the container
# install it runs everything.
# ---------------------------------------------------------------------------
require_docker() {
    command -v docker >/dev/null 2>&1 ||
        die "Docker is not installed. Windows: winget install Docker.DockerDesktop — Linux: https://docs.docker.com/engine/install/"
    if ! docker version --format '{{.Server.Version}}' >/dev/null 2>&1; then
        if [ "$PLATFORM" = windows ] && [ -x "/c/Program Files/Docker/Docker/Docker Desktop.exe" ]; then
            note "Docker Desktop is not running — starting it (this takes a minute) ..."
            "/c/Program Files/Docker/Docker/Docker Desktop.exe" &
            for _ in $(seq 1 90); do
                docker version --format '{{.Server.Version}}' >/dev/null 2>&1 && break
                sleep 2
            done
        fi
        docker version --format '{{.Server.Version}}' >/dev/null 2>&1 ||
            die "The Docker daemon is not reachable. Start Docker, wait for it to settle, then re-run."
    fi
    ok "Docker $(docker version --format '{{.Server.Version}}')"
}

# ---------------------------------------------------------------------------
# Container install (Linux, macOS, or --docker)
# ---------------------------------------------------------------------------
install_with_docker() {
    step "Container install"
    require_docker
    command -v git >/dev/null 2>&1 || die "git is not installed."

    local dir="${INSTALL_DIR:-$HOME/scribe}"
    if [ -f "$dir/Dockerfile" ]; then
        note "using the existing checkout at $dir"
        git -C "$dir" pull --ff-only >/dev/null 2>&1 || true
    elif [ -f "./Dockerfile" ] && [ -f "./docker-compose.yml" ]; then
        dir="$PWD"
        note "using the checkout in the current directory"
    else
        step "Downloading the repository into $dir"
        git clone -q "https://github.com/$REPO.git" "$dir"
    fi
    cd "$dir"

    step "Building and starting the stack"
    note "The first build compiles the ML stack and downloads ~750 MB of models."
    note "Expect 15-30 minutes. Later runs take seconds."
    docker compose up -d --build

    local url="http://127.0.0.1:8443"
    if [ "$USE_TAILSCALE" = 1 ] && command -v tailscale >/dev/null 2>&1; then
        tailscale serve --bg "http://127.0.0.1:8443" >/dev/null 2>&1 || true
        local dns
        dns="$(tailscale status --json 2>/dev/null | sed -n 's/.*"DNSName": *"\([^"]*\)\..*/\1/p' | head -1)"
        if [ -n "$dns" ]; then
            url="https://$dns"
            grep -v '^[[:space:]]*SCRIBE_PUBLIC_BASE_URL[[:space:]]*=' .env 2>/dev/null > .env.tmp || true
            printf 'SCRIBE_PUBLIC_BASE_URL=%s\n' "$url" >> .env.tmp
            mv .env.tmp .env
            docker compose up -d >/dev/null
        fi
    fi

    for _ in $(seq 1 90); do
        curl -fsS "http://127.0.0.1:8443/health" >/dev/null 2>&1 && break
        sleep 2
    done
    local token
    token="$(docker compose exec -T scribe-serve sh -c "sed -n 's/^phone = \"\(.*\)\"/\1/p' /data/devices.toml" 2>/dev/null | tr -d '\r\n' || true)"
    summary "$url" "$token" "$dir" "docker compose logs -f scribe-serve scribe-worker" "docker compose down"
}

# ---------------------------------------------------------------------------
# Native Windows install from the release bundle
# ---------------------------------------------------------------------------
extract_zip() {
    # Git Bash ships GNU tar, which cannot read a zip; Windows ships bsdtar, which can.
    local zip="$1" dest="$2"
    if [ -x /c/Windows/System32/tar.exe ]; then
        /c/Windows/System32/tar.exe -xf "$zip" -C "$dest"
    elif command -v unzip >/dev/null 2>&1; then
        unzip -q -o "$zip" -d "$dest"
    else
        powershell -NoProfile -Command "Expand-Archive -Force -LiteralPath '$(cygpath -w "$zip")' -DestinationPath '$(cygpath -w "$dest")'"
    fi
}

ffmpeg_year() {
    # "ffmpeg version N-… Copyright (c) 2000-2024 the FFmpeg developers"
    "$1" -version 2>/dev/null | head -1 | sed -n 's/.*Copyright (c) [0-9]\{4\}-\([0-9]\{4\}\).*/\1/p'
}

ensure_ffmpeg() {
    local dir="$1" year=""
    if [ -x "$dir/ffmpeg.exe" ]; then
        year="$(ffmpeg_year "$dir/ffmpeg.exe")"
        if [ -n "$year" ] && [ "$year" -ge "$FFMPEG_MIN_YEAR" ]; then
            ok "ffmpeg $(("$year")) already installed here"
            return
        fi
    fi
    if command -v ffmpeg >/dev/null 2>&1; then
        year="$(ffmpeg_year ffmpeg)"
        if [ -n "$year" ] && [ "$year" -ge "$FFMPEG_MIN_YEAR" ]; then
            ok "using ffmpeg from PATH ($year)"
            return
        fi
        note "the ffmpeg on PATH is from $year — too old to read current phone recordings"
    fi
    note "downloading a current ffmpeg (~110 MB) ..."
    local tmp="$dir/.ffmpeg-tmp"
    rm -rf "$tmp"; mkdir -p "$tmp"
    curl -fsSL -o "$tmp/ffmpeg.zip" "$FFMPEG_URL" || die "could not download ffmpeg from $FFMPEG_URL"
    extract_zip "$tmp/ffmpeg.zip" "$tmp"
    local found
    found="$(find "$tmp" -name ffmpeg.exe | head -1)"
    [ -n "$found" ] || die "the ffmpeg archive did not contain ffmpeg.exe"
    # Beside scribe.exe on purpose: Windows resolves a bare command name from the
    # calling program's own directory before PATH, so this works for the services
    # too, without editing the machine's PATH.
    cp "$found" "$dir/ffmpeg.exe"
    rm -rf "$tmp"
    ok "ffmpeg $(ffmpeg_year "$dir/ffmpeg.exe") installed beside scribe.exe"
}

toml_set() {
    # Replace `key<spaces>= ...` on its own line. Anchored so `device` does not
    # also match `device_keys`.
    local file="$1" key="$2" value="$3"
    sed -i "s|^${key}\([[:space:]]*\)=.*|${key}\1= ${value}|" "$file"
}

install_native_windows() {
    local dir="${INSTALL_DIR:-}"
    if [ -z "$dir" ]; then
        if [ -x "./scribe.exe" ]; then dir="$PWD"; else dir="$HOME/scribe"; fi
    fi
    mkdir -p "$dir"
    dir="$(cd "$dir" && pwd)"

    step "Checking prerequisites"
    require_docker

    if [ ! -x "$dir/scribe.exe" ]; then
        step "Downloading the latest release bundle"
        local url
        url="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
            grep -o '"browser_download_url"[[:space:]]*:[[:space:]]*"[^"]*windows-x64[^"]*"' |
            head -1 | sed 's/.*"\(https[^"]*\)"/\1/')"
        [ -n "$url" ] || die "no windows-x64 asset on the latest release of $REPO"
        note "$(basename "$url")"
        curl -fL --progress-bar -o "$dir/bundle.zip" "$url"
        extract_zip "$dir/bundle.zip" "$dir"
        # The archive holds a scribe-server/ directory; lift its contents up.
        if [ -x "$dir/scribe-server/scribe.exe" ]; then
            cp -r "$dir/scribe-server/." "$dir/"
            rm -rf "$dir/scribe-server"
        fi
        rm -f "$dir/bundle.zip"
        [ -x "$dir/scribe.exe" ] || die "the bundle did not contain scribe.exe"
        ok "installed into $dir"
    else
        ok "using the bundle already in $dir"
    fi

    cd "$dir"
    ensure_ffmpeg "$dir"

    step "Configuring"
    local cfg="deploy/server.toml"
    [ -f "$cfg" ] || die "$cfg is missing. Is $dir really a Scribe bundle?"

    # An install that already has secrets keeps its ports, so a re-run cannot
    # bump them and point a configured phone at an empty database.
    if ! grep -q 'CHANGE-ME' "$cfg"; then
        local prev_api prev_db
        prev_api="$(grep -m1 '^bind' "$cfg" | grep -o '[0-9][0-9]*"' | tr -d '"')"
        prev_db="$(grep -m1 '^url' "$cfg" | grep -o 'localhost:[0-9][0-9]*' | grep -o '[0-9][0-9]*')"
        [ "$API_PORT_PINNED" = 0 ] && [ -n "$prev_api" ] && API_PORT="$prev_api"
        [ "$DB_PORT_PINNED" = 0 ] && [ -n "$prev_db" ] && DB_PORT="$prev_db"
        ok "keeping the ports of the existing install (API $API_PORT, database $DB_PORT)"
    else
        local want_api="$API_PORT" want_db="$DB_PORT"
        [ "$API_PORT_PINNED" = 0 ] && API_PORT="$(pick_free_port "$API_PORT")"
        [ "$DB_PORT_PINNED" = 0 ] && DB_PORT="$(pick_free_port "$DB_PORT")"
        [ "$API_PORT" = "$want_api" ] || note "port $want_api is busy, so the API uses $API_PORT"
        [ "$DB_PORT" = "$want_db" ] || note "port $want_db is busy, so the database uses $DB_PORT"
    fi

    # Secrets are generated once. Re-running must not rotate them: the phone
    # holds the device token, and the signing secret validates in-flight URLs.
    if grep -q 'CHANGE-ME' "$cfg"; then
        toml_set "$cfg" signing_secret "\"$(openssl rand -hex 32 2>/dev/null || head -c32 /dev/urandom | od -An -tx1 | tr -d ' \n')\""
        ok "generated the URL signing secret"
    else
        ok "keeping the existing signing secret"
    fi
    if [ ! -s deploy/devices.toml ]; then
        printf '# Per-device API keys — device_id = "api_key"\n# The app sends this as Authorization: Bearer <key>.\nphone = "%s"\n' \
            "$(openssl rand -hex 32 2>/dev/null || head -c32 /dev/urandom | od -An -tx1 | tr -d ' \n')" > deploy/devices.toml
        ok "generated a device token for the phone"
    else
        ok "keeping the existing device token"
    fi

    # The published bundle is CPU-only, and its config ships with a CUDA default.
    toml_set "$cfg" model "\"$ASR_MODEL\""
    if ls onnxruntime_providers_cuda.dll >/dev/null 2>&1; then
        toml_set "$cfg" device '"cuda"'
        ok "model $ASR_MODEL on the GPU"
    else
        toml_set "$cfg" device '"cpu"'
        ok "model $ASR_MODEL on the CPU"
    fi
    toml_set "$cfg" url "\"postgres://scribe:scribe@localhost:$DB_PORT/scribe?sslmode=disable\""
    export SCRIBE_DATABASE__URL="postgres://scribe:scribe@localhost:$DB_PORT/scribe?sslmode=disable"
    # Loopback only. `tailscale serve` is what exposes the API, so binding a
    # routable interface here would publish every transcript to the LAN.
    toml_set "$cfg" bind "\"127.0.0.1:$API_PORT\""


    step "Starting Postgres"
    POSTGRES_PORT="$DB_PORT" docker compose up -d postgres >/dev/null
    local cid
    cid="$(docker compose ps -q postgres | tr -d '\r\n')"
    for _ in $(seq 1 60); do
        [ "$(docker inspect --format '{{.State.Health.Status}}' "$cid" 2>/dev/null)" = healthy ] && break
        sleep 2
    done
    ok "pgvector is up on 127.0.0.1:$DB_PORT"

    step "Applying migrations"
    ./scribe.exe --config "$cfg" migrate 2>&1 | grep -v "slow threshold" | tail -1

    step "Downloading the speech models"
    note "About 750 MB the first time. Files that are already there are skipped."
    ./scribe.exe --config "$cfg" models pull || die "the model download did not finish — re-run this installer to continue it"

    local url="http://127.0.0.1:$API_PORT"
    if [ "$USE_TAILSCALE" = 1 ]; then
        step "Publishing on your tailnet"
        local ts=""
        command -v tailscale >/dev/null 2>&1 && ts="tailscale"
        [ -z "$ts" ] && [ -x "/c/Program Files/Tailscale/tailscale.exe" ] && ts="/c/Program Files/Tailscale/tailscale.exe"
        if [ -z "$ts" ]; then
            note "tailscale is not installed — skipping (the phone will not reach this server)"
        else
            local out
            out="$("$ts" serve --bg "http://127.0.0.1:$API_PORT" 2>&1 || true)"
            case "$out" in
            *"not enabled"*) note "Tailscale Serve is off for your tailnet: https://login.tailscale.com/f/serve" ;;
            esac
            local dns
            dns="$("$ts" status --json 2>/dev/null | sed -n 's/.*"DNSName": *"\([^"]*\)\..*/\1/p' | head -1)"
            if [ -n "$dns" ]; then
                url="https://$dns"
                ok "$url"
            fi
        fi
    fi
    toml_set "$cfg" public_base_url "\"$url\""

    ./scribe.exe --config "$cfg" doctor 2>&1 | sed 's/^/    /' || true

    local logcmd="tail -f $dir/serve.log $dir/worker.log"
    local stopcmd="pkill -f 'scribe.exe .* serve' ; pkill -f 'scribe.exe .* worker'"
    if [ "$DO_START" = 1 ]; then
        if [ "$AS_SERVICE" = 1 ]; then
            step "Installing the Windows services"
            powershell -NoProfile -ExecutionPolicy Bypass -File ./scripts/install-service.ps1 -DbPort "$DB_PORT" -ApiPort "$API_PORT" ||
                die "the service install failed — it needs an Administrator shell and NSSM (winget install NSSM.NSSM)"
            logcmd="tail -f $dir/logs/scribe-serve.log"
            stopcmd="nssm stop scribe-serve ; nssm stop scribe-worker"
        else
            step "Starting the API and the worker"
            ./scribe.exe --config "$cfg" serve > serve.log 2>&1 &
            ./scribe.exe --config "$cfg" worker > worker.log 2>&1 &
            for _ in $(seq 1 60); do
                curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1 && break
                sleep 1
            done
        fi
        if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then
            ok "API health: $(curl -s "http://127.0.0.1:$API_PORT/health")"
        else
            note "the API has not answered yet. The last line of serve.log:"
            tail -1 serve.log 2>/dev/null | sed 's/^/      /'
            note "full log: $dir/serve.log"
        fi
    fi

    summary "$url" "$(sed -n 's/^phone = "\(.*\)"/\1/p' deploy/devices.toml)" "$dir" "$logcmd" "$stopcmd"
    [ "$AS_SERVICE" = 1 ] || note "For an always-on server, re-run with --service (Administrator + NSSM)."
}

summary() {
    local url="$1" token="$2" dir="$3" logcmd="$4" stopcmd="$5"
    printf '\n\033[32m== Scribe is installed ==\033[0m\n'
    echo "  Server URL   (enter in the app): $url"
    [ -n "$token" ] && echo "  Device token (enter in the app): $token"
    echo "  Directory:  $dir"
    echo
    echo "  Logs: $logcmd"
    echo "  Stop: $stopcmd"
    case "$url" in
    http://127.0.0.1*)
        echo
        echo "  The phone cannot reach 127.0.0.1. Install Tailscale on this machine"
        echo "  and on the phone, then run this installer again."
        ;;
    esac
}

if [ "$PLATFORM" = windows ] && [ "$FORCE_DOCKER" = 0 ]; then
    install_native_windows
else
    install_with_docker
fi
