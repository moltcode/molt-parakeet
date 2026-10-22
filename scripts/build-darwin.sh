#!/usr/bin/env bash
# Universal macOS binary: arm64 + x86_64 builds lipo'd together, signed with
# Molt's Developer ID (hardened runtime, timestamp) and notarized.
#
# usage: build-darwin.sh <out-binary>
# env:   MOLT_SIGN_IDENTITY (default: Molt's Developer ID)
#        NOTARIZE_ENV (default ../acp/.notarize.env) with APPLE_ID,
#        APPLE_PASSWORD (app-specific), APPLE_TEAM_ID
#        SKIP_NOTARIZE=1 to sign only
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
out="$1"
identity="${MOLT_SIGN_IDENTITY:-Developer ID Application: Utpun Tech Labs Private Limited (ZW445NH299)}"

cd "$root"
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  MACOSX_DEPLOYMENT_TARGET=12.0 cargo build --release --locked --target "$target"
done
mkdir -p "$(dirname "$out")"
lipo -create -output "$out" \
  target/aarch64-apple-darwin/release/molt-parakeet \
  target/x86_64-apple-darwin/release/molt-parakeet
codesign --force --options runtime --timestamp --sign "$identity" "$out"
codesign --verify --strict "$out"
lipo -info "$out"

if [ "${SKIP_NOTARIZE:-}" != "1" ]; then
  env_file="${NOTARIZE_ENV:-$(cd "$root/.." && pwd)/acp/.notarize.env}"
  # shellcheck disable=SC1090
  source "$env_file"
  zip="$(mktemp -d)/molt-parakeet.zip"
  ditto -c -k "$out" "$zip"
  result="$(xcrun notarytool submit "$zip" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" --wait --timeout 30m)"
  rm -rf "$(dirname "$zip")"
  echo "$result" >&2
  grep -q "status: Accepted" <<< "$result" || { echo "notarization was not accepted" >&2; exit 1; }
  # A bare Mach-O can't carry a stapled ticket; Gatekeeper looks it up online.
  echo "notarized $out"
fi
