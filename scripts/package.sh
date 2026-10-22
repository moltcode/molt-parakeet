#!/usr/bin/env bash
# One platform's npm-layout plugin tarball (package/ root) around a built
# molt-parakeet binary. package.json's molt.platforms narrows to the
# tarball's platforms.
#
# usage: package.sh <bundle: darwin|linux-x64|linux-arm64> <binary> <out.tgz>
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
bundle="$1" binary="$2" out="$3"

case "$bundle" in
  darwin) platforms='["darwin-arm64","darwin-x64"]' ;;
  linux-x64|linux-arm64) platforms="[\"$bundle\"]" ;;
  *) echo "unsupported bundle: $bundle" >&2; exit 1 ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
package="$work/package"
mkdir -p "$package/bin" "$(dirname "$out")"
cp "$binary" "$package/bin/molt-parakeet"
chmod 755 "$package/bin/molt-parakeet"
cp "$root/LICENSE" "$root/NOTICE" "$root/README.md" "$package/"
jq --argjson platforms "$platforms" '.molt.platforms = $platforms' "$root/package.json" > "$package/package.json"

if tar --version 2>/dev/null | grep -q bsdtar; then
  COPYFILE_DISABLE=1 tar --no-mac-metadata -czf "$out" -C "$work" package
else
  tar -czf "$out" -C "$work" package
fi
echo "built $out ($(du -h "$out" | cut -f1))"
