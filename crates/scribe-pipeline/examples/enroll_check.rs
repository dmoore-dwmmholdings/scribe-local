//! Does a name given in one meeting stick to the same voice in the next?
//!
//! Enrollment is the only part of speaker detection that spans recordings, and
//! it is the part the other harnesses cannot reach: within one recording a voice
//! only has to be told apart from the others present, and it is compared against
//! itself through the same microphone in the same room. Across recordings it has
//! to be recognised through a different one.
//!
//! ```text
//! cargo run --release -p scribe-pipeline --example enroll_check -- \
//!     models <A.wav> <A-truth.json> <B.wav> <B-truth.json> [withheld-name ...]
//! ```
//!
//! Voiceprints are taken from the diarized voices of A — which is exactly what
//! enrolling from a recording does — and matched against the diarized voices of
//! B. A withheld name is enrolled by nobody, so it is the false-positive test:
//! that person is in both meetings and must come back unrecognised.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use scribe_asr::{Diarization, SpeechEngine};
use scribe_core::config::AsrConfig;
use scribe_pipeline::{enroll_match_threshold, resolve_identities};
use uuid::Uuid;

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

/// Diarize, then work out which cluster is which person, by how much of each
/// person's speech the cluster accounts for.
fn diarize_and_name(
    engine: &SpeechEngine,
    wav: &PathBuf,
    truth: &Truth,
) -> (Diarization, HashMap<i32, String>) {
    let d = engine.diarizer().diarize(wav, None).expect("diarize");

    let mut overlap: HashMap<(String, i32), i64> = HashMap::new();
    for turn in &d.turns {
        for t in &truth.turns {
            let ms = (turn.end_ms.min(t.end_ms) - turn.start_ms.max(t.start_ms)).max(0);
            if ms > 0 {
                *overlap.entry((t.speaker.clone(), turn.local_idx)).or_insert(0) += ms;
            }
        }
    }
    let mut pairs: Vec<((String, i32), i64)> = overlap.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));

    let mut named = HashMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    for ((name, idx), _) in pairs {
        if named.contains_key(&idx) || taken.contains(&name) {
            continue;
        }
        named.insert(idx, name.clone());
        taken.insert(name);
    }
    (d, named)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 5 {
        eprintln!(
            "usage: enroll_check <models_dir> <A.wav> <A-truth> <B.wav> <B-truth> [withheld ...]"
        );
        std::process::exit(2);
    }
    // A name prefixed with "-" is present in the audio but treated as absent
    // from meeting B: their voice is not offered to the matcher, so anyone who
    // resembles them competes for their identity unopposed. That is the case a
    // one-to-one assignment cannot help with.
    let mut withheld: HashSet<&str> = HashSet::new();
    let mut absent: HashSet<&str> = HashSet::new();
    for a in &args[5..] {
        match a.strip_prefix('-') {
            Some(name) => {
                absent.insert(name);
            }
            None => {
                withheld.insert(a.as_str());
            }
        }
    }

    let load = |p: &str| -> Truth {
        serde_json::from_str(&std::fs::read_to_string(p).expect("read truth")).expect("parse truth")
    };
    let (truth_a, truth_b) = (load(&args[2]), load(&args[4]));

    let cfg = AsrConfig::default();
    let engine = SpeechEngine::load_diarizer_only(
        &scribe_asr::models::DiarizationModelPaths::discover(&PathBuf::from(&args[0]))
            .expect("diarization models"),
        &cfg.device,
        cfg.resolved_num_threads(),
    )
    .expect("load diarizer");

    let (da, names_a) = diarize_and_name(&engine, &PathBuf::from(&args[1]), &truth_a);
    let (db, names_b) = diarize_and_name(&engine, &PathBuf::from(&args[3]), &truth_b);

    // Enrol every voice from meeting A except the withheld ones.
    let mut enrolled: Vec<(Uuid, Vec<f32>)> = Vec::new();
    let mut who: HashMap<Uuid, String> = HashMap::new();
    for (idx, name) in &names_a {
        if withheld.contains(name.as_str()) {
            continue;
        }
        let Some(emb) = da.embeddings.get(idx) else { continue };
        let id = Uuid::new_v4();
        enrolled.push((id, emb.clone()));
        who.insert(id, name.clone());
    }
    enrolled.sort_by_key(|(id, _)| *id);

    let voices: Vec<(i32, Vec<f32>)> = names_b
        .iter()
        .filter(|(_, name)| !absent.contains(name.as_str()))
        .filter_map(|(idx, _)| db.embeddings.get(idx).map(|e| (*idx, e.clone())))
        .collect();

    println!("── enroll_check ──────────────────────────────────────────");
    println!("meeting A        {} voices, {} enrolled", names_a.len(), enrolled.len());
    println!("meeting B        {} voices", voices.len());
    if !absent.is_empty() {
        let mut a: Vec<&str> = absent.iter().copied().collect();
        a.sort();
        println!("treated absent   {}", a.join(", "));
    }
    if !withheld.is_empty() {
        let mut w: Vec<&str> = withheld.iter().copied().collect();
        w.sort();
        println!("withheld         {}", w.join(", "));
    }

    // The similarities themselves, so the numbers behind the verdict are visible.
    println!("─── similarity of each B voice to each enrolled voice ────");
    let mut idxs: Vec<&i32> = names_b
        .iter()
        .filter(|(_, name)| !absent.contains(name.as_str()))
        .map(|(idx, _)| idx)
        .collect();
    idxs.sort();
    for idx in &idxs {
        let Some(emb) = db.embeddings.get(idx) else { continue };
        let mut row: Vec<String> = enrolled
            .iter()
            .map(|(id, vp)| format!("{}={:.3}", who[id], cosine(emb, vp)))
            .collect();
        row.sort();
        println!("  {:<10} {}", names_b[idx], row.join("  "));
    }

    let resolved = resolve_identities(&voices, &enrolled, enroll_match_threshold());

    let (mut right, mut wrong, mut missed, mut false_positive) = (0, 0, 0, 0);
    println!("─── verdict ─────────────────────────────────────────────");
    for idx in &idxs {
        let truth_name = &names_b[idx];
        let got = resolved.get(idx).map(|(id, sim)| (who[id].as_str(), *sim));
        let enrolled_here = !withheld.contains(truth_name.as_str())
            && names_a.values().any(|n| n == truth_name);
        match (got, enrolled_here) {
            (Some((name, sim)), true) if name == truth_name => {
                right += 1;
                println!("  {truth_name:<10} recognised (similarity {sim:.3})");
            }
            (Some((name, sim)), _) => {
                if enrolled_here {
                    wrong += 1;
                } else {
                    false_positive += 1;
                }
                println!("  {truth_name:<10} MISIDENTIFIED as {name} (similarity {sim:.3})");
            }
            (None, true) => {
                missed += 1;
                println!("  {truth_name:<10} not recognised, though enrolled");
            }
            (None, false) => println!("  {truth_name:<10} correctly left unnamed"),
        }
    }
    println!("─────────────────────────────────────────────────────────");
    println!("recognised       {right}");
    println!("misidentified    {wrong}");
    println!("missed           {missed}");
    println!("false positives  {false_positive}  (someone who was never enrolled, named anyway)");
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na <= f32::EPSILON || nb <= f32::EPSILON {
        return 0.0;
    }
    dot / (na * nb)
}
