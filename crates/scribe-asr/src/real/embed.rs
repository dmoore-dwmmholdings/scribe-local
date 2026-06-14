//! Real speaker embedding via `sherpa_onnx::SpeakerEmbeddingExtractor`.
//!
//! Used by `scribe enroll` to turn a reference voice sample into the 192-dim
//! vector stored in the `speakers` table (design §8).

use std::path::Path;

use scribe_core::{Error, Result};
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

use crate::types::SpeakerEmbedder;
use crate::wav;

/// Wraps a `SpeakerEmbeddingExtractor` for single-sample enrollment.
pub struct SherpaEmbedder {
    extractor: SpeakerEmbeddingExtractor,
    dim: usize,
}

impl SherpaEmbedder {
    pub fn load(model_path: &Path, device: &str, num_threads: i32) -> Result<Self> {
        let config = SpeakerEmbeddingExtractorConfig {
            model: Some(
                model_path
                    .to_str()
                    .ok_or_else(|| {
                        Error::Model(format!("non-UTF-8 model path: {}", model_path.display()))
                    })?
                    .to_string(),
            ),
            num_threads,
            debug: false,
            provider: Some(provider_for(device)),
        };
        let extractor = SpeakerEmbeddingExtractor::create(&config)
            .ok_or_else(|| Error::Model("failed to create SpeakerEmbeddingExtractor".into()))?;
        let dim = extractor.dim().max(0) as usize;
        Ok(SherpaEmbedder { extractor, dim })
    }
}

impl SpeakerEmbedder for SherpaEmbedder {
    fn embed_speaker(&self, wav_path: &Path) -> Result<Vec<f32>> {
        let audio = wav::read_wav(wav_path)?;

        let stream = self
            .extractor
            .create_stream()
            .ok_or_else(|| Error::Model("failed to create embedding stream".into()))?;
        stream.accept_waveform(audio.sample_rate as i32, &audio.samples);
        stream.input_finished();

        if !self.extractor.is_ready(&stream) {
            return Err(Error::Model(
                "embedding sample too short for the model".into(),
            ));
        }

        self.extractor
            .compute(&stream)
            .ok_or_else(|| Error::Model("embedding computation returned nothing".into()))
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

fn provider_for(device: &str) -> String {
    match device.trim().to_ascii_lowercase().as_str() {
        "cuda" | "gpu" => "cuda".to_string(),
        "coreml" => "coreml".to_string(),
        _ => "cpu".to_string(),
    }
}
