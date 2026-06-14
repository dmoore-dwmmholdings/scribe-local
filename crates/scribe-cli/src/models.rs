//! `scribe models <list|pull>` — model-asset reporting (design §4).
//!
//! The expected ONNX layout under `cfg.worker.models_dir` is owned by
//! [`scribe_asr::models`]; we report each expected asset and whether it exists,
//! then list the configured Ollama models. `pull` doesn't download multi-GB
//! models in this environment — it verifies presence and prints exactly what to
//! fetch and where (it never fails hard on a missing asset).

use std::path::{Path, PathBuf};

use scribe_asr::models::{AsrModelPaths, DiarizationModelPaths};
use scribe_core::config::Config;

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
    println!("Ollama models (configured):");
    println!("  ollama_url:      {}", cfg.llm.ollama_url);
    println!("  summarize_model: {}", cfg.llm.summarize_model);
    println!("  embed_model:     {}  (dim {})", cfg.llm.embed_model, cfg.llm.embed_dim);
    println!();
    println!("ASR config: model `{}`, diarization {}", cfg.asr.model, cfg.asr.diarization);
}

/// `scribe models pull`: verify presence and print exactly what to fetch and
/// where. This environment can't download multi-GB models, so we never fail
/// hard — we surface guidance for any missing asset.
pub async fn pull(cfg: &Config) {
    let dir = &cfg.worker.models_dir;
    println!("models_dir: {}", dir.display());
    println!();

    let assets = expected_assets(dir);
    let missing: Vec<&Asset> = assets.iter().filter(|a| !a.present).collect();

    if missing.is_empty() {
        println!("all expected ONNX assets are present — nothing to fetch.");
    } else {
        println!("missing ONNX assets ({}):", missing.len());
        for a in &missing {
            println!("  - {} → {}", a.label, a.path.display());
        }
        println!();
        println!("This environment does not auto-download multi-GB models. Fetch the");
        println!("sherpa-onnx assets and unpack them into the layout above:");
        println!();
        println!("  ASR (Parakeet TDT 0.6B v3, the default in [asr].model):");
        println!("    https://github.com/k2-fsa/sherpa-onnx/releases  (sherpa-onnx-* parakeet bundle)");
        println!("    place encoder/decoder/joiner.onnx + tokens.txt under {}/asr/", dir.display());
        println!();
        println!("  Diarization (pyannote segmentation 3.0 + 3D-Speaker/WeSpeaker embedding):");
        println!("    https://github.com/k2-fsa/sherpa-onnx/releases  (speaker-diarization models)");
        println!("    place segmentation.onnx + embedding.onnx under {}/diarization/", dir.display());
    }

    println!();
    println!("Ollama models (pull with the Ollama CLI on the processing node):");
    println!("  ollama pull {}", cfg.llm.summarize_model);
    println!(
        "  embeddings: model `{}` (dim {}) — pull/serve per your embedding backend",
        cfg.llm.embed_model, cfg.llm.embed_dim
    );
    println!();
    println!(
        "Ollama endpoint: {} (run `scribe doctor` to verify reachability).",
        cfg.llm.ollama_url
    );
}
