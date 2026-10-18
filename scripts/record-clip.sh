#!/usr/bin/env bash
# Record a short test clip from the echo-cancelled mic and the raw mic at the same time.
#   scripts/record-clip.sh <name> [seconds]      -> .scratch/clips/<name>-aec.wav, <name>-raw.wav
# Say the phrase once the countdown ends. Clips are git-ignored; evaluate with `make wake-eval`.
set -euo pipefail
NAME=${1:?usage: record-clip.sh <name> [seconds]}
SECS=${2:-4}
DIR=$(cd "$(dirname "$0")/.." && pwd)/.scratch/clips
mkdir -p "$DIR"
AEC=${GHOST_INPUT_DEVICE:-ghost.mic}
RAW=${GHOST_RAW_MIC:-$(pactl get-default-source)}
for i in 3 2 1; do printf '\r%s… ' "$i"; sleep 1; done; printf '\rspeak now (%ss)\n' "$SECS"
timeout "$SECS" parecord --device="$AEC" --rate=16000 --channels=1 --format=s16le --file-format=wav "$DIR/$NAME-aec.wav" &
timeout "$SECS" parecord --device="$RAW" --rate=16000 --channels=1 --format=s16le --file-format=wav "$DIR/$NAME-raw.wav" &
wait || true
echo "saved $DIR/$NAME-{aec,raw}.wav"
