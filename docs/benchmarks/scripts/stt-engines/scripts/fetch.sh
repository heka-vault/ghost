#!/bin/bash
# Fetch the pinned GGUF files and check each SHA-256. Generic client, no identifying headers.
set -u
B=~/.cache/ghost-stt-bench/models
while read id repo rev file size sha; do
  mkdir -p "$B/$id"
  out="$B/$id/$file"
  if [[ -f "$out" ]] && [[ $(stat -c %s "$out") == "$size" ]]; then echo "have $file"; continue; fi
  curl -sSfL -A "curl" -o "$out.part" "https://huggingface.co/$repo/resolve/$rev/$file" || { echo "FAIL $file"; continue; }
  got=$(sha256sum "$out.part" | cut -d' ' -f1)
  if [[ "$got" == "$sha" ]]; then mv "$out.part" "$out"; echo "ok $file"; else echo "SHA MISMATCH $file"; fi
done < "$(dirname "$0")/models.txt"
