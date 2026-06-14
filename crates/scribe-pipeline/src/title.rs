//! Continuous title regeneration from the partial transcript during live
//! (incremental) transcription. Best-effort — a failure never blocks the stage.

use scribe_core::config::Config;
use scribe_core::Result;
use scribe_db::Db;
use scribe_llm::{ChatMessage, OllamaClient};
use uuid::Uuid;

const SYSTEM: &str = "You name meetings. Given a partial meeting transcript, reply with \
                      ONLY a short, specific title (at most 8 words). No quotes, no trailing \
                      punctuation, no preamble.";

/// Cap on transcript characters fed to the title model — keep the prompt small
/// and recent-biased; titles don't need the whole thing.
const MAX_CHARS: usize = 6000;

/// Regenerate the recording's provisional title from its current utterances.
/// Best-effort: logs and returns `Ok` on any LLM/transport issue, and only
/// updates an auto/empty title (never a user-provided one).
pub async fn regenerate(
    cfg: &Config,
    db: &Db,
    ollama: &OllamaClient,
    recording_id: Uuid,
) -> Result<()> {
    let utts = db.list_utterances_by_recording(recording_id).await?;
    if utts.is_empty() {
        return Ok(());
    }

    let mut text = String::new();
    for u in &utts {
        text.push_str(u.text.trim());
        text.push('\n');
    }
    if text.len() > MAX_CHARS {
        // Keep the most recent context.
        text = text.split_off(text.len() - MAX_CHARS);
    }

    let messages = [ChatMessage::system(SYSTEM), ChatMessage::user(text)];
    match ollama.chat(&cfg.llm.summarize_model, &messages).await {
        Ok(raw) => {
            if let Some(title) = clean_title(&raw) {
                let _ = db.set_recording_title_auto(recording_id, &title).await;
                tracing::info!(%recording_id, title = %title, "regenerated provisional title");
            }
        }
        Err(e) => {
            tracing::debug!(%recording_id, error = %e, "title regeneration skipped (LLM unavailable)");
        }
    }
    Ok(())
}

/// First non-empty line, surrounding quotes stripped, length-capped.
fn clean_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let t = line
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .trim();
    if t.is_empty() {
        return None;
    }
    Some(t.chars().take(120).collect())
}
