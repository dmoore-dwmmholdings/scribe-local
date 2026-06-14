//! Stage 3 — transcribe (design §7/§8).
//!
//! Run ASR over the whole transcoded WAV (WhisperX "approach A": full-file
//! context for better text and punctuation; speaker assignment happens later in
//! `merge`). The word-level result is parked in the scratch artifacts table for
//! the merge stage. No speaker info is assigned here.

use scribe_asr::SpeechEngine;
use scribe_core::config::Config;
use scribe_core::storage;
use scribe_core::types::Word;
use scribe_core::Result;
use scribe_db::Db;
use uuid::Uuid;

use crate::artifacts::{self, TranscriptArtifact};
use crate::stages::stage_err;

const STAGE: &str = "transcribe";

/// Run the transcribe stage for `recording_id`.
pub async fn run(
    cfg: &Config,
    db: &Db,
    speech: &SpeechEngine,
    recording_id: Uuid,
) -> Result<()> {
    let wav = storage::wav_path(cfg.storage.blobs.as_path(), recording_id);
    if !wav.exists() {
        return Err(stage_err(
            STAGE,
            format!("transcoded WAV missing: {}", wav.display()),
        ));
    }

    let transcript = speech
        .transcriber()
        .transcribe(&wav)
        .map_err(|e| stage_err(STAGE, e))?;

    // Map ASR words → core `Word` (no speaker yet; `local_idx` is None).
    let words: Vec<Word> = transcript
        .words
        .iter()
        .map(|w| Word {
            text: w.text.clone(),
            start_ms: w.start_ms,
            end_ms: w.end_ms,
            conf: w.conf,
            local_idx: None,
        })
        .collect();

    let artifact = TranscriptArtifact {
        text: transcript.text,
        words,
    };
    artifacts::put_transcript(db, recording_id, &artifact).await?;

    tracing::info!(
        %recording_id,
        words = artifact.words.len(),
        "transcribe complete"
    );
    Ok(())
}
