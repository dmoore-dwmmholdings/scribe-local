//! `scribe models <list|pull>` — model-asset reporting and download (design §4).
//!
//! The expected ONNX layout under `cfg.worker.models_dir` is owned by
//! [`scribe_asr::models`]; we report each expected asset and whether it exists,
//! then list the configured LLM models.
//!
//! `pull` downloads the assets the default stack needs straight from the
//! upstream publishers (Hugging Face for the ASR checkpoint and the pyannote
//! segmentation model, the sherpa-onnx release page for the speaker-embedding
//! extractor) and writes them into the layout `models.rs` discovers. Downloads
//! stream to a `.part` file and are renamed only once the byte count matches
//! `Content-Length`, so an interrupted pull never leaves a truncated model that
//! would fail deep inside the ASR loader.
//!
//! The two checkpoints the shipped configs name — Parakeet-TDT-0.6B-v3 and
//! Whisper large-v3-turbo — have download recipes. Any other checkpoint still
//! reports what is missing and where to put it — see `models/README.md`.

use std::path::{Path, PathBuf};

use scribe_asr::models::{AsrModelPaths, DiarizationModelPaths};
use scribe_core::config::Config;

use futures::StreamExt;
use tokio::io::AsyncWriteExt;

/// The ASR checkpoints `pull` knows how to download — the two named by the
/// shipped configs. Anything else is reported as manual-fetch guidance.
const PARAKEET: &str = "parakeet-tdt-0.6b-v3";
const WHISPER_TURBO: &str = "whisper-large-v3-turbo";

/// One expected asset on disk and whether it is present.
struct Asset {
    label: &'static str,
    path: PathBuf,
    present: bool,
}

impl Asset {
    fn new(label: &'static str, path: PathBuf) -> Self {
        let present = path.exists();
        Self { label, path, present }
    }

    fn mark(&self) -> &'static str {
        if self.present {
            "[present]"
        } else {
            "[missing]"
        }
    }
}

/// The canonical expected asset set under `{models_dir}`, regardless of what's
/// actually on disk. `discover` only returns paths once files exist, so for a
/// "what should be here" report we name the default transducer + diarization
/// layout from the module docs.
fn expected_assets(models_dir: &Path) -> Vec<Asset> {
    let asr = models_dir.join("asr");
    let diar = models_dir.join("diarization");
    vec![
        Asset::new("asr/encoder.onnx", asr.join("encoder.onnx")),
        Asset::new("asr/decoder.onnx", asr.join("decoder.onnx")),
        Asset::new("asr/joiner.onnx", asr.join("joiner.onnx")),
        Asset::new("asr/tokens.txt", asr.join("tokens.txt")),
        Asset::new("diarization/segmentation.onnx", diar.join("segmentation.onnx")),
        Asset::new("diarization/embedding.onnx", diar.join("embedding.onnx")),
    ]
}

/// `scribe models list`: report expected ONNX assets + presence, plus the
/// configured Ollama models.
pub fn list(cfg: &Config) {
    let dir = &cfg.worker.models_dir;
    println!("models_dir: {}", dir.display());
    println!();

    println!("ONNX assets (sherpa-onnx):");
    let assets = expected_assets(dir);
    for a in &assets {
        println!("  {:<9} {:<30} {}", a.mark(), a.label, a.path.display());
    }

    // Use the loaders' own discovery to state the effective readiness verdict —
    // this matches exactly what the real engine checks before loading.
    let asr_ready = AsrModelPaths::discover(dir).is_some();
    let diar_ready = DiarizationModelPaths::discover(dir).is_some();
    println!();
    println!(
        "  ASR model:         {}",
        if asr_ready { "ready" } else { "not found (stub engine will be used)" }
    );
    println!(
        "  Diarization model: {}",
        if diar_ready { "ready" } else { "not found (stub engine will be used)" }
    );

    println!();
    println!("LLM server (configured):");
    println!("  provider:        {:?}", cfg.llm.provider);
    println!("  base_url:        {}", cfg.llm.base_url);
    println!("  summarize_model: {}", cfg.llm.summarize_model);
    println!("  embed_model:     {}  (dim {})", cfg.llm.embed_model, cfg.llm.embed_dim);
    println!();
    println!("ASR config: model `{}`, diarization {}", cfg.asr.model, cfg.asr.diarization);
}

// ---------------------------------------------------------------------------
// pull
// ---------------------------------------------------------------------------

/// One file to fetch: where it comes from, where it lands, and how big we
/// expect it to be.
///
/// `bytes` is advisory — it is what upstream served when this recipe was
/// written, and is used to size the plan we print and to flag a republished
/// file. The hard integrity check is the transferred byte count against the
/// server's `Content-Length`.
struct Download {
    label: String,
    url: String,
    dest: PathBuf,
    bytes: u64,
}

impl Download {
    fn new(label: impl Into<String>, url: impl Into<String>, dest: PathBuf, bytes: u64) -> Self {
        Self { label: label.into(), url: url.into(), dest, bytes }
    }
}

/// Hugging Face raw-file URL for a repo at `main`.
fn hf(repo: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/main/{file}")
}

/// The download recipe for the default stack.
///
/// ASR is the sherpa-onnx INT8 export of Parakeet-TDT-0.6B-v3, installed under
/// a per-model subdirectory so a second checkpoint can sit beside it (see
/// `AsrModelPaths::discover_for`). Diarization is pyannote-segmentation-3.0
/// plus NeMo TitaNet-large, whose 192-dim output matches the `vector(192)`
/// speaker-embedding column.
fn plan(cfg: &Config) -> Vec<Download> {
    let dir = &cfg.worker.models_dir;
    let mut out = Vec::new();

    // The ASR recipes install into a per-model subdirectory so both checkpoints
    // can sit side by side and be chosen with `[asr].model`. Whisper's exported
    // filenames differ from the names the loader looks for, so each entry
    // carries its own (remote name, local name) pair.
    let asr: &[(&str, &str, u64)] = match cfg.asr.model.as_str() {
        m if m == PARAKEET => &[
            ("encoder.int8.onnx", "encoder.int8.onnx", 652_184_281),
            ("decoder.int8.onnx", "decoder.int8.onnx", 11_845_275),
            ("joiner.int8.onnx", "joiner.int8.onnx", 6_355_277),
            ("tokens.txt", "tokens.txt", 93_939),
        ],
        m if m == WHISPER_TURBO => &[
            ("turbo-encoder.onnx", "encoder.onnx", 735_920),
            ("turbo-decoder.onnx", "decoder.onnx", 636_209_532),
            ("turbo-tokens.txt", "tokens.txt", 816_730),
            // `encoder.onnx` is an ONNX external-data stub that names this file
            // verbatim, so it must keep the upstream name and sit beside it.
            ("turbo-encoder.weights", "turbo-encoder.weights", 2_600_325_120),
        ],
        _ => &[],
    };
    if !asr.is_empty() {
        let repo = if cfg.asr.model == PARAKEET {
            "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8"
        } else {
            "csukuangfj/sherpa-onnx-whisper-turbo"
        };
        let model = &cfg.asr.model;
        let asr_dir = dir.join("asr").join(model);
        for (remote, local, bytes) in asr {
            out.push(Download::new(
                format!("asr/{model}/{local}"),
                hf(repo, remote),
                asr_dir.join(local),
                *bytes,
            ));
        }
    }

    if cfg.asr.diarization {
        let diar = dir.join("diarization");
        out.push(Download::new(
            "diarization/segmentation.onnx",
            hf("csukuangfj/sherpa-onnx-pyannote-segmentation-3-0", "model.onnx"),
            diar.join("segmentation.onnx"),
            5_992_913,
        ));
        out.push(Download::new(
            "diarization/embedding.onnx",
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_large.onnx",
            diar.join("embedding.onnx"),
            101_405_493,
        ));
    }

    out
}

fn human(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if (bytes as f64) < MB {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else if (bytes as f64) < 1024.0 * MB {
        format!("{:.0} MB", bytes as f64 / MB)
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * MB))
    }
}

/// `scribe models pull`: download every missing asset in the recipe.
///
/// Returns `false` if anything the worker needs is still absent afterwards, so
/// the caller can exit non-zero — a container that silently starts with the
/// stub engine is the failure mode this guards against.
pub async fn pull(cfg: &Config, force: bool, dry_run: bool) -> bool {
    let dir = &cfg.worker.models_dir;
    println!("models_dir: {}", dir.display());
    println!();

    let all = plan(cfg);
    if all.is_empty() {
        println!("no download recipe for `[asr].model = {}`.", cfg.asr.model);
        println!();
        return manual_guidance(cfg);
    }

    // A configured checkpoint with no recipe would otherwise look like a
    // successful short download, so say so before the plan rather than only in
    // the verdict at the end.
    if !all.iter().any(|d| d.label.starts_with("asr/")) {
        println!("note: no download recipe for `[asr].model = {}`.", cfg.asr.model);
        println!("      automatic recipes exist for `{PARAKEET}` and `{WHISPER_TURBO}`.");
        println!();
    }

    let todo: Vec<&Download> = all.iter().filter(|d| force || !d.dest.exists()).collect();

    if todo.is_empty() {
        println!("all model assets are already present — nothing to download.");
        println!("(re-download with `scribe models pull --force`)");
    } else {
        let total: u64 = todo.iter().map(|d| d.bytes).sum();
        println!("to download: {} file(s), about {}", todo.len(), human(total));
        for d in &todo {
            println!("  {:<44} {:>9}", d.label, human(d.bytes));
        }
        println!();

        if dry_run {
            println!("--dry-run: nothing was downloaded.");
            return true;
        }

        let client = match reqwest::Client::builder()
            // Model files are large and links redirect across CDNs, so there is
            // no overall deadline; a read timeout still catches a stalled
            // connection instead of hanging a container start forever.
            .read_timeout(std::time::Duration::from_secs(120))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: could not build HTTP client: {e}");
                return false;
            }
        };

        for (i, d) in todo.iter().enumerate() {
            println!("[{}/{}] {} …", i + 1, todo.len(), d.label);
            if let Err(e) = fetch(&client, d).await {
                eprintln!("      failed: {e}");
                eprintln!("      source: {}", d.url);
                return false;
            }
        }
        println!();
        println!("downloaded {} file(s).", todo.len());
    }

    println!();
    manual_guidance(cfg)
}

/// Stream one file to disk via a `.part` sibling, then rename it into place.
async fn fetch(client: &reqwest::Client, d: &Download) -> anyhow::Result<()> {
    if let Some(parent) = d.dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let resp = client.get(&d.url).send().await?.error_for_status()?;
    let expected = resp.content_length();
    if let Some(len) = expected {
        if len != d.bytes {
            println!(
                "      note: upstream serves {} (recipe expected {}) — using upstream.",
                human(len),
                human(d.bytes)
            );
        }
    }

    let part = d.dest.with_extension("part");
    let mut file = tokio::fs::File::create(&part).await?;
    let mut written: u64 = 0;
    let mut next_report: u64 = 64 * 1024 * 1024;
    let mut stream = resp.bytes_stream();

    // Any failure below leaves a partial `.part`; drop it so a re-run starts
    // clean rather than tripping over a half-written file.
    let result: anyhow::Result<()> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
            if written >= next_report {
                match expected {
                    Some(total) if total > 0 => println!(
                        "      {} / {} ({}%)",
                        human(written),
                        human(total),
                        written * 100 / total
                    ),
                    _ => println!("      {}", human(written)),
                }
                next_report = written + 64 * 1024 * 1024;
            }
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok(())
    }
    .await;

    drop(file);
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }

    // Truncated transfers are the failure we most want to catch: a short
    // encoder.onnx surfaces as an opaque ONNX load error much later.
    if let Some(total) = expected {
        if written != total {
            let _ = tokio::fs::remove_file(&part).await;
            anyhow::bail!("truncated download: got {written} of {total} bytes");
        }
    }

    tokio::fs::rename(&part, &d.dest).await?;
    println!("      ok  {} ({})", d.dest.display(), human(written));
    Ok(())
}

/// Report the effective readiness verdict, plus what to fetch by hand for any
/// asset outside the automatic recipe. Returns whether the worker is ready.
fn manual_guidance(cfg: &Config) -> bool {
    let dir = &cfg.worker.models_dir;
    let asr_ready = AsrModelPaths::discover(dir).is_some();
    let diar_ready = !cfg.asr.diarization || DiarizationModelPaths::discover(dir).is_some();

    println!("ASR model:         {}", if asr_ready { "ready" } else { "MISSING" });
    println!(
        "Diarization model: {}",
        if !cfg.asr.diarization {
            "disabled in config"
        } else if diar_ready {
            "ready"
        } else {
            "MISSING"
        }
    );

    if !asr_ready {
        println!();
        println!("Fetch the ASR checkpoint by hand and unpack it into {}/asr/:", dir.display());
        println!("  https://github.com/k2-fsa/sherpa-onnx/releases  (tag: asr-models)");
        println!("  expected files: encoder/decoder/joiner .onnx (or .int8.onnx) + tokens.txt");
        println!("  see models/README.md for the per-model layout.");
    }
    if cfg.asr.diarization && !diar_ready {
        println!();
        println!("Fetch the diarization models into {}/diarization/:", dir.display());
        println!("  segmentation.onnx — pyannote-segmentation-3.0");
        println!("  embedding.onnx    — a 192-dim speaker-embedding extractor");
    }

    println!();
    println!("LLM models are managed by the LLM server, not this directory:");
    println!("  ollama pull {}", cfg.llm.summarize_model);
    println!(
        "  embeddings: `{}` (dim {}) — fastembed downloads this on first use",
        cfg.llm.embed_model, cfg.llm.embed_dim
    );

    asr_ready && diar_ready
}
