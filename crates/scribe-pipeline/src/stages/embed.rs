//! Stage 5 — embed (design §9).
//!
//! Chunk the merged transcript into retrieval windows, embed each window, and
//! replace the recording's chunks. Chunking is by **speaker turn**, further
//! split by a sliding ~45 s window so long monologues become several
//! well-scoped chunks (each carries its own `start_ms`/`end_ms`/`local_idx`).
//! The embedder's dimension equals `cfg.llm.embed_dim` by construction
//! (`build_embedder` enforces it), which matches the `chunks.embedding` width.

use std::sync::Arc;

use scribe_core::config::Config;
use scribe_core::types::Utterance;
use scribe_core::Result;
use scribe_db::chunks::NewChunk;
use scribe_db::Db;
use scribe_llm::Embedder;
use uuid::Uuid;

use crate::stages::stage_err;

const STAGE: &str = "embed";

/// Target chunk duration before a speaker turn is split into windows.
const WINDOW_MS: i64 = 45_000;
/// Overlap carried between adjacent windows of the same turn (context bleed).
const WINDOW_OVERLAP_MS: i64 = 5_000;

/// A retrieval chunk before embedding.
struct PendingChunk {
    start_ms: i64,
    end_ms: i64,
    local_idx: Option<i32>,
    text: String,
}

/// Run the embed stage for `recording_id`.
pub async fn run(
    _cfg: &Config,
    db: &Db,
    embedder: &Arc<dyn Embedder>,
    recording_id: Uuid,
) -> Result<()> {
    let utterances = db.list_utterances_by_recording(recording_id).await?;
    let pending = build_chunks(&utterances);

    if pending.is_empty() {
        // Nothing to embed (empty transcript). Clear any stale chunks and return.
        db.delete_chunks_by_recording(recording_id).await?;
        tracing::info!(%recording_id, "embed: no transcript content, cleared chunks");
        return Ok(());
    }

    let texts: Vec<String> = pending.iter().map(|c| c.text.clone()).collect();
    let vectors = embedder.embed(&texts).await?;
    if vectors.len() != pending.len() {
        return Err(stage_err(
            STAGE,
            format!(
                "embedder returned {} vectors for {} chunks",
                vectors.len(),
                pending.len()
            ),
        ));
    }

    let new: Vec<NewChunk> = pending
        .into_iter()
        .zip(vectors)
        .map(|(c, embedding)| NewChunk {
            start_ms: Some(c.start_ms),
            end_ms: Some(c.end_ms),
            local_idx: c.local_idx,
            text: c.text,
            embedding,
        })
        .collect();

    db.delete_chunks_by_recording(recording_id).await?;
    let inserted = db.insert_chunks(recording_id, &new).await?;
    tracing::info!(%recording_id, chunks = inserted, "embed complete");
    Ok(())
}

/// Build retrieval chunks from utterances: one or more per turn, each ≤ ~45 s.
fn build_chunks(utterances: &[Utterance]) -> Vec<PendingChunk> {
    let mut out = Vec::new();
    for u in utterances {
        let text = u.text.trim();
        if text.is_empty() {
            continue;
        }
        let span = u.end_ms - u.start_ms;
        if span <= WINDOW_MS || u.words.is_empty() {
            out.push(PendingChunk {
                start_ms: u.start_ms,
                end_ms: u.end_ms,
                local_idx: u.local_idx,
                text: text.to_string(),
            });
        } else {
            out.extend(window_utterance(u));
        }
    }
    out
}

/// Split a long utterance into overlapping ~45 s windows by word timing.
fn window_utterance(u: &Utterance) -> Vec<PendingChunk> {
    let mut out = Vec::new();
    let mut win_start = u.start_ms;
    while win_start < u.end_ms {
        let win_end = (win_start + WINDOW_MS).min(u.end_ms);
        let words: Vec<&str> = u
            .words
            .iter()
            .filter(|w| w.end_ms > win_start && w.start_ms < win_end)
            .map(|w| w.text.as_str())
            .collect();
        if !words.is_empty() {
            // Tighten the window bounds to the words actually captured.
            let actual_start = u
                .words
                .iter()
                .find(|w| w.end_ms > win_start && w.start_ms < win_end)
                .map(|w| w.start_ms)
                .unwrap_or(win_start);
            let actual_end = u
                .words
                .iter()
                .filter(|w| w.end_ms > win_start && w.start_ms < win_end)
                .map(|w| w.end_ms)
                .max()
                .unwrap_or(win_end);
            out.push(PendingChunk {
                start_ms: actual_start,
                end_ms: actual_end,
                local_idx: u.local_idx,
                text: words.join(" "),
            });
        }
        if win_end >= u.end_ms {
            break;
        }
        win_start = win_end - WINDOW_OVERLAP_MS;
    }
    out
}
