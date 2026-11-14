"""The hero's sound (docs/site.md): one turn, rendered from site/audio/turn.json.

    pnpm --dir site site:audio            # render, encode, write the timeline
    pnpm --dir site site:audio --check    # and read every line back with Parakeet, measure

Runs in tools/oracle's environment (`uv run --project tools/oracle python -I ...`): Kokoro
through kokoro-onnx, the voice files `scripts/models.sh kokoro` pins in ~/.cache/ghost/kokoro,
and for `--check` Parakeet from `scripts/models.sh parakeet`'s folder. Nothing is fetched.

- Each line is Kokoro at the voice and speed turn.json gives its speaker: Ghost is the
  daemon's default (`GHOST_TTS_VOICE` af_heart, `GHOST_TTS_SPEED` 1.0), the person another
  voice. Leading and trailing silence is trimmed so the layout is the words'.
- Ghost's chimes and thinking loop are the daemon's own samples, written by
  `cargo run -p ghost-audio --example site-sounds`, mixed as the daemon mixes them: the
  rising chime when the wake fires, the thinking loop at 0.12 from half a second after the
  request until Ghost speaks, the falling chime when the session ends. They keep their level
  relative to Ghost's voice; the person's voice is matched to Ghost's loudness.
- The whole loop is then set to `target_lufs` and encoded twice, Opus in WebM (what Chrome,
  Firefox and current Safari play) and AAC in MP4 (the fallback the page picks only when the
  browser says it cannot play the first). The page loads one of them, and only on request.
- The timeline (each line's start and length, the wake, the session's end, the loop's length)
  and Ghost's voice level per 50 ms window, computed the way `ghost_voice::level` computes
  the level the daemon sends (dBFS from -50 to 0 as 0 to 1, from the voice as played), go to
  the mock's fixture, so the floating room's light moves with these words with sound off too.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import subprocess
import sys
import tempfile
import wave
from pathlib import Path

import numpy as np

RATE = 24_000  # Kokoro's rate and the daemon's playback rate (ghost_audio::CHIME_RATE)
WINDOW_MS = 50  # ghost_voice::level::WINDOW
FLOOR_DBFS = -50.0  # ghost_voice::level::FLOOR_DBFS

SITE = Path(__file__).resolve().parent.parent
ROOT = SITE.parent
SOURCE = SITE / "audio" / "turn.json"
OUT = SITE / "public" / "audio"
TIMELINE = (
    ROOT / "apps" / "desktop" / "src" / "design" / "mock" / "fixtures" / "floating-site-turn.json"
)
CACHE = Path.home() / ".cache" / "ghost"
KOKORO = CACHE / "kokoro"
PARAKEET = CACHE / "models" / "parakeet-tdt-0.6b-v3"


def read_wav(path: Path) -> np.ndarray:
    with wave.open(str(path)) as w:
        assert w.getframerate() == RATE and w.getnchannels() == 1 and w.getsampwidth() == 2, path
        return np.frombuffer(w.readframes(w.getnframes()), dtype="<i2").astype(np.float32) / 32768.0


def write_wav(path: Path, pcm: np.ndarray, rate: int = RATE) -> None:
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes((np.clip(pcm, -1, 1) * 32767).round().astype("<i2").tobytes())


def cues(tmp: Path) -> dict[str, np.ndarray]:
    """The daemon's own chimes and thinking loop, from ghost-audio itself."""
    subprocess.run(
        ["cargo", "run", "-q", "-p", "ghost-audio", "--example", "site-sounds", "--", str(tmp)],
        cwd=ROOT,
        check=True,
        stdout=subprocess.DEVNULL,
    )
    return {name: read_wav(tmp / f"{name}.wav") for name in ("chime-up", "chime-down", "thinking")}


def trim(pcm: np.ndarray, floor_db: float = -45.0, pad_ms: int = 30) -> np.ndarray:
    """Drop the silence Kokoro leaves at either end, keeping a short pad."""
    hop = RATE // 100
    frames = [pcm[i : i + hop] for i in range(0, len(pcm), hop)]
    loud = [
        i
        for i, f in enumerate(frames)
        if len(f) and 20 * math.log10(max(1e-9, float(np.sqrt(np.mean(f**2))))) > floor_db
    ]
    if not loud:
        return pcm
    pad = RATE * pad_ms // 1000
    return pcm[max(0, loud[0] * hop - pad) : min(len(pcm), (loud[-1] + 1) * hop + pad)]


def lufs(pcm: np.ndarray, tmp: Path) -> tuple[float, float]:
    """Integrated loudness and true peak (dBTP) by ffmpeg's EBU R128 meter."""
    path = tmp / "measure.wav"
    write_wav(path, pcm)
    out = subprocess.run(
        [
            "ffmpeg",
            "-hide_banner",
            "-nostats",
            "-i",
            str(path),
            "-af",
            "ebur128=peak=true",
            "-f",
            "null",
            "-",
        ],
        capture_output=True,
        text=True,
        check=True,
    ).stderr
    summary = out[out.rfind("Summary:") :]
    integrated = float(re.search(r"I:\s+(-?[\d.]+|-inf) LUFS", summary).group(1))
    peak = float(re.search(r"Peak:\s+(-?[\d.]+|-inf) dBFS", summary).group(1))
    return integrated, peak


def limit(pcm: np.ndarray, tmp: Path) -> np.ndarray:
    """ffmpeg's look-ahead limiter at -2.5 dBFS (5 ms attack, 60 ms release), in float."""
    raw = subprocess.run(
        [
            "ffmpeg",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "f32le",
            "-ar",
            str(RATE),
            "-ac",
            "1",
            "-i",
            "-",
            "-af",
            "alimiter=limit=0.75:attack=5:release=60:level=disabled",
            "-f",
            "f32le",
            "-",
        ],
        input=pcm.astype("<f4").tobytes(),
        capture_output=True,
        check=True,
    ).stdout
    return np.frombuffer(raw, dtype="<f4").copy()[: len(pcm)]


def level(window: np.ndarray) -> float:
    """ghost_voice::level::level_of over one window (full scale 1.0)."""
    power = float(np.mean(window.astype(np.float64) ** 2)) if len(window) else 0.0
    if power <= 0:
        return 0.0
    return min(1.0, max(0.0, (10 * math.log10(power) - FLOOR_DBFS) / -FLOOR_DBFS))


def render(spec: dict, tmp: Path) -> tuple[dict, np.ndarray, np.ndarray]:
    from kokoro_onnx import Kokoro

    kokoro = Kokoro(str(KOKORO / "kokoro-v1.0.onnx"), str(KOKORO / "voices-v1.0.bin"))
    sounds = cues(tmp)
    clips: list[np.ndarray] = []
    for line in spec["lines"]:
        v = spec["voices"][line["who"]]
        pcm, rate = kokoro.create(line["text"], voice=v["voice"], speed=v["speed"], lang="en-us")
        assert rate == RATE
        clips.append(trim(np.asarray(pcm, dtype=np.float32)))

    # The person's voice at Ghost's loudness; Ghost's voice and cues as the daemon plays them.
    def voice_of(who: str) -> np.ndarray:
        pairs = zip(clips, spec["lines"], strict=True)
        return np.concatenate([c for c, ln in pairs if ln["who"] == who])

    ghost_l, _ = lufs(voice_of("ghost"), tmp)
    you_l, _ = lufs(voice_of("you"), tmp)
    you_gain = 10 ** ((ghost_l - you_l) / 20)

    # The layout: each line after the one before, on the loop's clock (ms).
    lines, t = [], 0
    for line, clip in zip(spec["lines"], clips, strict=True):
        t = line["at_ms"] if "at_ms" in line else t + line["gap_ms"]
        ms = round(len(clip) * 1000 / RATE)
        lines.append(
            {"id": line["id"], "who": line["who"], "text": line["text"], "at": t, "ms": ms}
        )
        t += ms
    by = {line["id"]: line for line in lines}
    wake = by["wake"]["at"] + by["wake"]["ms"] + spec["wake_after_ms"]
    sleep = by["bye"]["at"] + by["bye"]["ms"]
    chime_down = sleep + spec["chime_down_after_ms"]
    think_from = by["request"]["at"] + by["request"]["ms"] + spec["think_delay_ms"]
    think_to = by["reply"]["at"]
    loop_ms = chime_down + round(len(sounds["chime-down"]) * 1000 / RATE) + spec["rest_ms"]

    n = loop_ms * RATE // 1000
    mix = np.zeros(n, dtype=np.float32)
    ghost_voice = np.zeros(n, dtype=np.float32)

    def put(track: np.ndarray, pcm: np.ndarray, at_ms: int) -> None:
        i = at_ms * RATE // 1000
        track[i : i + len(pcm)] += pcm[: max(0, n - i)]

    for line, clip in zip(lines, clips, strict=True):
        if line["who"] == "ghost":
            put(mix, clip, line["at"])
            put(ghost_voice, clip, line["at"])
        else:
            put(mix, clip * you_gain, line["at"])
    put(mix, sounds["chime-up"], wake)
    put(mix, sounds["chime-down"], chime_down)
    if think_to > think_from:
        loops = int(math.ceil((think_to - think_from) * RATE / 1000 / len(sounds["thinking"])))
        bed = np.tile(sounds["thinking"], loops)[: (think_to - think_from) * RATE // 1000]
        put(mix, bed * spec["think_volume"], think_from)

    # Ghost's level per window, from the voice as the daemon plays it (before the site's gain).
    hop = RATE * WINDOW_MS // 1000
    levels = [round(level(ghost_voice[i : i + hop]), 2) for i in range(0, n, hop)]

    # The loop at the target loudness: a gain, then a fast peak limiter (Kokoro's plosives
    # peak some 20 dB over its loudness): under -1 dBTP (--check measures it), nothing clipped.
    integrated, _ = lufs(mix, tmp)
    mix = limit(mix * 10 ** ((spec["target_lufs"] - integrated) / 20), tmp)

    timeline = {
        "about": (
            "Written by `pnpm --dir site site:audio` from site/audio/turn.json: the floating-site"
            " loop's clock (ms) and Ghost's voice level per 50 ms window, as the daemon computes"
            " it. Don't edit by hand."
        ),
        "loop_ms": loop_ms,
        "wake": wake,
        "think": [think_from, think_to],
        "sleep": sleep,
        "lines": lines,
        "voice": {"window_ms": WINDOW_MS, "levels": levels},
    }
    return timeline, mix, ghost_voice


def encode(mix: np.ndarray, tmp: Path) -> list[Path]:
    OUT.mkdir(parents=True, exist_ok=True)
    src = tmp / "mix.wav"
    write_wav(src, mix)
    webm, m4a = OUT / "hero-turn.webm", OUT / "hero-turn.m4a"
    common = [
        "ffmpeg",
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-i",
        str(src),
        "-ac",
        "1",
        "-map_metadata",
        "-1",
        "-fflags",
        "+bitexact",
        "-flags:a",
        "+bitexact",
    ]
    subprocess.run(
        [
            *common,
            "-c:a",
            "libopus",
            "-b:a",
            "32k",
            "-application",
            "voip",
            "-ar",
            "48000",
            str(webm),
        ],
        check=True,
    )
    subprocess.run(
        [
            *common,
            "-c:a",
            "aac",
            "-b:a",
            "48k",
            "-ar",
            "24000",
            "-movflags",
            "+faststart",
            str(m4a),
        ],
        check=True,
    )
    return [webm, m4a]


def decode(path: Path, rate: int) -> np.ndarray:
    raw = subprocess.run(
        [
            "ffmpeg",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            str(path),
            "-ac",
            "1",
            "-ar",
            str(rate),
            "-f",
            "f32le",
            "-",
        ],
        capture_output=True,
        check=True,
    ).stdout
    return np.frombuffer(raw, dtype="<f4")


def words(text: str) -> list[str]:
    return re.findall(r"[a-z0-9']+", text.lower())


def wer(want: list[str], got: list[str]) -> float:
    d = list(range(len(got) + 1))
    for i, w in enumerate(want, 1):
        prev, d[0] = d[0], i
        for j, g in enumerate(got, 1):
            prev, d[j] = d[j], min(d[j] + 1, d[j - 1] + 1, prev + (w != g))
    return d[-1] / max(1, len(want))


def check(timeline: dict, files: list[Path], tmp: Path) -> bool:
    """Read every line back from the encoded files with Parakeet, and measure the loudness."""
    import onnx_asr
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.intra_op_num_threads = 4
    so.inter_op_num_threads = 1
    asr = onnx_asr.load_model(
        "nemo-parakeet-tdt-0.6b-v3", str(PARAKEET), quantization=None, sess_options=so
    )
    ok = True
    for path in files:
        print(f"\n{path.relative_to(ROOT)}: {path.stat().st_size / 1024:.1f} KiB")
        pcm16 = decode(path, 16_000)
        for line in timeline["lines"]:
            a = max(0, (line["at"] - 150) * 16)
            b = (line["at"] + line["ms"] + 150) * 16
            got = (asr.recognize(pcm16[a:b], sample_rate=16_000) or "").strip()
            e = wer(words(line["text"]), words(got))
            ok &= e <= 0.2
            print(
                f"  {line['id']:8} {line['who']:5} {line['at']:6} ms  wer {e:.2f}  heard: {got!r}"
            )
        pcm = decode(path, RATE)
        whole, peak = lufs(pcm, tmp)
        per = {}
        for who in ("you", "ghost"):
            parts = [
                pcm[ln["at"] * RATE // 1000 : (ln["at"] + ln["ms"]) * RATE // 1000]
                for ln in timeline["lines"]
                if ln["who"] == who
            ]
            per[who] = lufs(np.concatenate(parts), tmp)[0]
        clipped = int(np.sum(np.abs(pcm) >= 0.999))
        print(
            f"  loudness {whole:.1f} LUFS, true peak {peak:.1f} dBTP, clipped samples {clipped};"
            f" you {per['you']:.1f} LUFS, Ghost {per['ghost']:.1f} LUFS"
        )
        ok &= clipped == 0 and peak <= -0.5 and abs(per["you"] - per["ghost"]) <= 1.5
    total = sum(p.stat().st_size for p in files)
    print(f"\nboth files {total / 1024:.1f} KiB (a visitor loads one)")
    ok &= total <= 400 * 1024
    return ok


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument(
        "--check",
        action="store_true",
        help="read every line back with Parakeet and measure the levels",
    )
    args = ap.parse_args()
    spec = json.loads(SOURCE.read_text())
    with tempfile.TemporaryDirectory() as d:
        tmp = Path(d)
        timeline, mix, _ = render(spec, tmp)
        files = encode(mix, tmp)
        TIMELINE.write_text(json.dumps(timeline, separators=(",", ":")) + "\n")
        for line in timeline["lines"]:
            print(f"{line['at']:6} ms  {line['ms']:5} ms  {line['who']:5} {line['text']}")
        print(
            f"loop {timeline['loop_ms']} ms, wake {timeline['wake']},"
            f" thinking {timeline['think']}, sleep {timeline['sleep']}"
        )
        if args.check and not check(timeline, files, tmp):
            print("check failed", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
