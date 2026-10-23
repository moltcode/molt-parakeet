use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use molt_parakeet::api::{ErrorBody, Health, Transcript};
use molt_parakeet::engine::{Engine, Sherpa};
use molt_parakeet::{audio, client, serve};

#[derive(Parser)]
#[command(
    name = "molt-parakeet",
    version,
    about = "Speech-to-text for Molt Code: NVIDIA Parakeet TDT 0.6B v3 on sherpa-onnx"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve Molt's stt provider API on a unix socket.
    Serve {
        #[arg(long)]
        socket: PathBuf,
        /// Directory holding encoder/decoder/joiner .int8.onnx and tokens.txt.
        #[arg(long)]
        model: PathBuf,
        /// Inference threads (default: up to 4).
        #[arg(long)]
        threads: Option<i32>,
        /// Unload the model after this many idle seconds; 0 keeps it loaded.
        #[arg(long, default_value_t = 600)]
        idle_unload: u64,
        /// Longest accepted recording.
        #[arg(long, default_value_t = 600)]
        max_seconds: u32,
    },
    /// Transcribe a wav file (or raw 16 kHz mono s16le with --raw).
    Transcribe {
        file: PathBuf,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        raw: bool,
        /// Service socket (default: $MOLT_PLUGIN_STATE_DIR/stt.sock).
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Load the model in-process when no service answers
        /// (default: $MOLT_PARAKEET_MODEL, else $MOLT_PLUGIN_ASSET_PARAKEET_V3/model).
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long)]
        threads: Option<i32>,
    },
    /// Print the running service's health JSON.
    Health {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

fn default_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(4) as i32)
        .unwrap_or(2)
}

fn default_socket(socket: Option<PathBuf>) -> Option<PathBuf> {
    socket.or_else(|| {
        std::env::var_os("MOLT_PLUGIN_STATE_DIR").map(|d| PathBuf::from(d).join("stt.sock"))
    })
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Serve {
            socket,
            model,
            threads,
            idle_unload,
            max_seconds,
        } => {
            let engine: Arc<dyn Engine> =
                Arc::new(Sherpa::new(model, threads.unwrap_or_else(default_threads))?);
            let opts = serve::ServeOptions {
                socket,
                idle_unload: (idle_unload > 0).then(|| Duration::from_secs(idle_unload)),
                max_seconds,
                stdin_liveness: std::env::var("MOLT_PARAKEET_PARENT_LIVENESS_STDIN").as_deref()
                    == Ok("1"),
            };
            tokio::runtime::Runtime::new()?.block_on(serve::run(engine, opts))
        }
        Command::Transcribe {
            file,
            json,
            raw,
            socket,
            model,
            threads,
        } => {
            let body =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let content_type = if raw {
                "audio/x-s16le; rate=16000; channels=1"
            } else if body.starts_with(b"RIFF") {
                "audio/wav"
            } else {
                bail!(
                    "{} is not a wav file (use --raw for 16 kHz mono s16le)",
                    file.display()
                );
            };
            let model = model
                .or_else(|| std::env::var_os("MOLT_PARAKEET_MODEL").map(PathBuf::from))
                .or_else(|| {
                    // Molt's shim exports the model asset's directory.
                    std::env::var_os("MOLT_PLUGIN_ASSET_PARAKEET_V3")
                        .map(|dir| PathBuf::from(dir).join("model"))
                });
            let transcript = transcribe(
                default_socket(socket).as_deref(),
                model,
                threads,
                content_type,
                &body,
            )?;
            if json {
                println!("{}", serde_json::to_string(&transcript)?);
            } else {
                println!("{}", transcript.text);
            }
            Ok(())
        }
        Command::Health { socket } => {
            let socket = default_socket(socket)
                .context("no socket: pass --socket or set MOLT_PLUGIN_STATE_DIR")?;
            let reply = client::request(&socket, "GET", "/v1/health", None, &[])?;
            let health: Health =
                serde_json::from_slice(&reply.body).context("bad health response")?;
            println!("{}", serde_json::to_string_pretty(&health)?);
            Ok(())
        }
    }
}

/// Through the running service when one answers, so the model is shared;
/// otherwise in-process.
fn transcribe(
    socket: Option<&Path>,
    model: Option<PathBuf>,
    threads: Option<i32>,
    content_type: &str,
    body: &[u8],
) -> Result<Transcript> {
    if let Some(socket) = socket.filter(|s| std::os::unix::net::UnixStream::connect(s).is_ok()) {
        let reply = client::request(socket, "POST", "/v1/transcribe", Some(content_type), body)?;
        if reply.status != 200 {
            let message = serde_json::from_slice::<ErrorBody>(&reply.body)
                .map(|e| e.error)
                .unwrap_or_else(|_| String::from_utf8_lossy(&reply.body).into_owned());
            bail!("service error ({}): {message}", reply.status);
        }
        return serde_json::from_slice(&reply.body).context("bad transcribe response");
    }

    let model = model
        .context("the Parakeet service is not running; pass --model DIR to load the model here")?;
    let audio = audio::decode(content_type, body)?;
    let engine = Sherpa::new(model, threads.unwrap_or_else(default_threads))?;
    let started = Instant::now();
    let text = engine.transcribe(&audio.samples, audio.sample_rate)?;
    Ok(Transcript {
        text,
        language: None,
        duration_ms: audio.duration_ms(),
        decode_ms: started.elapsed().as_millis() as u64,
    })
}
