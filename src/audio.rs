//! Request bodies to mono f32 samples. Resampling is left to sherpa-onnx,
//! which accepts any input rate.

use anyhow::{Context, Result, bail};

#[derive(Clone, Debug, PartialEq)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl Audio {
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        self.samples.len() as u64 * 1000 / self.sample_rate as u64
    }
}

/// Decodes a body by its `Content-Type`.
pub fn decode(content_type: &str, body: &[u8]) -> Result<Audio> {
    let mut parts = content_type.split(';').map(str::trim);
    let mime = parts.next().unwrap_or_default().to_ascii_lowercase();
    let params: Vec<(String, String)> = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let param = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };

    match mime.as_str() {
        "audio/x-s16le" | "audio/pcm" => {
            let rate = parse_param(param("rate"), 16_000).context("bad rate")?;
            let channels = parse_param(param("channels"), 1).context("bad channels")?;
            pcm_s16le(body, rate, channels)
        }
        "audio/wav" | "audio/wave" | "audio/x-wav" => wav(body),
        "" => bail!("missing Content-Type"),
        other => bail!("unsupported Content-Type {other}"),
    }
}

fn parse_param(value: Option<&str>, default: u32) -> Result<u32> {
    match value {
        None => Ok(default),
        Some(v) => {
            let n: u32 = v.parse()?;
            if n == 0 {
                bail!("must be positive");
            }
            Ok(n)
        }
    }
}

/// Raw interleaved little-endian 16-bit PCM, downmixed to mono.
pub fn pcm_s16le(body: &[u8], sample_rate: u32, channels: u32) -> Result<Audio> {
    if !(8_000..=192_000).contains(&sample_rate) {
        bail!("sample rate {sample_rate} out of range");
    }
    let frame = 2 * channels as usize;
    if !body.len().is_multiple_of(frame) {
        bail!("body is not a whole number of {channels}-channel s16le frames");
    }
    let samples = body
        .chunks_exact(frame)
        .map(|f| {
            let sum: f32 = f
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
                .sum();
            sum / channels as f32
        })
        .collect();
    Ok(Audio {
        samples,
        sample_rate,
    })
}

/// RIFF/WAVE with PCM 16-bit or IEEE float 32-bit samples.
pub fn wav(body: &[u8]) -> Result<Audio> {
    if body.len() < 12 || &body[0..4] != b"RIFF" || &body[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file");
    }
    let mut pos = 12;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    while pos + 8 <= body.len() {
        let id = &body[pos..pos + 4];
        let size = u32::from_le_bytes(body[pos + 4..pos + 8].try_into()?) as usize;
        let start = pos + 8;
        let end = start.saturating_add(size).min(body.len());
        let chunk = &body[start..end];
        match id {
            b"fmt " => {
                if chunk.len() < 16 {
                    bail!("short fmt chunk");
                }
                let mut tag = u16::from_le_bytes([chunk[0], chunk[1]]);
                let channels = u16::from_le_bytes([chunk[2], chunk[3]]);
                let rate = u32::from_le_bytes(chunk[4..8].try_into()?);
                let bits = u16::from_le_bytes([chunk[14], chunk[15]]);
                // WAVE_FORMAT_EXTENSIBLE: the real tag is the subformat GUID's first two bytes.
                if tag == 0xFFFE && chunk.len() >= 26 {
                    tag = u16::from_le_bytes([chunk[24], chunk[25]]);
                }
                format = Some((tag, channels, rate, bits));
            }
            b"data" => {
                let (tag, channels, rate, bits) = format.context("data chunk before fmt")?;
                if channels == 0 {
                    bail!("zero channels");
                }
                return match (tag, bits) {
                    (1, 16) => pcm_s16le(
                        trim_frames(chunk, 2 * channels as usize),
                        rate,
                        channels as u32,
                    ),
                    (3, 32) => f32le(
                        trim_frames(chunk, 4 * channels as usize),
                        rate,
                        channels as u32,
                    ),
                    _ => bail!("unsupported wav encoding (format {tag}, {bits} bits)"),
                };
            }
            _ => {}
        }
        // Chunks are word-aligned.
        pos = start + size + (size & 1);
    }
    bail!("wav has no data chunk")
}

fn trim_frames(data: &[u8], frame: usize) -> &[u8] {
    &data[..data.len() - data.len() % frame]
}

fn f32le(body: &[u8], sample_rate: u32, channels: u32) -> Result<Audio> {
    let frame = 4 * channels as usize;
    let samples = body
        .chunks_exact(frame)
        .map(|f| {
            let sum: f32 = f
                .chunks_exact(4)
                .map(|s| f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
                .sum();
            sum / channels as f32
        })
        .collect();
    Ok(Audio {
        samples,
        sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_bytes(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        let block = channels * bits / 8;
        out.extend_from_slice(&(rate * block as u32).to_le_bytes());
        out.extend_from_slice(&block.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn raw_pcm_defaults_to_16k_mono() {
        let body = [0x00, 0x40, 0x00, 0xC0];
        let audio = decode("audio/x-s16le", &body).unwrap();
        assert_eq!(audio.sample_rate, 16_000);
        assert_eq!(audio.samples, vec![0.5, -0.5]);
    }

    #[test]
    fn raw_pcm_downmixes_stereo() {
        let body = [0x00, 0x40, 0x00, 0x00];
        let audio = decode("audio/x-s16le; rate=48000; channels=2", &body).unwrap();
        assert_eq!(audio.sample_rate, 48_000);
        assert_eq!(audio.samples, vec![0.25]);
    }

    #[test]
    fn raw_pcm_rejects_partial_frames() {
        assert!(decode("audio/x-s16le", &[0, 0, 0]).is_err());
    }

    #[test]
    fn wav_pcm16() {
        let body = wav_bytes(1, 1, 22_050, 16, &[0x00, 0x40]);
        let audio = decode("audio/wav", &body).unwrap();
        assert_eq!(audio.sample_rate, 22_050);
        assert_eq!(audio.samples, vec![0.5]);
    }

    #[test]
    fn wav_float32_stereo() {
        let mut data = Vec::new();
        data.extend_from_slice(&1.0f32.to_le_bytes());
        data.extend_from_slice(&0.0f32.to_le_bytes());
        let audio = wav(&wav_bytes(3, 2, 16_000, 32, &data)).unwrap();
        assert_eq!(audio.samples, vec![0.5]);
    }

    #[test]
    fn rejects_unknown_types() {
        assert!(decode("audio/mpeg", &[]).is_err());
        assert!(decode("", &[]).is_err());
        assert!(wav(b"not a wav").is_err());
    }

    #[test]
    fn duration() {
        let audio = Audio {
            samples: vec![0.0; 24_000],
            sample_rate: 16_000,
        };
        assert_eq!(audio.duration_ms(), 1_500);
    }
}
