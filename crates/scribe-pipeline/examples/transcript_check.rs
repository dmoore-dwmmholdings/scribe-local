//! Score a finished transcript: are the words right, and is the right person
//! credited with saying them?
//!
//! `diarize_check` in `scribe-asr` measures the diarizer alone. This runs the
//! part a reader actually sees — real ASR word timings, real diarization, and
//! the merge stage's own labelling — because everything upstream can be right
//! and a transcript still put the wrong name on every third line.
//!
//! ```text
//! cargo run --release -p scribe-pipeline --example transcript_check -- \
//!     models <conversation.wav> <truth.json>
//! ```
//!
//! See docs/measuring-diarization.md for the models and the fixtures.

use std::collections::HashMap;
use std::path::PathBuf;

use scribe_asr::SpeechEngine;
use scribe_core::config::AsrConfig;
use scribe_core::types::Word;
use scribe_pipeline::{label_words, utterance_spans};

#[derive(serde::Deserialize)]
struct Truth {
    turns: Vec<TruthTurn>,
}

#[derive(serde::Deserialize)]
struct TruthTurn {
    speaker: String,
    start_ms: i64,
    end_ms: i64,
    #[serde(default)]
    text: String,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: transcript_check <models_dir> <audio.wav> <truth.json>");
        std::process::exit(2);
    }
    let models_dir = PathBuf::from(&args[0]);
    let wav = PathBuf::from(&args[1]);
    let truth: Truth =
        serde_json::from_str(&std::fs::read_to_string(&args[2]).expect("read truth"))
            .expect("parse truth");

    let mut cfg = AsrConfig::default();
    // SCRIBE_ASR_MODEL selects the checkpoint under <models_dir>/asr.
    if let Ok(model) = std::env::var("SCRIBE_ASR_MODEL") {
        cfg.model = model;
    }
    // SCRIBE_ASR_HOTWORDS points at a hotwords file, for measuring what biasing
    // recognition toward known names and terms is worth.
    if let Ok(hw) = std::env::var("SCRIBE_ASR_HOTWORDS") {
        cfg.hotwords_file = Some(hw);
        if let Ok(sc) = std::env::var("SCRIBE_ASR_HOTWORD_SCORE") {
            if let Ok(v) = sc.parse() {
                cfg.hotwords_score = v;
            }
        }
    }
    println!("model            {}", cfg.model);
    println!(
        "hotwords         {}",
        cfg.hotwords_file.as_deref().unwrap_or("(none)")
    );
    let engine = SpeechEngine::load(&models_dir, &cfg).expect("load engine");
    println!("backend          {}", engine.backend().as_str());
    println!("threads          {}", cfg.resolved_num_threads());

    let t0 = std::time::Instant::now();
    let transcript = engine.transcriber().transcribe(&wav).expect("transcribe");
    let asr_secs = t0.elapsed().as_secs_f64();

    // TRANSCRIPT_CHECK_ASR_ONLY skips diarization, for sweeps that only care
    // about the words.
    let t1 = std::time::Instant::now();
    let diarization = if std::env::var("TRANSCRIPT_CHECK_ASR_ONLY").is_ok() {
        scribe_asr::Diarization {
            turns: Vec::new(),
            embeddings: Default::default(),
            num_speakers: 0,
        }
    } else {
        engine.diarizer().diarize(&wav, None).expect("diarize")
    };
    let diar_secs = t1.elapsed().as_secs_f64();

    let audio_ms = truth.turns.last().map(|t| t.end_ms).unwrap_or(0);
    let audio_secs = audio_ms as f64 / 1000.0;

    let mut words: Vec<Word> = transcript
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
    let t2 = std::time::Instant::now();
    let islands = label_words(&mut words, &diarization.turns);
    let merge_secs = t2.elapsed().as_secs_f64();

    println!("── transcript_check ──────────────────────────────────────");
    println!("audio            {audio_secs:.1}s");
    println!(
        "transcribe       {asr_secs:.1}s  ({:.1}x real time)",
        audio_secs / asr_secs.max(1e-9)
    );
    println!(
        "diarize          {diar_secs:.1}s  ({:.1}x real time)",
        audio_secs / diar_secs.max(1e-9)
    );
    println!("merge            {merge_secs:.3}s");
    println!(
        "total            {:.1}s  ({:.1}x real time)",
        asr_secs + diar_secs + merge_secs,
        audio_secs / (asr_secs + diar_secs + merge_secs).max(1e-9)
    );
    println!("words            {}", words.len());
    println!("speaker islands  {islands}  (brief runs both neighbours disagree with)");

    let spoken: Vec<String> = truth.turns.iter().flat_map(|t| normalise(&t.text)).collect();
    if !spoken.is_empty() {
        let heard: Vec<String> = words.iter().flat_map(|w| normalise(&w.text)).collect();
        let distance = edit_distance(&spoken, &heard);
        println!(
            "word error rate  {:.1}%  ({distance} edits over {} spoken words)",
            100.0 * distance as f64 / spoken.len() as f64,
            spoken.len()
        );
    }

    // Who was talking at a given moment — plural, because two people can be.
    // A word spoken over somebody else has two defensible answers and is scored
    // right for either, which is the usual lenient treatment of overlap.
    let truth_at = |ms: i64| -> Vec<&str> {
        truth
            .turns
            .iter()
            .filter(|t| ms >= t.start_ms && ms <= t.end_ms)
            .map(|t| t.speaker.as_str())
            .collect()
    };

    // Map clusters to people using only the words nobody was talking over, so a
    // contested moment cannot decide who a cluster is.
    let mut pair: HashMap<(&str, i32), usize> = HashMap::new();
    for w in &words {
        let who = truth_at((w.start_ms + w.end_ms) / 2);
        if let (1, Some(idx)) = (who.len(), w.local_idx) {
            *pair.entry((who[0], idx)).or_insert(0) += 1;
        }
    }
    let mut pairs: Vec<((&str, i32), usize)> = pair.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    let mut map: HashMap<i32, &str> = HashMap::new();
    let mut taken: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for ((name, idx), _) in &pairs {
        if map.contains_key(idx) || taken.contains(name) {
            continue;
        }
        map.insert(*idx, name);
        taken.insert(name);
    }

    let (mut scored, mut correct, mut unlabelled) = (0usize, 0usize, 0usize);
    let (mut contested, mut contested_ok) = (0usize, 0usize);
    for w in &words {
        let who = truth_at((w.start_ms + w.end_ms) / 2);
        if who.is_empty() {
            continue;
        }
        scored += 1;
        let overlapped = who.len() > 1;
        if overlapped {
            contested += 1;
        }
        match w.local_idx {
            None => unlabelled += 1,
            Some(idx) => {
                let hit = map.get(&idx).is_some_and(|name| who.contains(name));
                if hit {
                    correct += 1;
                    if overlapped {
                        contested_ok += 1;
                    }
                }
            }
        }
    }
    // Words whose midpoint lands where nobody was speaking. A direct measure of
    // whether the timings are real: a word placed in silence is a word the
    // playback highlighter will light up at the wrong moment, whatever name the
    // transcript put on it.
    let adrift = words.len().saturating_sub(scored);
    let pct = |n: usize| 100.0 * n as f64 / scored.max(1) as f64;
    println!("─────────────────────────────────────────────────────────");
    println!(
        "timed into silence {adrift} of {} words ({:.1}%)",
        words.len(),
        100.0 * adrift as f64 / words.len().max(1) as f64
    );
    println!("words scored     {scored}");
    println!("right speaker    {:.1}%", pct(correct));
    println!("wrong speaker    {:.1}%", pct(scored - correct - unlabelled));
    println!("no speaker       {:.1}%", pct(unlabelled));
    if contested > 0 {
        println!(
            "  spoken over    {contested} words ({:.1}%), {:.1}% of those on one of the two",
            pct(contested),
            100.0 * contested_ok as f64 / contested as f64
        );
        let clear = scored - contested;
        let clear_ok = correct - contested_ok;
        println!(
            "  in the clear   {clear} words, {:.1}% right",
            100.0 * clear_ok as f64 / clear.max(1) as f64
        );
    }

    let spans = utterance_spans(&words);
    println!("utterances       {} (from {} spoken turns)", spans.len(), truth.turns.len());
    if std::env::var("TRANSCRIPT_CHECK_LINES").is_ok() {
        println!("─────────────────────────────────────────────────────────");
        for (idx, start, _end, text) in &spans {
            let who = idx.and_then(|i| map.get(&i).copied()).unwrap_or("(unknown)");
            println!("  {:>7}  {who:<10} {text}", format_ms(*start));
        }
    }
}

fn format_ms(ms: i64) -> String {
    format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60)
}

/// Lowercase, strip punctuation, split on whitespace — so scoring compares
/// words rather than typography.
fn normalise(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Levenshtein distance over word sequences: substitutions + insertions +
/// deletions, which is what a word error rate counts.
fn edit_distance(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}
