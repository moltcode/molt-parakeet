//! Molt's `stt` provider contract. Every stt plugin implements these two
//! endpoints; Molt owns the shapes.
//!
//! * `GET /v1/health` → [`Health`]
//! * `POST /v1/transcribe` with a raw audio body → [`Transcript`]
//!   - `Content-Type: audio/x-s16le; rate=16000; channels=1` (raw little-endian
//!     PCM; `rate` and `channels` default to 16000 and 1), or
//!   - `Content-Type: audio/wav` (PCM 16-bit or float 32-bit, any rate).
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
