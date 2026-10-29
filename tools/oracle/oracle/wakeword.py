"""The trained wake detectors through livekit-wakeword, as ``ghost_voice.wakeword`` scored
them (ADR 0025). The Rust port is ``crates/ghost-infer/src/wake/``; the fixtures compare
each model's score per 80 ms frame and the first hit at the production thresholds.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

import numpy as np

from oracle.wake import WakeMatch, match_opening, match_wake

SAMPLE_RATE = 16000
STEP = 1280  # 80 ms: how often the detector scores
WINDOW = 32000  # 2 s: what each score covers

DETECTOR_PHRASE = "hey ghost"
ADDRESS_PHRASE = "ghost"  # what a ghost_*.onnx model hears: the name opening a request

# What follows the name when the operator addresses Ghost with a request.
OPENERS = (
    "i need you",
    "let's work on",
    "what is the status of",
    "what's the status of",
    "what did we leave off",
    "where did we leave off",
)
DEFAULT_MODEL_NAME = "hey_ghost.onnx"
ADDRESS_MODEL_GLOB = "ghost_*.onnx"  # Ghost addressed by name: one model, or one per opener


def model_meaning(stem: str) -> tuple[str, bool]:
    """A model file's stem (how livekit-wakeword names its score) -> the phrase the check
    and the first turn go by, and whether that phrase must open what was said."""
    if stem.startswith("ghost_"):
        return ADDRESS_PHRASE, True
    return DETECTOR_PHRASE, False


class WakeWordScorer:
    """The models, loaded once: ``score()`` takes the last 2 s and returns each model's
    score (0-1) by name. The audio features are computed once for all of them."""

    def __init__(self, model_paths: Sequence[Path], *, threads: int = 1) -> None:
        import onnxruntime as ort
        from livekit.wakeword import WakeWordModel

        so = ort.SessionOptions()
        so.intra_op_num_threads = threads
        so.inter_op_num_threads = 1
        self.paths = list(model_paths)
        self._model = WakeWordModel(models=self.paths, sess_options=so)

    def score(self, window: np.ndarray) -> dict[str, float]:
        """``window``: 2 s of 16 kHz int16 PCM."""
        return self._model.predict(window)


@dataclass(frozen=True)
class WakeModel:
    """One trained model and what its hit means."""

    name: str  # the model file's stem
    phrase: str
    threshold: float
    opening: bool = False  # the phrase opens what was said; the rest is the request
    verify: bool | None = None  # Parakeet checks this model's hits; None: the detector's rule

    @classmethod
    def known(cls, path: Path, threshold: float, *, verify: bool | None = None) -> WakeModel:
        phrase, opening = model_meaning(path.stem)
        return cls(
            name=path.stem, phrase=phrase, threshold=threshold, opening=opening, verify=verify
        )

    def match(self, text: str, fuzzy: float) -> WakeMatch | None:
        if self.opening:
            return match_opening(text, self.phrase, fuzzy=fuzzy, openers=OPENERS)
        return match_wake(text, (self.phrase,), fuzzy=fuzzy)
