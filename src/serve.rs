//! `molt-parakeet serve`: the stt provider over HTTP/1.1 on a unix socket.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use tokio::io::AsyncReadExt;
use tokio::net::UnixListener;

use crate::api::{self, ErrorBody, Health, Transcript};
use crate::audio;
use crate::engine::Engine;

/// Big enough for `max_seconds` of 48 kHz stereo 16-bit wav; the decoded
/// duration is checked against the limit separately.
fn body_limit(max_seconds: u32) -> usize {
    max_seconds as usize * 48_000 * 2 * 2 + 4096
}

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<dyn Engine>,
    pub max_seconds: u32,
}

pub fn router(state: AppState) -> Router {
    let limit = body_limit(state.max_seconds);
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/transcribe", post(transcribe))
        .layer(DefaultBodyLimit::max(limit))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        ready: true,
        loaded: state.engine.loaded(),
        model: crate::MODEL_ID.to_string(),
        languages: crate::LANGUAGES.iter().map(|l| l.to_string()).collect(),
        sample_rate: crate::SAMPLE_RATE,
        max_seconds: state.max_seconds,
    })
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorBody {
            error: message.into(),
        }),
    )
        .into_response()
}

async fn transcribe(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let audio = match audio::decode(content_type, &body) {
        Ok(audio) => audio,
        Err(err) => return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, format!("{err:#}")),
    };
    let hotwords = match headers.get(api::HOTWORDS_HEADER).map(|v| v.to_str()) {
        None => Vec::new(),
        Some(Ok(value)) => match api::decode_hotwords(value) {
            Ok(hotwords) => hotwords,
            Err(err) => return error(StatusCode::BAD_REQUEST, format!("bad X-Hotwords: {err:#}")),
        },
        Some(Err(_)) => return error(StatusCode::BAD_REQUEST, "bad X-Hotwords: not ASCII"),
    };
    let duration_ms = audio.duration_ms();
    if duration_ms > state.max_seconds as u64 * 1000 {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "audio is {} s, the limit is {} s",
                duration_ms / 1000,
                state.max_seconds
            ),
        );
    }
    if audio.samples.is_empty() {
        return Json(Transcript {
            text: String::new(),
            language: None,
            duration_ms: 0,
            decode_ms: 0,
        })
        .into_response();
    }

    let engine = state.engine.clone();
    let started = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        engine.transcribe(&audio.samples, audio.sample_rate, &hotwords)
    })
    .await;
    let decode_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(text)) => Json(Transcript {
            text,
            language: None,
            duration_ms,
            decode_ms,
        })
        .into_response(),
        Ok(Err(err)) => error(StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}")),
        Err(err) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("transcription panicked: {err}"),
        ),
    }
}

pub struct ServeOptions {
    pub socket: PathBuf,
    pub idle_unload: Option<Duration>,
    pub max_seconds: u32,
    /// Exit when stdin reaches EOF: Molt holds the service's stdin open for
    /// as long as it wants the service running.
    pub stdin_liveness: bool,
}

pub async fn run(engine: Arc<dyn Engine>, opts: ServeOptions) -> Result<()> {
    let listener = bind(&opts.socket)?;
    eprintln!("molt-parakeet: listening on {}", opts.socket.display());

    if let Some(idle) = opts.idle_unload {
        let engine = engine.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15).min(idle));
            loop {
                tick.tick().await;
                let engine = engine.clone();
                let _ = tokio::task::spawn_blocking(move || engine.unload_if_idle(idle)).await;
            }
        });
    }

    let app = router(AppState {
        engine,
        max_seconds: opts.max_seconds,
    });
    let socket = opts.socket.clone();
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown(opts.stdin_liveness))
        .await
        .context("server failed");
    let _ = std::fs::remove_file(&socket);
    result
}

/// Binds the socket, replacing a stale one left by a crashed run but never
/// one another live server answers on.
fn bind(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            anyhow::bail!("another server is already listening on {}", path.display());
        }
        std::fs::remove_file(path).with_context(|| format!("removing stale {}", path.display()))?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))
}

async fn shutdown(stdin_liveness: bool) {
    let stdin_eof = async {
        if stdin_liveness {
            let mut stdin = tokio::io::stdin();
            let mut buf = [0u8; 1024];
            while let Ok(n) = stdin.read(&mut buf).await {
                if n == 0 {
                    break;
                }
            }
            eprintln!("molt-parakeet: stdin closed, exiting");
        } else {
            std::future::pending::<()>().await;
        }
    };
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    tokio::select! {
        _ = stdin_eof => {}
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
