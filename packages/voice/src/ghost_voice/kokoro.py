"""Kokoro TTS whose loaded model is shared by every session's processor."""

from __future__ import annotations

import copy
from pathlib import Path

from kokoro_onnx import Kokoro
from pipecat.audio.utils import create_stream_resampler
from pipecat.services.kokoro.tts import KOKORO_CACHE_DIR, KokoroTTSService, _ensure_model_files
from pipecat.services.tts_service import TTSService
from pipecat.transcriptions.language import Language
from pipecat.utils.text.base_text_aggregator import AggregationType
from pipecat.utils.types import require_given

from ghost_voice.clauses import ClauseTextAggregator

_UNSET = object()


class KokoroTTS(KokoroTTSService):
    """``KokoroTTSService`` built over an already-loaded ``Kokoro``.

    The daemon loads the model once with ``load()`` and keeps that instance as a
    template; each pipeline uses ``fork()``. A Pipecat processor that sees a
    ``CancelFrame`` stays cancelling for good, so sharing one processor across
    sessions would leave every session after a cancelled one mute.
    """

    def __init__(
        self,
        *,
        kokoro: Kokoro,
        settings: KokoroTTSService.Settings | None = None,
        clause_min_words: int | None = None,
    ):
        """``clause_min_words``: release text to Kokoro at a clause boundary once this
        many words are pending (ghost_voice.clauses); None: whole sentences."""
        # Mirrors KokoroTTSService.__init__ minus the model load, which it does inline.
        defaults = self.Settings(model=None, voice=None, language=Language.EN, speed=1.0)
        if settings is not None:
            defaults.apply_update(settings)
        TTSService.__init__(self, push_start_frame=True, push_stop_frames=True, settings=defaults)
        self._kokoro = kokoro
        self._fork_settings = copy.deepcopy(settings)
        voice = require_given(self._settings.voice, "Kokoro TTS voice")
        if voice not in kokoro.voices:
            raise ValueError(f"Kokoro TTS voice '{voice}' is not in the voices file")
        self._resampler = create_stream_resampler()
        self._clause_min_words = clause_min_words
        if clause_min_words is not None:
            self._text_aggregator = ClauseTextAggregator(
                min_words=clause_min_words,
                aggregation_type=AggregationType.SENTENCE,
                language=self._text_aggregator.language,
            )

    @classmethod
    def load(
        cls,
        *,
        model_path: Path | None = None,
        voices_path: Path | None = None,
        settings: KokoroTTSService.Settings | None = None,
    ) -> KokoroTTS:
        """Load the model (about 300 MB), downloading it first when no path is given."""
        model = model_path or KOKORO_CACHE_DIR / "kokoro-v1.0.onnx"
        voices = voices_path or KOKORO_CACHE_DIR / "voices-v1.0.bin"
        _ensure_model_files(model, voices)
        return cls(kokoro=Kokoro(str(model), str(voices)), settings=settings)

    @property
    def kokoro(self) -> Kokoro:
        """The loaded model, for voice listings and samples outside a pipeline."""
        return self._kokoro

    def fork(self, *, clause_min_words: int | None | object = _UNSET) -> KokoroTTS:
        """A fresh processor sharing this instance's loaded model, with the settings
        it was built with (not any voice a session later switched to)."""
        words = self._clause_min_words if clause_min_words is _UNSET else clause_min_words
        return KokoroTTS(
            kokoro=self._kokoro,
            settings=copy.deepcopy(self._fork_settings),
            clause_min_words=words,  # type: ignore[arg-type]
        )
