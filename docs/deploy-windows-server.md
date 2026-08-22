# Deploy Scribe to a Windows server (single box)

A focused guide for running Scribe on one always-on **Windows** PC, where you
**build on your dev machine and copy a runtime bundle** to the server. The server
needs no Rust/Visual Studio.

> This is the single-box Windows path. For a split storage+compute setup on
> Linux/systemd, see [deployment.md](deployment.md). For the Tailscale details,
> see [networking-tailscale.md](networking-tailscale.md).

Chosen shape for this guide: **build-and-copy · NVIDIA GPU (CUDA) · LLM on the
server · serve+worker as auto-start services.**

---

## 0. What the server needs (install these once)

| Prereq | Why | Get it |
|---|---|---|
| **Docker Desktop** | runs the `pgvector` Postgres container | docker.com — set it to **start on login** (Settings → General) |
| **Visual C++ Redistributable 2015–2022 x64** | the MSVC CRT the binary links against | `winget install Microsoft.VCRedist.2015+.x64` |
| **ffmpeg** on `PATH` | the transcode stage shells out to it | `winget install Gyan.FFmpeg` (then re-open the shell) |
| **NVIDIA driver** (recent) | CUDA ASR. The CUDA/cuDNN DLLs are **bundled**, so you do NOT need the CUDA toolkit | GeForce/Studio driver |
| **LM Studio** or **Ollama** | LLM for summaries / RAG / translation | lmstudio.ai / ollama.com — load a model |
| **Tailscale** | phone access via `tailscale serve` | tailscale.com — sign in to the same tailnet |
| **NSSM** | run serve+worker as services | `winget install NSSM.NSSM` |

---

## 1. Build + package the bundle (on your DEV PC)

```powershell
.\scripts\package-release.ps1 -Zip
```

This builds the real binary (MSVC dev shell), stages the CUDA/cuDNN DLLs, and
assembles `dist\scribe-server\` (+ `.zip`) containing: `scribe.exe` + all DLLs,
`deploy\server.toml`, `docker-compose.yml`, the server scripts, and a README.

The ONNX models are not in the bundle. On the server, this command downloads
them (about 750 MB). It keeps each file that it finds, thus you can start it
again safely:

```powershell
.\scribe.exe --config deploy\server.toml models pull
```

Add `-WithModels` to put the models in the bundle for a server with no network
connection.

> CPU-only server instead? `.\scripts\package-release.ps1 -Cpu` (and set
> `asr.device = "cpu"` in the config below).

## 2. Copy it to the server

Copy `dist\scribe-server\` (or the `.zip`) to the server — e.g. `C:\scribe\`.
Everything runs relative to that folder.

## 3. Configure (on the SERVER)

Edit `deploy\server.toml`:
- **`storage.signing_secret`** — replace with a real 32-byte hex secret:
  `powershell -c "[guid]::NewGuid().ToString('N')+[guid]::NewGuid().ToString('N')"`
- **`api.public_base_url`** — the server's tailnet name (you'll see it in step 4),
  e.g. `https://scribe-server.your-tailnet.ts.net`
- **`llm.summarize_model`** — the model id you loaded in LM Studio/Ollama
- (GPU + LLM on one card? see the **VRAM note** at the bottom.)

Create `deploy\devices.toml` from `deploy\devices.toml.example` (device-token
auth is **ON** in this config). One line per device:
```toml
# device_id = "api_key"
"my-phone" = "scribe_sk_pick_a_long_random_string"
```
You'll enter that key in the app (**Settings → Device API key**), and the
matching device id (**Settings → Device ID**, or set it to `my-phone`).

## 4. First run / smoke test (on the SERVER)

Start LM Studio's server (or `ollama serve`) with a model loaded, then:

```powershell
cd C:\scribe        # the bundle root
.\scripts\run-server.ps1
```

It starts Postgres, migrates, sets up `tailscale serve`, and opens serve+worker
windows. It prints the **tailnet API URL** — put that in `api.public_base_url`
(step 3) and in the app. Verify:
- `http://127.0.0.1:8443/health` → `{"status":"ok","db":true}`
- the server's tailnet URL `/health` from your phone's browser
- record (or import) a short clip end-to-end

If `tailscale serve` says **Serve is not enabled**, toggle it once at
<https://login.tailscale.com/f/serve> and re-run.

## 5. Install as always-on services (on the SERVER, as Administrator)

Once the smoke test passes, close the serve/worker windows and install services:

```powershell
# Elevated PowerShell, from C:\scribe
.\scripts\install-service.ps1
```

This registers **`scribe-serve`** and **`scribe-worker`** (auto-start on boot,
auto-restart on crash), pins the Postgres container to restart, and re-runs
migrations. Manage them with `Get-Service scribe-*`, `nssm restart scribe-serve`,
and read `logs\scribe-serve.log` / `logs\scribe-worker.log`. Uninstall with
`.\scripts\install-service.ps1 -Uninstall`.

Make sure **Docker Desktop starts on login** so Postgres is up before the
services need it.

## 6. Point the phone at the server

In the app: **Settings → Base URL** = the server's `https://…ts.net` (no port),
**Device API key** = the key from `devices.toml`, **Device ID** = its id. Test
connection → green.

---

## Updating later

Two options:
1. **Re-package + copy** — `package-release.ps1` on dev, stop the services
   (`nssm stop scribe-serve scribe-worker`), replace `scribe.exe` + DLLs (+ run
   `scribe migrate` if there are new migrations), start the services.
2. **Self-update** — enable `[update]` (token + ed25519 key) and push a signed
   package via `POST /admin/update` from the app's admin screen — no file copy.
   See [self-update.md](self-update.md).

## Backups

- **Database** (source of truth): `docker exec <pg> pg_dump -U scribe scribe | gzip > scribe-YYYY-MM-DD.sql.gz`
- **Audio blobs** (`.\data\blobs`): mirror with `robocopy` to another disk/host.
  Transcoded `audio.wav` files are a regenerable cache — safe to skip.

## VRAM note (GPU + LLM on one card)

CUDA ASR and a GPU LLM compete for VRAM. If LM Studio fills the card,
Whisper-large won't load on CUDA. Options: load a **smaller LLM**, or set
`asr.device = "cpu"` in `server.toml` (CPU ASR, GPU LLM). This is the same
tradeoff documented for the dev box.

## Troubleshooting

- **`/health` db=false** → Postgres not up / wrong `SCRIBE_DATABASE__URL`. Check Docker Desktop + the container.
- **Transcripts are placeholders** → the models are not on the server (stub fallback). Run `.\scribe.exe --config deploy\server.toml models pull`.
- **Service won't start** → read `logs\scribe-serve.log`; common causes: missing VC++ Redistributable, missing DLL beside `scribe.exe`, or a bad config path.
- **App times out** → the phone must be on the **same tailnet** and `tailscale serve` enabled; the URL is `https://<server>.<tailnet>.ts.net` with **no port**.
- **CUDA errors** → update the NVIDIA driver; or fall back to `asr.device = "cpu"`.
