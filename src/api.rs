//! Molt's `stt` provider contract. Every stt plugin implements these two
//! endpoints; Molt owns the shapes.
//!
//! * `GET /v1/health` → [`Health`]
//! * `POST /v1/transcribe` with a raw audio body → [`Transcript`]
//!   - `Content-Type: audio/x-s16le; rate=16000; channels=1` (raw little-endian
//!     PCM; `rate` and `channels` default to 16000 and 1), or
//!   - `Content-Type: audio/wav` (PCM 16-bit or float 32-bit, any rate).
//!
//!   - optional `X-Hotwords`: base64 of a JSON array of [`Hotword`], words to
//!     bias decoding toward (names, jargon). Providers that can't bias
//!     decoding ignore it.
//!
//! Errors are `{"error": "..."}` with a 4xx/5xx status.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Health {
    pub ready: bool,
    pub loaded: bool,
    pub model: String,
    pub languages: Vec<String>,
    pub sample_rate: u32,
    pub max_seconds: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Transcript {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub duration_ms: u64,
    pub decode_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ErrorBody {
    pub error: String,
}

/// One word or phrase to bias decoding toward.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Hotword {
    pub text: String,
    /// Score added per matched token; omitted means the service default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost: Option<f32>,
}

pub const HOTWORDS_HEADER: &str = "x-hotwords";

/// Encodes hotwords for the `X-Hotwords` header.
pub fn encode_hotwords(hotwords: &[Hotword]) -> String {
    crate::b64::encode(
        serde_json::to_string(hotwords)
            .expect("hotwords serialize")
            .as_bytes(),
    )
}

/// Decodes an `X-Hotwords` header value.
pub fn decode_hotwords(value: &str) -> anyhow::Result<Vec<Hotword>> {
    let json = crate::b64::decode(value.trim())?;
    Ok(serde_json::from_slice(&json)?)
}
