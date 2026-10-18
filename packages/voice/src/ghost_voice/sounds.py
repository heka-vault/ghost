"""Generated UI sounds. Nothing is downloaded; files are rendered into the cache."""

from __future__ import annotations

import math
from pathlib import Path

import numpy as np


def _blip(sample_rate: int, freq: float, length: float, bend: float = 0.82) -> np.ndarray:
    """One bouncing blip: fast attack, exponential decay, pitch dropping to ``bend`` x."""
    n = int(sample_rate * length)
    t = np.arange(n) / sample_rate
    f = freq * (1 - (1 - bend) * t / length)  # glides down like a bounce losing height
    phase = 2 * math.pi * np.cumsum(f) / sample_rate
    env = np.minimum(t / 0.004, 1.0) * np.exp(-t / (length * 0.28))
    return env * (np.sin(phase) + 0.25 * np.sin(2 * phase))


def thinking_loop(sample_rate: int, seconds: float = 1.6) -> np.ndarray:
    """A light, bouncy figure: four short blips hopping between notes at 150 BPM, each
    with a small downward pitch bend, over a very soft low pulse. Loops seamlessly."""
    n = int(sample_rate * seconds)
    audio = np.zeros(n)
    beat = seconds / 4
    for i, freq in enumerate((659.3, 830.6, 987.8, 830.6)):  # E5 G#5 B5 G#5
        start = int(i * beat * sample_rate)
        blip = _blip(sample_rate, freq, min(0.18, beat))
        end = min(n, start + len(blip))
        audio[start:end] += blip[: end - start]
    t = np.arange(n) / sample_rate
    pulse = 0.5 - 0.5 * np.cos(2 * math.pi * t / seconds)
    audio += 0.12 * pulse * np.sin(2 * math.pi * 164.8 * t)  # E3 breathing underneath
    audio *= 0.55 / max(1e-6, float(np.abs(audio).max()))
    return (audio * 32767).astype(np.int16)


def ensure_thinking_sound(cache_dir: Path, sample_rate: int) -> Path:
    """Write the loop for this sample rate once; return its path."""
    import soundfile as sf

    cache_dir.mkdir(parents=True, exist_ok=True)
    path = cache_dir / f"thinking-v2-{sample_rate}.wav"
    if not path.exists():
        sf.write(str(path), thinking_loop(sample_rate), sample_rate, subtype="PCM_16")
    return path
