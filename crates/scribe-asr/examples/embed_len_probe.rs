//! Feed a speaker-embedding model one slice of audio and report whether it
//! survives. Used to find the longest slice a model tolerates.
//!
//! The failure this hunts for is not a Rust error: onnxruntime throws a C++
//! exception, which crosses the FFI boundary and aborts the process outright
//! ("Rust cannot catch foreign exceptions"). So the answer has to be read from
//! the child's exit status, one length per process.
//!
//! ```text
//! cargo run -p scribe-asr --example embed_len_probe -- models/diarization/embedding.onnx 120
//! ```

#[cfg(feature = "onnx")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: embed_len_probe <model.onnx> <seconds>");
        std::process::exit(2);
    }
    let path = args[0].clone();
    let seconds: f32 = args[1].parse().expect("seconds must be a number");

    let config = sherpa_onnx::SpeakerEmbeddingExtractorConfig {
        model: Some(path.clone()),
        num_threads: 4,
        debug: false,
        provider: Some("cpu".to_string()),
    };
    let Some(extractor) = sherpa_onnx::SpeakerEmbeddingExtractor::create(&config) else {
        eprintln!("failed to load {path}");
        std::process::exit(2);
    };

    // Speech-shaped enough to keep the model out of any silence short circuit:
    // a 120 Hz buzz under a slow amplitude sweep.
    let sr = 16_000u32;
    let n = (seconds * sr as f32) as usize;
    let samples: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / sr as f32;
            0.3 * (2.0 * std::f32::consts::PI * 120.0 * t).sin()
                * (0.5 + 0.5 * (2.0 * std::f32::consts::PI * 0.7 * t).sin())
        })
        .collect();

    let stream = extractor.create_stream().expect("create_stream");
    stream.accept_waveform(sr as i32, &samples);
    stream.input_finished();
    if !extractor.is_ready(&stream) {
        println!("{seconds:>7.1}s  not ready");
        return;
    }
    match extractor.compute(&stream) {
        Some(emb) => println!("{seconds:>7.1}s  ok (dim {})", emb.len()),
        None => println!("{seconds:>7.1}s  compute returned none"),
    }
}

#[cfg(not(feature = "onnx"))]
fn main() {
    eprintln!("embed_len_probe needs the real ML stack: build without --no-default-features");
    std::process::exit(2);
}
