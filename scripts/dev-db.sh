#!/usr/bin/env bash
# scripts/dev-db.sh — start/stop the development pgvector container
#                     and (optionally) run `scribe migrate`.
#
# Usage:
#   ./scripts/dev-db.sh up        # Start the container and run migrations
#   ./scripts/dev-db.sh start     # Start only (skip migrate)
#   ./scripts/dev-db.sh stop      # Stop the container (data preserved in volume)
#   ./scripts/dev-db.sh down      # Stop + remove container (data preserved)
#   ./scripts/dev-db.sh reset     # Stop + destroy data volume (destructive!)
#   ./scripts/dev-db.sh status    # Show container status
#
# The script is idempotent: running `up` when the DB is already up is safe.
#
# Requirements: docker (or podman aliased as docker), cargo + scribe binary
#               (for `up` which runs scribe migrate).

set -euo pipefail

CONTAINER_NAME="scribe-postgres-dev"
IMAGE="pgvector/pgvector:pg17"
DB_NAME="scribe"
DB_USER="scribe"
DB_PASS="scribe"
# Use 5433 to avoid colliding with a local Postgres on 5432.
HOST_PORT="${SCRIBE_DEV_DB_PORT:-5433}"
DATABASE_URL="postgres://${DB_USER}:${DB_PASS}@127.0.0.1:${HOST_PORT}/${DB_NAME}?sslmode=disable"

# ──────────────────────────────────────────────
# Helpers
# ──────────────────────────────────────────────

log()  { echo "[dev-db] $*"; }
die()  { echo "[dev-db] ERROR: $*" >&2; exit 1; }

container_running() {
    docker inspect -f '{{.State.Running}}' "${CONTAINER_NAME}" 2>/dev/null | grep -q true
}

container_exists() {
    docker inspect "${CONTAINER_NAME}" >/dev/null 2>&1
}

wait_for_postgres() {
    log "Waiting for Postgres to be ready..."
    local attempts=30
    until docker exec "${CONTAINER_NAME}" pg_isready -U "${DB_USER}" -d "${DB_NAME}" -q; do
        attempts=$((attempts - 1))
        [ "${attempts}" -le 0 ] && die "Postgres did not become ready in time."
        sleep 1
    done
    log "Postgres is ready."
}

cmd_start() {
    if container_running; then
        log "Container '${CONTAINER_NAME}' is already running on port ${HOST_PORT}."
        return 0
    fi

    if container_exists; then
        log "Starting existing container '${CONTAINER_NAME}'..."
        docker start "${CONTAINER_NAME}"
    else
        log "Creating container '${CONTAINER_NAME}' from ${IMAGE}..."
        docker run -d \
            --name "${CONTAINER_NAME}" \
            -e POSTGRES_DB="${DB_NAME}" \
            -e POSTGRES_USER="${DB_USER}" \
            -e POSTGRES_PASSWORD="${DB_PASS}" \
            -p "${HOST_PORT}:5432" \
            "${IMAGE}"
    fi

    wait_for_postgres
    log "DB available at: ${DATABASE_URL}"
}

cmd_migrate() {
    log "Running scribe migrate..."
    # Look for the binary in PATH, Cargo release dir, or debug dir.
    local binary
    if command -v scribe >/dev/null 2>&1; then
        binary="scribe"
    elif [ -f "target/release/scribe" ]; then
        binary="./target/release/scribe"
    elif [ -f "target/debug/scribe" ]; then
        binary="./target/debug/scribe"
    else
        log "scribe binary not found; skipping migrate."
        log "Build with: cargo build -p scribe-cli --no-default-features"
        return 0
    fi

    SCRIBE_DATABASE__URL="${DATABASE_URL}" \
        "${binary}" migrate
    log "Migrations complete."
}

cmd_stop() {
    if container_running; then
        log "Stopping container '${CONTAINER_NAME}'..."
        docker stop "${CONTAINER_NAME}"
    else
        log "Container '${CONTAINER_NAME}' is not running."
    fi
}

cmd_down() {
    cmd_stop || true
    if container_exists; then
        log "Removing container '${CONTAINER_NAME}'..."
        docker rm "${CONTAINER_NAME}"
    fi
}

cmd_reset() {
    log "WARNING: this will destroy all data in the dev database."
    read -r -p "Type 'yes' to confirm: " confirm
    [ "${confirm}" = "yes" ] || { log "Aborted."; exit 0; }
    cmd_down || true
    # Remove the named volume created by docker run (if any).
    docker volume rm "scribe_pgdata_dev" 2>/dev/null || true
    log "Dev database reset complete."
}

cmd_status() {
    if container_running; then
        log "Container '${CONTAINER_NAME}' is RUNNING on host port ${HOST_PORT}."
    elif container_exists; then
        log "Container '${CONTAINER_NAME}' EXISTS but is stopped."
    else
        log "Container '${CONTAINER_NAME}' does not exist."
    fi
}

# ──────────────────────────────────────────────
# Dispatch
# ──────────────────────────────────────────────

ACTION="${1:-up}"
case "${ACTION}" in
    up)
        cmd_start
        cmd_migrate
        ;;
    start)
        cmd_start
        ;;
    migrate)
        cmd_migrate
        ;;
    stop)
        cmd_stop
        ;;
    down)
        cmd_down
        ;;
    reset)
        cmd_reset
        ;;
    status)
        cmd_status
        ;;
    *)
        echo "Usage: $0 {up|start|migrate|stop|down|reset|status}" >&2
        exit 1
        ;;
esac
