#!/usr/bin/env bash
# The AppImage on another family of Linux (ADR 0075): run inside a clean container of that
# distribution, with the AppImage extracted at /app (read-only) and, optionally, Kokoro's two
# files at /kokoro. The release workflow runs it in archlinux and fedora; on a workstation:
#
#   docker run --rm -v "$PWD/squashfs-root":/app:ro -v ~/.cache/ghost/kokoro:/kokoro:ro \
#     -v "$PWD/scripts/distro-check.sh":/check.sh:ro archlinux:latest bash /check.sh [commit]
#
# It installs a minimal desktop stack and nothing of espeak-ng, then proves: every binary the
# bundle runs finds its libraries; the daemon says its build; `ghostd check` finds the bundle's
# runtime, model files and espeak-ng data on its own; Kokoro renders a sentence into memory
# (when /kokoro has the files); and a voice that cannot start (the bundle's espeak data taken
# away, as in #168) is said on the waiting list and reported with its part and kind, to a
# stub on 127.0.0.1 (GHOST_REPORTS_URL), never to the real service.
set -euo pipefail
COMMIT=${1:-}

. /etc/os-release
echo "== $PRETTY_NAME"
case "$ID" in
  arch)
    pacman -Syu --noconfirm --needed pipewire alsa-lib libx11 libxcb mesa libglvnd fontconfig \
      freetype2 harfbuzz fribidi wayland python >/dev/null
    ;;
  fedora)
    dnf install -y -q pipewire-libs alsa-lib libX11 libX11-xcb libxcb mesa-libEGL mesa-libGL \
      mesa-libgbm libdrm fontconfig freetype harfbuzz fribidi libwayland-client python3 \
      findutils >/dev/null
    ;;
  debian | ubuntu)
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      libpipewire-0.3-0t64 libasound2t64 libx11-6 libx11-xcb1 libxcb1 libegl1 libgl1 libgbm1 \
      libdrm2 fontconfig libfreetype6 libharfbuzz0b libfribidi0 libwayland-client0 python3 \
      >/dev/null
    ;;
  *) echo "no package list for $ID" >&2; exit 2 ;;
esac
if command -v espeak-ng >/dev/null || ls /usr/share/espeak-ng-data /usr/lib/*/espeak-ng-data >/dev/null 2>&1; then
  echo "this container has espeak-ng's data of its own; the check needs one without" >&2
  exit 1
fi

APP=/app
B=$APP/usr/bin
export HOME=/tmp/home XDG_RUNTIME_DIR=/tmp/run
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"

echo "== libraries"
missing=$(for f in "$B"/ghostd "$B"/heka-ghost-desktop "$B"/ghost-mock-github "$B"/ghost-scripts \
  "$APP"/usr/lib/webkit2gtk-4.1/*Process; do ldd "$f"; done | grep "not found" | sort -u || true)
if [ -n "$missing" ]; then echo "missing libraries:"; echo "$missing"; exit 1; fi
echo "every binary finds its libraries"

echo "== version"
"$B"/ghostd version | tee /tmp/version
[ -z "$COMMIT" ] || grep -qF "release ${COMMIT:0:7}" /tmp/version

echo "== ghostd check"
out=$(ANTHROPIC_BASE_URL=http://x "$B"/ghostd check 2>&1) || true
printf '%s\n' "$out"
grep -q "^onnxruntime: $APP/usr/lib/heka-ghost-desktop/onnxruntime/libonnxruntime.so.1.24.4$" <<<"$out"
grep -q "^models: $APP/usr/lib/heka-ghost-desktop/models$" <<<"$out"
grep -q "^espeak-ng: [0-9.]*, data at $APP/usr/lib/heka-ghost-desktop/espeak-ng-data (the bundle's)$" <<<"$out"
! grep -q "Ghost can't speak" <<<"$out"

if [ -f /kokoro/kokoro-v1.0.onnx ]; then
  echo "== Kokoro, one sentence into memory"
  GHOST_TTS_MODEL_PATH=/kokoro/kokoro-v1.0.onnx GHOST_TTS_VOICES_PATH=/kokoro/voices-v1.0.bin \
    "$B"/ghostd models voice-test 2>/dev/null | tee /tmp/voice
  grep -q "^voice: rendered" /tmp/voice
fi

echo "== a voice that cannot start, reported (#168)"
# The tester's AppImage: the library with no data of its own here. A copy of the bundle
# without its espeak-ng data, the daemon's first run (no speech models), reports to a stub.
cp -a "$APP" /tmp/app-no-data
rm -rf /tmp/app-no-data/usr/lib/heka-ghost-desktop/espeak-ng-data
cat > /tmp/stub.py <<'PY'
import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        open("/tmp/reports.jsonl", "ab").write(body + b"\n")
        self.send_response(200); self.send_header("content-length", "2"); self.end_headers()
        self.wfile.write(b"{}")
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
python3 -I /tmp/stub.py 18765 &
stub=$!
: > /tmp/reports.jsonl
APPIMAGE=/tmp/Ghost.AppImage GHOST_REPORTS_URL=http://127.0.0.1:18765 GHOST_POSTHOG_KEY=${GHOST_POSTHOG_KEY:-phc_container_check} \
  ANTHROPIC_BASE_URL=http://127.0.0.1:9 /tmp/app-no-data/usr/bin/ghostd run >/tmp/run.log 2>&1 &
daemon=$!
for _ in $(seq 1 60); do
  grep -q speech_models_load /tmp/reports.jsonl 2>/dev/null && break
  kill -0 "$daemon" 2>/dev/null || break
  sleep 1
done
alive=yes; kill -0 "$daemon" 2>/dev/null || alive=no
kill "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true
kill "$stub" 2>/dev/null || true
grep -E "tts: |espeak" /tmp/run.log | tail -3 || true
[ "$alive" = yes ] || { echo "the daemon exited (espeak's exit(1)?)"; tail -5 /tmp/run.log; exit 1; }
python3 -I - <<'PY'
import json
reports = [json.loads(l) for l in open("/tmp/reports.jsonl") if l.strip()]
load = [r for r in reports if r["properties"]["$exception_list"][0]["type"] == "speech_models_load"]
assert load, f"no speech_models_load report among {len(reports)}"
p = load[0]["properties"]
facts = p["ghost_facts"]
print("report:", json.dumps({k: p[k] for k in ("installed_as", "os_name", "ghost_component")}), json.dumps(facts))
assert facts["part"] == "espeak" and facts["espeak"] == "data_missing", facts
assert facts["error"] == "EspeakData", facts
assert p["installed_as"] == "appimage", p["installed_as"]
text = json.dumps(load[0])
assert "/tmp/" not in text and "/app" not in text, "a path rode in the report"
PY
echo "ok: $PRETTY_NAME"
