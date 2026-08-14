//! Real diarization via `sherpa_onnx::OfflineSpeakerDiarization`.
//!
//! Silero VAD + pyannote segmentation + speaker-embedding extraction +
//! FastClustering, all inside one sherpa-onnx object (design §8). When the
//! caller knows the participant count we pin `num_clusters`; otherwise we let
//! clustering discover the count via a cosine threshold.

use std::collections::HashMap;
use std::path::Path;

use scribe_core::{Error, Result};
use sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig,
};

use crate::models::DiarizationModelPaths;
use crate::types::{Diarization, Diarizer, SpeakerTurn};
use crate::wav::{self, WavData};

/// Cosine-similarity threshold used when the speaker count is unknown.
const CLUSTER_THRESHOLD: f32 = 0.5;

/// Wraps `OfflineSpeakerDiarization` plus a standalone embedding extractor used
/// to compute the per-speaker mean embeddings the pipeline needs for enrollment.
pub struct SherpaDiarizer {
    paths: DiarizationModelPaths,
    device: String,
    num_threads: i32,
    // The diarization object's clustering config depends on `expected_speakers`,
    // which varies per call, so we build the object lazily in `diarize`.
}

impl SherpaDiarizer {
    pub fn load(paths: DiarizationModelPaths, device: &str, num_threads: i32) -> Result<Self> {
        // Validate the extractor can be built up front (fail fast at load time).
        let _ = build_extractor(&paths, device, num_threads)?;
        Ok(SherpaDiarizer {
            paths,
            device: device.to_string(),
            num_threads,
        })
    }

    fn build_diarizer(&self, expected: Option<i32>) -> Result<OfflineSpeakerDiarization> {
        let provider = provider_for(&self.device);

        // `participants_expected` is treated as a soft HINT, not a hard count:
        // always discover the speaker count via the cosine threshold so the
        // system adapts when more people speak (someone joins) or fewer do
        // (someone stays silent) than the user guessed. Pinning `num_clusters`
        // to the hint would force-merge or force-split real voices.
        if let Some(n) = expected {
            tracing::debug!(hint = n, "diarize: speaker-count hint (advisory, threshold discovery)");
        }
        let clustering = FastClusteringConfig {
            num_clusters: -1,
            threshold: CLUSTER_THRESHOLD,
        };

        let config = OfflineSpeakerDiarizationConfig {
            segmentation: OfflineSpeakerSegmentationModelConfig {
                pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                    model: Some(path_str(&self.paths.segmentation)?),
                },
                num_threads: self.num_threads,
                debug: false,
                provider: Some(provider.clone()),
            },
            embedding: SpeakerEmbeddingExtractorConfig {
                model: Some(path_str(&self.paths.embedding)?),
                num_threads: self.num_threads,
                debug: false,
                provider: Some(provider),
            },
            clustering,
            ..Default::default()
        };

        OfflineSpeakerDiarization::create(&config)
            .ok_or_else(|| Error::Model("failed to create OfflineSpeakerDiarization".into()))
    }
}

impl Diarizer for SherpaDiarizer {
    fn diarize(&self, wav_path: &Path, expected_speakers: Option<i32>) -> Result<Diarization> {
        let audio = wav::read_wav(wav_path)?;
        let diarizer = self.build_diarizer(expected_speakers)?;

        let result = diarizer
            .process(&audio.samples)
            .ok_or_else(|| Error::Model("diarization produced no result".into()))?;

        let segments = result.sort_by_start_time();

        let mut turns = Vec::with_capacity(segments.len());
        let mut speaker_set = std::collections::BTreeSet::new();
        for seg in &segments {
            speaker_set.insert(seg.speaker);
            turns.push(SpeakerTurn {
                local_idx: seg.speaker,
                start_ms: (seg.start as f64 * 1000.0).round() as i64,
                end_ms: (seg.end as f64 * 1000.0).round() as i64,
            });
        }

        // Per-speaker mean embedding for enrollment matching. sherpa's
        // diarization result doesn't hand back the cluster centroids, so we
        // re-extract an embedding over each speaker's concatenated segment audio
        // and average. This reuses the same embedding model the clusterer used.
        let extractor = build_extractor(&self.paths, &self.device, self.num_threads)?;
        let embeddings = compute_speaker_embeddings(&extractor, &audio, &turns)?;

        let num_speakers = if result.num_speakers() > 0 {
            result.num_speakers() as usize
        } else {
            speaker_set.len()
        };

        Ok(Diarization {
            turns,
            embeddings,
            num_speakers,
        })
    }
}

/// Build a standalone embedding extractor from the diarization embedding model.
fn build_extractor(
    paths: &DiarizationModelPaths,
    device: &str,
    num_threads: i32,
) -> Result<SpeakerEmbeddingExtractor> {
    let config = SpeakerEmbeddingExtractorConfig {
        model: Some(path_str(&paths.embedding)?),
        num_threads,
        debug: false,
        provider: Some(provider_for(device)),
    };
    SpeakerEmbeddingExtractor::create(&config)
        .ok_or_else(|| Error::Model("failed to create SpeakerEmbeddingExtractor".into()))
}

/// Compute one mean embedding per speaker by extracting an embedding over the
/// audio of each of that speaker's turns and averaging (then L2-normalizing).
fn compute_speaker_embeddings(
    extractor: &SpeakerEmbeddingExtractor,
    audio: &WavData,
    turns: &[SpeakerTurn],
) -> Result<HashMap<i32, Vec<f32>>> {
    let sr = audio.sample_rate as i64;
    let mut acc: HashMap<i32, (Vec<f32>, usize)> = HashMap::new();

    for turn in turns {
        let start = ((turn.start_ms.max(0) * sr) / 1000) as usize;
        let end = (((turn.end_ms.max(turn.start_ms)) * sr) / 1000) as usize;
        let end = end.min(audio.samples.len());
        if end <= start {
            continue;
        }
        let slice = &audio.samples[start..end];

        let Some(stream) = extractor.create_stream() else {
            continue;
        };
        stream.accept_waveform(audio.sample_rate as i32, slice);
        stream.input_finished();
        if !extractor.is_ready(&stream) {
            continue;
        }
        let Some(emb) = extractor.compute(&stream) else {
            continue;
        };

        let entry = acc
            .entry(turn.local_idx)
            .or_insert_with(|| (vec![0.0; emb.len()], 0));
        if entry.0.len() != emb.len() {
            entry.0 = vec![0.0; emb.len()];
        }
        for (a, b) in entry.0.iter_mut().zip(emb.iter()) {
            *a += *b;
        }
        entry.1 += 1;
    }

    let mut out = HashMap::new();
    for (idx, (mut sum, count)) in acc {
        if count == 0 {
            continue;
        }
        for x in sum.iter_mut() {
            *x /= count as f32;
        }
        l2_normalize(&mut sum);
        out.insert(idx, sum);
    }
    Ok(out)
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn provider_for(device: &str) -> String {
    match device.trim().to_ascii_lowercase().as_str() {
        "cuda" | "gpu" => "cuda".to_string(),
        "coreml" => "coreml".to_string(),
        _ => "cpu".to_string(),
    }
}

fn path_str(p: &Path) -> Result<String> {
    p.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::Model(format!("non-UTF-8 model path: {}", p.display())))
}
