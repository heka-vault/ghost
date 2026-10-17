"""Generated UI sounds. Nothing is downloaded; files are rendered into the cache."""

from __future__ import annotations

import math
from pathlib import Path

import numpy as np


def thinking_loop(sample_rate: int, seconds: float = 2.0) -> np.ndarray:
    """A soft, slow pulse: a low tone breathing in and out. Loops seamlessly."""
    n = int(sample_rate * seconds)
    t = np.arange(n) / sample_rate
    breath = 0.5 - 0.5 * np.cos(2 * math.pi * t / seconds)  # 0 -> 1 -> 0 over the loop
    tone = np.sin(2 * math.pi * 196.0 * t) + 0.35 * np.sin(2 * math.pi * 392.0 * t)
    audio = 0.6 * breath**2 * tone / 1.35
    return (audio * 32767).astype(np.int16)


def ensure_thinking_sound(cache_dir: Path, sample_rate: int) -> Path:
    """Write the loop for this sample rate once; return its path."""
    import soundfile as sf

    cache_dir.mkdir(parents=True, exist_ok=True)
    path = cache_dir / f"thinking-{sample_rate}.wav"
    if not path.exists():
        sf.write(str(path), thinking_loop(sample_rate), sample_rate, subtype="PCM_16")
    return path
