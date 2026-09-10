//! Score the real diarizer against a conversation with known ground truth.
//!
//! Not a unit test: it needs the ONNX diarization models and a WAV, so it is an
//! example run by hand rather than something `cargo test` picks up.
//!
//! ```text
//! cargo run --release -p scribe-asr --example diarize_check -- \
//!     models/diarization <conversation.wav> <truth.json> [expected_speakers]
//! ```
//!
//! `truth.json` is `{"turns": [{"speaker": "<name>", "start_ms": N, "end_ms": N}, ...]}`.
//! Speaker names are opaque labels; the diarizer's numbering is matched to them
//! by whichever pairing accounts for the most speech, so the score measures
//! whether the right stretches were grouped together, not what they were called.

use std::collections::HashMap;
use std::path::PathBuf;

use scribe_asr::models::DiarizationModelPaths;
use scribe_asr::SpeechEngine;

#[derive(serde::Deserialize)]
struct Truth {
    turns: Vec<TruthTurn>,
}

#[derive(serde::Deserialize)]
struct TruthTurn {
    speaker: String,
    start_ms: i64,
    end_ms: i64,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!(
            "usage: diarize_check <models_dir> <audio.wav> <truth.json> [expected_speakers]"
        );
        std::process::exit(2);
    }
    let models_dir = PathBuf::from(&args[0]);
    let wav = PathBuf::from(&args[1]);
    let expected: Option<i32> = args.get(3).and_then(|s| s.parse().ok());

    let truth: Truth =
        serde_json::from_str(&std::fs::read_to_string(&args[2]).expect("read truth")).expect("parse truth");
    let real_speakers: std::collections::BTreeSet<&str> =
        truth.turns.iter().map(|t| t.speaker.as_str()).collect();

    // Load the diarizer straight from the model files.
    let paths = DiarizationModelPaths::discover(&models_dir)
        .expect("no diarization models under <models_dir>/diarization");
    // SCRIBE_ASR_THREADS overrides, so the effect of the thread count can be
    // measured rather than assumed.
    let threads = std::env::var("SCRIBE_ASR_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| scribe_core::config::AsrConfig::default().resolved_num_threads());
    println!("threads          {threads}");
    // SCRIBE_ASR_DEVICE selects the ONNX execution provider (cpu / coreml / cuda).
    let device = std::env::var("SCRIBE_ASR_DEVICE").unwrap_or_else(|_| "cpu".into());
    println!("device           {device}");
    let engine =
        SpeechEngine::load_diarizer_only(&paths, &device, threads).expect("load diarizer");

    let started = std::time::Instant::now();
    let result = engine.diarizer().diarize(&wav, expected).expect("diarize");
    let elapsed = started.elapsed();

    let audio_ms: i64 = truth.turns.last().map(|t| t.end_ms).unwrap_or(0);
    println!("── diarize_check ─────────────────────────────────────────");
    println!("audio            {:.1}s", audio_ms as f64 / 1000.0);
    println!("wall clock       {:.1}s  ({:.1}x real time)", elapsed.as_secs_f64(),
             audio_ms as f64 / 1000.0 / elapsed.as_secs_f64().max(1e-9));
    println!("expected count   {}", expected.map(|n| n.to_string()).unwrap_or("(discover)".into()));
    println!("truth speakers   {}", real_speakers.len());
    println!("found speakers   {}", result.num_speakers);
    println!("turns emitted    {}", result.turns.len());
    println!(
        "embedding dim    {}",
        result.embeddings.values().next().map(|e| e.len()).unwrap_or(0)
    );

    // Millisecond-level frame scoring at 10 ms, the usual DER resolution.
    const STEP_MS: i64 = 10;
    let mut truth_at: Vec<Option<&str>> = vec![None; (audio_ms / STEP_MS) as usize + 1];
    for t in &truth.turns {
        for f in (t.start_ms / STEP_MS)..=(t.end_ms / STEP_MS).min(truth_at.len() as i64 - 1) {
            truth_at[f as usize] = Some(t.speaker.as_str());
        }
    }
    let mut hyp_at: Vec<Option<i32>> = vec![None; truth_at.len()];
    for t in &result.turns {
        for f in (t.start_ms / STEP_MS)..=(t.end_ms / STEP_MS).min(hyp_at.len() as i64 - 1) {
            hyp_at[f as usize] = Some(t.local_idx);
        }
    }

    // Best one-to-one pairing of hypothesis clusters to real speakers, greedily
    // by how much speech each pairing accounts for.
    let mut pair_frames: HashMap<(&str, i32), usize> = HashMap::new();
    for (t, h) in truth_at.iter().zip(hyp_at.iter()) {
        if let (Some(name), Some(idx)) = (t, h) {
            *pair_frames.entry((name, *idx)).or_insert(0) += 1;
        }
    }
    let mut pairs: Vec<((&str, i32), usize)> = pair_frames.into_iter().collect();
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

    let mut speech = 0usize;
    let mut correct = 0usize;
    let mut missed = 0usize;
    for (t, h) in truth_at.iter().zip(hyp_at.iter()) {
        let Some(name) = t else { continue };
        speech += 1;
        match h {
            None => missed += 1,
            Some(idx) => {
                if map.get(idx) == Some(name) {
                    correct += 1;
                }
            }
        }
    }
    let pct = |n: usize| 100.0 * n as f64 / speech.max(1) as f64;
    println!("─────────────────────────────────────────────────────────");
    println!("speech frames    {speech}");
    println!("correct speaker  {:.1}%", pct(correct));
    println!("wrong speaker    {:.1}%", pct(speech - correct - missed));
    println!("no speaker       {:.1}%", pct(missed));

    println!("─────────────────────────────────────────────────────────");
    let mut idxs: Vec<&i32> = map.keys().collect();
    idxs.sort();
    for idx in idxs {
        println!("  cluster {idx} → {}", map[idx]);
    }
    let mut cluster_ms: HashMap<i32, i64> = HashMap::new();
    for t in &result.turns {
        *cluster_ms.entry(t.local_idx).or_insert(0) += t.end_ms - t.start_ms;
    }
    let mut sizes: Vec<(i32, i64)> = cluster_ms.into_iter().collect();
    sizes.sort_by_key(|(_, ms)| -*ms);
    println!("─── speech per cluster ──────────────────────────────────");
    let total: i64 = sizes.iter().map(|(_, ms)| *ms).sum();
    for (idx, ms) in &sizes {
        println!("  cluster {idx:<3} {:>7.1}s  {:>5.1}%   {}", *ms as f64 / 1000.0,
                 100.0 * *ms as f64 / total.max(1) as f64,
                 map.get(idx).copied().unwrap_or("(unmapped)"));
    }

    if std::env::var("DIARIZE_CHECK_TURNS").is_ok() {
        println!("─── turns ───────────────────────────────────────────────");
        println!("  {:>8} {:>8}  {:<12} {:<12}", "start", "end", "truth", "found");
        let mut all: Vec<(i64, i64, String, String)> = Vec::new();
        for t in &truth.turns {
            all.push((t.start_ms, t.end_ms, t.speaker.clone(), String::new()));
        }
        for t in &result.turns {
            let label = map.get(&t.local_idx).map(|s| s.to_string())
                .unwrap_or_else(|| format!("#{}", t.local_idx));
            all.push((t.start_ms, t.end_ms, String::new(), label));
        }
        all.sort_by_key(|(s, e, _, _)| (*s, *e));
        for (start, end, truth_name, found) in all {
            println!("  {start:>8} {end:>8}  {truth_name:<12} {found:<12}");
        }
    }

    // Assertions, for using this as a regression check rather than a report.
    // DIARIZE_EXPECT_SPEAKERS and DIARIZE_MIN_CORRECT make a shortfall an exit
    // code, so a change that quietly costs ten points of accuracy is noticed by
    // something other than a person reading numbers.
    let mut failed = Vec::new();
    if let Some(want) = std::env::var("DIARIZE_EXPECT_SPEAKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        if result.num_speakers != want {
            failed.push(format!("speakers: got {}, want {want}", result.num_speakers));
        }
    }
    if let Some(floor) = std::env::var("DIARIZE_MIN_CORRECT")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    {
        if pct(correct) < floor {
            failed.push(format!("correct: {:.1}% below {floor:.1}%", pct(correct)));
        }
    }

    let unmapped: Vec<i32> = (0..result.num_speakers as i32)
        .filter(|i| !map.contains_key(i))
        .collect();
    if !unmapped.is_empty() {
        println!("  spurious clusters: {unmapped:?}");
    }

    if !failed.is_empty() {
        println!("─────────────────────────────────────────────────────────");
        for f in &failed {
            println!("FAIL  {f}");
        }
        std::process::exit(1);
    }
}
