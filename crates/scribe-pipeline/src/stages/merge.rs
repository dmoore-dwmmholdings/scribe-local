//! Stage 4 — merge (design §8, WhisperX pattern).
//!
//! Read the transcribe stage's words and the diarize stage's speaker turns back
//! from the scratch table, then assign each word the speaker whose turn
//! **maximally overlaps** the word's `[start_ms, end_ms]` interval. Group runs
//! of consecutive same-speaker words into utterances, breaking on a speaker
//! change or a long silent gap. Replace the recording's utterances atomically.

use scribe_core::config::Config;
use scribe_core::types::Word;
use scribe_core::Result;
use scribe_db::transcript::NewUtterance;
use scribe_db::Db;
use uuid::Uuid;

use crate::artifacts::{self, TurnArtifact};
use crate::stages::stage_err;

const STAGE: &str = "merge";

/// Break an utterance when the silent gap between consecutive words exceeds this.
const GAP_BREAK_MS: i64 = 1_500;

/// Run the merge stage for `recording_id`.
pub async fn run(cfg: &Config, db: &Db, recording_id: Uuid) -> Result<()> {
    let transcript = artifacts::get_transcript(db, recording_id).await?;
    let diarization = artifacts::get_diarization(db, recording_id).await?;

    let mut words = transcript.words;
    // Strip non-lexical filler words (uh, um, …) before speaker assignment.
    let removed = crate::fillers::FillerFilter::from_config(&cfg.asr).clean(&mut words);
    if removed > 0 {
        tracing::debug!(%recording_id, removed, "merge: stripped filler words");
    }
    assign_speakers(&mut words, &diarization.turns);
    let utterances = group_into_utterances(&words);

    // Atomic replace: clear then bulk-insert.
    db.delete_utterances_by_recording(recording_id).await?;
    let new: Vec<NewUtterance> = utterances
        .into_iter()
        .map(|u| NewUtterance {
            local_idx: u.local_idx,
            start_ms: u.start_ms,
            end_ms: u.end_ms,
            text: u.text,
            words: u.words,
        })
        .collect();
    let inserted = db.insert_utterances(recording_id, &new).await?;

    if inserted == 0 && !words.is_empty() {
        return Err(stage_err(STAGE, "produced no utterances from a non-empty transcript"));
    }
    tracing::info!(%recording_id, utterances = inserted, "merge complete");
    Ok(())
}

/// Length of the overlap between `[a0,a1]` and `[b0,b1]` (0 if disjoint).
fn overlap(a0: i64, a1: i64, b0: i64, b1: i64) -> i64 {
    (a1.min(b1) - a0.max(b0)).max(0)
}

/// Assign each word the diarized speaker whose turn overlaps it most.
///
/// Ties and zero-overlap words (silence, ASR/diarizer drift) fall back to the
/// turn whose midpoint is nearest the word's midpoint, so every word still gets
/// a speaker when any turn exists. With no turns at all, `local_idx` stays None.
fn assign_speakers(words: &mut [Word], turns: &[TurnArtifact]) {
    if turns.is_empty() {
        return;
    }
    for w in words.iter_mut() {
        let mut best_idx: Option<i32> = None;
        let mut best_overlap: i64 = 0;
        for t in turns {
            let ov = overlap(w.start_ms, w.end_ms, t.start_ms, t.end_ms);
            if ov > best_overlap {
                best_overlap = ov;
                best_idx = Some(t.local_idx);
            }
        }
        if best_idx.is_none() {
            // No overlap with any turn: nearest-midpoint fallback.
            let wmid = (w.start_ms + w.end_ms) / 2;
            let mut best_dist = i64::MAX;
            for t in turns {
                let tmid = (t.start_ms + t.end_ms) / 2;
                let dist = (wmid - tmid).abs();
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = Some(t.local_idx);
                }
            }
        }
        w.local_idx = best_idx;
    }
}

/// An assembled utterance before it hits the DB.
pub(crate) struct GroupedUtterance {
    pub local_idx: Option<i32>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub words: Vec<Word>,
}

/// Group consecutive words into utterances, breaking on a speaker change or a
/// silent gap longer than [`GAP_BREAK_MS`]. Each word keeps its assigned
/// `local_idx`; the utterance text is the words joined by single spaces.
pub(crate) fn group_into_utterances(words: &[Word]) -> Vec<GroupedUtterance> {
    let mut out: Vec<GroupedUtterance> = Vec::new();
    let mut cur: Option<GroupedUtterance> = None;
    let mut prev_end: i64 = 0;

    for w in words {
        let same_speaker = cur.as_ref().map(|c| c.local_idx == w.local_idx);
        let gap_ok = w.start_ms - prev_end <= GAP_BREAK_MS;

        let continues = matches!(same_speaker, Some(true)) && gap_ok;
        if continues {
            let c = cur.as_mut().unwrap();
            c.end_ms = c.end_ms.max(w.end_ms);
            if !c.text.is_empty() {
                c.text.push(' ');
            }
            c.text.push_str(&w.text);
            c.words.push(w.clone());
        } else {
            if let Some(done) = cur.take() {
                out.push(done);
            }
            cur = Some(GroupedUtterance {
                local_idx: w.local_idx,
                start_ms: w.start_ms,
                end_ms: w.end_ms,
                text: w.text.clone(),
                words: vec![w.clone()],
            });
        }
        prev_end = w.end_ms;
    }
    if let Some(done) = cur.take() {
        out.push(done);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: i64, end: i64) -> Word {
        Word {
            text: text.to_string(),
            start_ms: start,
            end_ms: end,
            conf: 1.0,
            local_idx: None,
        }
    }

    fn turn(idx: i32, start: i64, end: i64) -> TurnArtifact {
        TurnArtifact {
            local_idx: idx,
            start_ms: start,
            end_ms: end,
        }
    }

    #[test]
    fn overlap_basic() {
        assert_eq!(overlap(0, 100, 50, 150), 50);
        assert_eq!(overlap(0, 100, 100, 200), 0);
        assert_eq!(overlap(0, 100, 200, 300), 0);
        assert_eq!(overlap(0, 1000, 100, 200), 100);
    }

    #[test]
    fn assigns_by_max_overlap() {
        let mut words = vec![word("hi", 0, 400), word("there", 600, 1000)];
        let turns = vec![turn(0, 0, 500), turn(1, 500, 1000)];
        assign_speakers(&mut words, &turns);
        assert_eq!(words[0].local_idx, Some(0));
        assert_eq!(words[1].local_idx, Some(1));
    }

    #[test]
    fn no_overlap_falls_back_to_nearest_midpoint() {
        // Word sits in the silent gap but closer to turn 1's midpoint (900) than
        // turn 0's (250): word midpoint 850 → dist 50 vs 600.
        let mut words = vec![word("um", 800, 900)];
        let turns = vec![turn(0, 0, 500), turn(1, 950, 1200)];
        assign_speakers(&mut words, &turns);
        assert_eq!(words[0].local_idx, Some(1));
    }

    #[test]
    fn groups_break_on_speaker_change_and_gap() {
        let words = vec![
            Word { local_idx: Some(0), ..word("a", 0, 100) },
            Word { local_idx: Some(0), ..word("b", 100, 200) },
            // speaker change
            Word { local_idx: Some(1), ..word("c", 200, 300) },
            // long gap (> 1.5s) within same speaker → break
            Word { local_idx: Some(1), ..word("d", 2000, 2100) },
        ];
        let utts = group_into_utterances(&words);
        assert_eq!(utts.len(), 3);
        assert_eq!(utts[0].text, "a b");
        assert_eq!(utts[0].local_idx, Some(0));
        assert_eq!(utts[1].text, "c");
        assert_eq!(utts[2].text, "d");
    }

    #[test]
    fn empty_turns_leaves_words_unassigned() {
        let mut words = vec![word("solo", 0, 100)];
        assign_speakers(&mut words, &[]);
        assert_eq!(words[0].local_idx, None);
        let utts = group_into_utterances(&words);
        assert_eq!(utts.len(), 1);
        assert_eq!(utts[0].local_idx, None);
    }
}
