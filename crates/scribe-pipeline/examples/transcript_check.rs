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

    let cfg = AsrConfig::default();
    let engine = SpeechEngine::load(&models_dir, &cfg).expect("load engine");
    println!("backend          {}", engine.backend().as_str());
    println!("threads          {}", cfg.resolved_num_threads());

    let t0 = std::time::Instant::now();
    let transcript = engine.transcriber().transcribe(&wav).expect("transcribe");
    let asr_secs = t0.elapsed().as_secs_f64();

    let t1 = std::time::Instant::now();
    let diarization = engine.diarizer().diarize(&wav, None).expect("diarize");
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
    let smoothed = label_words(&mut words, &diarization.turns);
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
    println!("smoothed         {smoothed}");

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

    // A word's true speaker is whoever was talking at its midpoint.
    let truth_at = |ms: i64| -> Option<&str> {
        truth
            .turns
            .iter()
            .find(|t| ms >= t.start_ms && ms <= t.end_ms)
            .map(|t| t.speaker.as_str())
    };

    let mut pair: HashMap<(&str, i32), usize> = HashMap::new();
    for w in &words {
        if let (Some(name), Some(idx)) = (truth_at((w.start_ms + w.end_ms) / 2), w.local_idx) {
            *pair.entry((name, idx)).or_insert(0) += 1;
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
    for w in &words {
        let Some(name) = truth_at((w.start_ms + w.end_ms) / 2) else {
            continue;
        };
        scored += 1;
        match w.local_idx {
            None => unlabelled += 1,
            Some(idx) => {
                if map.get(&idx) == Some(&name) {
                    correct += 1;
                }
            }
        }
    }
    let pct = |n: usize| 100.0 * n as f64 / scored.max(1) as f64;
    println!("─────────────────────────────────────────────────────────");
    println!("words scored     {scored}");
    println!("right speaker    {:.1}%", pct(correct));
    println!("wrong speaker    {:.1}%", pct(scored - correct - unlabelled));
    println!("no speaker       {:.1}%", pct(unlabelled));

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
