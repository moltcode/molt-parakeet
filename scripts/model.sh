#!/usr/bin/env bash
# Builds the model asset from the pinned upstream export in models.lock.json:
# downloads the k2-fsa bz2, checks its sha256 and every model file's sha256,
# and repacks the four files Molt needs as a tar.gz with a `model/` root
# (Molt's installer only extracts tar.gz). Writes the archive and
# model.json (url, sha256, size for the Molt catalog's `assets`) to <out-dir>.
#
# usage: model.sh <cache-dir> <out-dir>
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cache="$1" out="$2"
lock="$root/models.lock.json"

id=$(jq -r .id "$lock")
model=$(jq -r .model "$lock")
revision=$(jq -r .revision "$lock")
url=$(jq -r .upstream.url "$lock")
want=$(jq -r .upstream.sha256 "$lock")
upstream_root=$(jq -r .upstream.root "$lock")
name="parakeet-tdt-0.6b-v3-int8-$revision"
tag="model-$name"

mkdir -p "$cache" "$out"
source="$cache/$(basename "$url")"
if [ ! -f "$source" ]; then
  echo "downloading $url"
  curl -fL --retry 3 -C - -o "$source.part" "$url"
  mv "$source.part" "$source"
fi
got=$(shasum -a 256 "$source" | awk '{print $1}')
[ "$got" = "$want" ] || { echo "$source sha256 $got != pinned $want" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/model"
members=()
for file in $(jq -r '.files | keys[]' "$lock"); do members+=("$upstream_root/$file"); done
tar -xjf "$source" -C "$work" "${members[@]}"

for file in $(jq -r '.files | keys[]' "$lock"); do
  want_file=$(jq -r --arg f "$file" '.files[$f].sha256' "$lock")
  got_file=$(shasum -a 256 "$work/$upstream_root/$file" | awk '{print $1}')
  [ "$got_file" = "$want_file" ] || { echo "$file sha256 $got_file != pinned $want_file" >&2; exit 1; }
  mv "$work/$upstream_root/$file" "$work/model/$file"
done

cat > "$work/model/NOTICE" <<NOTICE
NVIDIA Parakeet TDT 0.6B v3 (https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3),
licensed CC-BY-4.0 by NVIDIA. ONNX int8 export by the k2-fsa/sherpa-onnx
project ($model). Repacked unmodified for Molt Code.
NOTICE

archive="$out/$name.tgz"
if tar --version 2>/dev/null | grep -q bsdtar; then
  COPYFILE_DISABLE=1 tar --no-mac-metadata -czf "$archive" -C "$work" model
else
  tar -czf "$archive" -C "$work" model
fi

sha=$(shasum -a 256 "$archive" | awk '{print $1}')
size=$(wc -c < "$archive" | tr -d ' ')
jq -n --arg id "$id" --arg url "https://github.com/moltcode/molt-parakeet/releases/download/$tag/$name.tgz" \
  --arg sha "$sha" --argjson size "$size" --arg tag "$tag" \
  '{id: $id, tag: $tag, url: $url, sha256: $sha, size: $size}' > "$out/model.json"
cat "$out/model.json"
