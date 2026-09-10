//! Stage 4 — merge (design §8, WhisperX pattern).
//!
//! Read the transcribe stage's words and the diarize stage's speaker turns back
//! from the scratch table, then assign each word the speaker whose turns
//! **cover most** of the word's `[start_ms, end_ms]` interval, and move brief
//! flickers the overlap barely supported onto their neighbours. Group runs of
//! consecutive same-speaker words into utterances, breaking on a speaker change
//! or a long silent gap. Replace the recording's utterances atomically.

use std::collections::HashMap;

use scribe_core::config::Config;
use scribe_core::types::Word;
use scribe_core::Result;
use scribe_db::transcript::NewUtterance;
use scribe_db::Db;
use scribe_llm::{ChatMessage, OllamaClient};
use serde::Deserialize;
use uuid::Uuid;

use crate::artifacts::{self, TurnArtifact};
use crate::stages::stage_err;

const STAGE: &str = "merge";

/// Break an utterance when the silent gap between consecutive words exceeds this.
const GAP_BREAK_MS: i64 = 1_500;

/// Run the merge stage for `recording_id`.
pub async fn run(cfg: &Config, db: &Db, ollama: &OllamaClient, recording_id: Uuid) -> Result<()> {
    let transcript = artifacts::get_transcript(db, recording_id).await?;
    let diarization = artifacts::get_diarization(db, recording_id).await?;

    let mut words = transcript.words;
    // Strip non-lexical filler words (uh, um, …) before speaker assignment.
    let removed = crate::fillers::FillerFilter::from_config(&cfg.asr).clean(&mut words);
    if removed > 0 {
        tracing::debug!(%recording_id, removed, "merge: stripped filler words");
    }
    let coverage = assign_speakers(&mut words, &diarization.turns);
    let smoothed = smooth_islands(&mut words, &coverage);
    if smoothed > 0 {
        tracing::debug!(%recording_id, words = smoothed, "merge: reassigned stray speaker words");
    }
    let mut utterances = group_into_utterances(&words);

    // Best-effort LLM cleanup of misrecognised names / proper nouns. Never fails
    // the stage and only ever replaces individual utterance texts.
    if cfg.llm.correct_transcript {
        let names = speaker_display_names(db, recording_id).await.unwrap_or_default();
        let fixed = correct_names(ollama, &cfg.llm.summarize_model, &names, &mut utterances).await;
        if fixed > 0 {
            tracing::info!(%recording_id, lines = fixed, "merge: LLM corrected transcript lines");
        }
    }

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

/// Resolved speaker display names for this recording (hints for the corrector).
async fn speaker_display_names(db: &Db, recording_id: Uuid) -> Result<Vec<String>> {
    let rows = db.list_recording_speakers(recording_id).await?;
    Ok(rows.into_iter().filter_map(|rs| rs.display_name).collect())
}

#[derive(Deserialize)]
struct Correction {
    i: usize,
    text: String,
}

/// Best-effort: ask the LLM to fix misrecognised names/proper nouns in the
/// utterance texts. Returns the number of lines changed. NEVER errors — on any
/// problem (LLM down, bad JSON, suspicious rewrite) it leaves the text untouched
/// so a flaky model can't corrupt the transcript.
async fn correct_names(
    ollama: &OllamaClient,
    model: &str,
    names: &[String],
    utts: &mut [GroupedUtterance],
) -> usize {
    if utts.is_empty() {
        return 0;
    }

    let mut transcript = String::new();
    for (i, u) in utts.iter().enumerate() {
        let line = u.text.trim();
        if !line.is_empty() {
            transcript.push_str(&format!("{i}: {line}\n"));
        }
    }
    if transcript.is_empty() {
        return 0;
    }

    let names_hint = if names.is_empty() {
        String::new()
    } else {
        format!(
            "Known correct names/terms (prefer these spellings): {}.\n",
            names.join(", ")
        )
    };
    let system = ChatMessage::system(
        "You correct speech-to-text transcription errors. Respond with only JSON.",
    );
    let user = ChatMessage::user(format!(
        "{names_hint}Below are numbered transcript lines from speech-to-text. Fix ONLY clear \
         transcription errors: misheard proper nouns, names, and obviously wrong words. Do NOT \
         paraphrase, translate, summarise, reorder, merge, or split lines, and do not change \
         text that is already correct. Keep the wording and length as close to the original as \
         possible. Return ONLY a JSON array of objects {{\"i\": <line number>, \"text\": \
         \"<corrected line>\"}} for the lines you actually changed; return [] if nothing needs \
         fixing.\n\nLines:\n{transcript}"
    ));

    let raw = match ollama.chat(model, &[system, user]).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "merge: transcript correction skipped (LLM unavailable)");
            return 0;
        }
    };

    let Some(arr) = first_json_array(&raw) else {
        return 0;
    };
    let parsed: Vec<Correction> = match serde_json::from_str(&arr) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let mut applied = 0;
    for c in parsed {
        let Some(u) = utts.get_mut(c.i) else { continue };
        let new = c.text.trim();
        if new.is_empty() || new == u.text {
            continue;
        }
        // Reject wholesale rewrites (summaries/garbage): the corrected line must
        // stay close in length to the original.
        let orig_len = u.text.chars().count() as i64;
        let new_len = new.chars().count() as i64;
        if (orig_len - new_len).abs() > (orig_len / 3).max(12) {
            continue;
        }
        u.text = new.to_string();
        applied += 1;
    }
    applied
}

/// First balanced top-level `[...]` substring of `s` (respecting JSON strings),
/// or `None`. Lets us tolerate a model that wraps the array in prose.
fn first_json_array(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let start = s.find('[')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for i in start..bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Length of the overlap between `[a0,a1]` and `[b0,b1]` (0 if disjoint).
fn overlap(a0: i64, a1: i64, b0: i64, b1: i64) -> i64 {
    (a1.min(b1) - a0.max(b0)).max(0)
}

/// Assign each word the diarized speaker who covers most of it.
///
/// Overlap is accumulated **per speaker**, not per turn. pyannote does not emit
/// one turn per stretch of speech — it emits many short ones, and a word
/// straddling a pause can overlap three turns of the person actually talking and
/// one of somebody else. Taking the single longest turn overlap hands the word
/// to that somebody else even though they cover less of it; summing per speaker
/// first asks the question that was meant.
///
/// Ties and zero-overlap words (silence, ASR/diarizer drift) fall back to the
/// turn whose midpoint is nearest the word's midpoint, so every word still gets
/// a speaker when any turn exists. With no turns at all, `local_idx` stays None.
///
/// Returns each word's coverage — the fraction of the word the winning speaker
/// actually accounts for — so [`smooth_islands`] can tell a confident assignment
/// from a coin flip.
fn assign_speakers(words: &mut [Word], turns: &[TurnArtifact]) -> Vec<f64> {
    let mut coverage = vec![0.0; words.len()];
    if turns.is_empty() {
        return coverage;
    }
    let mut by_speaker: HashMap<i32, i64> = HashMap::new();
    for (i, w) in words.iter_mut().enumerate() {
        by_speaker.clear();
        for t in turns {
            let ov = overlap(w.start_ms, w.end_ms, t.start_ms, t.end_ms);
            if ov > 0 {
                *by_speaker.entry(t.local_idx).or_insert(0) += ov;
            }
        }

        // Max by overlap, ties broken by the lower speaker index so the result
        // does not depend on hash order.
        let best = by_speaker
            .iter()
            .max_by_key(|(idx, ov)| (**ov, std::cmp::Reverse(**idx)))
            .map(|(idx, ov)| (*idx, *ov));

        let best_idx = match best {
            Some((idx, ov)) => {
                let span = (w.end_ms - w.start_ms).max(0);
                coverage[i] = if span > 0 {
                    (ov as f64 / span as f64).min(1.0)
                } else {
                    1.0
                };
                Some(idx)
            }
            None => {
                // No overlap with any turn: nearest-midpoint fallback, and
                // coverage stays 0 — nothing about this word was witnessed.
                let wmid = (w.start_ms + w.end_ms) / 2;
                let mut best_dist = i64::MAX;
                let mut nearest = None;
                for t in turns {
                    let tmid = (t.start_ms + t.end_ms) / 2;
                    let dist = (wmid - tmid).abs();
                    if dist < best_dist {
                        best_dist = dist;
                        nearest = Some(t.local_idx);
                    }
                }
                nearest
            }
        };
        w.local_idx = best_idx;
    }
    coverage
}

/// Longest run of words that may be reassigned as a stray.
///
/// A word or two. Long enough to catch the flicker a turn boundary a few
/// hundred milliseconds out of step with the ASR produces, short enough that a
/// real interjection — "no, wait" — is left alone.
const MAX_ISLAND_MS: i64 = 800;

/// Coverage below which an assignment is treated as unsupported rather than
/// merely close. At half, most of the word fell outside the turn that claimed it.
const WEAK_COVERAGE: f64 = 0.5;

/// Reassign brief speaker flickers that the overlap evidence barely supported.
///
/// Diarization turn boundaries and ASR word boundaries are produced by different
/// models and do not agree to the millisecond. Where they disagree, one word in
/// the middle of somebody's sentence gets handed to whoever spoke next — which
/// then breaks the sentence into three utterances and puts a stranger's name on
/// the middle one. It reads as the speaker detection failing even when the
/// diarization was right.
///
/// A run is only moved when the evidence for it was weak (every word in it
/// mostly outside the turn that claimed it), it is brief, and the words on both
/// sides agree with each other and disagree with it. A confidently-assigned
/// word, or a genuine short turn between two different speakers, is left as it is.
fn smooth_islands(words: &mut [Word], coverage: &[f64]) -> usize {
    let mut moved = 0;
    let mut i = 0;
    while i < words.len() {
        let mut j = i + 1;
        while j < words.len() && words[j].local_idx == words[i].local_idx {
            j += 1;
        }
        // `i..j` is a maximal run of one speaker. Interior runs only.
        if i > 0 && j < words.len() {
            let before = words[i - 1].local_idx;
            let after = words[j].local_idx;
            let span = words[j - 1].end_ms - words[i].start_ms;
            let weak = coverage[i..j].iter().all(|c| *c < WEAK_COVERAGE);

            if before == after && before != words[i].local_idx && span <= MAX_ISLAND_MS && weak {
                for w in &mut words[i..j] {
                    w.local_idx = before;
                }
                moved += j - i;
            }
        }
        i = j;
    }
    moved
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
        let _ = assign_speakers(&mut words, &turns);
        assert_eq!(words[0].local_idx, Some(0));
        assert_eq!(words[1].local_idx, Some(1));
    }

    #[test]
    fn no_overlap_falls_back_to_nearest_midpoint() {
        // Word sits in the silent gap but closer to turn 1's midpoint (900) than
        // turn 0's (250): word midpoint 850 → dist 50 vs 600.
        let mut words = vec![word("um", 800, 900)];
        let turns = vec![turn(0, 0, 500), turn(1, 950, 1200)];
        let _ = assign_speakers(&mut words, &turns);
        assert_eq!(words[0].local_idx, Some(1));
    }

    #[test]
    fn a_speakers_turns_are_weighed_together() {
        // Speaker 0 holds the word across three short turns (300 ms total);
        // speaker 1 has one longer turn (200 ms). Per-turn, speaker 1 wins on
        // the single longest overlap; per-speaker, speaker 0 covers more.
        let mut words = vec![word("straddling", 0, 1000)];
        let turns = vec![
            turn(0, 0, 100),
            turn(1, 100, 300),
            turn(0, 300, 400),
            turn(0, 400, 500),
        ];
        let _ = assign_speakers(&mut words, &turns);
        assert_eq!(words[0].local_idx, Some(0));
    }

    #[test]
    fn coverage_reports_how_much_of_the_word_was_witnessed() {
        let mut words = vec![word("half", 0, 1000), word("all", 2000, 2500)];
        let turns = vec![turn(0, 0, 500), turn(0, 2000, 2500)];
        let coverage = assign_speakers(&mut words, &turns);
        assert!((coverage[0] - 0.5).abs() < 1e-9, "coverage = {coverage:?}");
        assert!((coverage[1] - 1.0).abs() < 1e-9, "coverage = {coverage:?}");
    }

    #[test]
    fn a_weakly_held_stray_word_rejoins_its_neighbours() {
        // "in" was handed to speaker 1 by a turn boundary a fraction out of step
        // with the ASR, in the middle of speaker 0's sentence.
        let mut words = vec![
            Word { local_idx: Some(0), ..word("the", 0, 300) },
            Word { local_idx: Some(0), ..word("point", 300, 600) },
            Word { local_idx: Some(1), ..word("in", 600, 900) },
            Word { local_idx: Some(0), ..word("question", 900, 1200) },
            Word { local_idx: Some(0), ..word("is", 1200, 1500) },
        ];
        // Barely any of "in" fell inside the turn that claimed it.
        let coverage = vec![1.0, 1.0, 0.1, 1.0, 1.0];

        assert_eq!(smooth_islands(&mut words, &coverage), 1);
        assert!(words.iter().all(|w| w.local_idx == Some(0)));
    }

    #[test]
    fn a_confidently_assigned_word_is_left_alone() {
        let mut words = vec![
            Word { local_idx: Some(0), ..word("so", 0, 300) },
            Word { local_idx: Some(1), ..word("yes", 300, 600) },
            Word { local_idx: Some(0), ..word("anyway", 600, 900) },
        ];
        // Fully witnessed: the diarizer really did hear a second voice.
        let coverage = vec![1.0, 1.0, 1.0];
        assert_eq!(smooth_islands(&mut words, &coverage), 0);
        assert_eq!(words[1].local_idx, Some(1));
    }

    #[test]
    fn a_real_turn_is_not_smoothed_away() {
        // Weakly held, but long enough to be somebody actually taking the floor.
        let mut words = vec![
            Word { local_idx: Some(0), ..word("go", 0, 300) },
            Word { local_idx: Some(1), ..word("well", 300, 900) },
            Word { local_idx: Some(1), ..word("actually", 900, 1600) },
            Word { local_idx: Some(0), ..word("right", 1600, 1900) },
        ];
        let coverage = vec![0.1, 0.1, 0.1, 0.1];
        assert_eq!(smooth_islands(&mut words, &coverage), 0);
    }

    #[test]
    fn a_genuine_handover_is_not_smoothed() {
        // Flanking speakers differ, so there is no consensus to snap back to.
        let mut words = vec![
            Word { local_idx: Some(0), ..word("done", 0, 300) },
            Word { local_idx: Some(1), ..word("ok", 300, 600) },
            Word { local_idx: Some(2), ..word("next", 600, 900) },
        ];
        let coverage = vec![0.1, 0.1, 0.1];
        assert_eq!(smooth_islands(&mut words, &coverage), 0);
        assert_eq!(words[1].local_idx, Some(1));
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
        let _ = assign_speakers(&mut words, &[]);
        assert_eq!(words[0].local_idx, None);
        let utts = group_into_utterances(&words);
        assert_eq!(utts.len(), 1);
        assert_eq!(utts[0].local_idx, None);
    }
}
