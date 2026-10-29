#!/usr/bin/env bash
# The four small model files the Python wheels used to ship (heka-vault/ghost#39, W10):
# Silero VAD and Smart Turn v3.2 (pipecat-ai 1.12.0), and openWakeWord's melspectrogram and
# embedding graphs, the wake front end (livekit-wakeword 0.3.0). Each is fetched once from
# its upstream release, pinned by SHA-256, into Ghost's cache, where `ghost_infer::models`
# finds it for `cargo nextest`, `make dev` and `ghostd` outside the bundle. The pins are the
# hashes of the files the Python daemon ran (the wheels' copies); on 2026-10-29 every
# upstream file matched its wheel's byte for byte, so the parity fixtures stand as recorded.
# Each file's licence comes with it: MIT (Silero), BSD-2-Clause (Smart Turn), Apache-2.0
# (openWakeWord).
#
#   scripts/models.sh          ensure the files; print the folder's absolute path on stdout
#   scripts/models.sh stage    also copy them (and the licences) under apps/desktop/src-tauri/
#                              resources/models/ for the bundle (scripts/sidecars.sh runs
#                              this; the folder is not committed)
#   scripts/models.sh --help   this
#
# Cache: ${GHOST_CACHE_DIR:-$HOME/.cache/ghost}/models/{silero_vad.onnx, smart-turn-v3.2-cpu.onnx,
# melspectrogram.onnx, embedding_model.onnx, LICENSE.silero-vad, LICENSE.smart-turn,
# LICENSE.openwakeword}. Nothing is fetched for a file that is there. A file that does not
# match its pin is refused, both hashes printed, and nothing of it is kept.
# `GHOST_MODELS_DIR` still wins at run time (crates/ghost-infer/src/models.rs). Parakeet,
# Kokoro and the operator's trained wake classifiers are not here (docs/releasing.md).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CACHE="${GHOST_CACHE_DIR:-$HOME/.cache/ghost}/models"
STAGE="$ROOT/apps/desktop/src-tauri/resources/models"

# Upstream, pinned. Silero VAD: the snakers4/silero-vad repository at v6.0 (v6.1 carries
# the same file; v6.2 is another model). Smart Turn: the pipecat-ai/smart-turn-v3 Hugging
# Face repository at the revision that holds v3.2; its licence from the pipecat-ai/smart-turn
# repository, since the model repository names BSD-2-Clause in its card and ships no file.
# openWakeWord: the v0.5.1 release assets of dscripka/openWakeWord, the graphs
# livekit-wakeword copies.
SILERO_TAG=v6.0
SMART_TURN_REV=f766f81d3cfdf7737ac64aad813d91bbfd56bf93
SMART_TURN_LICENSE_REV=24c720337e17befe0413bbc93b3504036c3a3bdc
OPENWAKEWORD_TAG=v0.5.1

# name|url|sha256|size|mode
FILES=(
  "silero_vad.onnx|https://raw.githubusercontent.com/snakers4/silero-vad/$SILERO_TAG/src/silero_vad/data/silero_vad.onnx|597d30b3ec076608d059477bb14cfeffdf951bf5cae370d38f65d33bbfe82004|2327524|644"
  "LICENSE.silero-vad|https://raw.githubusercontent.com/snakers4/silero-vad/$SILERO_TAG/LICENSE|2e63e9a38b6e8fc0c7bc37ce174caca1862870856c6daf5697cfb785e925520b|1075|644"
  "smart-turn-v3.2-cpu.onnx|https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/$SMART_TURN_REV/smart-turn-v3.2-cpu.onnx|2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f|8679182|644"
  "LICENSE.smart-turn|https://raw.githubusercontent.com/pipecat-ai/smart-turn/$SMART_TURN_LICENSE_REV/LICENSE|0d66364067f678c08586ebb60a16a2aed4fa081ec11057df35585759ce0e774f|1301|644"
  "melspectrogram.onnx|https://github.com/dscripka/openWakeWord/releases/download/$OPENWAKEWORD_TAG/melspectrogram.onnx|ba2b0e0f8b7b875369a2c89cb13360ff53bac436f2895cced9f479fa65eb176f|1087958|644"
  "embedding_model.onnx|https://github.com/dscripka/openWakeWord/releases/download/$OPENWAKEWORD_TAG/embedding_model.onnx|70d164290c1d095d1d4ee149bc5e00543250a7316b59f31d056cff7bd3075c1f|1326578|644"
  "LICENSE.openwakeword|https://raw.githubusercontent.com/dscripka/openWakeWord/$OPENWAKEWORD_TAG/LICENSE|c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4|11357|644"
)

usage() {
  sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
}

case "${1:-}" in
  "" | stage) ;;
  -h | --help | help) usage; exit 0 ;;
  *) echo "usage: $0 [stage|--help]" >&2; exit 2 ;;
esac

TMP=$(mktemp -d "${TMPDIR:-/tmp}/ghost-models.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

# One file: fetched to a temp folder, hashed, refused on a mismatch, moved into the cache
# whole (a .part is never read as a model).
fetch() {
  local name=$1 url=$2 sha256=$3 size=$4 mode=$5
  echo "models: fetching $url" >&2
  curl -fsSL --retry 3 -o "$TMP/$name" "$url"
  local got
  got=$(sha256sum "$TMP/$name" | cut -d' ' -f1)
  if [ "$got" != "$sha256" ]; then
    echo "models: $name does not match the pinned SHA-256" >&2
    echo "  expected $sha256" >&2
    echo "  got      $got ($(stat -c %s "$TMP/$name") bytes; $size expected)" >&2
    echo "  the file is not used; check the release and the pin in $0" >&2
    exit 1
  fi
  mkdir -p "$CACHE"
  install -m "$mode" "$TMP/$name" "$CACHE/$name.part"
  mv "$CACHE/$name.part" "$CACHE/$name"
  echo "models: $CACHE/$name ($(du -h "$CACHE/$name" | cut -f1))" >&2
}

for entry in "${FILES[@]}"; do
  IFS='|' read -r name url sha256 size mode <<<"$entry"
  if [ ! -f "$CACHE/$name" ]; then
    fetch "$name" "$url" "$sha256" "$size" "$mode"
  fi
done

if [ "${1:-}" = stage ]; then
  mkdir -p "$STAGE"
  for entry in "${FILES[@]}"; do
    IFS='|' read -r name _ _ _ mode <<<"$entry"
    install -m "$mode" "$CACHE/$name" "$STAGE/$name"
  done
  echo "models: staged ${#FILES[@]} files under $STAGE for the bundle" >&2
fi

echo "$CACHE"
