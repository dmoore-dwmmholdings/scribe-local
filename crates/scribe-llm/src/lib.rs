//! `scribe-llm` — local LLM indexing & search building blocks (design §9).
//!
//! This crate provides two things the pipeline and API depend on:
//!
//! * [`OllamaClient`] — a small HTTP client for the local Ollama server
//!   (summaries, RAG answers). Plain reqwest, always available.
//! * [`Embedder`] — the embedding abstraction used for semantic search / RAG,
//!   with a real fastembed/ONNX backend (default feature `local-embed`) and a
//!   dependency-free deterministic hash stub (`--no-default-features`).
//!
//! ## Cargo features
//! * `local-embed` (default): real in-process embeddings via `fastembed`.
//! * disabled: [`build_embedder`] returns the hash stub so search/RAG stay
//!   runnable without an ONNX runtime.
//!
//! ```no_run
//! # async fn demo() -> scribe_core::Result<()> {
//! use scribe_llm::{OllamaClient, ChatMessage, build_embedder};
//! let cfg = scribe_core::config::LlmConfig::default();
//!
//! let embedder = build_embedder(&cfg)?;
//! let q = embedder.embed_one("what did we decide about pricing?").await?;
//! assert_eq!(q.len(), cfg.embed_dim);
//!
//! let ollama = OllamaClient::new(&cfg.ollama_url);
//! let answer = ollama
//!     .chat(&cfg.summarize_model, &[ChatMessage::user("Summarize the meeting.")])
//!     .await?;
//! println!("{answer}");
//! # Ok(())
//! # }
//! ```

mod embed;
mod ollama;

pub use embed::{build_embedder, Embedder};
pub use ollama::{ChatMessage, ChatOptions, OllamaClient};
