//! The recognizer: sherpa-onnx's offline transducer with the Parakeet v3
//! int8 export, loaded on first use and dropped after an idle period.
//!
//! Decoding is `modified_beam_search` so each request can carry hotwords
//! (names and jargon to bias toward) without reloading the model. Hotwords
//! are tokenized with a BPE vocab that sherpa-onnx needs but the model export
//! doesn't ship; it is derived from `tokens.txt` on load.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};

use crate::api::Hotword;

/// Hotword score when a hotword names none. Above ~4 output degrades
/// ("main" heard as "mane"), so requests are capped there too.
pub const DEFAULT_BOOST: f32 = 2.0;
pub const MAX_BOOST: f32 = 4.0;
/// Most hotwords one request may carry.
pub const MAX_HOTWORDS: usize = 1000;

/// What `serve` needs from a recognizer. Tests use a fake.
pub trait Engine: Send + Sync + 'static {
    /// Mono samples in [-1, 1] at `sample_rate`. Blocks; callers run it off
    /// the async runtime. Calls are serialized.
    fn transcribe(&self, samples: &[f32], sample_rate: u32, hotwords: &[Hotword])
    -> Result<String>;
    fn loaded(&self) -> bool;
    /// Drops the model if it has not been used for `idle`.
    fn unload_if_idle(&self, idle: Duration);
}

pub const MODEL_FILES: &[&str] = &[
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];

/// Checks that `dir` holds the Parakeet export.
pub fn check_model_dir(dir: &Path) -> Result<()> {
    for file in MODEL_FILES {
        let path = dir.join(file);
        if !path.is_file() {
            bail!("model file missing: {}", path.display());
        }
    }
    Ok(())
}

/// sherpa-onnx's per-stream hotwords: one `phrase :score` per entry,
/// `/`-separated. Entries that would break that syntax, or carry nothing to
/// say, are dropped.
pub fn hotwords_spec(hotwords: &[Hotword]) -> String {
    hotwords
        .iter()
        .filter_map(|h| {
            let text = h.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let sayable = text.chars().any(char::is_alphanumeric);
            if !sayable || text.contains(['/', ':']) {
                return None;
            }
            let boost = h.boost.unwrap_or(DEFAULT_BOOST).clamp(0.0, MAX_BOOST);
            Some(format!("{text} :{boost:.2}"))
        })
        .take(MAX_HOTWORDS)
        .collect::<Vec<_>>()
        .join("/")
}

/// A sentencepiece-style vocab (`piece<TAB>score`) from sherpa's
/// `tokens.txt` (`piece id`), so sherpa can split hotwords into the model's
/// tokens.
pub fn bpe_vocab(tokens: &str) -> String {
    let mut out = String::with_capacity(tokens.len() + tokens.len() / 2);
    for line in tokens.lines() {
        if let Some((piece, id)) = line.rsplit_once(' ')
            && let Ok(id) = id.trim().parse::<i64>()
            && !piece.is_empty()
        {
            out.push_str(&format!("{piece}\t-{id}\n"));
        }
    }
    out
}

/// Writes the vocab once per distinct `tokens.txt` under the temp dir; the
/// model directory is a shared, content-addressed asset and stays untouched.
fn write_bpe_vocab(dir: &Path) -> Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let tokens = std::fs::read_to_string(dir.join("tokens.txt")).context("reading tokens.txt")?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tokens.hash(&mut hasher);
    let path =
        std::env::temp_dir().join(format!("molt-parakeet-bpe-{:016x}.vocab", hasher.finish()));
    if !path.is_file() {
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, bpe_vocab(&tokens))
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(path)
}

pub struct Sherpa {
    dir: PathBuf,
    bpe_vocab: PathBuf,
    threads: i32,
    state: Mutex<State>,
}

struct State {
    recognizer: Option<OfflineRecognizer>,
    last_used: Instant,
}

impl Sherpa {
    pub fn new(dir: PathBuf, threads: i32) -> Result<Self> {
        check_model_dir(&dir)?;
        let bpe_vocab = write_bpe_vocab(&dir)?;
        Ok(Self {
            dir,
            bpe_vocab,
            threads,
            state: Mutex::new(State {
                recognizer: None,
                last_used: Instant::now(),
            }),
        })
    }

    fn config(&self) -> OfflineRecognizerConfig {
        let file = |name: &str| Some(self.dir.join(name).to_string_lossy().into_owned());
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: file("encoder.int8.onnx"),
            decoder: file("decoder.int8.onnx"),
            joiner: file("joiner.int8.onnx"),
        };
        config.model_config.tokens = file("tokens.txt");
        config.model_config.model_type = Some("nemo_transducer".into());
        config.model_config.num_threads = self.threads;
        config.model_config.modeling_unit = Some("bpe".into());
        config.model_config.bpe_vocab = Some(self.bpe_vocab.to_string_lossy().into_owned());
        config.decoding_method = Some("modified_beam_search".into());
        config.max_active_paths = 4;
        config.hotwords_score = DEFAULT_BOOST;
        config
    }
}

impl Engine for Sherpa {
    fn transcribe(
        &self,
        samples: &[f32],
        sample_rate: u32,
        hotwords: &[Hotword],
    ) -> Result<String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.recognizer.is_none() {
            let started = Instant::now();
            let recognizer = OfflineRecognizer::create(&self.config())
                .with_context(|| format!("failed to load the model from {}", self.dir.display()))?;
            eprintln!(
                "molt-parakeet: model loaded in {} ms",
                started.elapsed().as_millis()
            );
            state.recognizer = Some(recognizer);
        }
        let recognizer = state.recognizer.as_ref().expect("loaded above");
        let spec = hotwords_spec(hotwords);
        let stream = if spec.is_empty() {
            recognizer.create_stream()
        } else {
            recognizer.create_stream_with_hotwords(&spec)
        };
        stream.accept_waveform(sample_rate as i32, samples);
        recognizer.decode(&stream);
        let result = stream
            .get_result()
            .context("recognizer returned no result")?;
        state.last_used = Instant::now();
        Ok(result.text.trim().to_string())
    }

    fn loaded(&self) -> bool {
        self.state
            .lock()
            .map(|s| s.recognizer.is_some())
            .unwrap_or(false)
    }

    fn unload_if_idle(&self, idle: Duration) {
        // try_lock: a running transcription is by definition not idle.
        if let Ok(mut state) = self.state.try_lock()
            && state.recognizer.is_some()
            && state.last_used.elapsed() >= idle
        {
            state.recognizer = None;
            eprintln!(
                "molt-parakeet: model unloaded after {} s idle",
                idle.as_secs()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(text: &str, boost: Option<f32>) -> Hotword {
        Hotword {
            text: text.into(),
            boost,
        }
    }

    #[test]
    fn hotwords_spec_scores_and_filters() {
        let spec = hotwords_spec(&[
            hw("Junaid", None),
            hw("  Molt   Connect ", Some(3.0)),
            hw("GPUI", Some(9.0)),
            hw("a/b", None),
            hw("key: value", None),
            hw("--", None),
        ]);
        assert_eq!(spec, "Junaid :2.00/Molt Connect :3.00/GPUI :4.00");
        assert_eq!(hotwords_spec(&[]), "");
    }

    #[test]
    fn bpe_vocab_from_tokens() {
        let vocab = bpe_vocab("<unk> 0\n\u{2581}the 5\nab 12\nbroken\n");
        assert_eq!(vocab, "<unk>\t-0\n\u{2581}the\t-5\nab\t-12\n");
    }
}
