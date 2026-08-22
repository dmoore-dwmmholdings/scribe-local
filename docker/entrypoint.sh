#!/bin/sh
# Scribe container entrypoint.
#
# Everything here exists so that `docker compose up` needs no prior setup:
#
#   1. the config file is supplied automatically, so a compose command reads as
#      just `serve` / `worker` / `models pull`;
#   2. the two values a real deployment must not ship with a default — the blob
#      URL signing secret and the phone's device token — are generated on first
#      start into the data volume, where every role reads the same copy;
#   3. the role that owns the schema applies migrations before it starts.
#
# It then execs `scribe`, so the container behaves exactly like the native CLI.
set -eu

CONFIG="${SCRIBE_CONFIG:-/etc/scribe/scribe.toml}"
DATA_DIR="${SCRIBE_DATA_DIR:-/data}"
SECRET_FILE="$DATA_DIR/signing-secret"
DEVICES_FILE="$DATA_DIR/devices.toml"

mkdir -p "$DATA_DIR" "$DATA_DIR/blobs"

# 32 random bytes as lowercase hex, using only coreutils.
random_hex() {
    od -An -tx1 -N32 /dev/urandom | tr -d ' \n'
}

# --- blob signing secret ----------------------------------------------------
# Signs the short-lived audio-pull URLs. An explicit env value always wins, so
# an operator can keep the secret outside the volume.
if [ -z "${SCRIBE_STORAGE__SIGNING_SECRET:-}" ]; then
    if [ ! -s "$SECRET_FILE" ]; then
        random_hex > "$SECRET_FILE"
        chmod 600 "$SECRET_FILE"
        echo "scribe: generated a blob signing secret in $SECRET_FILE"
    fi
    SCRIBE_STORAGE__SIGNING_SECRET="$(cat "$SECRET_FILE")"
    export SCRIBE_STORAGE__SIGNING_SECRET
fi

# --- device token -----------------------------------------------------------
# The API is closed by default: without a token a tailnet neighbour could read
# every transcript. One key is minted for the phone on first start; add more by
# appending lines to this file and restarting the API.
if [ ! -s "$DEVICES_FILE" ]; then
    cat > "$DEVICES_FILE" <<TOML
# Per-device API keys — format: device_id = "api_key"
# The app sends this as Authorization: Bearer <key>.
# Add a device by appending a line here, then restart scribe-serve.
phone = "$(random_hex)"
TOML
    chmod 600 "$DEVICES_FILE"
    echo "scribe: generated a device token for the mobile app in $DEVICES_FILE"
fi

# --- role-specific startup --------------------------------------------------
case "${1:-}" in
serve)
    # Printed on every start, not only the first: this is the one value a new
    # user must copy into the phone, and making them dig it out of a container's
    # oldest log line would defeat the point of a one-command install.
    echo "scribe: device token for the mobile app -> $(sed -n 's/^phone = "\(.*\)"/\1/p' "$DEVICES_FILE")"
    ;;
esac

# The schema is owned by whichever service compose marks as the migrator, so
# serve and worker never race to apply the same migration on a cold start.
if [ "${SCRIBE_MIGRATE_ON_START:-0}" = "1" ]; then
    echo "scribe: applying database migrations"
    scribe --config "$CONFIG" migrate
fi

# Supply --config unless the caller passed one explicitly.
case " $* " in
*" --config "*) exec scribe "$@" ;;
*) exec scribe --config "$CONFIG" "$@" ;;
esac
