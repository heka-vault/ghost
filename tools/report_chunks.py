#!/usr/bin/env python3
"""The page scripts' chunk ids for error reports (ADR 0067, docs/releasing.md).

  tools/report_chunks.py inject <dist> <maps out>

runs in the release build only (`tauri.release.conf.json`'s beforeBuildCommand, after `vite
build` wrote hidden source maps), before the shell embeds <dist>. For every script under
<dist>/assets with its `.js.map` beside it, it writes the two marks `posthog-cli sourcemap
inject` writes in its chunk-id-only form (posthog-cli 0.20.0, `sourcemaps/content.rs`):

- `\\n//# chunkId=<id>` appended to the script, so nothing before it moves and the map
  stays exact;
- `"chunk_id": "<id>"` at the top of its map;

with the id content-addressed as the CLI derives it (UUIDv5 of the script in the CLI's
namespace, `stable_chunk_id`). It leaves out the snippet the CLI prepends for posthog-js
(`window._posthogChunkIds`): Ghost's pages carry no PostHog SDK, and the shell learns the
ids from the manifest this writes instead, `<dist>/report-chunks.json`, `assets/<name>.js` to
its id, which ships in the bundle and is all the app knows of the maps.

Then it moves every map, with a copy of its script as injected, to <maps out> (for
`posthog-cli sourcemap upload`, which pairs `x.js` with `x.js.map`), and refuses to finish
while any `.map` is left in <dist>: the maps never ship. No network, no key. Standard
library only.
"""

from __future__ import annotations

import json
import sys
import uuid
from pathlib import Path

# posthog-cli's CHUNK_ID_NAMESPACE (cli/src/sourcemaps/constant.rs).
NAMESPACE = uuid.UUID(int=0x0E9B3C7A5D1F42A8B6C4E2D0F8A17593)
COMMENT = "\n//# chunkId="
MANIFEST = "report-chunks.json"
MAX_SCRIPTS = 256  # crates/ghost-reports chunks::MAX_ENTRIES


class InjectError(Exception):
    pass


def chunk_id(source: str) -> str:
    """The CLI's `stable_chunk_id`: UUIDv5 of the script's text."""
    return str(uuid.uuid5(NAMESPACE, source))


def inject(dist: Path, maps_out: Path) -> dict[str, str]:
    assets = dist / "assets"
    scripts = sorted(p for p in assets.glob("*.js") if p.is_file())
    pairs = [(s, s.with_name(s.name + ".map")) for s in scripts]
    pairs = [(s, m) for s, m in pairs if m.is_file()]
    if not pairs:
        raise InjectError(
            f"no script with a source map under {assets}: was vite told to write them?"
        )
    if len(pairs) > MAX_SCRIPTS:
        raise InjectError(f"{len(pairs)} scripts; the shell reads at most {MAX_SCRIPTS}")
    out_assets = maps_out / "assets"
    out_assets.mkdir(parents=True, exist_ok=True)
    manifest: dict[str, str] = {}
    for script, smap in pairs:
        source = script.read_bytes().decode("utf-8")
        if COMMENT.strip() in source:
            raise InjectError(f"{script.name} already carries a chunk id: build again from clean")
        cid = chunk_id(source)
        injected = source + COMMENT + cid
        script.write_bytes(injected.encode("utf-8"))
        m = json.loads(smap.read_text(encoding="utf-8"))
        if not isinstance(m, dict) or "mappings" not in m:
            raise InjectError(f"{smap.name} is not a source map")
        # What a resolved frame names is the map's `sources`: the repository's relative paths,
        # never the build machine's (a home folder, an absolute path).
        for src in m.get("sources") or []:
            if not isinstance(src, str) or src.startswith("/") or "://" in src or "/home/" in src:
                raise InjectError(f"{smap.name} names a source by an absolute path: {src!r}")
        m = {"chunk_id": cid, **{k: v for k, v in m.items() if k != "chunk_id"}}
        (out_assets / smap.name).write_text(json.dumps(m), encoding="utf-8")
        (out_assets / script.name).write_bytes(injected.encode("utf-8"))
        smap.unlink()
        manifest[f"assets/{script.name}"] = cid
    left = sorted(str(p.relative_to(dist)) for p in dist.rglob("*.map"))
    if left:
        raise InjectError(f"source maps left in {dist}, which would ship: {', '.join(left)}")
    (dist / MANIFEST).write_text(json.dumps(manifest, sort_keys=True), encoding="utf-8")
    return manifest


def main(argv: list[str]) -> int:
    if len(argv) != 3 or argv[0] != "inject":
        print("usage: tools/report_chunks.py inject <dist> <maps out>", file=sys.stderr)
        return 2
    dist, maps_out = Path(argv[1]), Path(argv[2])
    try:
        manifest = inject(dist, maps_out)
    except InjectError as e:
        print(f"report_chunks: {e}", file=sys.stderr)
        return 1
    print(
        f"report_chunks: {len(manifest)} script(s) given chunk ids; maps in {maps_out}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
