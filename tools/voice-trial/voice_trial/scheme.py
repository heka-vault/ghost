"""The windowed scheme as Ghost would run it, measured against the whole render.

The encoder once; the leading pad's frames skipped (less a margin); the decoder over windows
that start small and grow, each with context on both sides, the harmonic phase carried in,
and a short crossfade in the overlap; the tail trimmed. Against the whole render trimmed as
`Kokoro::synthesize` trims: first audio, the spectral distance (log-mel, dB), the loudness of
each window against the same span of the whole, the step at each join, DC and clipping.

uv run --project tools/voice-trial python -I tools/voice-trial/voice_trial/scheme.py SPLIT_DIR \
    [--first 8] [--grow 2] [--context 12] [--fade 2] [--save DIR]
"""

import argparse
import json
import os
import pathlib
import time

import numpy as np
import onnxruntime as ort

from voice_trial.windowed import HARMONICS, HOP, phase_before, save, session

RATE = 24000
X = "/decoder/decoder/decode.3/Div_4_output_0"
FIRST_RIGHT = int(os.environ.get("FIRST_RIGHT", "0"))


def trim(y: np.ndarray, ref: float | None = None) -> tuple[int, int]:
    """librosa.effects.trim with its defaults; `ref` replaces the peak RMS when given."""
    frame, hop, top_db = 2048, 512, 60.0
    pad = frame // 2
    yp = np.pad(y.astype(np.float64), pad)
    frames = 1 + (len(yp) - frame) // hop
    rms = np.array([np.sqrt(np.mean(yp[i * hop : i * hop + frame] ** 2)) for i in range(frames)])
    r = rms.max() if ref is None else ref
    db = 10 * np.log10(np.maximum(rms**2, 1e-10)) - 10 * np.log10(max(r**2, 1e-10))
    nz = np.nonzero(db > -top_db)[0]
    if len(nz) == 0:
        return 0, 0
    return int(nz[0] * hop), int(min(len(y), (nz[-1] + 1) * hop))


def mel_bank(n_fft=1024, n_mels=80, rate=RATE):
    def hz_to_mel(f):
        return 2595 * np.log10(1 + f / 700)

    def mel_to_hz(m):
        return 700 * (10 ** (m / 2595) - 1)

    mels = np.linspace(hz_to_mel(0), hz_to_mel(rate / 2), n_mels + 2)
    hz = mel_to_hz(mels)
    bins = np.floor((n_fft + 1) * hz / rate).astype(int)
    fb = np.zeros((n_mels, n_fft // 2 + 1))
    for m in range(1, n_mels + 1):
        a, b, c = bins[m - 1], bins[m], bins[m + 1]
        for k in range(a, b):
            fb[m - 1, k] = (k - a) / max(b - a, 1)
        for k in range(b, c):
            fb[m - 1, k] = (c - k) / max(c - b, 1)
    return fb


FB = mel_bank()


def logmel(y: np.ndarray, n_fft=1024, hop=240) -> np.ndarray:
    win = np.hanning(n_fft)
    frames = 1 + max(0, (len(y) - n_fft) // hop)
    spec = np.stack([np.abs(np.fft.rfft(win * y[i * hop : i * hop + n_fft])) ** 2 for i in range(frames)])
    return 10 * np.log10(np.maximum(spec @ FB.T, 1e-10))


def mel_distance(a: np.ndarray, b: np.ndarray) -> float:
    """Mean absolute log-mel difference in dB over frames where either is above -60 dB of
    the louder's peak."""
    n = min(len(a), len(b))
    ma, mb = logmel(a[:n]), logmel(b[:n])
    peak = max(ma.max(), mb.max())
    mask = np.maximum(ma, mb) > peak - 60
    return float(np.abs(ma - mb)[mask].mean())


def rms_db(x):
    return 20 * np.log10(np.sqrt(np.mean(np.asarray(x, dtype=np.float64) ** 2)) + 1e-12)


def plan(start: int, frames: int, first: int, grow: float) -> list[tuple[int, int]]:
    out, s, size = [], start, first
    while s < frames:
        e = min(frames, s + size)
        if frames - e < max(4, size // 2):
            e = frames
        out.append((s, e))
        s, size = e, int(round(size * grow))
    return out


def render(enc, dec, row, first, grow, context, fade, margin, front=None):
    tokens = np.array([[0, *row["tokens"], 0]], dtype=np.int64)
    style = np.array([row["style"]], dtype=np.float32)
    speed = np.array([1.0], dtype=np.float32)
    t0 = time.perf_counter()
    asr, f0, n, dur = enc.run(None, {"tokens": tokens, "style": style, "speed": speed})
    x = None
    if front is not None:
        (x,) = front.run(None, {"asr": asr, "f0": f0, "n": n, "style": style})
    t_enc = time.perf_counter() - t0
    frames = asr.shape[-1]
    lead = max(0, int(dur[0, 0]) - margin)
    out = np.zeros(0, dtype=np.float32)
    tail = None  # the previous window's audio past its end, for the crossfade
    times, first_audio, joins = [], None, []
    fade_n = fade * HOP
    for s, e in plan(lead, frames, first, grow):
        right = FIRST_RIGHT if (FIRST_RIGHT and not times) else context
        ws, we = max(0, s - context), min(frames, e + right)
        t = time.perf_counter()
        if x is not None:
            feed = {
                X: x[:, :, 2 * ws : 2 * we],
                "f0": f0[:, 2 * ws : 2 * we],
                "style": style,
                "phase": phase_before(f0, 2 * ws),
            }
        else:
            feed = {
                "asr": asr[:, :, ws:we],
                "f0": f0[:, 2 * ws : 2 * we],
                "n": n[:, 2 * ws : 2 * we],
                "style": style,
                "phase": phase_before(f0, 2 * ws),
            }
        (audio,) = dec.run(None, feed)
        times.append(time.perf_counter() - t)
        if first_audio is None:
            first_audio = t_enc + times[0]
        body = audio[(s - ws) * HOP : (e - ws) * HOP].copy()
        if tail is not None and fade_n:
            k = min(fade_n, len(tail), len(body))
            ramp = np.linspace(0, 1, k, endpoint=False, dtype=np.float32)
            body[:k] = tail[:k] * (1 - ramp) + body[:k] * ramp
        if len(out):
            joins.append(len(out))
        out = np.concatenate([out, body])
        tail = audio[(e - ws) * HOP : (e - ws) * HOP + fade_n]
    return out, lead, first_audio, t_enc, times, joins


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("split")
    p.add_argument("--first", type=int, default=8)
    p.add_argument("--grow", type=float, default=2.0)
    p.add_argument("--context", type=int, default=12)
    p.add_argument("--fade", type=int, default=0)
    p.add_argument("--margin", type=int, default=2)
    p.add_argument("--save")
    p.add_argument("--generator", action="store_true", help="the frame-rate front whole, the generator windowed")
    a = p.parse_args()
    split = pathlib.Path(a.split)
    lines = json.loads((split / "lines.json").read_text())
    enc = session(str(split / "kokoro-encoder.onnx"))
    dec = session(str(split / "kokoro-decoder.onnx"))
    front = gen = None
    if a.generator:
        front = session(str(split / "front.onnx"))
        gen = session(str(split / "gen.onnx"))
    zero = np.zeros((1, 1, HARMONICS), dtype=np.float32)
    print(
        f"load {os.getloadavg()[0]:.1f}; first {a.first} frames, x{a.grow}, context {a.context}, "
        f"fade {a.fade} frames, lead margin {a.margin}, {'generator only' if a.generator else 'whole decoder'} windowed"
    )
    for words, row in lines.items():
        tokens = np.array([[0, *row["tokens"], 0]], dtype=np.int64)
        style = np.array([row["style"]], dtype=np.float32)
        t = time.perf_counter()
        asr, f0, n, _ = enc.run(None, {"tokens": tokens, "style": style, "speed": np.array([1.0], np.float32)})
        (whole,) = dec.run(None, {"asr": asr, "f0": f0, "n": n, "style": style, "phase": zero})
        t_whole = time.perf_counter() - t
        ws, we = trim(whole)
        out, lead, first_audio, t_enc, times, joins = render(
            enc, gen or dec, row, a.first, a.grow, a.context, a.fade, a.margin, front
        )
        # Align: the windowed audio starts at frame `lead`.
        offset = lead * HOP
        ref = whole[offset : offset + len(out)]
        s2, e2 = trim(out, ref=np.sqrt(np.mean(whole[ws:we] ** 2)) * 4)
        dist = mel_distance(out, ref)
        loud = []
        bounds = [0, *joins, len(out)]
        for i in range(len(bounds) - 1):
            seg, rseg = out[bounds[i] : bounds[i + 1]], ref[bounds[i] : bounds[i + 1]]
            loud.append(rms_db(seg) - rms_db(rseg))
        steps = []
        for j in joins:
            before, after = out[j - 1440 : j], out[j : j + 1440]
            rb, ra = ref[j - 1440 : j], ref[j : j + 1440]
            steps.append((rms_db(after) - rms_db(before)) - (rms_db(ra) - rms_db(rb)))
        jump = max((abs(float(out[j] - out[j - 1])) for j in joins), default=0.0)
        typical = float(np.percentile(np.abs(np.diff(ref)), 99))
        print(
            f"  {words:>2}w: whole {t_whole*1000:5.0f} ms -> first audio {first_audio*1000:4.0f} ms "
            f"(enc {t_enc*1000:3.0f} + {times[0]*1000:3.0f}); {len(times)} windows, decode total "
            f"{sum(times)*1000:5.0f} ms; skipped lead {offset/RATE:.2f}s (whole's trim starts {ws/RATE:.2f}s, "
            f"ours at +{s2/RATE:.2f}s); mel dist {dist:.2f} dB; loudness by window "
            f"{', '.join(f'{x:+.1f}' for x in loud)}; level step at joins vs whole "
            f"{', '.join(f'{x:+.1f}' for x in steps)} dB; largest sample step at a join {jump:.3f} "
            f"(p99 step {typical:.3f}); DC {float(out.mean()):+.5f}; peak {float(np.abs(out).max()):.2f}"
        )
        if a.save:
            d = pathlib.Path(a.save)
            d.mkdir(parents=True, exist_ok=True)
            save(d / f"{words}-whole.wav", whole[ws:we])
            save(d / f"{words}-windowed.wav", out)


if __name__ == "__main__":
    main()
