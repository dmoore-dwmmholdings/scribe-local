# Model Assets

This directory is the `worker.models_dir` root.  The processing node's
`scribe worker` loads ONNX model files from here at startup; if any required
files are absent the worker falls back to the deterministic stub engine (no
real transcription, but the pipeline still runs end-to-end for testing).

The exact filenames and layout below come from `crates/scribe-asr/src/models.rs`
which is the authoritative source — check that file if anything does not match.

---

## Expected directory layout

```
models/
  asr/
    encoder.onnx          # or encoder.int8.onnx
    decoder.onnx          # or decoder.int8.onnx
    joiner.onnx           # or joiner.int8.onnx   (Parakeet/transducer only)
    tokens.txt
  diarization/
    segmentation.onnx     # pyannote-segmentation-3.0
    embedding.onnx        # 3D-Speaker / NeMo TitaNet (192-dim)
```

`models.rs` tries `.onnx` before `.int8.onnx` for each file.  INT8 quantised
variants are smaller and usually faster on CPU.

---

## 1. ASR model (choose one)

### Option A — Parakeet-TDT-0.6B-v3 (RECOMMENDED)
Tops the 2026 open ASR leaderboard.  Word-level timestamps.  Transducer
layout: `encoder + decoder + joiner + tokens`.

Download from the k2-fsa/sherpa-onnx releases:
- Release page: <https://github.com/k2-fsa/sherpa-onnx/releases>
- Search for `parakeet-tdt-0.6b-v3` in the release assets.
- Typical filenames (INT8 recommended for lower VRAM):
  ```
  encoder.int8.onnx
  decoder.int8.onnx
  joiner.int8.onnx
  tokens.txt
  ```
- Place all four files in `models/asr/`.

Direct Hugging Face source (sherpa-onnx converted):
<https://huggingface.co/k2-fsa/sherpa-onnx-parakeet-tdt-0.6b-v3>

### Option B — Whisper large-v3-turbo
Better multilingual robustness (messy accents, code-switching).  Encoder +
decoder layout (no joiner).

- sherpa-onnx release page: search `whisper-large-v3-turbo`
- HuggingFace: <https://huggingface.co/k2-fsa/sherpa-onnx-whisper-large-v3-turbo>
- Files expected in `models/asr/`:
  ```
  encoder.onnx   (or whisper-encoder.onnx)
  decoder.onnx   (or whisper-decoder.onnx)
  tokens.txt
  ```

---

## 2. Diarization models

Both files go in `models/diarization/`.

### 2a. pyannote segmentation (speaker-change boundary detection)
- `segmentation.onnx` — pyannote-segmentation-3.0 exported to ONNX.
- sherpa-onnx release page: search `pyannote-segmentation`
- HuggingFace: <https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0>
- Rename the downloaded file to `segmentation.onnx` (or `segmentation.int8.onnx`).

### 2b. Speaker-embedding extractor (192-dim)
- `embedding.onnx` — one of: 3D-Speaker, NeMo TitaNet, WeSpeaker extractor.
- **Important:** the database schema stores speaker embeddings as `vector(192)`.
  Use a model that produces 192-dim vectors or update the schema and re-enroll.
- sherpa-onnx release page: search `3d-speaker` or `wespeaker` or `nemo-titanet`
- Example (3D-Speaker):
  <https://github.com/k2-fsa/sherpa-onnx/releases?q=3d-speaker>
- Rename to `embedding.onnx`.

---

## 3. Ollama LLM models (on the processing node)

These are managed by Ollama, not placed here.  After installing Ollama:

```bash
# Choose one based on available VRAM:
ollama pull gemma3:27b      # recommended — 24 GB VRAM (Q4)
ollama pull gemma3:12b      # 12 GB VRAM
ollama pull llama3.2:8b     # 8 GB VRAM
ollama pull qwen3:8b        # 8 GB VRAM alternative
```

The `llm.summarize_model` config key in `deploy/compute.toml` must match the
model name you pulled.

---

## 4. fastembed embedding model (no download needed)

`fastembed-rs` (the in-process embedding library) **downloads its models
automatically** at first run and caches them in `~/.cache/huggingface/` (or
`$HF_HOME` if set).  No manual file placement is needed.

Supported models in fastembed v4 (as of mid-2026):
- `nomic-embed-text` → 768-dim  (**default**; well-supported)
- `all-minilm-l6-v2` → 384-dim
- `bge-small-en-v1.5` → 384-dim
- `bge-base-en-v1.5` → 768-dim
- `multilingual-e5-large` → 1024-dim

Note: `qwen3-embedding-0.6b` (the design's quality pick) is **not yet in
fastembed v4** — it falls back to `nomic-embed-text` with a warning.  To use
Qwen3 embeddings, serve the model via TEI or Infinity and configure a custom
HTTP embedder (a later enhancement).

**The `llm.embed_dim` config value must match the model's output dimension.**
If you change the embedding model, run:
```bash
scribe reindex --embeddings
```

---

## 5. VAD model (bundled inside sherpa-onnx)

The Silero VAD model used for voice activity detection is bundled inside the
`sherpa-onnx` Rust crate and does not need to be downloaded separately.

---

## Quick-start checklist

```
[ ] models/asr/encoder.onnx (or .int8.onnx)
[ ] models/asr/decoder.onnx (or .int8.onnx)
[ ] models/asr/joiner.onnx  (or .int8.onnx)  — Parakeet only
[ ] models/asr/tokens.txt
[ ] models/diarization/segmentation.onnx (or .int8.onnx)
[ ] models/diarization/embedding.onnx    (or .int8.onnx)
[ ] ollama pull <summarize_model>
```

After placing the files, verify with:
```bash
scribe doctor --config /etc/scribe/compute.toml
```
