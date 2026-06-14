//! `scribe-llm` — local LLM indexing & search building blocks (design §9).
//!
//! This crate provides two things the pipeline and API depend on:
//!
//! * [`ChatClient`] — a small HTTP client for the local LLM server (summaries,
//!   RAG answers). Supports Ollama and OpenAI-compatible servers (LM Studio,
//!   llama.cpp, vLLM) via `[llm].provider`. Plain reqwest, always available.
//!   (`OllamaClient` is a back-compat alias.)
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
//! use scribe_llm::{ChatClient, ChatMessage, build_embedder};
//! let cfg = scribe_core::config::LlmConfig::default();
//!
//! let embedder = build_embedder(&cfg)?;
//! let q = embedder.embed_one("what did we decide about pricing?").await?;
//! assert_eq!(q.len(), cfg.embed_dim);
//!
//! let chat = ChatClient::from_config(&cfg); // Ollama or LM Studio per cfg.provider
//! let answer = chat
//!     .chat(&cfg.summarize_model, &[ChatMessage::user("Summarize the meeting.")])
//!     .await?;
//! println!("{answer}");
//! # Ok(())
//! # }
//! ```

mod embed;
mod ollama;

pub use embed::{build_embedder, Embedder};
pub use ollama::{ChatClient, ChatMessage, ChatOptions};

/// Back-compat alias for [`ChatClient`] (the client now supports more than Ollama).
pub use ollama::ChatClient as OllamaClient;
