"""Ghost's voice from ElevenLabs, with Kokoro beside it (proposed ADR 0026).

With ``GHOST_TTS_ENGINE=elevenlabs`` a session speaks through ElevenLabs Flash v2.5 over
the multi-context websocket: Pipecat's ``ElevenLabsTTSService`` with Ghost's spoken-text
filter and clause aggregation, 24 kHz PCM like Kokoro. Only the reply text Ghost is about
to say goes there; the microphone never does, and STT stays local. The key is
``ELEVENLABS_API_KEY``, sent as the ``xi-api-key`` header and nowhere else.

Kokoro stays loaded, and Ghost never goes silent for the cloud voice's sake:

- no key or no voice id when the session is built: the session is plain Kokoro, logged
  once per process;
- the socket fails to connect, drops, errors, or stops returning audio: ``ElevenLabsTTS``
  writes itself off for the rest of the session at the first sign of it (no retry loop,
  which would be silence), and ``TTSFailover`` switches the slot to Kokoro and hands it
  the words of the reply in flight that were not heard yet (``Unheard``).

A line's voice (ADR 0014) reaches the TTS as Kokoro settings (a Kokoro voice name and a
speed). ``ElevenLabsTTS`` never sends that name to ElevenLabs: it speaks the line's
ElevenLabs voice from ``GHOST_ELEVENLABS_VOICES`` (by line slug) or the one voice in
``GHOST_ELEVENLABS_VOICE``. A per-line field in the profile and the Lines screen is a
follow-up.
"""

from __future__ import annotations

from collections import deque
from collections.abc import Awaitable, Callable
from itertools import islice
from typing import Any

from loguru import logger
from pipecat.frames.frames import (
    AggregatedTextFrame,
    EagerTranscriptionFrame,
    ErrorFrame,
    Frame,
    InterimTranscriptionFrame,
    InterruptionFrame,
    LLMFullResponseEndFrame,
    LLMFullResponseStartFrame,
    LLMTextFrame,
    ManuallySwitchServiceFrame,
    TextFrame,
    TranscriptionFrame,
    TTSAudioRawFrame,
    TTSSpeakFrame,
    TTSStartedFrame,
    TTSStoppedFrame,
    TTSTextFrame,
    TTSUpdateSettingsFrame,
)
from pipecat.pipeline.service_switcher import ServiceSwitcher, ServiceSwitcherStrategyFailover
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor
from pipecat.services.elevenlabs.tts import ElevenLabsTTSService, ElevenLabsTTSSettings
from pipecat.services.settings import TTSSettings
from pipecat.services.tts_service import TTSService
from pipecat.transcriptions.language import Language
from pipecat.utils.text.base_text_aggregator import AggregationType
from pipecat.utils.types import is_given

from ghost_voice.clauses import ClauseTextAggregator
from ghost_voice.config import VoiceSettings
from ghost_voice.kokoro import KokoroTTS
from ghost_voice.spoken import SpokenTextFilter

SAMPLE_RATE = 24000  # Kokoro's rate; the output transport runs at it (pipeline.TTS_SAMPLE_RATE)
DEFAULT_URL = "wss://api.elevenlabs.io"
# The handshake is about 0.1-0.4 s normally. Past this the network is not there, and the
# session's setup (and so its greeting) should not wait on it: Kokoro speaks instead.
CONNECT_TIMEOUT_SECS = 3.0
# ElevenLabs takes 0.7-1.2; Kokoro's line speeds (0.5-2.0) are clamped into that.
SPEED_RANGE = (0.7, 1.2)

OnWrittenOff = Callable[[str], Awaitable[None]]


def line_voices(raw: str) -> tuple[dict[str, str], list[str]]:
    """``GHOST_ELEVENLABS_VOICES`` ("accounting=<voice id>,research=<voice id>") as a map by
    line slug, and the entries that could not be read."""
    voices: dict[str, str] = {}
    bad: list[str] = []
    for entry in raw.split(","):
        entry = entry.strip()
        if not entry:
            continue
        slug, sep, voice = (s.strip() for s in entry.partition("="))
        if not sep or not slug or not voice:
            bad.append(entry)
            continue
        voices[slug] = voice
    return voices, bad


def tts_problems(s: VoiceSettings) -> list[str]:
    """What keeps the ElevenLabs voice from being used, without naming any value."""
    if s.tts_engine != "elevenlabs":
        return []
    problems = []
    if not s.elevenlabs_api_key:
        problems.append("GHOST_TTS_ENGINE=elevenlabs but ELEVENLABS_API_KEY is not set")
    if not s.elevenlabs_voice:
        problems.append("GHOST_TTS_ENGINE=elevenlabs but GHOST_ELEVENLABS_VOICE is not set")
    return problems


_warned: set[str] = set()


def _warn_once(message: str) -> None:
    if message not in _warned:
        _warned.add(message)
        logger.warning(message)


def describe_tts(s: VoiceSettings) -> str:
    """One line for ``ghostd check`` and the daemon's start-up log."""
    kokoro = f"Kokoro {s.tts_voice}"
    if s.tts_engine != "elevenlabs":
        return kokoro
    problems = tts_problems(s)
    if problems:
        return f"{kokoro} ({'; '.join(problems)})"
    voices, _ = line_voices(s.elevenlabs_voices)
    per_line = f", {len(voices)} line(s) with their own" if voices else ""
    return (
        f"ElevenLabs {s.elevenlabs_model}, voice {s.elevenlabs_voice}{per_line}; "
        f"{kokoro} as the fallback"
    )


def session_tts(
    s: VoiceSettings,
    kokoro: KokoroTTS,
    *,
    clause_min_words: int | None,
    line: Callable[[], str | None] | None = None,
) -> FrameProcessor:
    """The session's TTS processor: a fresh Kokoro fork, or ElevenLabs with that fork as its
    fallback. ``line`` names the active line, for its ElevenLabs voice. What is missing is
    logged once per process, not once per session."""
    local = kokoro.fork(clause_min_words=clause_min_words)
    if s.tts_engine != "elevenlabs":
        return local
    voices, bad = line_voices(s.elevenlabs_voices)
    if bad:  # counted, not echoed: whatever was pasted there stays out of the log
        _warn_once(f"tts: {len(bad)} GHOST_ELEVENLABS_VOICES entries not read (want line=id)")
    problems = tts_problems(s)
    for p in problems:
        _warn_once(f"tts: {p}; Kokoro speaks instead")
    if problems:
        return local
    cloud = ElevenLabsTTS(
        api_key=s.elevenlabs_api_key,
        voice=s.elevenlabs_voice,
        model=s.elevenlabs_model,
        voices=voices,
        line=line,
        clause_min_words=clause_min_words,
    )
    return TTSFailover(cloud, local)


def _cloud_speed(speed: float) -> float | None:
    """A line's speed for ElevenLabs: None (its default) at 1.0, else clamped."""
    if abs(speed - 1.0) < 1e-6:
        return None
    return min(SPEED_RANGE[1], max(SPEED_RANGE[0], speed))


class ElevenLabsTTS(ElevenLabsTTSService):
    """``ElevenLabsTTSService`` the way Ghost uses it.

    Speech, not Markdown (``SpokenTextFilter``), released a clause at a time
    (``ClauseTextAggregator``; Pipecat then turns ElevenLabs' auto mode on, as it does for
    sentence aggregation). Built per session; there is no model to share.

    Any error writes it off for the rest of the session: Pipecat would otherwise retry a
    dropped socket for up to half a minute, or quietly skip text it cannot send, and the
    operator would hear nothing. Once written off it says so through ``on_written_off``
    (``TTSFailover`` takes over), stops pausing for playback that will never come, ignores
    settings updates, and lets no speech frame out, so a late audio chunk or a held
    response end cannot cut across Kokoro.
    """

    # What a written-off service must not emit downstream.
    _SPEECH = (
        TTSAudioRawFrame,
        TTSStartedFrame,
        TTSStoppedFrame,
        AggregatedTextFrame,  # and TTSTextFrame
        LLMFullResponseStartFrame,
        LLMFullResponseEndFrame,
    )

    def __init__(
        self,
        *,
        api_key: str,
        voice: str,
        model: str = "eleven_flash_v2_5",
        voices: dict[str, str] | None = None,
        line: Callable[[], str | None] | None = None,
        clause_min_words: int | None = None,
        url: str = DEFAULT_URL,
        connect_timeout: float = CONNECT_TIMEOUT_SECS,
    ):
        """``voice`` is the ElevenLabs voice id for every line; ``voices`` overrides it by
        line slug, with ``line`` naming the active line. ``clause_min_words``: release text
        at a clause boundary once this many words are pending; None: whole sentences."""
        voices = dict(voices or {})
        start = (voices.get(line() or "") if line else None) or voice
        super().__init__(
            api_key=api_key,
            url=url,
            sample_rate=SAMPLE_RATE,
            settings=ElevenLabsTTSSettings(voice=start, model=model, language=Language.EN),
            text_filters=[SpokenTextFilter()],  # speech, not Markdown (ghost_voice.spoken)
            reconnect_on_error=False,  # a dropped socket is a failure, reported at once
        )
        self._default_voice = voice
        self._line_voices = voices
        self._line = line
        self._connect_timeout = connect_timeout
        self._written_off = False
        self.failure = ""  # the first error, with the key scrubbed
        self.on_written_off: OnWrittenOff | None = None
        if clause_min_words is not None:
            self._text_aggregator = ClauseTextAggregator(
                min_words=clause_min_words,
                aggregation_type=AggregationType.SENTENCE,
                language=self._text_aggregator.language,
            )

    @property
    def written_off(self) -> bool:
        return self._written_off

    def voice_for(self, slug: str | None) -> str:
        """The ElevenLabs voice a line speaks in."""
        return self._line_voices.get(slug or "") or self._default_voice

    def _scrub(self, text: str) -> str:
        return text.replace(self._api_key, "[ELEVENLABS_API_KEY]") if self._api_key else text

    async def _websocket_connect(self, uri: str, **kwargs: Any):
        kwargs.setdefault("open_timeout", self._connect_timeout)
        return await super()._websocket_connect(uri, **kwargs)

    # --- a line's voice (ADR 0014) ------------------------------------------------------

    def _line_delta(self, delta: TTSSettings) -> ElevenLabsTTSSettings:
        """Kokoro settings for a line, as ElevenLabs settings: the line's ElevenLabs voice,
        never the Kokoro name; the speed clamped to what ElevenLabs takes."""
        out = ElevenLabsTTSSettings()
        if is_given(delta.voice):
            out.voice = self.voice_for(self._line() if self._line else None)
        speed = getattr(delta, "speed", None)
        if is_given(speed) and isinstance(speed, int | float):
            out.speed = _cloud_speed(float(speed))
        if is_given(delta.language):
            out.language = delta.language
        return out

    async def _update_settings(self, delta: TTSSettings) -> dict[str, Any]:
        if self._written_off:
            return {}  # nothing is spoken through it any more; don't reconnect for it
        if not isinstance(delta, ElevenLabsTTSSettings):
            delta = self._line_delta(delta)
        return await super()._update_settings(delta)

    # --- failure --------------------------------------------------------------------------

    async def push_error_frame(self, error: ErrorFrame, force_treat_as_permanent: bool = False):
        error.error = self._scrub(error.error)
        if not self.failure:
            self.failure = error.error
        await super().push_error_frame(error, force_treat_as_permanent=True)

    async def set_usable(self, is_usable: bool):
        if is_usable and self._written_off:
            return  # a settings change does not bring it back this session
        if not is_usable and not self._written_off:
            self._written_off = True
            # Its contexts will get no more audio: end them now rather than after the
            # 3 s idle timeout each, which would hold up the session's ending.
            for context_id in self.get_audio_contexts():
                await self.remove_audio_context(context_id)
            await self.resume_processing_frames()  # no BotStoppedSpeaking is coming for it
            if self.on_written_off:
                await self.on_written_off(self.failure or "unusable")
        await super().set_usable(is_usable)

    async def _maybe_pause_frame_processing(self):
        if not self._written_off:
            await super()._maybe_pause_frame_processing()

    async def push_frame(self, frame: Frame, direction: FrameDirection = FrameDirection.DOWNSTREAM):
        if self._written_off and direction == FrameDirection.DOWNSTREAM:
            if isinstance(frame, self._SPEECH):
                return
        await super().push_frame(frame, direction)


def _key(word: str) -> str:
    return "".join(c for c in word.lower() if c.isalnum())


class Unheard:
    """The words handed to the cloud voice whose audio has not come back yet.

    Text is added as it is routed to ElevenLabs (LLM tokens, a whole ``TTSSpeakFrame``);
    each word ElevenLabs aligns with returned audio (a ``TTSTextFrame``) is struck off. A
    word that matches nothing is skipped; the next match a few words on strikes off what
    lies before it, so a Markdown marker or a link the spoken-text filter dropped doesn't
    leave the two out of step for long. What is left when the socket fails is what Kokoro
    says.
    """

    LOOKAHEAD = 8

    def __init__(self) -> None:
        self._words: deque[str] = deque()
        self._partial = ""  # a word still arriving token by token

    def add(self, text: str) -> None:
        text = self._partial + text
        words = text.split()
        self._partial = words.pop() if words and not text[-1].isspace() else ""
        self._words.extend(words)

    def end(self) -> None:
        """The text so far is complete: its last word is whole."""
        if self._partial:
            self._words.append(self._partial)
            self._partial = ""

    def heard(self, text: str) -> None:
        for word in text.split():
            key = _key(word)
            if not key:
                continue
            for i, pending in enumerate(islice(self._words, self.LOOKAHEAD)):
                if _key(pending) == key:
                    for _ in range(i + 1):
                        self._words.popleft()
                    break
            else:
                if self._partial and _key(self._partial) == key:
                    self._partial = ""

    @property
    def partial(self) -> bool:
        return bool(self._partial)

    def text(self) -> str:
        return " ".join([*self._words, self._partial]).strip()

    def clear(self) -> None:
        self._words.clear()
        self._partial = ""


class TTSFailover(ServiceSwitcher[ServiceSwitcherStrategyFailover]):
    """The TTS slot with ElevenLabs active and Kokoro behind it.

    A line's voice update (an unaddressed ``TTSUpdateSettingsFrame``) reaches both, so
    Kokoro already speaks in the active line's voice when it takes over. When
    ``ElevenLabsTTS`` writes itself off, the switch happens at once (not when its error
    frame gets here), the error stays inside this switcher (the session keeps going), and
    Kokoro is first handed what of the reply in flight was not heard: as more of the same
    response when the model is still streaming it, else as a line of its own.
    """

    def __init__(self, primary: ElevenLabsTTS, fallback: TTSService):
        super().__init__([primary, fallback], ServiceSwitcherStrategyFailover)
        self._primary = primary
        self._fallback = fallback
        self._unheard = Unheard()
        self._open = False  # a response is streaming into ElevenLabs
        self._end_owed = False  # ElevenLabs holds a response end it hasn't sent on
        primary.on_written_off = self._take_over

    @property
    def primary(self) -> ElevenLabsTTS:
        return self._primary

    @property
    def fallback(self) -> TTSService:
        return self._fallback

    def _cloud(self) -> bool:
        return self.strategy.active_service is self._primary and not self._primary.written_off

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        if direction == FrameDirection.DOWNSTREAM:
            if isinstance(frame, TTSUpdateSettingsFrame) and frame.service is None:
                frame.reach_inactive_services = True  # a line's voice is for both engines
            if self._cloud():
                self._routed(frame)
        await super().process_frame(frame, direction)

    def _routed(self, frame: Frame) -> None:
        """Keep the text going into ElevenLabs, as TTSService decides what it speaks."""
        if isinstance(frame, InterruptionFrame):
            self._unheard.clear()  # cut off on purpose: nothing to make up for
            self._open = self._end_owed = False
        elif isinstance(frame, TTSSpeakFrame):
            self._unheard.add(frame.text)
            self._unheard.end()
        elif isinstance(frame, LLMFullResponseStartFrame) and not frame.skip_tts:
            self._open = True
        elif isinstance(frame, LLMFullResponseEndFrame) and not frame.skip_tts:
            self._unheard.end()
            self._open = False
            self._end_owed = True
        elif (
            isinstance(frame, TextFrame)
            and not frame.skip_tts
            and not isinstance(
                frame, (TranscriptionFrame, InterimTranscriptionFrame, EagerTranscriptionFrame)
            )
        ):
            self._unheard.add(frame.text)
            if isinstance(frame, AggregatedTextFrame):
                self._unheard.end()

    async def push_frame(self, frame: Frame, direction: FrameDirection = FrameDirection.DOWNSTREAM):
        if direction == FrameDirection.DOWNSTREAM and self._cloud():
            if isinstance(frame, TTSTextFrame):
                self._unheard.heard(frame.text)
            elif isinstance(frame, LLMFullResponseEndFrame):
                self._end_owed = False
        await super().push_frame(frame, direction)

    async def _take_over(self, reason: str) -> None:
        if self.strategy.active_service is not self._primary:
            return
        logger.warning(
            f"tts: ElevenLabs failed ({reason}); Kokoro speaks for the rest of this session"
        )
        frames: list[Frame] = []
        text = self._unheard.text()
        if text and self._open:
            frames.append(LLMTextFrame(text if self._unheard.partial else text + " "))
        elif text:
            frames.append(TTSSpeakFrame(text))
        if not self._open and self._end_owed:
            frames.append(LLMFullResponseEndFrame())
        self._unheard.clear()
        self._end_owed = False
        # Into Kokoro before the switch, so they come ahead of anything routed after it.
        for f in frames:
            await self._fallback.queue_frame(f)
        await self.strategy.handle_frame(
            ManuallySwitchServiceFrame(service=self._fallback), FrameDirection.DOWNSTREAM
        )
