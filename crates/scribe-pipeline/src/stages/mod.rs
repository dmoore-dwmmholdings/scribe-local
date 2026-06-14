//! The pipeline stages (design §7 DAG): one module per job kind.
//!
//! Each stage exposes a `run(...)` that takes whatever it needs (config, the
//! [`Db`](scribe_db::Db) handle, and — for the ML stages — a borrowed engine or
//! embedder/Ollama client) and performs its side effects (write the WAV, write
//! artifacts, write utterances/chunks/summary). Stage drivers live in
//! [`crate::worker`] (queue) and [`crate::process_recording_inline`] (inline).

use scribe_core::Error;

pub mod diarize;
pub mod embed;
pub mod merge;
pub mod summarize;
pub mod transcode;
pub mod transcribe;
pub mod transcribe_segment;

/// Build a [`Error::Pipeline`] tagged with the stage name.
pub(crate) fn stage_err<E: std::fmt::Display>(stage: &'static str, source: E) -> Error {
    Error::pipeline(stage, source)
}
