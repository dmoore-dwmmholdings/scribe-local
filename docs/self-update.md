# Backend self-update

Scribe's backend can update itself: you upload a **signed package** (the new
`scribe` binary) to an authenticated endpoint, and the server verifies it,
installs it atomically, runs the new binary's migrations, and restarts into it —
all from the phone app or `curl`. Implemented in the `scribe-update` crate.

> This is, by design, **remote code execution on the server**. It is **disabled
> by default** and gated by *both* an admin token *and* an ed25519 signature, so
> a leaked token alone cannot install a forged binary.

## Trust model

| Layer | Protects against | Mechanism |
|---|---|---|
| Admin token | Random callers | `Authorization: Bearer <update token>` (constant-time check), distinct from device keys |
| ed25519 signature | A stolen token pushing a malicious binary | Manifest must be signed by the operator's private key; server holds only the public key |
| sha256 | Corruption / tampering after signing | Manifest binds the binary by hash; signature covers the manifest |
| Version / target policy | Accidental downgrade or wrong-arch install | Rejected unless `allow_downgrade` / `allow_target_mismatch` |
| `.old` backup | A bad-but-valid release | Atomic swap keeps the previous binary; one-command rollback |

## Package format

A `.tar.gz` containing:

```
manifest.json   # {name, version, target, sha256, created_at, notes}
manifest.sig    # hex ed25519 signature over the exact bytes of manifest.json
scribe          # the new binary (scribe.exe on Windows)
```

## One-time setup

1. **Generate a signing keypair** (keep the private key off the server):

   ```bash
   scribe update keygen --out scribe-release.key
   # writes scribe-release.key (private, chmod 600) and scribe-release.key.pub
   ```

2. **Enable updates on the storage node** — in `storage.toml` (or via env):

   ```toml
   [update]
   enabled    = true
   token      = "…"            # SCRIBE_UPDATE__TOKEN in prod; openssl rand -hex 32
   public_key = "…"            # contents of scribe-release.key.pub
   restart    = "self-exec"    # or "supervisor" with systemd/launchd
   staging_dir = "/var/lib/scribe/updates"
   ```

   Restart `scribe serve` once so it picks up the config.

## Cutting a release

```bash
# 1. Build the new binary (real-ML build needs the platform ONNX runtime;
#    on macOS/Linux the default features work — see docs/deployment.md).
cargo build --release -p scribe-cli

# 2. Package + sign it for the server's platform.
scribe update sign \
  --key scribe-release.key \
  --binary target/release/scribe \
  --version 0.2.0 \
  --notes "faster diarization" \
  --out scribe-0.2.0.tar.gz

# 3. (optional) Dry-run verify against the server's configured public key.
scribe --config storage.toml update verify scribe-0.2.0.tar.gz
```

`sign` stamps the host's target triple by default; pass `--target` to
cross-package (e.g. `--target aarch64-apple-darwin`).

## Installing

### From the phone (the intended path)

Settings → **Backend update**: set the **Update token**, pick the `.tar.gz`,
tap **Install**. The app uploads, shows "restarting…", polls `/health`, and
reports the new version. A **Roll back** button appears when a backup exists.

### Over HTTP

```bash
curl -X POST https://scribe.<tailnet>.ts.net/admin/update \
  -H "Authorization: Bearer $UPDATE_TOKEN" \
  --data-binary @scribe-0.2.0.tar.gz
# → {"from_version":"0.1.0","to_version":"0.2.0","restarting":true,"restart_in_ms":750}
```

The server verifies → stages → runs `scribe --version` and `scribe migrate` with
the *new* binary → atomically swaps it in (backing up the old one) → answers the
request → restarts after `restart_delay_ms`.

### Locally on the box

```bash
scribe --config storage.toml update apply scribe-0.2.0.tar.gz   # installs, no restart
scribe --config storage.toml update info                        # version + rollback availability
```

`update apply` does NOT restart a running server (there isn't one in the CLI
process) — restart `scribe serve` yourself, or use the HTTP endpoint which does.

## Endpoints

| Method | Path | Auth | Body / result |
|---|---|---|---|
| `GET` | `/admin/info` | update token | `{version, target, update_enabled, restart_mode, has_backup}` |
| `POST` | `/admin/update` | update token | body = `.tar.gz`; → `{from_version, to_version, restarting, restart_in_ms}` |
| `POST` | `/admin/update/rollback` | update token | restores `.old`; → `{restored_version, restarting, …}` |

When `[update].enabled = false`, all three return **404** (the feature is invisible).

## Restart modes

- **`self-exec`** (default) — after the swap the process replaces its own image
  (`execv`) with the new binary, reusing the same arguments. No service manager
  required; works on macOS and Linux. On the brief restart the client just sees
  `/health` drop and recover.
- **`supervisor`** — the process exits cleanly and systemd (`Restart=always`) or
  launchd (`KeepAlive`) relaunches it. Preferred when you already run under one,
  since the same mechanism also recovers from crashes. See
  `deploy/systemd/` and `deploy/launchd/`.

## Rollback

Every install backs up the prior binary as `<binary>.old`. To revert:

```bash
curl -X POST https://scribe.<tailnet>.ts.net/admin/update/rollback \
  -H "Authorization: Bearer $UPDATE_TOKEN"
# or locally:
scribe --config storage.toml update rollback
```

Rollback is itself reversible (it swaps current ↔ backup). Note: migrations are
forward-only — if a release added an incompatible migration, rolling the binary
back may require a DB restore from `pg_dump` (design §12).

## Operational notes

- The **worker** binary updates the same way (`scribe update apply` on the
  compute node, or point a second `[update]`-enabled `serve` there). The worker
  itself has no HTTP endpoint.
- Keep the **private signing key offline**. Rotating it = generate a new keypair,
  update `[update].public_key`, restart `serve`.
- The staging dir and the binary's directory must be writable by the `scribe`
  user. With `self-exec`, the binary file is renamed (not overwritten), which is
  permitted even while running on Unix.
