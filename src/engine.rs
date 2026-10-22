//! The recognizer: sherpa-onnx's offline transducer with the Parakeet v3
//! int8 export, loaded on first use and dropped after an idle period.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};

/// What `serve` needs from a recognizer. Tests use a fake.
pub trait Engine: Send + Sync + 'static {
    /// Mono samples in [-1, 1] at `sample_rate`. Blocks; callers run it off
    /// the async runtime. Calls are serialized.
    fn transcribe(&self, samples: &[f32], sample_rate: u32) -> Result<String>;
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

pub struct Sherpa {
    dir: PathBuf,
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
        Ok(Self {
            dir,
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
        config.decoding_method = Some("greedy_search".into());
        config
    }
}

impl Engine for Sherpa {
    fn transcribe(&self, samples: &[f32], sample_rate: u32) -> Result<String> {
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
        let stream = recognizer.create_stream();
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
