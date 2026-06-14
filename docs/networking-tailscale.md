# Tailscale Networking

How Scribe uses Tailscale to be reachable from anywhere without exposing the
server to the public internet (design §5).

---

## Why Tailscale

Tailscale builds a WireGuard mesh (a "tailnet") between all your devices — phone,
storage node, processing node — so they can talk to each other using stable
private hostnames from any network: home Wi-Fi, LTE, hotel, coffee shop. The
server is never exposed to the public internet. There are no open inbound ports,
no dynamic DNS, no port forwarding.

Comparison for this use case:

| Option | Mobile / CGNAT | Large uploads | Public exposure | Verdict |
|---|---|---|---|---|
| **Tailscale** | Yes (DERP relay fallback) | No cap | None | Chosen |
| Cloudflare Tunnel | Yes | 100 MB cap (free tier) | Public hostname | Breaks large uploads |
| Plain WireGuard | Often no (CGNAT) | No cap | None (but needs port-forward) | More work, worse mobile UX |
| Port-forward + DDNS | Needs non-CGNAT ISP | No cap | Exposes server publicly | Largest attack surface |

---

## Tailscale Funnel — why we don't use it

**Tailscale Funnel** publishes a service to the _public_ internet (any client
can reach it). We do _not_ want that: the phone is on the tailnet, so plain
in-tailnet access is sufficient and keeps exposure at zero.

---

## Setup

### 1. Install Tailscale on all three devices

```bash
# Linux (storage node and processing node):
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up

# Windows (processing node):
winget install Tailscale.Tailscale
# Then sign in from the Tailscale system tray icon.

# Phone: install the Tailscale app (App Store / Google Play) and sign in.
```

All devices should appear in the Tailscale admin console at
<https://login.tailscale.com/admin/machines>.

### 2. Enable MagicDNS

In the Tailscale admin console → DNS → enable **MagicDNS**. This lets the
phone resolve `scribe.<tailnet>.ts.net` to the storage node's tailnet IP
automatically, from any network.

Without MagicDNS you can hard-code the `100.x.y.z` tailnet IP in the app
settings, but it breaks if the IP changes.

### 3. Configure TLS with `tailscale serve` (recommended)

`tailscale serve` terminates TLS using a publicly-trusted Let's Encrypt
certificate for your `*.ts.net` name and reverse-proxies to the scribe HTTP
server on localhost. The phone trusts it without any custom CA.

```bash
# On the storage node — run once; survives reboots:
tailscale serve --bg "http://localhost:8443"

# Verify:
tailscale serve status
```

`scribe serve` binds `127.0.0.1:8443` (plain HTTP). Tailscale terminates HTTPS
on `0.0.0.0:443` on the tailnet interface and proxies to localhost. The
certificate auto-renews via DNS-01 challenge with no cron job needed.

Configure the app with:
```
Server URL: https://scribe.<your-tailnet>.ts.net
```

### 4. Alternative: terminate TLS in scribe with `tailscale cert`

If you prefer to terminate TLS inside the scribe process itself (no proxy hop):

```bash
./scripts/gen-tailscale-cert.sh cert
# Writes: /etc/scribe/tls/<fqdn>.crt
#         /etc/scribe/tls/<fqdn>.key
```

Then point scribe at these files (requires a future `api.tls_cert` / `api.tls_key`
config option — not yet in the current build; use `tailscale serve` for now).

The `scripts/gen-tailscale-cert.sh` script documents both modes in detail.

---

## MagicDNS names used in configs

| Device role | MagicDNS hostname | Used in |
|---|---|---|
| Storage node (API) | `scribe.<tailnet>.ts.net` | App `Server URL`; worker's `api.public_base_url` |
| Processing node | `scribe-gpu.<tailnet>.ts.net` | Storage node's `llm.ollama_url` (optional) |

Replace `<tailnet>` with your actual tailnet name (visible in the admin console,
e.g. `example` for `scribe.example.ts.net`).

---

## ACLs (access control)

Tailscale's default policy allows all tailnet devices to reach each other on
all ports. For a tighter setup, use ACL tags in the admin console:

```json
// Tailscale ACL — paste into the admin console (Policy tab)
{
  "tagOwners": {
    "tag:scribe-server":  ["autogroup:owner"],
    "tag:scribe-client":  ["autogroup:owner"]
  },
  "acls": [
    // Only scribe-client devices can reach port 443 on the storage node.
    {
      "action": "accept",
      "src":    ["tag:scribe-client"],
      "dst":    ["tag:scribe-server:443"]
    },
    // Processing node can reach Postgres (5432) and the API (443) on storage.
    {
      "action": "accept",
      "src":    ["tag:scribe-server"],  // processing node also has this tag
      "dst":    ["tag:scribe-server:443", "tag:scribe-server:5432"]
    }
  ]
}
```

Apply the `tag:scribe-server` tag to both server machines and `tag:scribe-client`
to the phone in the admin console, then the phone cannot accidentally reach
Postgres, and the processing node cannot reach Ollama on the phone.

---

## Relay reality

Roughly 5% of Tailscale connections fall back to DERP relays when peers cannot
establish a direct connection (carrier-grade NAT, strict firewalls). DERP
provides ~35 Mbps and ~20–50 ms additional latency. For compressed AAC audio
uploads of tens of MB, this is a non-issue.

---

## The processing node and Postgres

The processing node connects to Postgres on the storage node over the same
tailnet. No additional networking is needed. Configure the worker's DB URL to
use the MagicDNS name:

```toml
# deploy/compute.toml
[database]
url = "postgres://scribe:PASSWORD@scribe.<your-tailnet>.ts.net/scribe?sslmode=disable"
```

For TLS on the DB connection (recommended): install the tailnet cert on the
storage node and enable `ssl = on` in `postgresql.conf`, then change to
`sslmode=require`.
