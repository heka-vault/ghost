#!/usr/bin/env bash
# The model files the daemon loads outside the bundle, each fetched once into Ghost's cache
# and pinned by SHA-256.
#
# The four small files the Python wheels used to ship (heka-vault/ghost#39, W10): Silero VAD
# and Smart Turn v3.2 (pipecat-ai 1.12.0), and openWakeWord's melspectrogram and embedding
# graphs, the wake front end (livekit-wakeword 0.3.0). Each comes from its upstream release;
# the pins are the hashes of the files the Python daemon ran (the wheels' copies), and on
# 2026-10-29 every upstream file matched its wheel's byte for byte, so the parity fixtures
# stand as recorded. Each file's licence comes with it: MIT (Silero), BSD-2-Clause (Smart
# Turn), Apache-2.0 (openWakeWord). `ghost_infer::models` finds them for `cargo nextest`,
# `make dev` and `ghostd` outside the bundle; the bundle carries its own copies.
#
# And Parakeet TDT 0.6B v3 at full precision (ADR 0043): the five files of the
# istupakov/parakeet-tdt-0.6b-v3-onnx repository at the revision the oracle's onnx-asr
# snapshot carries, 2.5 GB, into models/parakeet-tdt-0.6b-v3/ as plain files, since ONNX
# Runtime refuses the Hugging Face cache's symlinked external data. A file the oracle's
# snapshot already holds is copied from there when it matches its pin. Never staged: the
# bundle doesn't carry it (docs/releasing.md). CC-BY-4.0, as NVIDIA's model card says.
#
#   scripts/models.sh          ensure the four small files; print their folder on stdout
#   scripts/models.sh parakeet ensure Parakeet's files; print their folder on stdout
#                              (scripts/dev.sh and make models run both)
#   scripts/models.sh kokoro   ensure Kokoro v1.0's two files (Ghost's voice, 340 MB, Apache-2.0;
#                              thewh1teagle/kokoro-onnx's model-files-v1.0 release) in
#                              ~/.cache/ghost/kokoro/; print their folder on stdout (make models)
#   scripts/models.sh kokoro-windowed
#                              Kokoro's two halves for the windowed voice (ADR 0053,
#                              GHOST_TTS_STREAM=windowed): made from Kokoro v1.0 in
#                              ~/.cache/ghost/kokoro/ by tools/voice-trial's split_kokoro.py
#                              (uv), each half pinned by SHA-256, into kokoro/windowed/;
#                              print the folder on stdout. Never staged: the bundle does
#                              not carry Kokoro either
#   scripts/models.sh stage    the four, also copied (with the licences) under apps/desktop/
#                              src-tauri/resources/models/ for the bundle (scripts/sidecars.sh
#                              runs this; the folder is not committed)
#   scripts/models.sh --help   this
#
# Cache: ${GHOST_CACHE_DIR:-$HOME/.cache/ghost}/models/{silero_vad.onnx, smart-turn-v3.2-cpu.onnx,
# melspectrogram.onnx, embedding_model.onnx, LICENSE.silero-vad, LICENSE.smart-turn,
# LICENSE.openwakeword, parakeet-tdt-0.6b-v3/}. Nothing is fetched for a file that is there.
# A file that does not match its pin is refused, both hashes printed, and nothing of it is
# kept. `GHOST_MODELS_DIR` and `GHOST_STT_DIR` still win at run time (crates/ghost-infer).
# The operator's trained wake classifiers are not here (docs/releasing.md); without them the
# wake word is read from Parakeet's transcript (GHOST_WAKE_MODE=transcript, the default).
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

# Kokoro v1.0, the voice (ADR 0004): the model-files-v1.0 release of thewh1teagle/kokoro-onnx,
# the ONNX of hexgrad/Kokoro-82M (Apache-2.0). The model's pin is the one the windowed halves
# are cut from (below).
KOKORO_URL=https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0
KOKORO_FILES=(
  "kokoro-v1.0.onnx|$KOKORO_URL/kokoro-v1.0.onnx|7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5|325532387|644"
  "voices-v1.0.bin|$KOKORO_URL/voices-v1.0.bin|bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d|28214398|644"
)

# Parakeet: the revision is the snapshot onnx-asr 0.12 fetched for the oracle on 2026-10-30,
# the pins the hashes of its files. The encoder's weights are the one large file.
PARAKEET_REV=8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce
PARAKEET_FOLDER=parakeet-tdt-0.6b-v3
PARAKEET_URL="https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/$PARAKEET_REV"
PARAKEET_SNAPSHOT="${HF_HOME:-$HOME/.cache/huggingface}/hub/models--istupakov--parakeet-tdt-0.6b-v3-onnx/snapshots/$PARAKEET_REV"
PARAKEET_FILES=(
  "encoder-model.onnx|$PARAKEET_URL/encoder-model.onnx|98a74b21b4cc0017c1e7030319a4a96f4a9506e50f0708f3a516d02a77c96bb1|41770866|644"
  "encoder-model.onnx.data|$PARAKEET_URL/encoder-model.onnx.data|9a22d372c51455c34f13405da2520baefb7125bd16981397561423ed32d24f36|2435420160|644"
  "decoder_joint-model.onnx|$PARAKEET_URL/decoder_joint-model.onnx|e978ddf6688527182c10fde2eb4b83068421648985ef23f7a86be732be8706c1|72520893|644"
  "vocab.txt|$PARAKEET_URL/vocab.txt|d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d|93939|644"
  "config.json|$PARAKEET_URL/config.json|666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466|97|644"
)

usage() {
  sed -n '/^# The model files/,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

mode="${1:-}"
case "$mode" in
  "" | stage | parakeet | kokoro | kokoro-windowed) ;;
  -h | --help | help) usage; exit 0 ;;
  *) echo "usage: $0 [stage|parakeet|kokoro|kokoro-windowed|--help]" >&2; exit 2 ;;
esac

# The temp folder is inside the cache, so a finished file moves into place by one rename
# (the encoder's weights are 2.4 GB; /tmp may be memory).
mkdir -p "$CACHE"
TMP=$(mktemp -d "$CACHE/.fetch.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

matches() { [ "$(sha256sum "$1" | cut -d' ' -f1)" = "$2" ]; }

# One file into $dir: copied from $snapshot when its copy matches the pin, else fetched to
# the temp folder, hashed, refused on a mismatch, and moved into place whole (nothing half
# written is ever read as a model).
place() {
  local name=$1 url=$2 sha256=$3 size=$4 mode=$5 dir=$6 snapshot=${7:-}
  if [ -n "$snapshot" ] && [ -e "$snapshot/$name" ] && matches "$snapshot/$name" "$sha256"; then
    echo "models: copying $snapshot/$name" >&2
    cp -L "$snapshot/$name" "$TMP/$name"
  else
    echo "models: fetching $url" >&2
    curl -fsSL --retry 3 -o "$TMP/$name" "$url"
  fi
  if ! matches "$TMP/$name" "$sha256"; then
    echo "models: $name does not match the pinned SHA-256" >&2
    echo "  expected $sha256" >&2
    echo "  got      $(sha256sum "$TMP/$name" | cut -d' ' -f1) ($(stat -c %s "$TMP/$name") bytes; $size expected)" >&2
    echo "  the file is not used; check the release and the pin in $0" >&2
    exit 1
  fi
  mkdir -p "$dir"
  chmod "$mode" "$TMP/$name"
  mv "$TMP/$name" "$dir/$name"
  echo "models: $dir/$name ($(du -h "$dir/$name" | cut -f1))" >&2
}

if [ "$mode" = kokoro-windowed ]; then
  # Kokoro v1.0 (Apache-2.0, hexgrad/Kokoro-82M; the ONNX of thewh1teagle/kokoro-onnx's
  # model-files-v1.0 release) cut in two, nothing inside either half changed but one added
  # input. The split is deterministic (onnx 1.19.1, pinned in tools/voice-trial/uv.lock),
  # so the halves are pinned like any download.
  KOKORO_DIR="${GHOST_CACHE_DIR:-$HOME/.cache/ghost}/kokoro"
  KOKORO_SHA=7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5
  ENCODER_SHA=2f8a0b8abbaca5c66317ee5672bf3e68f2dfc7b1ade23a9500ae77407c3d6ec7
  DECODER_SHA=88b5172fd6a0061fcb87102b553b6f2966dd9b1fa52ce45435730eceb0e66231
  dir="$KOKORO_DIR/windowed"
  if [ -f "$dir/kokoro-encoder.onnx" ] && [ -f "$dir/kokoro-decoder.onnx" ]; then
    echo "$dir"
    exit 0
  fi
  if ! [ -f "$KOKORO_DIR/kokoro-v1.0.onnx" ] || ! matches "$KOKORO_DIR/kokoro-v1.0.onnx" "$KOKORO_SHA"; then
    echo "models: $KOKORO_DIR/kokoro-v1.0.onnx is missing or not Kokoro v1.0 as pinned;" >&2
    echo "  scripts/models.sh kokoro fetches it" >&2
    exit 1
  fi
  echo "models: cutting Kokoro in two (tools/voice-trial, about a minute)" >&2
  uv run --quiet --project "$ROOT/tools/voice-trial" python -I \
    "$ROOT/tools/voice-trial/voice_trial/split_kokoro.py" \
    "$KOKORO_DIR/kokoro-v1.0.onnx" "$TMP/windowed" --phase >&2
  for pair in "kokoro-encoder.onnx|$ENCODER_SHA" "kokoro-decoder.onnx|$DECODER_SHA"; do
    IFS='|' read -r name sha256 <<<"$pair"
    if ! matches "$TMP/windowed/$name" "$sha256"; then
      echo "models: $name does not match the pinned SHA-256" >&2
      echo "  expected $sha256" >&2
      echo "  got      $(sha256sum "$TMP/windowed/$name" | cut -d' ' -f1)" >&2
      exit 1
    fi
  done
  mkdir -p "$dir"
  chmod 644 "$TMP/windowed/"*.onnx
  mv "$TMP/windowed/kokoro-encoder.onnx" "$TMP/windowed/kokoro-decoder.onnx" "$dir/"
  echo "models: $dir ($(du -sh "$dir" | cut -f1))" >&2
  echo "$dir"
  exit 0
fi

if [ "$mode" = kokoro ]; then
  dir="${GHOST_CACHE_DIR:-$HOME/.cache/ghost}/kokoro"
  for entry in "${KOKORO_FILES[@]}"; do
    IFS='|' read -r name url sha256 size perm <<<"$entry"
    if [ ! -f "$dir/$name" ]; then
      place "$name" "$url" "$sha256" "$size" "$perm" "$dir"
    fi
  done
  echo "$dir"
  exit 0
fi

if [ "$mode" = parakeet ]; then
  dir="$CACHE/$PARAKEET_FOLDER"
  for entry in "${PARAKEET_FILES[@]}"; do
    IFS='|' read -r name url sha256 size perm <<<"$entry"
    if [ ! -f "$dir/$name" ]; then
      place "$name" "$url" "$sha256" "$size" "$perm" "$dir" "$PARAKEET_SNAPSHOT"
    fi
  done
  echo "$dir"
  exit 0
fi

for entry in "${FILES[@]}"; do
  IFS='|' read -r name url sha256 size perm <<<"$entry"
  if [ ! -f "$CACHE/$name" ]; then
    place "$name" "$url" "$sha256" "$size" "$perm" "$CACHE"
  fi
done

if [ "$mode" = stage ]; then
  mkdir -p "$STAGE"
  for entry in "${FILES[@]}"; do
    IFS='|' read -r name _ _ _ perm <<<"$entry"
    install -m "$perm" "$CACHE/$name" "$STAGE/$name"
  done
  echo "models: staged ${#FILES[@]} files under $STAGE for the bundle" >&2
fi

echo "$CACHE"
