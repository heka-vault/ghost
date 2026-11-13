"""Dynamic-range processing of the microphone before the "hey ghost" detector and before
Parakeet's check of a weak hit, replayed over the labelled streams of the 2026-11-12 wake
check (docs/benchmarks/2026-11-12-wake-dynamics.md).

    make wake-dynamics
    make wake-dynamics ARGS="--only base,c4-fast --set my-streams.txt --workers 6"
    uv run --project tools/oracle python -I docs/benchmarks/scripts/wake_dynamics.py \\
        [--set FILE] [--only V1,V2] [--workers N] [--cache DIR] [--onsets]

Each variant is applied to the whole stream as the daemon would apply it frame by frame
(causal: nothing looks ahead, except ``win-norm``, which scales each 2 s window the detector
or Parakeet is given as a whole). For each variant three placements are replayed with the
shipped rule (detector, a weak hit from the 0.3 floor read by Parakeet twice, ``match_wake``
at 0.8 or ``match_heys_spelling``; wake_check_replay.py): ``det`` (the detector hears the
processed stream, Parakeet the unprocessed), ``stt`` (the reverse) and ``both``. The ``twin``
variant gives the detector the array's raw stream recorded beside a ``ghost.mic`` one, and
Parakeet the ``ghost.mic`` one; streams without a twin are replayed unchanged.

``--onsets`` prints, per operator take, the energy over time of both streams around the
peak, in four bands, by how the take scored on ``ghost.mic``.

Only numpy, no new dependency. The detector's scores are cached as numbers; Parakeet's
readings are kept in memory and never written or printed, since they are the operator's
words (CLAUDE.md).
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import sys
import time
from collections.abc import Callable
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("wake_check_replay", HERE / "wake_check_replay.py")
replay = importlib.util.module_from_spec(_spec)
assert _spec.loader
sys.modules["wake_check_replay"] = replay
_spec.loader.exec_module(replay)

RATE = replay.RATE
STEP = replay.STEP
WINDOW = replay.WINDOW
FLOOR_DB = -96.0
CACHE = Path.home() / ".cache" / "ghost" / "wake-dynamics"

# ---------------------------------------------------------------------------------------
# The processing. Every function takes int16 PCM and returns int16 PCM of the same length.


def db(x: np.ndarray) -> np.ndarray:
    return 10 * np.log10(np.maximum(x, 1e-10))


def block_db(x: np.ndarray, n: int) -> np.ndarray:
    """The level (dBFS, RMS) of each block of ``n`` samples; the tail is padded."""
    m = -(-len(x) // n)
    y = np.zeros(m * n, np.float64)
    y[: len(x)] = x / 32768.0
    return db(np.mean(y.reshape(m, n) ** 2, axis=1))


def apply_gain_db(x: np.ndarray, g_db: np.ndarray, n: int) -> np.ndarray:
    """A gain per block of ``n`` samples, interpolated per sample, clipped to 16 bits."""
    centres = np.arange(len(g_db)) * n + n / 2
    g = np.interp(np.arange(len(x)), centres, g_db)
    y = x.astype(np.float64) * 10 ** (g / 20)
    return np.clip(np.round(y), -32768, 32767).astype(np.int16)


def smooth(target: np.ndarray, block_s: float, attack_s: float, release_s: float) -> np.ndarray:
    """A one-pole follower of ``target`` (dB) per block, ``attack`` when it falls (gain
    reduction comes in), ``release`` when it rises."""
    a = np.exp(-block_s / max(attack_s, 1e-6))
    r = np.exp(-block_s / max(release_s, 1e-6))
    out = np.empty_like(target)
    s = 0.0
    for i, t in enumerate(target.tolist()):
        c = a if t < s else r
        s = c * s + (1 - c) * t
        out[i] = s
    return out


def compressor(
    ratio: float, thr: float, attack: float, release: float, makeup: float, pre: float = 0.0
) -> Callable[[np.ndarray], np.ndarray]:
    """Feed-forward, RMS over 1 ms blocks, hard knee, the gain smoothed in dB."""

    def f(x: np.ndarray) -> np.ndarray:
        n = 16
        lvl = block_db(x, n) + pre
        over = np.maximum(lvl - thr, 0.0)
        g = smooth(-over * (1 - 1 / ratio), n / RATE, attack, release)
        return apply_gain_db(x, g + makeup + pre, n)

    return f


def noise_floor(lvl: np.ndarray, block_s: float, rise_db_s: float = 3.0) -> np.ndarray:
    """A minimum follower: drops to a quieter block at once, rises ``rise_db_s`` dB a second;
    never under -70 dBFS."""
    out = np.empty_like(lvl)
    f = lvl[0] if len(lvl) else -90.0
    step = rise_db_s * block_s
    for i, v in enumerate(lvl.tolist()):
        f = v if v < f else f + step
        out[i] = f
    return np.maximum(out, -70.0)  # a canceller's digital silence is not the room's floor


def agc(
    target: float, tau: float, max_gain: float, gate: str
) -> Callable[[np.ndarray], np.ndarray]:
    """A speech-level follower over 10 ms blocks, updated only on blocks 10 dB over the noise
    floor and over -60 dBFS; gain = target - level, within [-10, max_gain] dB. Out of speech
    the gain is held (``hold``) or falls back to 0 dB within 100 ms (``gate``), so room noise
    is never lifted by more than the gain the last speech was given."""

    def f(x: np.ndarray) -> np.ndarray:
        n, bs = 160, 0.01
        lvl = block_db(x, n)
        fl = noise_floor(lvl, bs)
        speech = (lvl > fl + 10) & (lvl > -60)
        c = np.exp(-bs / tau)
        cg = np.exp(-bs / 0.1)
        est, g = target, 0.0
        out = np.empty_like(lvl)
        for i, (v, sp) in enumerate(zip(lvl.tolist(), speech.tolist(), strict=True)):
            if sp:
                est = c * est + (1 - c) * v
                want = min(max(target - est, -10.0), max_gain)
                g = cg * g + (1 - cg) * want
            elif gate == "gate":
                g = cg * g
            out[i] = g
        return apply_gain_db(x, out, n)

    return f


def win_norm(target: float = -20.0, max_gain: float = 24.0, gate: float = -55.0):
    """Scale a window so its loudest 400 ms reads ``target`` dBFS, by at most ``max_gain``,
    and leave a window whose loudest 400 ms is under ``gate`` alone."""

    def f(x: np.ndarray) -> np.ndarray:
        n = 6400
        if len(x) < n:
            return x
        c = np.concatenate([[0.0], np.cumsum((x.astype(np.float64) / 32768) ** 2)])
        e = (c[n:] - c[:-n]) / n  # the mean power of every 400 ms
        loud = float(db(e[::160].max()))
        if loud < gate:
            return x
        g = min(target - loud, max_gain)
        return np.clip(np.round(x * 10 ** (g / 20)), -32768, 32767).astype(np.int16)

    return f


def fir_highpass(cut: float, taps: int = 511) -> Callable[[np.ndarray], np.ndarray]:
    """A linear-phase FIR high-pass (Blackman), by FFT in chunks; the delay is removed."""
    t = np.arange(taps) - (taps - 1) / 2
    lp = np.sinc(2 * cut / RATE * t) * np.blackman(taps)
    lp /= lp.sum()
    h = -lp
    h[(taps - 1) // 2] += 1

    def f(x: np.ndarray) -> np.ndarray:
        y = fft_convolve(x.astype(np.float64), h)[(taps - 1) // 2 :][: len(x)]
        return np.clip(np.round(y), -32768, 32767).astype(np.int16)

    return f


def fft_convolve(x: np.ndarray, h: np.ndarray, chunk: int = 1 << 16) -> np.ndarray:
    m = len(h)
    nfft = 1 << int(np.ceil(np.log2(chunk + m - 1)))
    hf = np.fft.rfft(h, nfft)
    out = np.zeros(len(x) + m - 1)
    for s in range(0, len(x), chunk):
        seg = x[s : s + chunk]
        y = np.fft.irfft(np.fft.rfft(seg, nfft) * hf, nfft)[: len(seg) + m - 1]
        out[s : s + len(y)] += y
    return out


def pre_emphasis(a: float = 0.97, makeup: float = 0.0) -> Callable[[np.ndarray], np.ndarray]:
    def f(x: np.ndarray) -> np.ndarray:
        y = x.astype(np.float64)
        y[1:] -= a * y[:-1]
        return np.clip(np.round(y * 10 ** (makeup / 20)), -32768, 32767).astype(np.int16)

    return f


def onset_lift(lift: float, length: float = 0.15) -> Callable[[np.ndarray], np.ndarray]:
    """``lift`` dB over the first ``length`` s after an onset (a 10 ms block 12 dB over the
    noise floor after 250 ms under floor + 6 dB), faded in and out over 20 ms."""

    def f(x: np.ndarray) -> np.ndarray:
        n, bs = 160, 0.01
        lvl = block_db(x, n)
        fl = noise_floor(lvl, bs)
        g = np.zeros_like(lvl)
        quiet, left = 0, 0
        span = int(length / bs)
        for i in range(len(lvl)):
            if left:
                k = span - left
                g[i] = lift * min(1.0, (k + 1) / 2, left / 2)
                left -= 1
            elif lvl[i] > fl[i] + 12 and lvl[i] > -60 and quiet >= 25:
                left = span
                g[i] = lift / 2
                left -= 1
            quiet = quiet + 1 if lvl[i] < fl[i] + 6 else 0
        return apply_gain_db(x, g, n)

    return f


@dataclass(frozen=True)
class Variant:
    key: str
    what: str
    stream: Callable[[np.ndarray], np.ndarray] | None = None  # the whole stream, causally
    window: Callable[[np.ndarray], np.ndarray] | None = None  # each window given to a model
    twin: bool = False  # the detector hears the raw array twin


VARIANTS = (
    Variant("base", "unprocessed"),
    Variant(
        "c2-fast",
        "compressor 2:1 at -30 dBFS, 5/100 ms, no make-up",
        compressor(2, -30, 0.005, 0.1, 0),
    ),
    Variant(
        "c4-fast",
        "compressor 4:1 at -30 dBFS, 5/100 ms, no make-up",
        compressor(4, -30, 0.005, 0.1, 0),
    ),
    Variant(
        "c4-fast-mk",
        "compressor 4:1 at -30 dBFS, 5/100 ms, +12 dB make-up",
        compressor(4, -30, 0.005, 0.1, 12),
    ),
    Variant(
        "c4-slow-mk",
        "compressor 4:1 at -30 dBFS, 50/500 ms, +12 dB make-up",
        compressor(4, -30, 0.05, 0.5, 12),
    ),
    Variant(
        "c8-fast-mk",
        "compressor 8:1 at -35 dBFS, 2/80 ms, +18 dB make-up",
        compressor(8, -35, 0.002, 0.08, 18),
    ),
    Variant(
        "limit",
        "+12 dB then a limiter at -6 dBFS (20:1, 1/50 ms)",
        compressor(20, -6, 0.001, 0.05, 0, pre=12),
    ),
    Variant(
        "win-norm",
        "each 2 s window's loudest 400 ms to -20 dBFS (gain <= 24 dB, gate -55)",
        window=win_norm(),
    ),
    Variant(
        "norm-slow",
        "rolling speech-level normalisation to -26 dBFS, 3 s, held in noise",
        agc(-26, 3.0, 20, "hold"),
    ),
    Variant(
        "agc-gate",
        "AGC to -26 dBFS, 300 ms, gain <= 20 dB, back to 0 dB out of speech",
        agc(-26, 0.3, 20, "gate"),
    ),
    Variant("pre-emph", "pre-emphasis 0.97, +12 dB make-up", pre_emphasis(0.97, 12)),
    Variant("hp200", "high-pass 200 Hz (FIR)", fir_highpass(200)),
    Variant("hp400", "high-pass 400 Hz (FIR)", fir_highpass(400)),
    Variant("onset+6", "+6 dB over the first 150 ms after an onset", onset_lift(6)),
    Variant("onset+12", "+12 dB over the first 150 ms after an onset", onset_lift(12)),
    Variant("twin", "the detector hears the array's raw stream; Parakeet ghost.mic", twin=True),
)
BY_KEY = {v.key: v for v in VARIANTS}

# ---------------------------------------------------------------------------------------
# The streams.


def expand(path: str) -> str:
    env = dict(os.environ)
    env.setdefault("SCRATCH", str(Path.home() / "projects/active/ghost/.scratch"))
    env.setdefault("HEAR", str(Path.home() / ".local/share/ghost/hear/sessions"))
    return os.path.expanduser(re.sub(r"\$(\w+)", lambda m: env.get(m.group(1), m.group(0)), path))


def read_stream(spec: str) -> np.ndarray:
    """PATH[#CH]; a PATH with ``*`` is its sorted matches, each followed by 1 s of silence
    (how the 2026-11-06 Hearing session was replayed)."""
    path, _, ch = spec.partition("#")
    if "*" not in path:
        return replay.read(spec)
    import glob

    parts = []
    for p in sorted(glob.glob(path)):
        parts += [replay.read(f"{p}#{ch}" if ch else p), np.zeros(RATE, np.int16)]
    if not parts:
        raise SystemExit(f"{path}: no match")
    return np.concatenate(parts)


def twin_of(spec: str) -> str | None:
    path = spec.partition("#")[0]
    for a, b in (("-aec.wav", "-raw.wav"), (".aec.wav", ".raw.wav#0")):
        if path.endswith(a):
            return path[: -len(a)] + b
    return None


def read_set(path: Path) -> list[tuple[str, list[tuple[str, str]]]]:
    sets: list[tuple[str, list[tuple[str, str]]]] = []
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("["):
            sets.append((line.strip("[]"), []))
            continue
        src, _, label = expand(line).rpartition(":")
        sets[-1][1].append((src, label))
    return sets


# ---------------------------------------------------------------------------------------
# The detector, in worker processes.

_scorer = None


def _init() -> None:
    global _scorer
    os.nice(10)
    from oracle.wakeword import WakeWordScorer

    _scorer = WakeWordScorer([replay.MODEL])


def processed(src: str, v: Variant) -> np.ndarray:
    if v.twin:
        t = twin_of(src)
        return read_stream(t) if t else read_stream(src)
    pcm = read_stream(src)
    return v.stream(pcm) if v.stream else pcm


def cache_file(cache: Path, src: str, key: str) -> Path:
    return cache / key / (re.sub(r"[^A-Za-z0-9._-]", "_", src.split("/")[-1] + src) + ".npz")


def score_job(job: tuple[str, str, str]) -> tuple[str, str, float]:
    src, key, cache = job
    f = cache_file(Path(cache), src, key)
    if f.exists():
        return src, key, 0.0
    v = BY_KEY[key]
    t0 = time.perf_counter()
    pcm = processed(src, v)
    padded = np.concatenate([np.zeros(WINDOW, np.int16), pcm])
    out = []
    for e in range(WINDOW, len(padded) + 1, STEP):
        w = padded[e - WINDOW : e]
        if v.window:
            w = v.window(w)
        out.append(_scorer.score(w)["hey_ghost"])
    f.parent.mkdir(parents=True, exist_ok=True)
    np.savez(f, score=np.array(out))
    return src, key, time.perf_counter() - t0


def scores(cache: Path, src: str, key: str) -> np.ndarray:
    return np.load(cache_file(cache, src, key))["score"]


# ---------------------------------------------------------------------------------------
# The cost of each variant per 80 ms frame, as numpy runs it here.


def cost(v: Variant, pcm: np.ndarray) -> float:
    """Microseconds per 80 ms frame."""
    if v.twin or (v.stream is None and v.window is None):
        return 0.0
    t0 = time.perf_counter()
    if v.stream:
        v.stream(pcm)
        frames = len(pcm) / STEP
    else:
        frames = 0
        for e in range(WINDOW, len(pcm) + 1, STEP * 10):
            v.window(pcm[e - WINDOW : e])
            frames += 1
    return (time.perf_counter() - t0) / frames * 1e6


# ---------------------------------------------------------------------------------------


def onsets(sets) -> None:
    """Energy over time of each operator take on both streams, by band, around the peak."""
    from oracle.wakeword import WakeWordScorer

    s = WakeWordScorer([replay.MODEL])
    bands = ((60, 300), (300, 1000), (1000, 3000), (3000, 7600))
    take_sets = [x for x in sets if x[0].startswith("operator takes, ghost.mic")]
    rows = []
    for _, streams in take_sets:
        for src, label in streams:
            if not label.startswith("pos@"):
                continue
            aec, raw = read_stream(src), read_stream(twin_of(src))
            for m in label[4:].split(","):
                if m.startswith("~"):
                    continue
                t = float(m)

                def best(pcm, t=t):
                    out = []
                    for e in range(int((t - 1.2) * RATE), int((t + 1.2) * RATE), STEP):
                        if e >= WINDOW and e <= len(pcm):
                            out.append((s.score(pcm[e - WINDOW : e])["hey_ghost"], e))
                    return max(out)

                sa, _ = best(aec)
                sr, er = best(raw)
                # 20 ms frames from 1.0 s before the raw peak's window end to 0.1 s after
                lo = er - RATE
                spec = {}
                for name, pcm in (("aec", aec), ("raw", raw)):
                    seg = pcm[lo : er + RATE // 10].astype(np.float64) / 32768
                    fr = seg[: len(seg) // 320 * 320].reshape(-1, 320) * np.hanning(320)
                    p = np.abs(np.fft.rfft(fr, axis=1)) ** 2
                    hz = np.fft.rfftfreq(320, 1 / RATE)
                    spec[name] = np.array(
                        [db(p[:, (hz >= a) & (hz < b)].sum(1) / 320 * 2) for a, b in bands]
                    )
                    spec[name + "_zeros"] = float(np.mean(pcm[lo : er + RATE // 10] == 0))
                rows.append((Path(src).name, t, sa, sr, spec))
    # the bands are 20 ms frames: index 0 is 1.0 s before the window end
    print("\n## takes: score on ghost.mic / raw, band energy (dB) aec minus raw, per 100 ms")
    print("take | aec | raw | band | -1.0 … +0.1 s (each 100 ms: aec - raw dB)")
    groups = {"low": [], "high": []}
    for name, t, sa, sr, spec in rows:
        d = spec["aec"] - spec["raw"]
        k = d.shape[1] // 5
        d5 = d[:, : k * 5].reshape(4, k, 5).mean(2)
        groups["low" if sa < 0.6 else "high"].append((d5, spec))
        for b in range(4):
            print(
                f"{name[:18]} {t:6.1f} | {sa:.2f} | {sr:.2f} | {bands[b]} | "
                + " ".join(f"{x:+5.1f}" for x in d5[b])
            )
        print(f"   zeros aec {spec['aec_zeros']:.3f} raw {spec['raw_zeros']:.3f}")
    for g, items in groups.items():
        if not items:
            continue
        d = np.mean([x[0] for x in items], axis=0)
        a = np.mean([x[1]["aec"] for x in items], axis=0)
        r = np.mean([x[1]["raw"] for x in items], axis=0)
        k = a.shape[1] // 5
        print(f"\n### {g} on ghost.mic ({len(items)} takes): mean per 100 ms")
        for b in range(4):
            aa = a[b, : k * 5].reshape(k, 5).mean(1)
            rr = r[b, : k * 5].reshape(k, 5).mean(1)
            print(f"{bands[b]} aec  " + " ".join(f"{x:6.1f}" for x in aa))
            print(f"{bands[b]} raw  " + " ".join(f"{x:6.1f}" for x in rr))
            print(f"{bands[b]} diff " + " ".join(f"{x:+6.1f}" for x in d[b]))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--set", type=Path, default=HERE / "wake_dynamics_set.txt")
    ap.add_argument("--only", default="", help="comma-separated variant keys (base is kept)")
    ap.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 2) * 2 // 3))
    ap.add_argument("--cache", type=Path, default=CACHE, help="the detector's scores (numbers)")
    ap.add_argument("--onsets", action="store_true", help="the energy of the takes, then stop")
    ap.add_argument("--score-only", action="store_true", help="the detector's scores, then stop")
    ap.add_argument("--threshold", type=float, default=0.6)
    ap.add_argument("--floor", type=float, default=0.3)
    a = ap.parse_args()

    sets = read_set(a.set)
    if a.onsets:
        onsets(sets)
        return
    keys = ["base"] + [k for k in (a.only.split(",") if a.only else BY_KEY) if k and k != "base"]
    for k in keys:
        if k not in BY_KEY:
            raise SystemExit(f"{k}: not a variant ({', '.join(BY_KEY)})")

    # 1. the detector, every stream under every variant (a twin needs no new scores:
    #    it is the raw stream's base scores)
    srcs = [src for _, streams in sets for src, _ in streams]
    jobs = [(s, k, str(a.cache)) for k in keys for s in srcs if not BY_KEY[k].twin]
    jobs += [(twin_of(s), "base", str(a.cache)) for s in srcs if "twin" in keys and twin_of(s)]
    jobs = list(dict.fromkeys(jobs))
    todo = [j for j in jobs if not cache_file(a.cache, j[0], j[1]).exists()]
    print(f"detector: {len(todo)} of {len(jobs)} stream x variant runs to score", file=sys.stderr)
    if todo:
        with ProcessPoolExecutor(a.workers, initializer=_init) as ex:
            for i, (src, key, secs) in enumerate(ex.map(score_job, todo), 1):
                print(f"  [{i}/{len(todo)}] {key} {Path(src).name} {secs:.0f}s", file=sys.stderr)
    if a.score_only:
        return

    # 2. Parakeet on the weak hits, readings in memory only
    from oracle.parakeet import ParakeetSTT
    from oracle.wake import match_heys_spelling, match_wake

    stt = ParakeetSTT(quantization=None)
    if replay.PARAKEET.is_dir():
        import onnx_asr
        import onnxruntime as ort

        so = ort.SessionOptions()
        so.intra_op_num_threads = 4
        so.inter_op_num_threads = 1
        stt.model = onnx_asr.load_model(
            "nemo-parakeet-tdt-0.6b-v3", replay.PARAKEET, sess_options=so
        )
    # whether each weak hit's readings matched: booleans only, kept so the checks can run in
    # batches; the readings themselves are never written
    vfile = a.cache / "verdicts.json"
    verdicts: dict[str, bool] = json.loads(vfile.read_text()) if vfile.exists() else {}

    def confirmed(src: str, pcm_for_read: np.ndarray, rkey: str, at: float) -> bool:
        k = f"{src}|{rkey}|{at:.2f}"
        if k not in verdicts:
            v = BY_KEY[rkey]
            lo = max(0, int((at - replay.READ_BEFORE) * RATE))
            texts = []
            for x in replay.READS_AFTER:
                seg = pcm_for_read[lo : min(len(pcm_for_read), int((at + x) * RATE))]
                if v.window:
                    seg = v.window(seg)
                texts.append(stt.recognize(seg.astype(np.float32) / 32768))
            verdicts[k] = any(match_wake(t, ("hey ghost",), fuzzy=0.8) for t in texts) or any(
                match_heys_spelling(t) for t in texts
            )
        return verdicts[k]

    # placement -> (detector variant, reading variant)
    placements = {
        "det": lambda k: (k, "base"),
        "stt": lambda k: ("base", k),
        "both": lambda k: (k, k),
    }
    results: dict[str, dict[tuple[str, str], list[int]]] = {}
    for set_name, streams in sets:
        res = results.setdefault(set_name, {})
        for src, label in streams:
            base_pcm = read_stream(src)
            pcm_cache: dict[str, np.ndarray] = {"base": base_pcm}
            if label.startswith("program="):
                pos, ignore = replay.program_positives(
                    Path(label[8:]), base_pcm, "room" in label or "rec/" in src
                )
            elif label.startswith("pos@"):
                marks = [x for x in label[4:].split(",") if x]
                pos = [
                    (float(x) - replay.NEAR, float(x) + replay.NEAR)
                    for x in marks
                    if not x.startswith("~")
                ]
                ignore = [
                    (float(x[1:]) - replay.NEAR, float(x[1:]) + replay.NEAR)
                    for x in marks
                    if x.startswith("~")
                ]
            else:
                pos, ignore = [], []
            for k in keys:
                for pl, f in placements.items():
                    if k == "base" and pl != "both":
                        continue
                    if BY_KEY[k].twin and pl != "det":
                        continue  # Parakeet stays on ghost.mic; on the twin it is the raw set
                    dk, rk = f(k)
                    if BY_KEY[dk].twin:
                        t = twin_of(src)
                        sc = scores(a.cache, t, "base") if t else scores(a.cache, src, "base")
                    else:
                        sc = scores(a.cache, src, dk)
                    if BY_KEY[rk].twin:
                        rk = "base"  # the reading stays ghost.mic
                    if rk not in pcm_cache:
                        v = BY_KEY[rk]
                        pcm_cache[rk] = v.stream(base_pcm) if v.stream else base_pcm
                    heard: set[int] = set()
                    false = reads = 0
                    for at, s in replay.hits(sc, a.threshold, a.floor, 0):
                        take = next(
                            (
                                i
                                for i, (lo, hi) in enumerate(pos)
                                if lo <= at <= hi and i not in heard
                            ),
                            None,
                        )
                        if take is None and any(lo <= at <= hi for lo, hi in ignore):
                            continue
                        woke = s >= a.threshold
                        if not woke:
                            reads += 1
                            woke = confirmed(src, pcm_cache[rk], rk, at)
                        if take is not None:
                            if woke:
                                heard.add(take)
                        else:
                            false += woke
                    r = res.setdefault((k, "base" if k == "base" else pl), [0, 0, 0, 0])
                    r[0] += len(pos)
                    r[1] += len(heard)
                    r[2] += false
                    r[3] += reads
        vfile.write_text(json.dumps(verdicts))
        print(f"done: {set_name}", file=sys.stderr)

    # 3. the tables
    costs = {}
    sample = read_stream(sets[0][1][0][0])[: RATE * 60]
    for k in keys:
        costs[k] = cost(BY_KEY[k], sample)
    names = [n for n, _ in sets]
    print("\n| variant | placement | " + " | ".join(names) + " | µs/frame (numpy) |")
    print("|---|---|" + "---|" * len(names) + "---|")
    rows = [("base", "base")] + [(k, p) for k in keys if k != "base" for p in placements]
    for k, p in rows:
        if (k, p) not in results[names[0]]:
            continue
        cells = []
        for n in names:
            t, h, fw, rd = results[n][(k, p)]
            cells.append(f"{h}/{t}, {fw} fw, {rd} rd" if t else f"{fw} fw, {rd} rd")
        print(f"| {k} | {p} | " + " | ".join(cells) + f" | {costs[k]:.0f} |")
    print("\n" + "\n".join(f"- `{v.key}`: {v.what}" for v in VARIANTS if v.key in keys))


if __name__ == "__main__":
    main()
