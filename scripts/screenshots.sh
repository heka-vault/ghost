#!/usr/bin/env bash
# The README's screenshots, taken from the desktop UI's mock daemon (`?mock=<scenario>`), so
# no daemon, no data of anyone's and no network are needed. Needs Chrome or Chromium and
# ImageMagick (for WebP), and the UI served: `make desktop-web` in another terminal, or
# GHOST_SHOTS_URL pointing at another port.
#
#   scripts/screenshots.sh            every shot below into docs/screenshots/ (make screenshots)
#   scripts/screenshots.sh home apps  only those
#
# Each is 1440x900 in the dark theme, as WebP at quality 80 (about 100 KB each).
#
# Then the floating room's (scripts/floating-shots.mjs, Node 22 or later): the real floating
# page with `?mock=floating-tour` laid over a made-up desktop (apps/desktop/shots/desktop.html),
# four stills at 2x and floating-loop.webp, one whole turn as an animated WebP (about 2 MB; the
# loop takes a minute to encode). `scripts/screenshots.sh floating-hero` takes only that one.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
URL="${GHOST_SHOTS_URL:-http://localhost:1420}"
OUT="$ROOT/docs/screenshots"
CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser || true)}"
[[ -n $CHROME ]] || { echo "screenshots: no Chrome or Chromium (set CHROME=/path/to/it)" >&2; exit 1; }
command -v magick >/dev/null || command -v convert >/dev/null || { echo "screenshots: ImageMagick is needed for WebP" >&2; exit 1; }
curl -fsS -o /dev/null "$URL/" || { echo "screenshots: nothing answers at $URL; run make desktop-web first" >&2; exit 1; }

# name | scenario | route | how long the story runs before the shot (ms of virtual time)
SHOTS=(
  "home|passive|/|4000"
  "session|thread|/session|9000"
  "session-pending|pending|/session|4000"
  "agents|passive|/settings/agents|4000"
  "apps|busy|/apps|4000"
  "watching|passive|/watching|4000"
  "model|passive|/settings/model|4000"
  "welcome|firstrun|/welcome|4000"
)

FLOATING=(floating-hero floating-speaking floating-yes floating-question floating-loop)

want=("$@")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for row in "${SHOTS[@]}"; do
  IFS='|' read -r name scenario route budget <<<"$row"
  if ((${#want[@]})) && [[ " ${want[*]} " != *" $name "* ]]; then continue; fi
  "$CHROME" --headless=new --disable-gpu --hide-scrollbars --force-dark-mode --no-first-run \
    --user-data-dir="$tmp/profile" --window-size=1440,900 --virtual-time-budget="$budget" \
    --screenshot="$tmp/$name.png" "$URL/?mock=$scenario#$route" >/dev/null 2>&1
  if command -v magick >/dev/null; then magick "$tmp/$name.png" -quality 80 "$OUT/$name.webp"
  else convert "$tmp/$name.png" -quality 80 "$OUT/$name.webp"; fi
  echo "screenshots: $OUT/$name.webp ($(du -h "$OUT/$name.webp" | cut -f1))"
done

floating=()
for name in "${FLOATING[@]}"; do
  if ((${#want[@]})) && [[ " ${want[*]} " != *" $name "* ]]; then continue; fi
  floating+=("$name")
done
if ((${#floating[@]})); then
  CHROME="$CHROME" node "$ROOT/scripts/floating-shots.mjs" "$URL" "$OUT" "$tmp" "${floating[@]}"
fi
