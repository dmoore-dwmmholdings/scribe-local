# Install Scribe

This document tells you how to install the Scribe backend on one machine. The
procedure uses Docker Compose. It starts the database, the API, and the worker
together. Docker is the only necessary tool.

This document does not tell you how to install the mobile app. For the app,
refer to [install-iphone.md](install-iphone.md).

---

## What the procedure installs

The procedure starts five containers. This table shows each container and its
task.

| Container | Task |
|---|---|
| `postgres` | The database, with the pgvector extension for the search index. |
| `scribe-init` | It applies the migrations and downloads the models one time. |
| `scribe-serve` | The API. The phone uploads audio to this container. |
| `scribe-worker` | The pipeline. It transcodes, transcribes, and diarizes the audio. |
| `ollama` | The LLM for summaries. This container is optional. |

The API and the worker use the same image. The image contains the ASR stack and
the ONNX runtime. The models are approximately 750 MB. The `scribe-init`
container downloads them into a Docker volume.

The image does the transcription on the CPU. Parakeet on a CPU is faster than
the recorded time. To use a GPU, refer to
[Alternative: Windows without containers](#alternative-windows-without-containers).

---

## Before you start

Prepare these items:

- Docker Desktop (Windows or macOS) or Docker Engine (Linux). The Docker Compose
  version must be 2 or more.
- 8 GB of free disk space for the image, the models, and the database.
- A network connection. Docker downloads the Rust packages and the models.
- Tailscale, if you want to record from the phone. The phone cannot connect to
  `127.0.0.1` on the server.

Rust, Visual Studio, ffmpeg, and the ONNX runtime are not necessary. The image
contains all of them.

The first time, Docker compiles the ASR stack. This continues for 15 minutes to
30 minutes. Subsequently, Docker uses its cache and continues for seconds.

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

---

## Install manually

Do this procedure if you do not want to use the script.

1. Download the repository, then go to its directory:

   ```bash
   git clone https://github.com/dmoore-dwmmholdings/scribe-local.git
   cd scribe-local
   ```

2. Start the stack:

   ```bash
   docker compose up -d --build
   ```

   Docker makes the image, starts the database, applies the migrations,
   downloads the models, and starts the API and the worker.

3. Monitor the setup container:

   ```bash
   docker compose logs -f scribe-init
   ```

4. When the API operates, do a check of its health:

   ```bash
   curl http://127.0.0.1:8443/health
   ```

   The API sends its status and the condition of the database.

5. Read the device token:

   ```bash
   docker compose exec scribe-serve cat /data/devices.toml
   ```

A password or a secret is not necessary before the first start. The API makes
the URL signature secret and the device token, then keeps the two values in the
data volume.

---

## Connect the phone

The phone connects to the API across your tailnet. Tailscale gives the server a
stable name and encrypts the connection.

1. Install Tailscale on the server and on the phone. Use the same account on
   the two devices.

2. On the server, publish the API on the tailnet:

   ```bash
   tailscale serve --bg http://127.0.0.1:8443
   ```

   If Tailscale tells you that `Serve` is not available, let it operate one time
   at <https://login.tailscale.com/f/serve>.

3. Find the name of the server:

   ```bash
   tailscale status --json
   ```

   The name is in the `Self.DNSName` field. An example is
   `my-server.tail1234.ts.net`.

4. Put the name in the `.env` file, then start the stack again:

   ```bash
   echo "SCRIBE_PUBLIC_BASE_URL=https://my-server.tail1234.ts.net" >> .env
   docker compose up -d
   ```

### Set up the app

`scripts/quickstart.sh` prints a pairing link at the end, as a QR code when
`qrencode` is installed:

```
scribe://pair?url=https://my-server.tail1234.ts.net&key=…
```

Scan it with the phone's camera, or open the link on the phone by any other
route. The app asks which server it is about to connect to, and fills in both
fields when you confirm. Enter them by hand only if the link cannot reach the
phone:

1. In the app, open `Settings`.

2. Put the server URL in the `Server` field.

3. Put the device token in the `Device API key` field.

Then:

4. Make a short test recording.

5. Wait for the worker to complete the pipeline.

6. Open the library screen. It shows the transcript.

### Drop the device token (optional)

On a tailnet the token is a second lock on a door Tailscale has already locked.
`tailscale serve` authenticates every peer it proxies and passes the login to
the API, so the API can accept that instead and the phone needs no key at all:

```toml
[auth]
trust_tailscale_identity = true
# tailnet_users = ["you@example.com"]   # empty = any user on your tailnet
```

Pairing is then just the server URL, and `Device API key` can stay empty.

CAUTION: this is only safe while the API stays bound to `127.0.0.1`, so that the
local `tailscale serve` is the only thing that can reach it. The header carrying
the login is forgeable by anything able to connect directly. If you bind the API
to `0.0.0.0` or a LAN address, leave this off and keep using tokens. On a shared
tailnet, set `tailnet_users` — otherwise every user on it is admitted.

CAUTION: DO NOT PUBLISH THE API ON THE INTERNET.

A PERSON WHO GETS THE DEVICE TOKEN CAN READ EACH TRANSCRIPT.

---

## Add summaries

The transcript, the speaker labels, the search index, and the embeddings do not
use an LLM. Summaries and the `/ask` endpoint do use one.

1. Start the stack again with the `ollama` profile:

   ```bash
   docker compose --profile ollama up -d
   ```

2. Download a model. Select a model that your hardware can hold in memory:

   ```bash
   docker compose exec ollama ollama pull gemma3:12b
   ```

   | Model | Approximate memory |
   |---|---|
   | `gemma3:27b` | 24 GB |
   | `gemma3:12b` | 12 GB |
   | `llama3.2:8b` | 8 GB |

3. For a different model, put its name in the `.env` file:

   ```bash
   echo "SCRIBE_SUMMARIZE_MODEL=gemma3:27b" >> .env
   docker compose up -d
   ```

Ollama in a container does its work on the CPU. A large model is slow on a CPU.
To use a GPU, install Ollama on the host machine. Then set the LLM
address in `deploy/docker.toml`.

---

## Operate the stack

These commands do the usual tasks:

- Show the logs: `docker compose logs -f scribe-serve scribe-worker`
- Stop the stack: `docker compose down`
- Start the stack: `docker compose up -d`
- Start the stack after a code change: `docker compose up -d --build`
- Do a check of the setup: `docker compose exec scribe-serve scribe doctor`
- Show the model status: `docker compose exec scribe-worker scribe models list`
- Open a database session: `docker compose exec postgres psql -U scribe scribe`

The stack starts again automatically after a start of the machine. Each
container has the `unless-stopped` policy. On Windows, set Docker
Desktop to start when you log in.

### Where the data is

Docker keeps the data in three volumes. The volumes stay when you make the image
again. Thus a new image does not erase your recordings.

| Volume | Contents |
|---|---|
| `scribe_pgdata` | The database: transcripts, summaries, and the search index. |
| `scribe_scribe-data` | The audio blobs, the signature secret, and the device token. |
| `scribe_scribe-models` | The ONNX models. |

CAUTION: `docker compose down -v` ERASES THE THREE VOLUMES. YOU CANNOT GET YOUR
RECORDINGS AGAIN. USE `docker compose down` WITHOUT `-v`.

---

## Change the configuration

The stack reads its configuration from `deploy/docker.toml`. Docker Compose
gives this file to each container as read-only.

1. Change `deploy/docker.toml`.
2. Apply the change:

   ```bash
   docker compose restart scribe-serve scribe-worker
   ```

The `.env` file holds the values that change most frequently: the server URL,
the two host ports, the LLM model, and the log level. Refer to `.env.example`
for the full list. An environment variable with the `SCRIBE_` prefix always
replaces the value in the file. An example is `SCRIBE_ASR__DIARIZATION=false`.

---

## If the install does not work

| Symptom | Cause and correction |
|---|---|
| `docker compose` cannot connect to the daemon. | Docker Desktop is not available. Start it, then wait for its icon to become stable. |
| The image build stops during `cargo build`. | The network connection stopped during the package download. Start the command again. The cache keeps the completed work. |
| `scribe-init` stops with a download error. | The model download did not complete. Start it again with `docker compose up -d scribe-init`. Docker does not download the completed files a second time. |
| The API is not available on port 8443. | A different service holds the port. Put `SCRIBE_API_PORT=8444` in `.env`, then start the stack again. |
| The transcript contains placeholder text. | The models are not in the volume, thus the worker used the stub engine. Do `docker compose exec scribe-worker scribe models list` for the status. |
| The app shows an authorization error. | The device token in the app is not correct. Read the token again from `/data/devices.toml`. |
| The summary is empty. | The LLM is not available. Start the `ollama` profile, or set the LLM address to a server on the host machine. |

For more data about an error, read the logs of the container:

```bash
docker compose logs scribe-init
docker compose logs scribe-worker
```

---

## Alternative: Windows without containers

The image does the transcription on the CPU. An install on Windows without containers can use
an NVIDIA GPU. It is the correct selection for a large archive, or for the
Whisper model.

This procedure uses the Rust toolchain, the MSVC toolchain, and manual
model files. Refer to [deploy-windows-server.md](deploy-windows-server.md) and
to the [README](../README.md).

These are the steps of the procedure:

1. Make the binary with `cargo build --release -p scribe-cli`.
2. Add the GPU libraries with `.\scripts\setup-gpu.ps1`.
3. Make the deploy bundle with `.\scripts\package-release.ps1 -Zip`.
4. Copy the bundle to the server.
5. Install the two services with `.\scripts\install-service.ps1`.
