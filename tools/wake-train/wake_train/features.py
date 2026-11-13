"""The detector's frozen front end, as ``ghost_infer::wake::WakeScorer`` runs it.

Every 80 ms the daemon scores the last 2 s: ``melspectrogram.onnx`` over the window,
``mel / 10 + 2``, then ``embedding_model.onnx`` over 76 mel frames every 8, which for 2 s is
exactly 16 embeddings of 96, and the classifier over those ``[16, 96]``. Each step moves the
window by 1,280 samples, one embedding's stride, so a window's 16 embeddings are the last 15
of the window before it and one new one. ``stream_embeddings`` therefore computes one
embedding per step (the window's last) and ``windows`` stacks the last 16; the scores of the
shipped classifier over those stacks match the scorer's own (``check_against_scorer``).
"""

from __future__ import annotations

import os
from pathlib import Path

import numpy as np

RATE = 16000
STEP = 1280
WINDOW = 32000
EMB_WINDOW = 76
EMB_STRIDE = 8
N_EMB = 16
EMB_DIM = 96


def models_dir() -> Path:
    env = os.environ.get("GHOST_MODELS_DIR")
    return Path(env) if env else Path.home() / ".cache" / "ghost" / "models"


class FrontEnd:
    """The two frozen graphs, one thread each unless told otherwise."""

    def __init__(self, models: Path | None = None, threads: int = 1) -> None:
        import onnxruntime as ort

        d = models or models_dir()
        so = ort.SessionOptions()
        so.intra_op_num_threads = threads
        so.inter_op_num_threads = 1
        so.add_session_config_entry("mlas.disable_kleidiai", "1")
        self.mel = ort.InferenceSession(str(d / "melspectrogram.onnx"), so)
        self.emb = ort.InferenceSession(str(d / "embedding_model.onnx"), so)
        self.mel_in = self.mel.get_inputs()[0].name
        self.emb_in = self.emb.get_inputs()[0].name

    def _mel(self, window_f32: np.ndarray) -> np.ndarray:
        out = self.mel.run(None, {self.mel_in: window_f32[None, :]})[0]
        return np.squeeze(out).reshape(-1, 32) / 10.0 + 2.0

    def last_embedding(self, window_i16: np.ndarray) -> np.ndarray:
        """The 16th embedding of a 2 s window: the one this step adds."""
        mel = self._mel(window_i16.astype(np.float32))
        n = (mel.shape[0] - EMB_WINDOW) // EMB_STRIDE
        start = n * EMB_STRIDE
        x = mel[start : start + EMB_WINDOW][None, :, :, None].astype(np.float32)
        return self.emb.run(None, {self.emb_in: x})[0].reshape(EMB_DIM)

    def all_embeddings(self, window_i16: np.ndarray) -> np.ndarray:
        """All 16 embeddings of one window, as the scorer computes them."""
        mel = self._mel(window_i16.astype(np.float32))
        out = []
        start = 0
        while start + EMB_WINDOW <= mel.shape[0]:
            x = mel[start : start + EMB_WINDOW][None, :, :, None].astype(np.float32)
            out.append(self.emb.run(None, {self.emb_in: x})[0].reshape(EMB_DIM))
            start += EMB_STRIDE
        return np.stack(out[-N_EMB:])

    def stream_embeddings(self, pcm: np.ndarray, pad: bool = True) -> np.ndarray:
        """One embedding per 80 ms step over ``pcm`` (int16, 16 kHz). With ``pad`` the stream
        starts after 2 s of silence, as the replay does, so step ``i`` ends ``i * STEP``
        samples into ``pcm``."""
        x = np.concatenate([np.zeros(WINDOW, np.int16), pcm]) if pad else pcm
        ends = range(WINDOW, len(x) + 1, STEP)
        return np.stack([self.last_embedding(x[e - WINDOW : e]) for e in ends]).astype(np.float32)


def windows(emb: np.ndarray, idx: np.ndarray | None = None) -> np.ndarray:
    """``[n, 16, 96]``: for each step, its embedding and the 15 before it. Steps before the
    16th repeat the first (they are silence in a padded stream)."""
    pad = np.concatenate([np.repeat(emb[:1], N_EMB - 1, axis=0), emb])
    idx = np.arange(len(emb)) if idx is None else idx
    return np.stack([pad[i : i + N_EMB] for i in idx])


class Classifier:
    """A ``[n, 16, 96] -> [n, 1]`` detector file, scored in batches."""

    def __init__(self, path: Path) -> None:
        import onnxruntime as ort

        so = ort.SessionOptions()
        so.intra_op_num_threads = 2
        self.s = ort.InferenceSession(str(path), so)
        self.inp = self.s.get_inputs()[0].name

    def __call__(self, w: np.ndarray, batch: int = 512) -> np.ndarray:
        out = []
        for i in range(0, len(w), batch):
            chunk = w[i : i + batch].astype(np.float32)
            # some exported graphs take a fixed batch of one
            try:
                out.append(self.s.run(None, {self.inp: chunk})[0].reshape(-1))
            except Exception:
                out.append(
                    np.array([self.s.run(None, {self.inp: c[None]})[0].item() for c in chunk])
                )
        return np.concatenate(out) if out else np.zeros(0, np.float32)
