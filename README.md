# Scribe

A self-hosted meeting recorder, transcriber, and semantic search system. Record
meetings on your phone; get speaker-labelled transcripts, LLM summaries, and
natural-language search over your full archive — entirely on hardware you control,
with no cloud APIs and no data leaving your network.

---

## Table of contents

- [Architecture](#architecture)
- [Crate layout](#crate-layout)
- [Prerequisites](#prerequisites)
- [Quickstart (stub build — no GPU / ONNX required)](#quickstart-stub-build)
- [Real-ML build (ONNX + GPU)](#real-ml-build)
- [Configuration reference](#configuration-reference)
- [API endpoints](#api-endpoints)
- [Mobile app](#mobile-app)
- [Design document and roadmap](#design-document-and-roadmap)

---

## Architecture

Two physical machines, three logical roles, all on one Tailscale tailnet:

```mermaid
flowchart LR
  subgraph Phone["React Native app — anywhere"]
    REC[Recorder: segmented AAC/m4a]
    UP[tus resumable upload]
  end

  subgraph Storage["STORAGE NODE — always-on, low power"]
    API["scribe serve (Axum API + tus endpoint)"]
    BLOB[(Audio blobs on disk)]
    PG[(PostgreSQL + pgvector)]
  end

  subgraph Compute["PROCESSING NODE — GPU"]
    WORK["scribe worker"]
    SHERPA[sherpa-onnx: VAD + diarize + ASR]
    EMB[fastembed-rs: embeddings]
    OLL[Ollama: summary + Q&A]
  end

  REC --> UP
  UP -->|HTTPS over tailnet| API
  API --> BLOB
  API --> PG
  WORK -->|claim jobs: SKIP LOCKED + NOTIFY| PG
  WORK -->|pull audio: signed HTTPS| API
  WORK --> SHERPA
  WORK --> EMB
  WORK --> OLL
  WORK -->|write transcript + vectors + summary| PG
```

The **storage node** is deliberately humble (a NAS, mini-PC, Raspberry Pi 5 —
anything always-on with disk). The **processing node** is where the GPU lives.
Postgres is the single rendezvous point; the processing node holds no durable
state.

For a deeper dive see [docs/architecture.md](docs/architecture.md).

---

## Crate layout

```
crates/
  scribe-core/     Config, domain types, errors — shared by all crates
  scribe-db/       sqlx queries, migrations, SKIP LOCKED job queue
  scribe-asr/      sherpa-onnx: VAD + speaker diarization + ASR
  scribe-llm/      Ollama HTTP client + fastembed-rs in-process embeddings
  scribe-pipeline/ Stage implementations + worker loop
  scribe-api/      Axum routers, handlers, device auth, blob serving
  scribe-cli/      clap subcommands → wires everything (builds `scribe` binary)
migrations/        SQL applied by `scribe migrate`
mobile/            React Native / Expo app
```

---

## Prerequisites

| Tool | Notes |
|---|---|
| Rust (stable, ≥ 1.82) | `rustup` recommended. MSVC toolchain required on Windows for the real-ML build. |
| Docker (or Podman) | For the Postgres + pgvector container. |
| ffmpeg | Used by the `transcode` stage; must be on `$PATH`. |
| Tailscale | For secure remote access. Free tier works. |
| Ollama | Processing node only. |
| CUDA toolkit | Processing node only, if using a GPU. |

---

## Quickstart (stub build)

The stub build requires no GPU, no ONNX runtime, and no model files. The
pipeline runs end-to-end with deterministic placeholder outputs — ideal for
development and CI.

### 1. Start Postgres

```bash
# Linux / Mac
./scripts/dev-db.sh up

# Windows (PowerShell)
.\scripts\dev-db.ps1 up
```

This starts a `pgvector/pgvector:pg17` container on port 5433 and runs
`scribe migrate` automatically.

Alternatively:
```bash
docker compose up -d postgres
```

### 2. Build (stub — no ONNX runtime)

```bash
cargo build -p scribe-cli --no-default-features
```

On Windows with the GNU toolchain, `--no-default-features` is required (the
real ONNX Runtime prebuilt only supports the MSVC ABI). See
[Real-ML build](#real-ml-build) below.

### 3. Run migrations

```bash
SCRIBE_DATABASE__URL="postgres://scribe:scribe@127.0.0.1:5433/scribe?sslmode=disable" \
  cargo run -p scribe-cli --no-default-features -- migrate
```

### 4. Start the storage API

```bash
SCRIBE_DATABASE__URL="postgres://scribe:scribe@127.0.0.1:5433/scribe?sslmode=disable" \
SCRIBE_STORAGE__SIGNING_SECRET="dev-secret" \
SCRIBE_API__PUBLIC_BASE_URL="http://127.0.0.1:8443" \
SCRIBE_AUTH__REQUIRE_DEVICE_TOKEN="false" \
  cargo run -p scribe-cli --no-default-features -- serve
```

The API is now live at `http://127.0.0.1:8443`.

### 5. Start the worker (separate terminal)

```bash
SCRIBE_DATABASE__URL="postgres://scribe:scribe@127.0.0.1:5433/scribe?sslmode=disable" \
SCRIBE_STORAGE__SIGNING_SECRET="dev-secret" \
SCRIBE_API__PUBLIC_BASE_URL="http://127.0.0.1:8443" \
  cargo run -p scribe-cli --no-default-features -- worker
```

### 6. Ingest a test file

```bash
# Ingest a local audio file (creates a recording and enqueues processing)
SCRIBE_DATABASE__URL="postgres://scribe:scribe@127.0.0.1:5433/scribe?sslmode=disable" \
SCRIBE_STORAGE__SIGNING_SECRET="dev-secret" \
SCRIBE_API__PUBLIC_BASE_URL="http://127.0.0.1:8443" \
  cargo run -p scribe-cli --no-default-features -- ingest sample.m4a --title "Test"

# Check it landed
curl http://127.0.0.1:8443/health
curl http://127.0.0.1:8443/recordings   # needs X-Device-Token if auth is on
```

---

## Real-ML build

The default build (no `--no-default-features`) enables the real ONNX speech
stack and fastembed embeddings.

```bash
cargo build --release -p scribe-cli
```

### Feature flags

| Flag | Default | Controls |
|---|---|---|
| `onnx` | on | sherpa-onnx ASR + diarization + VAD (native ONNX runtime required) |
| `local-embed` | on | fastembed-rs in-process embeddings (native ONNX runtime required) |

Both are disabled together with `--no-default-features`.

### Windows / MSVC toolchain requirement

The prebuilt ONNX Runtime native library (linked by `sherpa-onnx` and
`fastembed`) is built against the **MSVC** ABI. Building on
`x86_64-pc-windows-gnu` (MinGW/MSYS2) will fail at link time. Solutions:

1. Switch to `x86_64-pc-windows-msvc`: install [VS Build Tools](https://visualstudio.microsoft.com/downloads/)
   and run `rustup target add x86_64-pc-windows-msvc`.
2. Or: use `--no-default-features` for the stub build (fully supported on GNU).

On Linux this is not an issue — `gcc` or `clang` links the ONNX Runtime
shared library directly.

### Model files

Before starting the worker with the real build, populate `models/`:

```
models/
  asr/
    encoder.onnx (or .int8.onnx)
    decoder.onnx (or .int8.onnx)
    joiner.onnx  (or .int8.onnx)   — Parakeet transducer only
    tokens.txt
  diarization/
    segmentation.onnx
    embedding.onnx
```

See [models/README.md](models/README.md) for download instructions and sources.
fastembed models are downloaded automatically at first run.

---

## Configuration reference

Configuration is loaded in priority order:
1. Built-in defaults (see `crates/scribe-core/src/config.rs`)
2. A TOML file passed via `--config <path>`
3. `SCRIBE_<SECTION>__<KEY>` environment variables (double-underscore = nesting)

Examples of environment overrides:
```
SCRIBE_DATABASE__URL=postgres://...
SCRIBE_STORAGE__SIGNING_SECRET=...
SCRIBE_API__BIND=127.0.0.1:8443
SCRIBE_WORKER__CONCURRENCY=1
SCRIBE_LLM__SUMMARIZE_MODEL=gemma3:27b
```

Full configs with comments:
- Storage node: [`deploy/storage.toml`](deploy/storage.toml)
- Processing node: [`deploy/compute.toml`](deploy/compute.toml)
- Device keys example: [`deploy/devices.toml.example`](deploy/devices.toml.example)

Config sections and their keys (from `crates/scribe-core/src/config.rs`):

```toml
[database]
url             = "postgres://scribe@localhost/scribe"
max_connections = 10

[storage]
blobs              = "/var/lib/scribe/blobs"
signing_secret     = "change-me"
signed_url_ttl_secs = 600

[api]
bind            = "127.0.0.1:8443"
tus_upstream    = "http://127.0.0.1:1080"   # optional
max_segment_bytes = 16777216
public_base_url = "http://127.0.0.1:8443"

[auth]
device_keys          = "/etc/scribe/devices.toml"   # optional path
require_device_token = false

[asr]
model       = "parakeet-tdt-0.6b-v3"
diarization = true
device      = "cpu"    # or "cuda"

[worker]
stages         = ["all"]
concurrency    = 1
models_dir     = "/var/lib/scribe/models"
heartbeat_secs = 15
poll_secs      = 5
max_attempts   = 5

[llm]
ollama_url      = "http://127.0.0.1:11434"
summarize_model = "gemma3:27b"
embed_model     = "nomic-embed-text"
embed_dim       = 768
```

---

## API endpoints

All routes except `GET /health` require `X-Device-Token: <key>` (or
`Authorization: Bearer <key>`) when `auth.require_device_token = true`.

```
GET    /health                              liveness probe
POST   /recordings                         create recording {title, participants_expected}
GET    /recordings                         list recordings
GET    /recordings/{id}                    get recording + transcript + summary
POST   /recordings/{id}/complete           finish upload; enqueues transcode job
PUT    /recordings/{id}/segments/{seq}     upload one audio segment (stream to disk)
GET    /recordings/{id}/segments/{seq}     download a segment (HTTP range supported)
GET    /recordings/{id}/audio              full stitched audio (HTTP range supported)
POST   /recordings/{id}/speakers/{idx}/name  assign a name to a diarized speaker
GET    /search?q=…                         hybrid full-text + vector semantic search
POST   /ask                                RAG: {question} → {answer, citations}
```

---

## Mobile app

The React Native / Expo app lives in [`mobile/`](mobile/). It handles:
- Segmented AAC recording with iOS background audio + Android foreground service.
- tus resumable upload to the storage node.
- Transcript viewing, search, speaker labelling, and RAG Q&A.

See `mobile/` for its own README.

---

## Design document and roadmap

The full architecture and technology decision record is in
[`scribe-design.md`](scribe-design.md). Key sections:

- §3 — System architecture
- §4 — Subcommands (`serve`, `worker`, `migrate`, `ingest`, `reindex`, `enroll`, `speaker`, `models`, `doctor`)
- §5 — Tailscale networking
- §7 — Job queue and pipeline DAG
- §8 — ASR and speaker diarization (sherpa-onnx)
- §9 — LLM indexing, embeddings, search (Ollama + fastembed + pgvector)
- §13 — Hardware sizing
- §15 — Phased build roadmap

### Phased build roadmap (§15 summary)

| Phase | Description |
|---|---|
| 0 | Cargo workspace skeleton, config, `migrate`, Tailscale setup |
| 1 | Capture → upload → store (no ML); tus + segmented audio |
| 2 | Transcription (`scribe worker` + sherpa-onnx ASR) |
| 3 | Diarization + speaker labels + `scribe enroll` |
| 4 | LLM indexing, hybrid search, summaries, RAG `/ask` |
| 5 | Hardening: heartbeat/reaper, multi-worker, observability, backups |
