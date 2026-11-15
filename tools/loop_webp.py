"""The README's floating loop, encoded: PNG frames in, one animated WebP out.

Run by `scripts/floating-shots.mjs` (make screenshots):

    uv run --project tools --group shots python tools/loop_webp.py <frames> <out.webp> [--fps 30]

Why not ImageMagick, which made it before 2026-11-14: libwebp's animation encoder, lossy, keeps
a pixel from the previous frame whenever the new one is within a tolerance of it (about 8
levels at quality 60). The loop's glow fades a level or two a frame, so the old glow stayed
on screen while the patch around Wisp, which did change, was redrawn dark: a dark square
behind Wisp as it left. A keyframe every two to four frames redraws the whole frame often
enough that nothing stale builds up; ImageMagick, ffmpeg and sharp don't expose that, and
Pillow's binding of the same libwebp does (`kmin`, `kmax`). At quality 60 the file is about
the size it was (4.5 MB on 2026-11-14 against 3.8 MB, for a capture with more real frames).
The frame times are kept whole: 30 a second is 33 or 34 ms each, not ImageMagick's 3
centiseconds, which ran the loop 10% fast.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image

QUALITY = 60
KMIN = 2
KMAX = 4


def durations(n: int, fps: float) -> list[int]:
    """Whole milliseconds per frame that add up to n frames at fps, without drift."""
    out: list[int] = []
    for i in range(n):
        out.append(round((i + 1) * 1000 / fps) - round(i * 1000 / fps))
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("frames", type=Path, help="a folder of numbered PNG frames")
    ap.add_argument("out", type=Path, help="the animated WebP to write")
    ap.add_argument("--fps", type=float, default=30.0)
    args = ap.parse_args()
    files = sorted(args.frames.glob("*.png"))
    if not files:
        raise SystemExit(f"loop_webp: no PNG frames in {args.frames}")
    frames = [Image.open(f).convert("RGB") for f in files]
    frames[0].save(
        args.out,
        save_all=True,
        append_images=frames[1:],
        duration=durations(len(frames), args.fps),
        loop=0,
        quality=QUALITY,
        method=6,
        kmin=KMIN,
        kmax=KMAX,
        allow_mixed=False,
    )
    print(f"loop_webp: {args.out} ({len(frames)} frames, {args.out.stat().st_size / 1e6:.1f} MB)")


if __name__ == "__main__":
    main()
