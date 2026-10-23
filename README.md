# molt-parakeet

Voice input for Molt Code. This Molt plugin runs NVIDIA
[Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) on
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx), on your own machine.
Nothing you say leaves it.

It is Molt's first **provider** plugin: it contributes an `stt`
(speech-to-text) provider that the composer's mic button uses. The client
records the mic and streams 16 kHz PCM to the Molt backend over `voice:*`
RPCs; the backend forwards the finished recording to this plugin's service.
That works the same from the desktop, iOS, Android and over Molt Connect.

Parakeet v3 transcribes 25 European languages and detects the language
itself: bg, hr, cs, da, nl, en, et, fi, fr, de, el, hu, it, lv, lt, mt, pl,
pt, ro, sk, sl, es, sv, ru, uk.

## What it adds

- `services.stt`: `molt-parakeet serve`, owned by Molt, on
  `<plugin-state>/stt.sock`. It runs on demand: Molt starts it when a
  recording begins and stops it after 5 minutes without one, so an idle
  install runs no process. The model loads on the first request (about 1 s
  on Apple Silicon), holds about 1.2 GB, and is dropped after 2 idle minutes
  even while the service runs.
- `providers.parakeet-v3`: declares that service as an `stt` provider.
- `bin.molt-parakeet`: agents can transcribe audio files too, e.g.
  `molt-parakeet transcribe meeting.wav`. It goes through the service when it
  is running, and loads the model in-process otherwise.
- The model, about 620 MB, is a separate catalog **asset** (`parakeet-v3`).
  Molt stores it content-addressed, so upgrading the binary doesn't download
  it again.

## The `stt` provider API

HTTP/1.1 on a unix socket. Every stt plugin implements this; Molt owns the
shapes.

```
GET /v1/health
→ {"ready": true, "loaded": false, "model": "nemo-parakeet-tdt-0.6b-v3-int8",
   "languages": ["en", ...], "sample_rate": 16000, "max_seconds": 600}

POST /v1/transcribe
Content-Type: audio/x-s16le; rate=16000; channels=1   (raw little-endian PCM)
           or audio/wav                               (PCM16 or float32, any rate)
→ {"text": "...", "duration_ms": 3845, "decode_ms": 349}
```

Errors are `{"error": "..."}` with 413 (over `max_seconds`), 415 (bad
audio) or 500.

## CLI

```
molt-parakeet serve --socket PATH --model DIR [--threads N] [--idle-unload SECS] [--max-seconds N]
molt-parakeet transcribe FILE.wav [--json] [--raw] [--socket PATH] [--model DIR]
molt-parakeet health [--socket PATH]
```

`serve` exits when stdin closes if `MOLT_PARAKEET_PARENT_LIVENESS_STDIN=1`;
Molt holds the service's stdin open for as long as it wants it running.

## Package layout

One npm-layout tarball per platform (`package/` root), about 9 MB:

```
package/
  package.json         # manifest: molt.contributes services, providers, bin; molt.assets
  bin/molt-parakeet    # sherpa-onnx and onnxruntime statically linked
  LICENSE NOTICE README.md
```

The model asset is `parakeet-tdt-0.6b-v3-int8-<rev>.tgz` with a `model/` root
holding `encoder.int8.onnx`, `decoder.int8.onnx`, `joiner.int8.onnx`,
`tokens.txt` and `NOTICE`.

Platforms: `darwin-arm64` and `darwin-x64` share a universal binary, signed
with Molt's Developer ID and notarized; plus `linux-x64` and `linux-arm64`
(built on Ubuntu 22.04, glibc 2.35).

## Releasing

Two release streams, because the binary changes often and the model rarely.

Binary:

1. Bump `version` in `package.json` and `Cargo.toml`.
2. Tag `v<version>` and push. The release workflow builds and publishes the
   Linux tarballs.
3. `make release-darwin` on a Mac with the signing identity: builds the
   universal binary, signs and notarizes it, uploads it, then writes and
   uploads `artifacts.json` (url, sha256 and size per platform) for the Molt
   catalog.

Model: bump `revision` in `models.lock.json` and push the tag
`model-parakeet-tdt-0.6b-v3-int8-<revision>`. The workflow downloads the
upstream k2-fsa export, checks it against the pinned sha256s, repacks it and
publishes it with `model.json` (url, sha256, size). k2-fsa publishes no
checksums for these exports, so `models.lock.json` holds our own pins.

Published assets are never replaced: the Molt catalog pins their sha256.

## Credits

NVIDIA Parakeet TDT 0.6B v3 is licensed CC-BY-4.0 by NVIDIA. sherpa-onnx is
Apache-2.0 (k2-fsa). ONNX Runtime is MIT (Microsoft). This repo's code is MIT.
See `NOTICE`.
