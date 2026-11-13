"""Data augmentation for the positives: level, noise, room response, speed and dynamic-range
compression, each drawn from a seeded generator so a run can be repeated exactly.

Everything here is numpy on int16-range float audio at 16 kHz; nothing is written.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

RATE = 16000


@dataclass(frozen=True)
class Aug:
    gain_db: float = 0.0
    speed: float = 1.0
    rt60: float = 0.0  # 0: no room
    snr_db: float | None = None  # None: no noise
    noise: str = "pink"  # pink | music
    drc: bool = False


def draw(rng: np.random.Generator, have_music: bool) -> Aug:
    return Aug(
        gain_db=float(rng.uniform(-18, 12)),
        speed=float(rng.uniform(0.88, 1.12)),
        rt60=float(rng.choice([0.0, rng.uniform(0.15, 0.7)])),
        snr_db=None if rng.random() < 0.3 else float(rng.uniform(3, 30)),
        noise="music" if have_music and rng.random() < 0.6 else "pink",
        drc=bool(rng.random() < 0.35),
    )


def pink(n: int, rng: np.random.Generator) -> np.ndarray:
    white = rng.standard_normal(n)
    f = np.fft.rfft(white)
    scale = 1 / np.sqrt(np.maximum(np.arange(len(f)), 1))
    return np.fft.irfft(f * scale, n)


def room(rt60: float, rng: np.random.Generator) -> np.ndarray:
    """A synthetic impulse response: the direct path and exponentially decaying noise."""
    n = int(RATE * rt60)
    t = np.arange(n) / RATE
    tail = rng.standard_normal(n) * np.exp(-6.9 * t / rt60)
    tail[: int(0.003 * RATE)] = 0  # the first reflection a few ms after the direct sound
    h = np.concatenate([[1.0], 0.4 * tail / (np.sqrt(np.sum(tail**2)) + 1e-9)])
    return h / np.sqrt(np.sum(h**2))


def fftconvolve(x: np.ndarray, h: np.ndarray) -> np.ndarray:
    """Linear convolution by FFT: direct convolution stalled for minutes on some clips."""
    n = len(x) + len(h) - 1
    size = 1 << (n - 1).bit_length()
    return np.fft.irfft(np.fft.rfft(x, size) * np.fft.rfft(h, size), size)[:n]


def speed(x: np.ndarray, factor: float) -> np.ndarray:
    """Faster (factor > 1) or slower, pitch and tempo together, as a tape would."""
    import soxr

    if abs(factor - 1) < 1e-3:
        return x
    return soxr.resample(x.astype(np.float32), RATE * factor, RATE).astype(np.float64)


def compress(
    x: np.ndarray, threshold_db: float = -30, ratio: float = 4, makeup_db: float = 12
) -> np.ndarray:
    """A feed-forward compressor on 10 ms blocks, smoothed: the kind of dynamic-range
    processing a capture chain (an AGC, a canceller's gain) applies."""
    block = RATE // 100
    n = len(x) // block * block
    if n == 0:
        return x
    rms = np.sqrt(np.mean((x[:n] / 32768.0).reshape(-1, block) ** 2, axis=1) + 1e-12)
    level = 20 * np.log10(rms)
    over = np.maximum(level - threshold_db, 0)
    gain_db = -over * (1 - 1 / ratio) + makeup_db
    # attack fast, release slow
    smooth = np.empty_like(gain_db)
    g = gain_db[0]
    for i, v in enumerate(gain_db):
        g = v if v < g else g + 0.1 * (v - g)
        smooth[i] = g
    gain = np.repeat(10 ** (smooth / 20), block)
    gain = np.concatenate([gain, np.full(len(x) - n, gain[-1] if len(gain) else 1.0)])
    return x * gain


def apply(
    x: np.ndarray, a: Aug, rng: np.random.Generator, music: list[np.ndarray] | None = None
) -> np.ndarray:
    y = x.astype(np.float64)
    y = speed(y, a.speed)
    if a.rt60 > 0:
        y = fftconvolve(y, room(a.rt60, rng))[: len(y)]
    if a.snr_db is not None:
        if a.noise == "music" and music:
            m = music[int(rng.integers(len(music)))].astype(np.float64)
            if len(m) < len(y):
                m = np.tile(m, len(y) // max(len(m), 1) + 1)
            off = int(rng.integers(0, len(m) - len(y) + 1))
            n = m[off : off + len(y)]
        else:
            n = pink(len(y), rng)
        p_sig = np.mean(y**2) + 1e-9
        p_n = np.mean(n**2) + 1e-9
        y = y + n * np.sqrt(p_sig / (p_n * 10 ** (a.snr_db / 10)))
    if a.drc:
        y = compress(y)
    y = y * 10 ** (a.gain_db / 20)
    return np.clip(y, -32768, 32767).astype(np.int16)
