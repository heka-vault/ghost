"""Parakeet TDT 0.6B v3 through onnx-asr, as ``ghost_voice.parakeet`` loaded and ran it.

The Rust port (``crates/ghost-infer/src/parakeet.rs``) must return the same transcript for
the same samples; ``scripts/fixtures/record.py`` records this wrapper's output as the
fixture. The session options are the daemon's: ``intra_op_num_threads`` 4,
``inter_op_num_threads`` 1, the int8 quantization.
"""

from __future__ import annotations

import numpy as np

MODEL = "nemo-parakeet-tdt-0.6b-v3"
SAMPLE_RATE = 16000


class ParakeetSTT:
    """The model, loaded once; ``recognize`` takes float32 16 kHz PCM."""

    def __init__(
        self, *, model: str = MODEL, quantization: str | None = "int8", threads: int = 4
    ) -> None:
        self._model_name = model
        self._quantization = quantization
        self._threads = threads
        self.model = None

    def load(self) -> None:
        if self.model is not None:
            return
        import onnx_asr
        import onnxruntime as ort

        so = ort.SessionOptions()
        so.intra_op_num_threads = self._threads
        so.inter_op_num_threads = 1
        self.model = onnx_asr.load_model(
            self._model_name, quantization=self._quantization, sess_options=so
        )

    def recognize(self, pcm: np.ndarray, *, rate: int = SAMPLE_RATE) -> str:
        """The transcript of ``pcm`` (float32, -1..1), stripped; "" for nothing heard."""
        if self.model is None:
            self.load()
        assert self.model is not None
        return (self.model.recognize(pcm, sample_rate=rate) or "").strip()
