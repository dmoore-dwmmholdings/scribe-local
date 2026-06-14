//! Text embeddings.
//!
//! Two backends share one [`Embedder`] trait:
//!
//! * `real` (`#[cfg(feature = "local-embed")]`) — in-process ONNX embeddings via
//!   the `fastembed` crate (design §9). Real models, real native deps.
//! * `stub` — a deterministic hash embedding with no native dependencies, used
//!   when the crate is built `--no-default-features`. It keeps the search/RAG
//!   pipeline exercisable on machines without an ONNX runtime: identical text
//!   yields identical, L2-normalized vectors so cosine similarity is meaningful.
//!
//! [`build_embedder`] picks the backend, then verifies the produced dimension
//! matches `cfg.embed_dim` (a schema commitment — it must equal the
//! `chunks.embedding` column width) and errors with [`Error::Config`] otherwise.

use std::sync::Arc;

use async_trait::async_trait;

use scribe_core::config::LlmConfig;
use scribe_core::error::{Error, Result};

/// Produces embedding vectors for text.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Embed a batch of documents. Output order matches input order, and every
    /// vector has length [`Embedder::dim`].
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Embed a single query (convenience wrapper over [`Embedder::embed`]).
    async fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut out = self.embed(std::slice::from_ref(&text.to_string())).await?;
        out.pop()
            .ok_or_else(|| Error::Model("embedder returned no vector for input".to_string()))
    }

    /// The fixed output dimension.
    fn dim(&self) -> usize;
}

/// Build the configured embedder.
///
/// With the `local-embed` feature this returns a fastembed-backed embedder for
/// `cfg.embed_model`; otherwise a deterministic hash stub of width
/// `cfg.embed_dim`. Either way the returned dimension is checked against
/// `cfg.embed_dim` and a mismatch is an [`Error::Config`] (the column width and
/// the model must agree).
pub fn build_embedder(cfg: &LlmConfig) -> Result<Arc<dyn Embedder>> {
    #[cfg(feature = "local-embed")]
    let embedder: Arc<dyn Embedder> = Arc::new(real::FastEmbedder::new(cfg)?);

    #[cfg(not(feature = "local-embed"))]
    let embedder: Arc<dyn Embedder> = Arc::new(stub::HashEmbedder::new(cfg.embed_dim));

    let got = embedder.dim();
    if got != cfg.embed_dim {
        return Err(Error::Config(format!(
            "embedding dimension mismatch: model `{}` produces {got}-dim vectors but \
             config embed_dim = {} (must match the chunks.embedding column width)",
            cfg.embed_model, cfg.embed_dim
        )));
    }
    Ok(embedder)
}

// ===========================================================================
// Real backend: fastembed (ONNX)
// ===========================================================================
#[cfg(feature = "local-embed")]
mod real {
    use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

    use super::*;

    /// Map a config model id to a fastembed [`EmbeddingModel`].
    ///
    /// fastembed v4 ships `NomicEmbedTextV15` (768-dim) but has no Qwen3
    /// embedding variant, so the recommended `Qwen3-Embedding-0.6B` falls back
    /// to Nomic with a warning. Unknown ids likewise fall back so the worker
    /// still starts; the dimension check in [`build_embedder`] is the backstop
    /// that catches a fallback whose width doesn't match the configured column.
    fn resolve_model(id: &str) -> EmbeddingModel {
        let norm = id.trim().to_ascii_lowercase();
        match norm.as_str() {
            "nomic-embed-text" | "nomic-embed-text-v1.5" | "nomicembedtextv15" => {
                EmbeddingModel::NomicEmbedTextV15
            }
            "all-minilm-l6-v2" | "allminilml6v2" => EmbeddingModel::AllMiniLML6V2,
            "bge-small-en-v1.5" | "bgesmallenv15" => EmbeddingModel::BGESmallENV15,
            "bge-base-en-v1.5" | "bgebaseenv15" => EmbeddingModel::BGEBaseENV15,
            "multilingual-e5-large" | "multilinguale5large" => {
                EmbeddingModel::MultilingualE5Large
            }
            "qwen3-embedding-0.6b" => {
                tracing::warn!(
                    requested = id,
                    "fastembed v4 has no Qwen3 embedding model; falling back to \
                     nomic-embed-text (NomicEmbedTextV15, 768-dim). Set embed_dim=768 \
                     or serve Qwen3 via TEI/Infinity over HTTP."
                );
                EmbeddingModel::NomicEmbedTextV15
            }
            other => {
                tracing::warn!(
                    requested = other,
                    "unknown embed_model; falling back to nomic-embed-text (768-dim)"
                );
                EmbeddingModel::NomicEmbedTextV15
            }
        }
    }

    /// A fastembed-backed embedder. fastembed is synchronous/blocking, so the
    /// async trait methods hop onto `spawn_blocking`.
    pub struct FastEmbedder {
        /// `Arc` so we can move a clone into `spawn_blocking` without borrowing
        /// `&self` across the await point.
        model: Arc<TextEmbedding>,
        dim: usize,
        model_id: String,
    }

    impl FastEmbedder {
        pub fn new(cfg: &LlmConfig) -> Result<Self> {
            let model_kind = resolve_model(&cfg.embed_model);
            let model = TextEmbedding::try_new(
                InitOptions::new(model_kind).with_show_download_progress(false),
            )
            .map_err(|e| Error::Model(format!("loading fastembed model failed: {e}")))?;

            // fastembed exposes no direct dim accessor, so probe with one short
            // document and measure the output width. This is the ground truth we
            // compare against cfg.embed_dim.
            let probe = model
                .embed(vec!["dimension probe"], None)
                .map_err(|e| Error::Model(format!("fastembed dimension probe failed: {e}")))?;
            let dim = probe
                .first()
                .map(|v| v.len())
                .ok_or_else(|| Error::Model("fastembed returned no probe vector".to_string()))?;

            Ok(Self {
                model: Arc::new(model),
                dim,
                model_id: cfg.embed_model.clone(),
            })
        }
    }

    #[async_trait]
    impl Embedder for FastEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let model = Arc::clone(&self.model);
            let owned: Vec<String> = texts.to_vec();
            let model_id = self.model_id.clone();
            tokio::task::spawn_blocking(move || {
                model
                    .embed(owned, None)
                    .map_err(|e| Error::Model(format!("fastembed `{model_id}` embed failed: {e}")))
            })
            .await
            .map_err(|e| Error::Model(format!("embedding task panicked/cancelled: {e}")))?
        }

        fn dim(&self) -> usize {
            self.dim
        }
    }
}

// ===========================================================================
// Stub backend: deterministic hash embedding (no native deps)
// ===========================================================================
pub mod stub {
    use super::*;
    use std::hash::{Hash, Hasher};

    /// A dependency-free embedder. It is **not** semantically meaningful, but it
    /// is deterministic and L2-normalized, so identical inputs collide (cosine
    /// ≈ 1) and unrelated inputs spread out — enough to exercise the
    /// vector-search / RAG plumbing without an ONNX runtime.
    ///
    /// Construction: lowercase + split on non-alphanumerics into tokens, hash
    /// each token into a bucket in `[0, dim)` with a sign, accumulate, then L2
    /// normalize. Empty/whitespace text yields a fixed unit vector so it is
    /// still normalized.
    pub struct HashEmbedder {
        dim: usize,
    }

    impl HashEmbedder {
        pub fn new(dim: usize) -> Self {
            // A zero-width embedder can't produce normalizable vectors; clamp to
            // at least 1 so the invariant (unit length) always holds.
            Self { dim: dim.max(1) }
        }

        fn embed_text(&self, text: &str) -> Vec<f32> {
            let mut v = vec![0.0f32; self.dim];
            let mut any = false;
            for token in text
                .split(|c: char| !c.is_alphanumeric())
                .filter(|t| !t.is_empty())
            {
                any = true;
                let lower = token.to_ascii_lowercase();
                let h = stable_hash(&lower);
                let idx = (h % self.dim as u64) as usize;
                // Use a second hash bit for the sign so buckets don't all push
                // the same direction.
                let sign = if (h >> 1) & 1 == 0 { 1.0 } else { -1.0 };
                // Weight by a third hash slice so repeated tokens still vary the
                // magnitude a little; keeps near-duplicate texts close but not
                // forced identical unless they truly match.
                let weight = 1.0 + ((h >> 2) & 0x7) as f32 / 8.0;
                v[idx] += sign * weight;
            }
            if !any {
                // Deterministic non-zero vector for empty input.
                v[0] = 1.0;
            }
            l2_normalize(&mut v);
            v
        }
    }

    #[async_trait]
    impl Embedder for HashEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|t| self.embed_text(t)).collect())
        }

        fn dim(&self) -> usize {
            self.dim
        }
    }

    /// Hash a string to a stable u64. `DefaultHasher` (SipHash) is stable within
    /// a process run, which is all the stub needs; vectors are recomputed on
    /// reindex, never persisted across binaries.
    fn stable_hash(s: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }

    /// Normalize a vector to unit L2 length in place. A zero vector is left
    /// unchanged (it cannot be normalized), but `embed_text` never produces one.
    fn l2_normalize(v: &mut [f32]) {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn cosine(a: &[f32], b: &[f32]) -> f32 {
            a.iter().zip(b).map(|(x, y)| x * y).sum()
        }

        fn l2(v: &[f32]) -> f32 {
            v.iter().map(|x| x * x).sum::<f32>().sqrt()
        }

        #[tokio::test]
        async fn embed_one_has_configured_dim() {
            let e = HashEmbedder::new(768);
            let v = e.embed_one("the quarterly planning meeting").await.unwrap();
            assert_eq!(v.len(), 768);
            assert_eq!(e.dim(), 768);
        }

        #[tokio::test]
        async fn identical_inputs_are_identical_and_cosine_one() {
            let e = HashEmbedder::new(256);
            let a = e.embed_one("budget review with finance").await.unwrap();
            let b = e.embed_one("budget review with finance").await.unwrap();
            assert_eq!(a, b, "identical text must yield identical vectors");
            let cos = cosine(&a, &b);
            assert!((cos - 1.0).abs() < 1e-5, "cosine of identical = {cos}");
        }

        #[tokio::test]
        async fn output_is_l2_normalized() {
            let e = HashEmbedder::new(384);
            for text in [
                "hello world",
                "a much longer sentence about distributed systems and queues",
                "single",
                "",
                "    ",
                "punctuation!!! and---symbols???",
            ] {
                let v = e.embed_one(text).await.unwrap();
                assert_eq!(v.len(), 384);
                let n = l2(&v);
                assert!((n - 1.0).abs() < 1e-5, "‖v‖={n} for input {text:?}");
            }
        }

        #[tokio::test]
        async fn different_inputs_generally_differ() {
            let e = HashEmbedder::new(512);
            let a = e.embed_one("alpha beta gamma").await.unwrap();
            let b = e.embed_one("completely unrelated wording here").await.unwrap();
            assert_ne!(a, b);
            let cos = cosine(&a, &b);
            assert!(cos < 0.99, "unrelated texts should not be near-identical: {cos}");
        }

        #[tokio::test]
        async fn batch_matches_single() {
            let e = HashEmbedder::new(128);
            let texts = vec!["one".to_string(), "two".to_string(), "three".to_string()];
            let batch = e.embed(&texts).await.unwrap();
            assert_eq!(batch.len(), 3);
            for (i, t) in texts.iter().enumerate() {
                let single = e.embed_one(t).await.unwrap();
                assert_eq!(batch[i], single);
            }
        }

        #[tokio::test]
        async fn empty_batch_returns_empty() {
            let e = HashEmbedder::new(64);
            let out = e.embed(&[]).await.unwrap();
            assert!(out.is_empty());
        }
    }
}
