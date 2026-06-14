//! A tiny dependency-free RIFF/WAVE reader.
//!
//! The transcode stage (design §7) always hands these engines a **16 kHz mono
//! PCM** WAV, so a full WAV library would be overkill. We parse the canonical
//! `RIFF`/`fmt `/`data` chunk layout ourselves, tolerate extra chunks (`LIST`,
//! `fact`, …) by skipping on the chunk size, and decode 8/16/24/32-bit PCM and
//! 32-bit IEEE float samples to mono `f32` in `[-1.0, 1.0]`.
//!
//! Both the stub engine and the real (sherpa-onnx) engine use this: sherpa wants
//! `&[f32]` samples + the sample rate, and this gives exactly that without
//! pulling another native dependency into the build.

use std::path::Path;

use scribe_core::{Error, Result};

/// A decoded WAV: interleaved-then-downmixed mono `f32` samples + sample rate.
#[derive(Debug, Clone)]
pub struct WavData {
    pub sample_rate: u32,
    pub channels: u16,
    /// Mono samples in `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
}

impl WavData {
    /// Duration in milliseconds, derived from sample count and rate.
    pub fn duration_ms(&self) -> i64 {
        if self.sample_rate == 0 {
            return 0;
        }
        (self.samples.len() as i64 * 1000) / self.sample_rate as i64
    }
}

fn err(msg: impl Into<String>) -> Error {
    Error::Model(format!("wav: {}", msg.into()))
}

fn read_u16(b: &[u8], off: usize) -> Result<u16> {
    b.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| err("unexpected end of file"))
}

fn read_u32(b: &[u8], off: usize) -> Result<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| err("unexpected end of file"))
}

/// Parse a WAV file from disk into mono `f32` samples.
pub fn read_wav(path: &Path) -> Result<WavData> {
    let bytes = std::fs::read(path)?;
    parse_wav(&bytes)
}

/// Parse the canonical RIFF/WAVE layout from an in-memory buffer.
pub fn parse_wav(bytes: &[u8]) -> Result<WavData> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(err("not a RIFF/WAVE file"));
    }

    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (audio_format, channels, sample_rate, bits)
    let mut data: Option<&[u8]> = None;

    // Walk chunks starting just past the 12-byte RIFF header.
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_size = read_u32(bytes, pos + 4)? as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(chunk_size).min(bytes.len());
        let body = &bytes[body_start..body_end];

        match chunk_id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(err("fmt chunk too small"));
                }
                let audio_format = read_u16(body, 0)?;
                let channels = read_u16(body, 2)?;
                let sample_rate = read_u32(body, 4)?;
                let bits_per_sample = read_u16(body, 14)?;
                fmt = Some((audio_format, channels, sample_rate, bits_per_sample));
            }
            b"data" => {
                data = Some(body);
            }
            _ => { /* skip LIST/fact/other chunks */ }
        }

        // Chunks are word-aligned: an odd size is followed by a pad byte.
        let advance = 8 + chunk_size + (chunk_size & 1);
        pos = pos
            .checked_add(advance)
            .ok_or_else(|| err("chunk overflow"))?;
    }

    let (audio_format, channels, sample_rate, bits) =
        fmt.ok_or_else(|| err("missing fmt chunk"))?;
    let data = data.ok_or_else(|| err("missing data chunk"))?;

    if channels == 0 {
        return Err(err("zero channels"));
    }

    // 1 = PCM integer, 3 = IEEE float. WAVE_FORMAT_EXTENSIBLE (0xFFFE) commonly
    // wraps PCM; treat it as PCM and lean on bit depth.
    let is_float = audio_format == 3;
    if !is_float && audio_format != 1 && audio_format != 0xFFFE {
        return Err(err(format!("unsupported audio format {audio_format}")));
    }

    let interleaved = decode_samples(data, bits, is_float)?;
    let samples = downmix_to_mono(&interleaved, channels);

    Ok(WavData {
        sample_rate,
        channels,
        samples,
    })
}

/// Decode raw PCM/float bytes into interleaved `f32` in `[-1.0, 1.0]`.
fn decode_samples(data: &[u8], bits: u16, is_float: bool) -> Result<Vec<f32>> {
    let out = match (is_float, bits) {
        (true, 32) => data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        (false, 8) => data
            // 8-bit PCM is unsigned, centered on 128.
            .iter()
            .map(|&b| (b as f32 - 128.0) / 128.0)
            .collect(),
        (false, 16) => data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        (false, 24) => data
            .chunks_exact(3)
            .map(|c| {
                // Sign-extend 24-bit LE into i32.
                let v = (c[0] as i32) | ((c[1] as i32) << 8) | ((c[2] as i32) << 16);
                let v = (v << 8) >> 8;
                v as f32 / 8_388_608.0
            })
            .collect(),
        (false, 32) => data
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
            .collect(),
        (_, other) => return Err(err(format!("unsupported bit depth {other}"))),
    };
    Ok(out)
}

/// Average interleaved channels down to mono.
fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let ch = channels as usize;
    interleaved
        .chunks_exact(ch)
        .map(|frame| frame.iter().sum::<f32>() / ch as f32)
        .collect()
}

/// Write a minimal 16-bit PCM mono WAV (used by tests to synthesize fixtures).
#[cfg(test)]
pub fn write_pcm16_mono(path: &Path, sample_rate: u32, samples: &[i16]) -> Result<()> {
    use std::io::Write;

    let data_len = (samples.len() * 2) as u32;
    let byte_rate = sample_rate * 2;
    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }

    let mut f = std::fs::File::create(path)?;
    f.write_all(&buf)?;
    Ok(())
}
