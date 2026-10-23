//! The stt provider API against a fake engine, over a real unix socket.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use molt_parakeet::api::{self, ErrorBody, Health, Hotword, Transcript};
use molt_parakeet::client;
use molt_parakeet::engine::Engine;
use molt_parakeet::serve::{self, ServeOptions};

#[derive(Default)]
struct Fake {
    loaded: AtomicBool,
}

impl Engine for Fake {
    fn transcribe(
        &self,
        samples: &[f32],
        sample_rate: u32,
        hotwords: &[Hotword],
    ) -> anyhow::Result<String> {
        self.loaded.store(true, Ordering::SeqCst);
        if hotwords.is_empty() {
            Ok(format!("{} samples at {sample_rate}", samples.len()))
        } else {
            let words: Vec<_> = hotwords.iter().map(|h| h.text.as_str()).collect();
            Ok(format!("hotwords {}", words.join(",")))
        }
    }
    fn loaded(&self) -> bool {
        self.loaded.load(Ordering::SeqCst)
    }
    fn unload_if_idle(&self, _idle: Duration) {
        self.loaded.store(false, Ordering::SeqCst);
    }
}

fn start(max_seconds: u32) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("stt.sock");
    let opts = ServeOptions {
        socket: socket.clone(),
        idle_unload: None,
        max_seconds,
        stdin_liveness: false,
    };
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(serve::run(Arc::new(Fake::default()), opts))
            .unwrap();
    });
    for _ in 0..100 {
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    (dir, socket)
}

#[test]
fn health_reports_the_model() {
    let (_dir, socket) = start(600);
    let reply = client::request(&socket, "GET", "/v1/health", None, &[]).unwrap();
    assert_eq!(reply.status, 200);
    let health: Health = serde_json::from_slice(&reply.body).unwrap();
    assert!(health.ready);
    assert!(!health.loaded);
    assert_eq!(health.sample_rate, 16_000);
    assert!(health.languages.iter().any(|l| l == "en"));
}

#[test]
fn transcribes_raw_pcm() {
    let (_dir, socket) = start(600);
    let pcm = vec![0u8; 16_000 * 2];
    let reply = client::request(
        &socket,
        "POST",
        "/v1/transcribe",
        Some("audio/x-s16le; rate=16000; channels=1"),
        &pcm,
    )
    .unwrap();
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let t: Transcript = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(t.text, "16000 samples at 16000");
    assert_eq!(t.duration_ms, 1000);
}

#[test]
fn hotwords_reach_the_engine() {
    let (_dir, socket) = start(600);
    let pcm = vec![0u8; 3200];
    let hotwords = vec![
        Hotword {
            text: "Junaid".into(),
            boost: None,
        },
        Hotword {
            text: "Molt Connect".into(),
            boost: Some(3.0),
        },
    ];
    let reply = client::request_with(
        &socket,
        "POST",
        "/v1/transcribe",
        Some("audio/x-s16le; rate=16000; channels=1"),
        &[(api::HOTWORDS_HEADER, api::encode_hotwords(&hotwords))],
        &pcm,
    )
    .unwrap();
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let t: Transcript = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(t.text, "hotwords Junaid,Molt Connect");

    let bad = client::request_with(
        &socket,
        "POST",
        "/v1/transcribe",
        Some("audio/x-s16le; rate=16000; channels=1"),
        &[(api::HOTWORDS_HEADER, "not base64!".into())],
        &pcm,
    )
    .unwrap();
    assert_eq!(bad.status, 400);
}

#[test]
fn empty_audio_is_empty_text() {
    let (_dir, socket) = start(600);
    let reply = client::request(
        &socket,
        "POST",
        "/v1/transcribe",
        Some("audio/x-s16le"),
        &[],
    )
    .unwrap();
    let t: Transcript = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(t.text, "");
}

#[test]
fn rejects_unsupported_media() {
    let (_dir, socket) = start(600);
    let reply =
        client::request(&socket, "POST", "/v1/transcribe", Some("audio/mpeg"), b"xx").unwrap();
    assert_eq!(reply.status, 415);
    let e: ErrorBody = serde_json::from_slice(&reply.body).unwrap();
    assert!(e.error.contains("audio/mpeg"));
}

#[test]
fn rejects_audio_over_the_limit() {
    let (_dir, socket) = start(1);
    let pcm = vec![0u8; 16_000 * 2 * 2];
    let reply = client::request(
        &socket,
        "POST",
        "/v1/transcribe",
        Some("audio/x-s16le"),
        &pcm,
    )
    .unwrap();
    assert_eq!(reply.status, 413);
}

#[test]
fn refuses_a_live_socket() {
    let (_dir, socket) = start(600);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let opts = ServeOptions {
        socket,
        idle_unload: None,
        max_seconds: 600,
        stdin_liveness: false,
    };
    let err = rt
        .block_on(serve::run(Arc::new(Fake::default()), opts))
        .unwrap_err();
    assert!(err.to_string().contains("already listening"));
}
