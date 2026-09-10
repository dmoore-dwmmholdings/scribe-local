# Scribe

A self-hosted meeting recorder, transcriber, and semantic search system. Record
meetings on your phone; get speaker-labelled transcripts, LLM summaries, and
natural-language search over your full archive — entirely on hardware you control,
with no cloud APIs and no data leaving your network.

---

## Table of contents

Install and run:

- [Install with one command](#install-with-one-command)
- [Prerequisites](#prerequisites)
- [Quickstart (stub build — no GPU / ONNX required)](#quickstart-stub-build)
- [Real-ML build (ONNX + GPU)](#real-ml-build)
- [Mobile app](#mobile-app)

How it works:

- [Architecture](#architecture)
- [Crate layout](#crate-layout)
- [Measuring transcription and speaker detection](#measuring-transcription-and-speaker-detection)
- [Configuration reference](#configuration-reference)
- [API endpoints](#api-endpoints)
- [Self-update](#self-update)
- [Design document and roadmap](#design-document-and-roadmap)
- [License](#license)

---

## Install with one command

On Windows, in Git Bash:

```bash
curl -fsSL https://raw.githubusercontent.com/dmoore-dwmmholdings/scribe-local/master/install.sh | bash
```

On Linux or macOS, in a terminal:

```bash
curl -fsSL https://raw.githubusercontent.com/dmoore-dwmmholdings/scribe-local/master/install.sh | bash
```

The command does all of these steps:

- It downloads the server, or it makes the container image.
- It installs a current ffmpeg, because builds before 2025 cannot read the audio
  that current phones write.
- It makes the URL signature secret and the device token for the phone.
- It starts Postgres and applies the migrations.
- It downloads the ASR models (about 750 MB).
- It publishes the API on your tailnet.
- It starts the API and the worker.

At the end it shows the server URL and the device token. Put the two values in
the app.

No setup is necessary before the command, and the command has no questions for
you. To start again safely, do the command again: it keeps your secrets, your
models, and your database.

These alternatives go after `bash -s --`. An example:

```bash
curl -fsSL https://raw.githubusercontent.com/dmoore-dwmmholdings/scribe-local/master/install.sh | bash -s -- --service
```

- `--service` — install always-on Windows services. Start Git Bash as
  Administrator, and install NSSM first with `winget install NSSM.NSSM`.
- `--dir PATH` — install to a different directory (default `~/scribe`).
- `--model NAME` — `parakeet-tdt-0.6b-v3` (default) or `whisper-large-v3-turbo`.
- `--no-tailscale` — do not publish the API on your tailnet.
- `--docker` — use the containers on Windows, not the server bundle.

If a port is in use, the installer moves to the next free port and tells you.

The installer picks the right method for the machine: the prebuilt server bundle
on Windows, the container stack on Linux and macOS. To use the containers
directly:

```bash
git clone https://github.com/dmoore-dwmmholdings/scribe-local.git
cd scribe-local
docker compose up -d --build
```

Full procedure: **[docs/install.md](docs/install.md)**.

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
| OS | Linux, **macOS** (arm64/x86_64), or Windows. The backend is portable Rust; on macOS the real-ML build works out of the box (prebuilt ONNX Runtime + CoreML). |
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

> **Windows one-liner:** `\.scripts\launch-local.ps1` does all of the steps
> below (start Postgres → build → migrate → run `serve` + `worker`) using
> [`deploy/local.toml`](deploy/local.toml). Add `-Real` once you've installed
> the MSVC toolchain, ONNX models, and Ollama. The manual steps follow.

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
curl http://127.0.0.1:8443/recordings   # needs a bearer token if auth is on
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

`scribe models pull` downloads these assets for the default
`parakeet-tdt-0.6b-v3` stack. The download is about 750 MB.

The command keeps each file that it finds in the directory. Thus you can start
the command again safely.

For a different checkpoint, refer to [models/README.md](models/README.md).
fastembed downloads its own model at the first start.

### GPU acceleration (NVIDIA / CUDA)

The bundled `onnxruntime.dll` is CPU-only, so `[asr].device = "cuda"` falls back
to CPU until you install the CUDA execution provider. On Windows:

```powershell
.\scripts\setup-gpu.ps1     # installs the GPU onnxruntime provider + CUDA 12/cuDNN 9 DLLs
```

It drops Microsoft's `onnxruntime-gpu` (version-matched to the bundled
onnxruntime, so it stays compatible with both sherpa-onnx and fastembed) plus the
CUDA/cuDNN runtime DLLs beside `scribe.exe`. Then set `[asr].device = "cuda"`
(the default `deploy/local.toml` already does) and restart. Verify with
`nvidia-smi` — `scribe.exe` should appear as a compute app and GPU utilization
should spike during a transcription. Re-run after a clean `cargo build` (which
re-copies the CPU DLLs). CPU ASR (Parakeet) is already faster-than-real-time, so
the GPU is an optimization, not a requirement.

---

## Measuring transcription and speaker detection

Speaker detection is the part of this that is easiest to get subtly wrong and
hardest to eyeball, so it is measured rather than argued about.
[`docs/measuring-diarization.md`](docs/measuring-diarization.md) is the full
account: how to build conversations with known speakers, how to put them through
a room, and what the current numbers are.

Where it stands, on synthesised conversations with known answers:

| | |
|---|---|
| words given the right speaker | 99.6 – 100% clean, 99.6% with reverberation and noise |
| word error rate | 1.0 – 1.5% clean, ~5% in a bad room |
| speed | ~25x real time transcribing, ~10x diarizing, ~7x overall |
| names recognised across recordings | 3 of 3 through a different room, no false positives |

A regression check, which builds its own fixtures and fails if speaker detection
has got worse:

```bash
cargo build --release -p scribe-asr --example diarize_check
./scripts/diar-regression.sh
```

Four harnesses behind it, in rough order of how much of the system they touch:

```bash
# The diarizer alone: how many speakers, and did the right one get each stretch.
cargo run --release -p scribe-asr --example diarize_check -- models a.wav truth.json

# What a reader sees: real ASR timings, real diarization, the merge stage.
cargo run --release -p scribe-pipeline --example transcript_check -- models a.wav truth.json

# Does a name given in one meeting stick to the same voice in the next.
cargo run --release -p scribe-pipeline --example enroll_check -- models a.wav a.json b.wav b.json

# The whole pipeline through the CLI and a real database, on any audio.
./scripts/e2e-check.sh recording.wav
```

Fixtures are generated, not downloaded:

```bash
python3 scripts/make-diar-fixture.py /tmp/diar 30      # a conversation + ground truth
python3 scripts/degrade-audio.py in.wav out.wav \
    --reverb 0.4 --snr 15 --far Karen=0.3 --truth truth.json
python3 scripts/add-room-noise.py in.wav out.wav --truth truth.json
```

The two stages that call a language model — fixing misheard names, and
summarising — can be exercised without one:

```bash
python3 scripts/stub-llm.py 8799 &     # or: 8799 garbage, to check it degrades
```

Synthesised voices are cleaner and more separable than a room of people, so
these numbers are an upper bound and a way to compare one change against
another. **Run `e2e-check.sh` on a recording of real people** — it is worth more
than any of the above, and `scribe transcript <id>` will show you what it made
of it.

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

All routes except `GET /health` require `Authorization: Bearer <key>` when
`auth.require_device_token = true`. This is what the mobile app sends.

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
DELETE /recordings/{id}/speakers/{idx}     not a participant: drop the voice and its lines
PUT    /recordings/{id}/participants       state how many people are in the recording
POST   /recordings/{id}/rediarize          redo speaker detection, keeping the transcript
POST   /recordings/{id}/reprocess          re-run the whole pipeline from the audio
GET    /search?q=…                         hybrid full-text + vector semantic search
POST   /ask                                RAG: {question} → {answer, citations}
                                           (hits and citations name the speaker)
GET    /processing-schedule                weekly windows + live status + queue counts
PUT    /processing-schedule                replace the weekly windows
POST   /processing-schedule/override       run now / pause now / clear
```

When a transcript has too many speakers, which of the last four you want depends
on why: too many *people* means the count was guessed wrong, so state it and
redo speaker detection; a voice that is not a person at all — a television, a
conversation through a wall — should be dropped instead, because stating a count
cannot exclude anything and will merge two real speakers trying. See
[docs/measuring-diarization.md](docs/measuring-diarization.md).

The processing schedule limits the heavy stages of the pipeline to the hours you
select in the app. Full guide: **[docs/processing-schedule.md](docs/processing-schedule.md)**.

Backend self-update (only when `[update].enabled`, gated by the **update token**):

```
GET    /admin/info                         running version, target, rollback availability
POST   /admin/update                       body = signed .tar.gz; installs + restarts
POST   /admin/update/rollback              restore the previous binary + restart
```

---

## Self-update

The backend can update itself from a **signed package** — upload a new binary
from the phone (Settings → Backend update) or `curl`, and the server verifies
the ed25519 signature, installs it atomically (keeping a `.old` backup), runs
the new migrations, and restarts into it. Disabled by default; it is, by design,
remote code execution. Full guide: **[docs/self-update.md](docs/self-update.md)**.

```bash
scribe update keygen --out release.key            # one-time: signing keypair
scribe update sign  --key release.key --binary target/release/scribe \
                    --version 0.2.0 --out scribe-0.2.0.tar.gz
# then upload scribe-0.2.0.tar.gz via the app or POST /admin/update
```

CLI: `scribe update <keygen|sign|verify|apply|rollback|info>`.

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
- §4 — Subcommands (`serve`, `worker`, `migrate`, `ingest`, `reindex`, `enroll`, `speaker`, `models`, `update`, `doctor`)
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

---

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
