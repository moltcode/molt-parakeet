#!/usr/bin/env bash
# The Molt catalog's per-platform `artifacts` map for release v<version>,
# computed from the tarballs published on that release. darwin-arm64 and
# darwin-x64 share the universal tarball.
#
# usage: artifacts.sh <version>
set -euo pipefail
version="$1"
name=molt-parakeet
base="https://github.com/moltcode/$name/releases/download/v$version"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gh release download "v$version" --repo "moltcode/$name" --pattern '*.tgz' --dir "$work"

entry() {
  local file="$work/$name-$version-$1.tgz"
  [ -f "$file" ] || { echo "release v$version has no $(basename "$file")" >&2; exit 1; }
  jq -n --arg url "$base/$(basename "$file")" --arg sha "$(shasum -a 256 "$file" | awk '{print $1}')" \
    --argjson size "$(wc -c < "$file" | tr -d ' ')" '{url: $url, sha256: $sha, size: $size}'
}

jq -n --argjson darwin "$(entry darwin)" --argjson x64 "$(entry linux-x64)" --argjson arm64 "$(entry linux-arm64)" \
  '{"darwin-arm64": $darwin, "darwin-x64": $darwin, "linux-x64": $x64, "linux-arm64": $arm64}'
