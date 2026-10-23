//! molt-parakeet: speech-to-text for Molt Code, NVIDIA Parakeet TDT 0.6B v3
//! running on sherpa-onnx.
//!
//! `serve` implements Molt's `stt` provider contract (see `api`) as HTTP/1.1
//! on a unix socket. The model loads on the first request and unloads after
//! an idle period, so an installed but unused plugin costs no memory.

pub mod api;
pub mod audio;
mod b64;
pub mod client;
pub mod engine;
pub mod serve;

/// The sample rate every request is resampled to before decoding.
pub const SAMPLE_RATE: u32 = 16_000;

/// Upstream model id, reported by `/v1/health`.
pub const MODEL_ID: &str = "nemo-parakeet-tdt-0.6b-v3-int8";

/// Languages Parakeet TDT 0.6B v3 transcribes. It detects the language
/// itself; there is no language hint.
pub const LANGUAGES: &[&str] = &[
    "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt", "mt",
    "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk",
];
