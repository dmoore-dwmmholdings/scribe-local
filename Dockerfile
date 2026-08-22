# syntax=docker/dockerfile:1
#
# Scribe — one image, both roles.
#
# The binary is role-agnostic (design §4): the container command selects
# `serve`, `worker`, `migrate`, or any operator subcommand, exactly like the
# native binary. docker-compose.yml runs the same image three times.
#
# The build links the REAL ML stack (sherpa-onnx ASR + diarization, fastembed
# embeddings). `sherpa-onnx-sys` downloads its prebuilt linux-x64 shared
# libraries during the build and copies them next to the binary, linking with an
# `$ORIGIN` rpath — so the runtime stage keeps `scribe` and those `.so` files in
# one directory and needs no ONNX runtime of its own.
#
# ONNX here is CPU-only. Parakeet on CPU is comfortably faster than real time;
# the CUDA path is the native Windows bundle (scripts/setup-gpu.ps1), because
# the GPU execution provider needs a CUDA base image and a driver passthrough
# this image deliberately does not assume.

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS builder

WORKDIR /src

# `sqlx::migrate!` embeds migrations/ at compile time, so it is part of the
# build context rather than the runtime image.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations

# The cache mounts make a rebuild after an edit take seconds instead of
# re-fetching and re-compiling the whole ML tree. Artifacts are copied out to
# /out inside the same RUN, because a cache mount does not survive the layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    <<'EOF'
set -eux
cargo build --release -p scribe-cli
mkdir -p /out
cp target/release/scribe /out/scribe
# sherpa-onnx + onnxruntime shared objects, staged beside the binary.
find target/release -maxdepth 1 -name '*.so*' -exec cp -a {} /out/ \;
test -x /out/scribe
EOF

# ---------------------------------------------------------------------------
# Runtime
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# ffmpeg  — the transcode stage shells out to it.
# libgomp1 — OpenMP runtime the ONNX Runtime shared library links against.
# ca-certificates — HTTPS for `models pull` and the LLM client.
# curl — the compose healthcheck probes GET /health with it.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ffmpeg \
      libgomp1 \
      ca-certificates \
      curl \
      tini \
 && rm -rf /var/lib/apt/lists/*

# Binary and its sibling shared objects.
COPY --from=builder /out/ /opt/scribe/

# sherpa-onnx-sys asks Cargo for an `$ORIGIN` rpath, but `cargo:rustc-link-arg`
# from a dependency's build script applies only to that dependency, never to the
# final binary — so the linked `scribe` has no rpath and the loader would not
# find libsherpa-onnx-c-api.so beside it. On Windows this never shows up,
# because a DLL next to the .exe is found anyway. Registering the directory with
# ldconfig fixes it without exporting LD_LIBRARY_PATH into every child process
# the transcode stage spawns.
RUN echo "/opt/scribe" > /etc/ld.so.conf.d/scribe.conf && ldconfig

ENV PATH="/opt/scribe:${PATH}"

COPY docker/entrypoint.sh /usr/local/bin/scribe-entrypoint
RUN chmod +x /usr/local/bin/scribe-entrypoint

# Defaults for the containerised layout. Compose overrides what a user changes.
ENV SCRIBE_STORAGE__BLOBS=/data/blobs \
    SCRIBE_WORKER__MODELS_DIR=/models \
    SCRIBE_UPDATE__STAGING_DIR=/data/updates \
    SCRIBE_API__BIND=0.0.0.0:8443

VOLUME ["/data", "/models"]
EXPOSE 8443

# tini reaps the ffmpeg children the transcode stage spawns and forwards
# SIGTERM, so `docker compose down` stops a running pipeline cleanly.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/scribe-entrypoint"]
CMD ["serve"]
