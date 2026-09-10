//! Denoise a WAV with sherpa-onnx's GTCRN speech enhancer, to measure whether
//! cleaning the audio first helps diarization and transcription in a noisy room.
//!
//!     denoise <gtcrn.onnx> <in.wav> <out.wav>

use std::path::PathBuf;

#[cfg(feature = "onnx")]
fn main() {
    use sherpa_onnx::{
        OfflineSpeechDenoiser, OfflineSpeechDenoiserConfig, OfflineSpeechDenoiserGtcrnModelConfig,
        OfflineSpeechDenoiserModelConfig,
    };

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: denoise <gtcrn.onnx> <in.wav> <out.wav>");
        std::process::exit(2);
    }
    let wav = scribe_asr::read_wav(&PathBuf::from(&args[2])).expect("read wav");

    let threads = std::env::var("SCRIBE_ASR_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let config = OfflineSpeechDenoiserConfig {
        model: OfflineSpeechDenoiserModelConfig {
            gtcrn: OfflineSpeechDenoiserGtcrnModelConfig {
                model: Some(args[1].clone()),
            },
            num_threads: threads,
            ..Default::default()
        },
    };
    let denoiser = OfflineSpeechDenoiser::create(&config).expect("create denoiser");

    let started = std::time::Instant::now();
    let out = denoiser.run(&wav.samples, wav.sample_rate as i32);
    let elapsed = started.elapsed().as_secs_f64();
    let audio_secs = wav.samples.len() as f64 / wav.sample_rate as f64;
    eprintln!(
        "denoised {audio_secs:.1}s in {elapsed:.1}s ({:.1}x real time), {} Hz in, {} Hz out",
        audio_secs / elapsed.max(1e-9),
        wav.sample_rate,
        out.sample_rate
    );

    write_wav(&PathBuf::from(&args[3]), &out.samples, out.sample_rate as u32);
}

#[cfg(feature = "onnx")]
fn write_wav(path: &PathBuf, samples: &[f32], sample_rate: u32) {
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write wav");
}

#[cfg(not(feature = "onnx"))]
fn main() {
    eprintln!("build with --features onnx");
    std::process::exit(2);
}
