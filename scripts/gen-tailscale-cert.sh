#!/usr/bin/env bash
# scripts/gen-tailscale-cert.sh — TLS certificate provisioning for Scribe
#
# Background (design §5)
# ──────────────────────
# Scribe's storage node runs as a plain HTTP server on localhost (127.0.0.1:8443).
# `tailscale serve` terminates TLS on the tailnet using a publicly-trusted Let's
# Encrypt certificate for your *.ts.net hostname, then reverse-proxies to scribe.
# The phone trusts it out of the box — no custom CA to install.
#
# Two modes
# ─────────
# MODE 1 — tailscale serve (RECOMMENDED):
#   tailscale serve terminates TLS and forwards plain HTTP to scribe.
#   Pros: no cert files to manage; auto-renewing; one command.
#   After setup the app points to https://scribe.<tailnet>.ts.net
#
# MODE 2 — tailscale cert + rustls in scribe:
#   Downloads the cert/key files so scribe can terminate TLS itself.
#   Useful if you want to skip the tailscale proxy hop.
#
# Usage:
#   ./scripts/gen-tailscale-cert.sh serve   # configure tailscale serve (MODE 1)
#   ./scripts/gen-tailscale-cert.sh cert    # download cert files (MODE 2)
#   ./scripts/gen-tailscale-cert.sh status  # show current tailscale serve config
#   ./scripts/gen-tailscale-cert.sh off     # disable tailscale serve

set -euo pipefail

# ─── Configuration ──────────────────────────────────────────────────────────
# Override these with environment variables if needed.

SCRIBE_BIND_PORT="${SCRIBE_BIND_PORT:-8443}"       # scribe serve listens here
CERT_DIR="${SCRIBE_CERT_DIR:-/etc/scribe/tls}"    # where cert files land (MODE 2)

# ─── Helpers ────────────────────────────────────────────────────────────────

log() { echo "[tailscale-cert] $*"; }
die() { echo "[tailscale-cert] ERROR: $*" >&2; exit 1; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' not found. $2"
}

get_tailnet_fqdn() {
    # `tailscale status --self` prints the MagicDNS name if MagicDNS is enabled.
    local fqdn
    fqdn=$(tailscale status --json 2>/dev/null | python3 -c \
        "import sys,json; d=json.load(sys.stdin); print(d['Self']['DNSName'].rstrip('.'))" 2>/dev/null) || true
    if [ -z "${fqdn}" ]; then
        die "Could not determine tailnet FQDN. Is Tailscale connected and MagicDNS enabled?"
    fi
    echo "${fqdn}"
}

# ─── MODE 1: tailscale serve ─────────────────────────────────────────────────
#
# Sets up `tailscale serve` as a TLS-terminating reverse proxy in front of
# `scribe serve` on localhost:SCRIBE_BIND_PORT.
#
# After this command:
#   https://<fqdn>/   → http://127.0.0.1:SCRIBE_BIND_PORT/
#
# tailscale serve auto-renews the Let's Encrypt cert; no cron needed.
# This is the recommended path (design §5).
#
cmd_serve() {
    require_cmd tailscale "Install Tailscale: https://tailscale.com/download"
    local fqdn
    fqdn=$(get_tailnet_fqdn)

    log "Configuring tailscale serve → https://${fqdn}/ → http://127.0.0.1:${SCRIBE_BIND_PORT}/"
    log "This requires 'tailscale serve' which provisions a Let's Encrypt cert automatically."

    # Map HTTPS 443 on the tailnet to scribe's HTTP localhost port.
    tailscale serve --bg "http://localhost:${SCRIBE_BIND_PORT}"

    log ""
    log "Done. Configure the Scribe app to use:"
    log "  Server URL: https://${fqdn}"
    log ""
    log "To verify:  curl -s https://${fqdn}/health"
    log "To disable: $0 off"
}

# ─── MODE 2: tailscale cert ──────────────────────────────────────────────────
#
# Downloads the *.ts.net TLS certificate and private key so scribe can terminate
# TLS itself (with rustls + axum-server). The cert is renewed by re-running this
# script (add a cron/timer for that), or use MODE 1 which auto-renews.
#
# Files written:
#   $CERT_DIR/<fqdn>.crt   — full-chain PEM certificate
#   $CERT_DIR/<fqdn>.key   — private key (mode 0600)
#
# Point scribe at these by setting (in config or env):
#   SCRIBE_API__TLS_CERT=/etc/scribe/tls/<fqdn>.crt
#   SCRIBE_API__TLS_KEY=/etc/scribe/tls/<fqdn>.key
# (This config key is not yet implemented in the current codebase; scribe-api
#  currently delegates TLS to tailscale serve. Add axum-server TLS in Phase 5.)
#
cmd_cert() {
    require_cmd tailscale "Install Tailscale: https://tailscale.com/download"
    local fqdn
    fqdn=$(get_tailnet_fqdn)

    log "Provisioning cert for: ${fqdn}"
    log "Cert directory: ${CERT_DIR}"

    sudo mkdir -p "${CERT_DIR}"
    sudo tailscale cert \
        --cert-file "${CERT_DIR}/${fqdn}.crt" \
        --key-file  "${CERT_DIR}/${fqdn}.key" \
        "${fqdn}"
    sudo chmod 640  "${CERT_DIR}/${fqdn}.crt"
    sudo chmod 600  "${CERT_DIR}/${fqdn}.key"
    sudo chown root:scribe "${CERT_DIR}/${fqdn}.crt" "${CERT_DIR}/${fqdn}.key"

    log ""
    log "Certificate files:"
    log "  Cert: ${CERT_DIR}/${fqdn}.crt"
    log "  Key:  ${CERT_DIR}/${fqdn}.key"
    log ""
    log "tailscale cert auto-renews when you re-run this script."
    log "Add a systemd timer or cron job to keep it fresh:"
    log "  0 3 * * * root $(readlink -f "$0") cert"
}

cmd_status() {
    require_cmd tailscale ""
    log "tailscale serve status:"
    tailscale serve status 2>/dev/null || log "(no active serve config)"
    log ""
    log "tailscale status (connection):"
    tailscale status 2>/dev/null || true
}

cmd_off() {
    require_cmd tailscale ""
    log "Disabling tailscale serve..."
    tailscale serve off 2>/dev/null || true
    log "Done."
}

# ─── Dispatch ────────────────────────────────────────────────────────────────

ACTION="${1:-serve}"
case "${ACTION}" in
    serve)   cmd_serve  ;;
    cert)    cmd_cert   ;;
    status)  cmd_status ;;
    off)     cmd_off    ;;
    *)
        echo "Usage: $0 {serve|cert|status|off}" >&2
        exit 1
        ;;
esac
