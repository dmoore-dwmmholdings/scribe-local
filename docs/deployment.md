# Deployment Guide

End-to-end instructions for bringing up a production Scribe installation:
one **storage node** (always-on, low power, Postgres + API) and one
**processing node** (GPU desktop, ML pipeline + Ollama).

---

## Table of contents

1. [Prerequisites](#1-prerequisites)
2. [Stub vs real (ONNX) build modes](#2-stub-vs-real-onnx-build-modes)
3. [Storage node setup](#3-storage-node-setup)
4. [Processing node setup](#4-processing-node-setup)
5. [Tailscale networking](#5-tailscale-networking)
6. [First run and smoke-test](#6-first-run-and-smoke-test)
7. [Installing systemd services](#7-installing-systemd-services)
8. [Backups](#8-backups)
9. [Maintenance tasks](#9-maintenance-tasks)

---

## 1. Prerequisites

### All nodes

| Requirement | Notes |
|---|---|
| Tailscale | Free tier sufficient. Install on storage node, processing node, and phone. |
| Rust toolchain (MSVC on Windows, GNU on Linux/Mac) | `rustup` recommended; MSVC required for the real-ML build on Windows (see §2). |
| ffmpeg | Used by the `transcode` stage to decode AAC → 16 kHz mono WAV. Must be on `$PATH`. |

### Storage node only

| Requirement | Notes |
|---|---|
| Docker (or Podman) | For the pgvector/pgvector:pg17 container (or install Postgres natively with the pgvector extension). |
| Disk space | Audio blobs at ~32 kbps AAC ≈ ~14 MB/hour; years of meetings fit on a small SSD. |

### Processing node only

| Requirement | Notes |
|---|---|
| Ollama | `curl -fsSL https://ollama.com/install.sh | sh` (Linux). |
| NVIDIA GPU (recommended) | RTX 3060 12 GB minimum; 4070/4080/4090 comfortable. CPU-only works, but is slower. |
| CUDA toolkit + nvidia-container-toolkit | Only needed for GPU acceleration. |
| ONNX Runtime | Pulled automatically by `sherpa-onnx` when you build with default features. |

---

## 2. Stub vs real (ONNX) build modes

The binary has two build modes, controlled by Cargo features:

### Real-ML build (default)

```bash
cargo build --release -p scribe-cli
```

Enables `scribe-asr/onnx` (sherpa-onnx: VAD + diarization + ASR) and
`scribe-llm/local-embed` (fastembed-rs in-process embeddings). Requires:

- A C/C++ compiler that can link native libs (`cc`, MSVC, or clang).
- The ONNX Runtime native library, which `sherpa-onnx` downloads and links
  automatically via its build script.
- **Windows caveat:** the prebuilt ONNX Runtime library from Microsoft targets
  the **MSVC** ABI. If you are on the **GNU** toolchain (e.g. `x86_64-pc-windows-gnu`),
  the build will fail at the linking step. Switch to `x86_64-pc-windows-msvc`
  (install VS Build Tools) **or** use the stub build below.
- On Linux with CUDA: ensure `libcuda.so` is discoverable; set
  `CUDA_TOOLKIT_ROOT_DIR` if the build script cannot find it.

### Stub build (no ONNX runtime)

```bash
cargo build --release -p scribe-cli --no-default-features
```

Disables all native ML dependencies. The worker still runs the full pipeline
but produces deterministic placeholder outputs (no real transcription). Use
this for:
- Development machines without a GPU or ONNX toolkit.
- CI/CD pipelines.
- Windows GNU toolchain.
- Integration testing without real model files.

In the stub build, `scribe doctor` will report that real models are absent but
the pipeline still runs end-to-end.

---

## 3. Storage node setup

### 3a. Start Postgres

```bash
# Using docker-compose (recommended for dev):
docker compose up -d postgres

# Or start the dev container directly (port 5433 to avoid conflicts):
./scripts/dev-db.sh up   # Linux/Mac
.\scripts\dev-db.ps1 up  # Windows PowerShell
```

The docker-compose configuration uses `pgvector/pgvector:pg17` which includes
the `vector` extension. Native Postgres installs need:
```sql
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pgcrypto;
```

### 3b. Run migrations

```bash
# Set the DB URL and run migrations:
SCRIBE_DATABASE__URL="postgres://scribe:scribe@localhost/scribe?sslmode=disable" \
    scribe migrate
```

Or set `url` in a config file and pass `--config`:
```bash
scribe migrate --config /etc/scribe/storage.toml
```

Migrations are idempotent: re-running them on an up-to-date database is safe.

### 3c. Configure the storage node

Copy and edit the example config:
```bash
sudo mkdir -p /etc/scribe /var/lib/scribe/blobs
sudo cp deploy/storage.toml /etc/scribe/storage.toml
sudo nano /etc/scribe/storage.toml
```

Key values to change:
- `database.url` — set the real Postgres password.
- `storage.signing_secret` — generate with `openssl rand -hex 32`.
- `api.public_base_url` — your tailnet MagicDNS name.
- `auth.device_keys` — path to your devices.toml.
- `llm.ollama_url` — the processing node's tailnet address.

Secrets can also be set via environment variables (preferred):
```bash
export SCRIBE_STORAGE__SIGNING_SECRET="$(openssl rand -hex 32)"
export SCRIBE_DATABASE__URL="postgres://scribe:REALPASSWORD@localhost/scribe"
```

### 3d. Start the API server

```bash
scribe serve --config /etc/scribe/storage.toml
```

The server binds `127.0.0.1:8443` by default. `tailscale serve` terminates
TLS on the tailnet and reverse-proxies here (see [docs/networking-tailscale.md](networking-tailscale.md)).

---

## 4. Processing node setup

### 4a. Install Ollama and pull a model

```bash
# Linux:
curl -fsSL https://ollama.com/install.sh | sh
sudo systemctl enable --now ollama

# Pull the summarization model (choose by VRAM):
ollama pull gemma3:27b    # 24 GB VRAM
ollama pull gemma3:12b    # 12 GB VRAM
ollama pull llama3.2:8b   # 8 GB VRAM
```

### 4b. Download model assets

See [models/README.md](../models/README.md) for the full list. Quick summary:

```
models/
  asr/
    encoder.onnx (or .int8.onnx)
    decoder.onnx (or .int8.onnx)
    joiner.onnx  (or .int8.onnx)   — Parakeet transducer only
    tokens.txt
  diarization/
    segmentation.onnx  — pyannote-segmentation-3.0
    embedding.onnx     — 3D-Speaker or NeMo TitaNet (192-dim)
```

Download from the sherpa-onnx releases:
<https://github.com/k2-fsa/sherpa-onnx/releases>

### 4c. Configure the processing node

```bash
sudo mkdir -p /etc/scribe /var/lib/scribe/models
sudo cp deploy/compute.toml /etc/scribe/compute.toml
sudo nano /etc/scribe/compute.toml
```

Key values to change:
- `database.url` — the storage node's tailnet address + password.
- `api.public_base_url` — storage node's MagicDNS URL (for audio pulls).
- `worker.models_dir` — where you placed the ONNX files.
- `asr.device` — `"cuda"` for GPU, `"cpu"` otherwise.
- `llm.summarize_model` — match the model you pulled into Ollama.

### 4d. Start the worker

```bash
scribe worker --config /etc/scribe/compute.toml
```

The worker will:
1. Connect to Postgres on the storage node.
2. `LISTEN scribe_jobs` for instant wakeups.
3. Claim a job, pull audio via the signed URL from the storage API, run the
   ML pipeline, write results back to Postgres.

---

## 5. Tailscale networking

See [docs/networking-tailscale.md](networking-tailscale.md) for the full guide.

Quick setup:
```bash
# On the storage node:
tailscale up
tailscale serve --bg "http://localhost:8443"   # sets up TLS proxy

# On the processing node:
tailscale up

# On your phone: install the Tailscale app and sign in.
```

The app should point to `https://<storage-node-fqdn>` (e.g.
`https://scribe.example.ts.net`).

---

## 6. First run and smoke-test

```bash
# Ingest a test file (stub build: no real transcription, but tests the pipeline)
scribe ingest ./test.m4a --title "Smoke test" --participants 2 \
    --config /etc/scribe/storage.toml

# Or for the stub build without a real file, use --inline (runs locally):
# scribe ingest sample.wav --inline --config /etc/scribe/storage.toml

# Check health
curl https://scribe.<your-tailnet>.ts.net/health

# Run the full diagnostics
scribe doctor --config /etc/scribe/storage.toml    # storage node
scribe doctor --config /etc/scribe/compute.toml    # processing node
```

---

## 7. Installing systemd services

```bash
# Storage node
sudo cp deploy/systemd/scribe-serve.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now scribe-serve
sudo journalctl -u scribe-serve -f

# Processing node
sudo cp deploy/systemd/scribe-worker.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now scribe-worker
sudo journalctl -u scribe-worker -f
```

Create the environment files for secrets (do not put secrets in the TOML):
```bash
# /etc/scribe/scribe-serve.env
SCRIBE_DATABASE__URL=postgres://scribe:REALPASSWORD@localhost/scribe
SCRIBE_STORAGE__SIGNING_SECRET=<openssl rand -hex 32>
RUST_LOG=scribe=info,tower_http=warn

# /etc/scribe/scribe-worker.env
SCRIBE_DATABASE__URL=postgres://scribe:REALPASSWORD@scribe.<tailnet>.ts.net/scribe
RUST_LOG=scribe=info
```

---

## 8. Backups

### Database

The database is the source of truth for all metadata, transcripts, vectors,
and summaries.

```bash
# Full dump:
pg_dump -U scribe -h localhost scribe | gzip > scribe-$(date +%F).sql.gz

# Restore:
gunzip -c scribe-2026-01-15.sql.gz | psql -U scribe -h localhost scribe
```

For continuous protection, use Postgres point-in-time recovery (PITR) with
`pg_basebackup` + WAL archiving, or a managed backup tool like `pgbackrest` or
`barman`.

### Audio blobs

The blob directory (`storage.blobs`, default `/var/lib/scribe/blobs`) holds the
raw uploaded audio. It is the only data NOT in Postgres.

```bash
# Mirror to a backup location (rsync is idempotent):
rsync -av --progress /var/lib/scribe/blobs/ backup-host:/backups/scribe/blobs/
```

Transcoded WAV files (`{recording_id}/audio.wav`) are a cache and can be
regenerated by re-running `scribe worker` on the transcode stage; you may omit
them from backups to save space.

### What you need to restore

To fully restore Scribe:
1. Restore the Postgres dump.
2. Restore the blobs directory (raw segments at minimum; WAV cache optional).
3. Re-run `scribe migrate` if restoring to a fresh DB.
4. If you changed embedding models since the last backup, run
   `scribe reindex --embeddings` to rebuild vectors.

---

## 9. Maintenance tasks

```bash
# Rebuild embeddings after changing the embedding model
scribe reindex --embeddings --config /etc/scribe/compute.toml

# Rebuild summaries (e.g. after upgrading the LLM)
scribe reindex --summaries --config /etc/scribe/compute.toml

# Enroll a speaker for name labelling
scribe enroll --name "Alice" --audio alice-sample.wav \
    --config /etc/scribe/compute.toml

# Rename / manage speaker identities
scribe speaker --config /etc/scribe/compute.toml

# Pull new model assets
scribe models pull --config /etc/scribe/compute.toml

# Run diagnostics
scribe doctor --config /etc/scribe/storage.toml
scribe doctor --config /etc/scribe/compute.toml
```
