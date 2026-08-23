#!/usr/bin/env bash
# Collect everything needed to explain why a recording is not being processed.
#
#   curl -fsSL https://raw.githubusercontent.com/dmoore-dwmmholdings/scribe-local/master/scripts/diagnose.sh | bash
#
# Run it on the SERVER. It prints one report, safe to paste into an issue or a
# chat: the device token, the signing secret, and the update token are redacted.
#
#   --dir PATH   the install directory (default: the current one, else ~/scribe)
set -uo pipefail

DIR=""
while [ $# -gt 0 ]; do
    case "$1" in
    --dir) DIR="$2"; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

if [ -z "$DIR" ]; then
    if [ -f "./deploy/server.toml" ]; then DIR="$PWD"; else DIR="$HOME/scribe"; fi
fi
cd "$DIR" 2>/dev/null || { echo "no install at $DIR — pass --dir PATH"; exit 1; }

CFG="deploy/server.toml"
[ -f "$CFG" ] || { echo "no $CFG in $DIR — is this a Scribe install?"; exit 1; }

API_PORT="$(grep -m1 '^bind' "$CFG" | grep -o '[0-9][0-9]*"' | tr -d '"')"
DB_PORT="$(grep -m1 '^url' "$CFG" | grep -o 'localhost:[0-9][0-9]*' | grep -o '[0-9][0-9]*')"
API_PORT="${API_PORT:-8443}"
DB_PORT="${DB_PORT:-5433}"

hdr() { printf '\n===== %s =====\n' "$1"; }
psql_q() {
    docker exec -i "$(docker compose ps -q postgres 2>/dev/null | tr -d '\r\n')" \
        psql -U scribe -d scribe -P pager=off -c "$1" 2>&1
}

echo "Scribe diagnostic — $(date -u '+%Y-%m-%d %H:%M:%SZ')"
echo "install: $DIR   api port: $API_PORT   db port: $DB_PORT"

hdr "config (secrets redacted)"
sed -E 's/^(signing_secret|token|api_key)([[:space:]]*)=.*/\1\2= <redacted>/' "$CFG" |
    grep -vE '^[[:space:]]*#' | grep -vE '^[[:space:]]*$'

hdr "processes"
if command -v powershell >/dev/null 2>&1; then
    powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='scribe.exe'\" | Select-Object ProcessId,@{n='Args';e={\$_.CommandLine -replace '.*scribe\.exe','scribe.exe'}} | Format-Table -Auto | Out-String -Width 200" 2>/dev/null
    powershell -NoProfile -Command "Get-Service scribe-* -ErrorAction SilentlyContinue | Select-Object Name,Status | Format-Table -Auto | Out-String" 2>/dev/null
else
    ps aux 2>/dev/null | grep -E '[s]cribe (serve|worker)' || echo "no scribe serve/worker process found"
fi

hdr "docker"
docker compose ps 2>&1 | head -10

hdr "api health"
curl -fsS -m 10 "http://127.0.0.1:$API_PORT/health" 2>&1 || echo "API did not answer on 127.0.0.1:$API_PORT"

hdr "doctor"
./scribe.exe --config "$CFG" doctor 2>&1 | grep -vE 'slow threshold'

hdr "recordings (most recent 10)"
psql_q "select id, left(coalesce(title,''),28) as title, status, duration_ms, created_at
        from recordings order by created_at desc limit 10;"

hdr "segments of the most recent recording"
psql_q "select seq, bytes, created_at from segments
        where recording_id = (select id from recordings order by created_at desc limit 1)
        order by seq;"

hdr "jobs (most recent 20)"
psql_q "select id, kind, state, attempts, priority,
               locked_by, locked_at, run_after,
               left(coalesce(error,''),160) as error
        from jobs order by id desc limit 20;"

hdr "jobs blocked or waiting"
psql_q "select kind, state, count(*), min(run_after) as earliest_run_after
        from jobs where state in ('queued','running','failed')
        group by kind, state order by kind;"

hdr "processing schedule"
psql_q "select value from app_settings where key = 'processing_schedule';"
echo "(no row above = defaults = schedule disabled = always allowed to run)"

hdr "worker log (last 60 lines)"
if [ -f worker.log ]; then tail -60 worker.log
elif [ -f logs/scribe-worker.log ]; then tail -60 logs/scribe-worker.log
else echo "no worker log found in $DIR (looked for worker.log and logs/scribe-worker.log)"; fi

hdr "serve log (last 30 lines)"
if [ -f serve.log ]; then tail -30 serve.log
elif [ -f logs/scribe-serve.log ]; then tail -30 logs/scribe-serve.log
else echo "no serve log found in $DIR"; fi

hdr "end of report"
