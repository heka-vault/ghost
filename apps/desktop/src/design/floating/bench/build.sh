#!/usr/bin/env bash
# SPIKE: build the motion-stack bench outside the repo and serve it on :1432.
#   bench/build.sh            # install the candidates into /tmp/flt/bench, bundle, serve
# Then, from the repo root, one candidate at a time in the floating window:
#   FLOATING_URL=http://127.0.0.1:1432/three.html?bloom=1 target/debug/examples/floating
# and measure it with ../measure.sh. Nothing is added to the repo's lockfile.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out=/tmp/flt/bench
mkdir -p "$out"
cd "$out"
[[ -f package.json ]] || npm init -y >/dev/null
npm i --silent --ignore-scripts three@0.187.0 ogl@1.0.11 pixi.js@8.23.0 @rive-app/canvas@2.50.0 @rive-app/webgl2@2.50.0 lottie-web@5.13.0 motion@14.13.0 gsap@3.15.1 esbuild
cp "$here"/*.html "$out"/
for n in raw ogl three pixi; do
  NODE_PATH="$out/node_modules" "$out/node_modules/.bin/esbuild" "$here/$n.js" --bundle --minify --format=esm --outfile="$out/$n.dist.js" --log-level=error
done
node "$here/sizes.mjs" "$out/node_modules"
exec python3 -I -m http.server 1432 --bind 127.0.0.1 --directory "$out"
