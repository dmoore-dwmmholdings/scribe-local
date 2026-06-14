//! Incremental ("live") transcription.
//!
//! Triggered as each segment uploads *during* recording: transcode the newly
//! arrived segment(s), run ASR, strip fillers, offset the timing onto the global
//! timeline, and **append** provisional utterances (no speaker labels yet) so the
//! user sees text immediately. After each batch, the recording's provisional
//! title is regenerated from everything transcribed so far.
//!
//! Diarization needs the whole recording to cluster speakers stably, so it is
//! NOT done here — the full pass on `complete` (transcode → diarize → transcribe
//! → merge → …) re-transcribes with full context and replaces these provisional
//! utterances with the final, speaker-labelled transcript.

use scribe_asr::read_wav;
use scribe_core::config::Config;
use scribe_core::storage;
use scribe_core::types::{RecordingStatus, Word};
use scribe_core::Result;
use scribe_db::transcript::NewUtterance;
use scribe_db::Db;
use uuid::Uuid;

use crate::engines::Engines;
use crate::fillers::FillerFilter;
use crate::stages::merge::group_into_utterances;
use crate::stages::stage_err;
use crate::stages::transcode::transcode_to_wav;

const STAGE: &str = "transcribe_segment";

/// Transcribe all not-yet-transcribed segments of `recording_id` into provisional
/// utterances appended to the transcript.
pub async fn run(cfg: &Config, db: &Db, engines: &Engines, recording_id: Uuid) -> Result<()> {
    // Once `complete` has fired (status leaves `uploading`), the full diarized
    // pass owns the transcript — don't append provisional utterances behind it.
    if db.get_recording(recording_id).await?.status != RecordingStatus::Uploading {
        return Ok(());
    }
    let segments = db.list_untranscribed_segments(recording_id).await?;
    if segments.is_empty() {
        return Ok(());
    }

    let blobs = cfg.storage.blobs.as_path();
    let filler = FillerFilter::from_config(&cfg.asr);
    // Place this batch right after the last already-transcribed segment.
    let mut offset = db.max_transcribed_end_ms(recording_id).await?.unwrap_or(0);
    let mut total_inserted: u64 = 0;

    for seg in &segments {
        let seg_path = storage::resolve_key(blobs, &seg.storage_key);
        if !seg_path.exists() {
            tracing::warn!(%recording_id, seq = seg.seq, "live: segment file missing; skipping");
            continue;
        }

        // Transcode just this one segment to a temp WAV next to the recording.
        let wav = storage::recording_dir(blobs, recording_id).join(format!("seg-{:06}.wav", seg.seq));
        transcode_to_wav(std::slice::from_ref(&seg_path), &wav).await?;
        let duration_ms = read_wav(&wav).map_err(|e| stage_err(STAGE, e))?.duration_ms();

        let transcript = engines
            .speech
            .transcriber()
            .transcribe(&wav)
            .map_err(|e| stage_err(STAGE, e))?;
        let _ = tokio::fs::remove_file(&wav).await;

        // Offset each word onto the global recording timeline; no speaker yet.
        let mut words: Vec<Word> = transcript
            .words
            .iter()
            .map(|w| Word {
                text: w.text.clone(),
                start_ms: w.start_ms + offset,
                end_ms: w.end_ms + offset,
                conf: w.conf,
                local_idx: None,
            })
            .collect();
        filler.clean(&mut words);

        let new: Vec<NewUtterance> = group_into_utterances(&words)
            .into_iter()
            .map(|u| NewUtterance {
                local_idx: u.local_idx,
                start_ms: u.start_ms,
                end_ms: u.end_ms,
                text: u.text,
                words: u.words,
            })
            .collect();
        total_inserted += db.insert_utterances(recording_id, &new).await?;
        db.mark_segment_transcribed(seg.id, offset, duration_ms).await?;
        offset += duration_ms;
    }

    tracing::info!(
        %recording_id,
        segments = segments.len(),
        utterances = total_inserted,
        "transcribe_segment complete"
    );

    // Continuously refresh the provisional title from everything so far.
    crate::title::regenerate(cfg, db, &engines.ollama, recording_id).await?;
    Ok(())
}
