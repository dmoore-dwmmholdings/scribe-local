//! Model-asset layout under the worker's `models_dir`.
//!
//! `scribe models pull` (design §4) populates this directory with the ONNX
//! assets the real engine needs. This module is the single source of truth for
//! the expected filenames and the presence check the [`SpeechEngine`] loader
//! uses to decide between the real and stub engines.
//!
//! Layout (relative to `models_dir`):
//! ```text
//! models/
//!   asr/                         # ASR model (Parakeet TDT transducer by default)
//!     encoder.onnx
//!     decoder.onnx
//!     joiner.onnx
//!     tokens.txt
//!   diarization/
//!     segmentation.onnx          # pyannote-segmentation-3.0
//!     embedding.onnx             # 3D-Speaker / WeSpeaker / NeMo (192-dim)
//! ```
//!
//! Whisper-style models (a single `*-encoder.onnx` / `*-decoder.onnx` pair plus
//! `tokens.txt`) are also supported by [`AsrModelPaths::discover`].

use std::path::{Path, PathBuf};

/// Resolved paths for the ASR model.
#[derive(Debug, Clone)]
pub enum AsrModelPaths {
    /// A transducer model (Parakeet, Zipformer): encoder + decoder + joiner.
    Transducer {
        encoder: PathBuf,
        decoder: PathBuf,
        joiner: PathBuf,
        tokens: PathBuf,
    },
    /// A Whisper model: encoder + decoder + tokens.
    Whisper {
        encoder: PathBuf,
        decoder: PathBuf,
        tokens: PathBuf,
    },
}

/// Resolved paths for the diarization models.
#[derive(Debug, Clone)]
pub struct DiarizationModelPaths {
    /// pyannote segmentation ONNX.
    pub segmentation: PathBuf,
    /// Speaker-embedding extractor ONNX (also reused for `embed_speaker`).
    pub embedding: PathBuf,
}

/// First existing path among `candidates`, else `None`.
fn first_existing(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|p| p.exists())
}

impl AsrModelPaths {
    /// Locate the ASR model files under `{models_dir}/asr`, preferring a
    /// transducer layout (the Parakeet default) and falling back to Whisper.
    ///
    /// Looks in the flat `{models_dir}/asr` first, then in any per-model
    /// subdirectory, so `scribe models`/`doctor` report a model as present
    /// whichever layout is installed.
    pub fn discover(models_dir: &Path) -> Option<AsrModelPaths> {
        let base = models_dir.join("asr");
        if let Some(paths) = layout_in(&base) {
            return Some(paths);
        }
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&base)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs.iter().find_map(|d| layout_in(d))
    }

    /// Locate the model named by `[asr].model`, preferring `{models_dir}/asr/{model}`.
    ///
    /// Whisper and transducer checkpoints use the same generic filenames
    /// (`encoder.onnx`, `decoder.onnx`, `tokens.txt`), so a flat directory can
    /// only ever hold one of them. A per-model subdirectory lets several be
    /// installed at once and selected by config. Falls back to the flat layout
    /// so existing single-model installs keep working.
    pub fn discover_for(models_dir: &Path, model: &str) -> Option<AsrModelPaths> {
        let base = models_dir.join("asr");
        if let Some(dir) = model_subdir(&base, model) {
            if let Some(paths) = layout_in(&dir) {
                return Some(paths);
            }
        }
        layout_in(&base)
    }
}

impl AsrModelPaths {
    /// `"transducer"` or `"whisper"` — which decode path this checkpoint drives.
    pub fn layout_name(&self) -> &'static str {
        match self {
            AsrModelPaths::Transducer { .. } => "transducer",
            AsrModelPaths::Whisper { .. } => "whisper",
        }
    }

    /// The directory the model was loaded from (useful for logging which of
    /// several installed models actually got picked).
    pub fn dir(&self) -> &Path {
        let tokens = match self {
            AsrModelPaths::Transducer { tokens, .. } => tokens,
            AsrModelPaths::Whisper { tokens, .. } => tokens,
        };
        tokens.parent().unwrap_or(Path::new("."))
    }
}

/// Resolve `{base}/{model}`, matching the directory name case-insensitively so
/// the config value need not match the on-disk casing exactly.
fn model_subdir(base: &Path, model: &str) -> Option<PathBuf> {
    let want = model.trim();
    if want.is_empty() {
        return None;
    }
    let direct = base.join(want);
    if direct.is_dir() {
        return Some(direct);
    }
    std::fs::read_dir(base)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case(want))
        })
}

/// Detect the model layout from the files present in `dir`.
///
/// The joiner is decisive: only a transducer has one, and Whisper shares every
/// other filename with it. Keying off the files rather than the configured
/// model name means a misnamed `[asr].model` can no longer load a Parakeet
/// checkpoint as if it were Whisper (they differ only by the joiner).
fn layout_in(dir: &Path) -> Option<AsrModelPaths> {
    let tokens = dir.join("tokens.txt");
    if !tokens.exists() {
        return None;
    }
    transducer_paths(dir, &tokens).or_else(|| whisper_paths(dir, &tokens))
}

/// Transducer (Parakeet / Zipformer): encoder + decoder + joiner.
fn transducer_paths(dir: &Path, tokens: &Path) -> Option<AsrModelPaths> {
    let encoder = first_existing([dir.join("encoder.onnx"), dir.join("encoder.int8.onnx")])?;
    let decoder = first_existing([dir.join("decoder.onnx"), dir.join("decoder.int8.onnx")])?;
    let joiner = first_existing([dir.join("joiner.onnx"), dir.join("joiner.int8.onnx")])?;
    Some(AsrModelPaths::Transducer {
        encoder,
        decoder,
        joiner,
        tokens: tokens.to_path_buf(),
    })
}

/// Whisper: encoder + decoder (no joiner) + tokens.
fn whisper_paths(dir: &Path, tokens: &Path) -> Option<AsrModelPaths> {
    let encoder = first_existing([dir.join("encoder.onnx"), dir.join("whisper-encoder.onnx")])?;
    let decoder = first_existing([dir.join("decoder.onnx"), dir.join("whisper-decoder.onnx")])?;
    Some(AsrModelPaths::Whisper {
        encoder,
        decoder,
        tokens: tokens.to_path_buf(),
    })
}

impl DiarizationModelPaths {
    /// Locate the diarization model files under `{models_dir}/diarization`.
    pub fn discover(models_dir: &Path) -> Option<DiarizationModelPaths> {
        let dir = models_dir.join("diarization");
        let segmentation = first_existing([
            dir.join("segmentation.onnx"),
            dir.join("segmentation.int8.onnx"),
        ])?;
        let embedding =
            first_existing([dir.join("embedding.onnx"), dir.join("embedding.int8.onnx")])?;
        Some(DiarizationModelPaths {
            segmentation,
            embedding,
        })
    }
}

/// The speaker-embedding model path (reused by both diarization and enrollment).
pub fn embedding_model_path(models_dir: &Path) -> Option<PathBuf> {
    DiarizationModelPaths::discover(models_dir).map(|d| d.embedding)
}

/// True when *all* assets the real engine needs are present: the ASR model and
/// (when diarization is enabled) the diarization models. The loader uses this to
/// decide whether to instantiate the real engine or fall back to the stub.
pub fn real_models_present(models_dir: &Path, model: &str, diarization: bool) -> bool {
    // Check the model that will actually be loaded, not just "any model".
    if AsrModelPaths::discover_for(models_dir, model).is_none() {
        return false;
    }
    if diarization && DiarizationModelPaths::discover(models_dir).is_none() {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build `{root}/asr/{sub}` (or the flat `{root}/asr` when `sub` is empty)
    /// containing `files`, and return `root`.
    fn install(dir: &Path, sub: &str, files: &[&str]) {
        let target = if sub.is_empty() {
            dir.join("asr")
        } else {
            dir.join("asr").join(sub)
        };
        fs::create_dir_all(&target).unwrap();
        for f in files {
            fs::write(target.join(f), b"x").unwrap();
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("scribe_models_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    const WHISPER: &[&str] = &["encoder.onnx", "decoder.onnx", "tokens.txt"];
    const PARAKEET: &[&str] = &["encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt"];

    /// Both models installed side by side, each selected by name.
    #[test]
    fn per_model_subdirectories_can_coexist() {
        let root = tmp("coexist");
        install(&root, "whisper-large-v3-turbo", WHISPER);
        install(&root, "parakeet-tdt-0.6b-v3", PARAKEET);

        let w = AsrModelPaths::discover_for(&root, "whisper-large-v3-turbo").unwrap();
        assert_eq!(w.layout_name(), "whisper");
        let p = AsrModelPaths::discover_for(&root, "parakeet-tdt-0.6b-v3").unwrap();
        assert_eq!(p.layout_name(), "transducer");

        let _ = fs::remove_dir_all(&root);
    }

    /// The layout comes from the files, never the configured name. A Parakeet
    /// checkpoint in a directory named "whisper-…" must still load as a
    /// transducer — the old name-based rule loaded it as Whisper, because the
    /// two layouts differ only by the joiner.
    #[test]
    fn layout_is_detected_from_files_not_the_model_name() {
        let root = tmp("layout");
        install(&root, "whisper-but-actually-parakeet", PARAKEET);

        let paths = AsrModelPaths::discover_for(&root, "whisper-but-actually-parakeet").unwrap();
        assert_eq!(paths.layout_name(), "transducer");

        let _ = fs::remove_dir_all(&root);
    }

    /// Existing installs put the model straight in `asr/` with no subdirectory.
    #[test]
    fn flat_layout_still_resolves() {
        let root = tmp("flat");
        install(&root, "", WHISPER);

        assert_eq!(
            AsrModelPaths::discover_for(&root, "whisper-large-v3-turbo")
                .unwrap()
                .layout_name(),
            "whisper"
        );
        // And with no matching subdirectory, an unknown name still finds it.
        assert!(AsrModelPaths::discover_for(&root, "something-else").is_some());
        assert!(AsrModelPaths::discover(&root).is_some());

        let _ = fs::remove_dir_all(&root);
    }

    /// `discover` backs the CLI/doctor readiness check, so it must see a model
    /// installed only in a subdirectory.
    #[test]
    fn discover_finds_a_subdirectory_only_install() {
        let root = tmp("subonly");
        install(&root, "parakeet-tdt-0.6b-v3", PARAKEET);

        let found = AsrModelPaths::discover(&root).expect("should find the subdir model");
        assert_eq!(found.layout_name(), "transducer");

        let _ = fs::remove_dir_all(&root);
    }

    /// A model directory missing `tokens.txt` is not usable.
    #[test]
    fn incomplete_model_is_not_discovered() {
        let root = tmp("incomplete");
        install(&root, "broken", &["encoder.onnx", "decoder.onnx"]);

        assert!(AsrModelPaths::discover_for(&root, "broken").is_none());
        assert!(!real_models_present(&root, "broken", false));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn subdirectory_match_ignores_case() {
        let root = tmp("case");
        install(&root, "Parakeet-TDT", PARAKEET);

        assert_eq!(
            AsrModelPaths::discover_for(&root, "parakeet-tdt")
                .unwrap()
                .layout_name(),
            "transducer"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
