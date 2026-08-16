//! Print a speaker-embedding model's output dimension.
//!
//! The dimension is a schema commitment: `speakers.embedding` and
//! `recording_speakers.embedding` are `vector(192)`, so a model that emits
//! anything else cannot be installed without a migration. Checking before
//! swapping models beats finding out from a failed insert an hour into a run.
//!
//! ```text
//! cargo run -p scribe-asr --example embed_dim -- models/diarization/embedding.onnx
//! ```
//!
//! Embeddings are also not comparable across models: swapping one invalidates
//! every voiceprint already stored, so enrolled speakers must be re-enrolled.

#[cfg(feature = "onnx")]
fn main() {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_none() {
        eprintln!("usage: embed_dim <model.onnx> [more.onnx ...]");
        std::process::exit(2);
    }
    for path in args {
        let config = sherpa_onnx::SpeakerEmbeddingExtractorConfig {
            model: Some(path.clone()),
            num_threads: 1,
            debug: false,
            provider: Some("cpu".to_string()),
        };
        match sherpa_onnx::SpeakerEmbeddingExtractor::create(&config) {
            Some(extractor) => println!("{:>4}  {path}", extractor.dim()),
            None => println!("   ?  {path}  (failed to load)"),
        }
    }
}

#[cfg(not(feature = "onnx"))]
fn main() {
    eprintln!("embed_dim needs the real ML stack: build without --no-default-features");
    std::process::exit(2);
}
