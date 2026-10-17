"""Build one conversation session."""

from __future__ import annotations

import asyncio
import uuid
from collections.abc import Awaitable, Callable, Sequence
from dataclasses import dataclass
from typing import Any

from anthropic import AsyncAnthropic
from ghost_core import Events, LoadedAbility, Slot, WriteGate
from loguru import logger
from pipecat.audio.mixers.soundfile_mixer import SoundfileMixer
from pipecat.audio.turn.smart_turn.local_smart_turn_v3 import LocalSmartTurnAnalyzerV3
from pipecat.audio.vad.silero import SileroVADAnalyzer
from pipecat.audio.vad.vad_analyzer import VADParams
from pipecat.frames.frames import LLMMessagesAppendFrame, LLMRunFrame
from pipecat.pipeline.pipeline import Pipeline
from pipecat.pipeline.worker import PipelineParams, PipelineWorker, ProcessorUnusablePolicy
from pipecat.processors.aggregators.llm_context import LLMContext
from pipecat.processors.aggregators.llm_response_universal import (
    LLMContextAggregatorPair,
    LLMUserAggregatorParams,
)
from pipecat.services.anthropic.llm import AnthropicLLMService
from pipecat.services.kokoro.tts import KokoroTTSService
from pipecat.transports.base_transport import BaseTransport
from pipecat.turns.user_stop.turn_analyzer_user_turn_stop_strategy import (
    TurnAnalyzerUserTurnStopStrategy,
)
from pipecat.turns.user_turn_strategies import UserTurnStrategies
from pipecat.workers.runner import WorkerRunner

from ghost_voice.config import VoiceSettings
from ghost_voice.confirmation import ConfirmationProcessor
from ghost_voice.intents import QuickIntents
from ghost_voice.intercept import InterceptorProcessor
from ghost_voice.parakeet import ParakeetSTTService
from ghost_voice.prompt import build_system_prompt
from ghost_voice.sounds import ensure_thinking_sound
from ghost_voice.thinking import ThinkingSoundProcessor, ThinkingStartFrame
from ghost_voice.tools import register_tools
from ghost_voice.transport import SoundDeviceTransport, SoundDeviceTransportParams
from ghost_voice.turns import BargeInUserTurnStartStrategy

TTS_SAMPLE_RATE = 24000  # Kokoro's native rate; the output transport runs at it


@dataclass
class Models:
    """Loaded once by the daemon and shared across sessions (about 3 GB).

    ``stt`` is a template: pipelines use ``stt.fork()`` so each gets its own
    processor over the one loaded model."""

    stt: ParakeetSTTService
    tts: KokoroTTSService
    turn: LocalSmartTurnAnalyzerV3

    @classmethod
    def load(cls, s: VoiceSettings) -> Models:
        stt = ParakeetSTTService(
            model=s.stt_model, quantization=s.stt_quantization, threads=s.stt_threads
        )
        stt.load()
        # Prefer files already in Ghost's cache; otherwise Pipecat downloads them.
        model_path = s.tts_model_path or s.cache_dir / "kokoro" / "kokoro-v1.0.onnx"
        voices_path = s.tts_voices_path or s.cache_dir / "kokoro" / "voices-v1.0.bin"
        tts = KokoroTTSService(
            model_path=str(model_path) if model_path.exists() else None,
            voices_path=str(voices_path) if voices_path.exists() else None,
            settings=KokoroTTSService.Settings(voice=s.tts_voice, speed=s.tts_speed),
        )
        turn = LocalSmartTurnAnalyzerV3(cpu_count=s.smart_turn_threads)
        return cls(stt=stt, tts=tts, turn=turn)


@dataclass
class GhostSession:
    session_id: str
    worker: PipelineWorker
    runner: WorkerRunner
    _ended: asyncio.Event

    async def run(self) -> None:
        await self.runner.run()

    async def end(self) -> None:
        if not self._ended.is_set():
            self._ended.set()
            await self.runner.stop_when_done()

    async def cancel(self) -> None:
        self._ended.set()
        await self.runner.cancel()


def last_assistant_text(context: LLMContext) -> str:
    """Ghost's most recent reply, as plain text, for the confirmation classifier."""
    for m in reversed(context.get_messages()):
        if m.get("role") != "assistant":
            continue
        content = m.get("content")
        if isinstance(content, str):
            return content.strip()
        if isinstance(content, list):
            return " ".join(
                str(b.get("text", "")) for b in content if isinstance(b, dict) and "text" in b
            ).strip()
    return ""


def assemble(core: list[tuple[Slot | None, Any]], slots: dict[Slot, list[Any]]) -> list[Any]:
    """Splice ability processors into the fixed pipeline. ``core`` is the ordered list of
    Ghost's own processors, each tagged with the slot that comes right after it."""
    out: list[Any] = []
    for slot, proc in core:
        out.append(proc)
        if slot is not None:
            out.extend(slots.get(slot, []))
    return out


def build_session(
    settings: VoiceSettings,
    *,
    models: Models,
    abilities: Sequence[LoadedAbility],
    gate: WriteGate,
    events: Events | None = None,
    transport: BaseTransport | None = None,
    greeting: str | None = "Ghost. What do you need?",
    first_utterance: str | None = None,
    on_sleep: Callable[[], Awaitable[None]] | None = None,
) -> GhostSession:
    """Build one conversation. ``first_utterance`` is what the operator said right
    after the wake phrase; when given it is the first user turn and there is no
    greeting."""
    session_id = uuid.uuid4().hex
    ended = asyncio.Event()

    mixer = None
    if settings.thinking_sound:
        sound = ensure_thinking_sound(settings.cache_dir, TTS_SAMPLE_RATE)
        mixer = SoundfileMixer(
            sound_files={"thinking": str(sound)},
            default_sound="thinking",
            volume=settings.thinking_volume,
            mixing=False,
        )
    transport = transport or SoundDeviceTransport(
        SoundDeviceTransportParams(
            audio_in_enabled=True,
            audio_out_enabled=True,
            audio_out_sample_rate=TTS_SAMPLE_RATE,
            audio_out_mixer=mixer,
            input_device=settings.input_device,
            output_device=settings.output_device,
        )
    )

    llm = AnthropicLLMService(
        api_key=settings.anthropic_auth_token,
        client=AsyncAnthropic(
            api_key=settings.anthropic_auth_token, base_url=settings.anthropic_base_url
        ),
        settings=AnthropicLLMService.Settings(
            model=settings.llm_model,
            system_instruction=build_system_prompt(
                [a.contributions.prompt for a in abilities if a.contributions.prompt]
            ),
            max_tokens=settings.llm_max_tokens,
            enable_prompt_caching=True,
            thinking=AnthropicLLMService.ThinkingConfig(type=settings.llm_thinking),
        ),
    )

    session: GhostSession | None = None

    async def sleep() -> None:
        logger.info("go_to_sleep")
        if on_sleep:
            await on_sleep()
        if session:
            await session.end()

    tools = register_tools(
        llm, abilities=abilities, gate=gate, session_id=session_id, on_sleep=sleep, events=events
    )
    context = LLMContext(tools=tools)

    user_agg, assistant_agg = LLMContextAggregatorPair(
        context,
        user_params=LLMUserAggregatorParams(
            vad_analyzer=SileroVADAnalyzer(
                params=VADParams(
                    confidence=settings.vad_confidence,
                    start_secs=settings.vad_start_secs,
                    stop_secs=settings.vad_stop_secs,
                    min_volume=settings.vad_min_volume,
                )
            ),
            user_turn_strategies=UserTurnStrategies(
                start=[BargeInUserTurnStartStrategy(min_words=settings.barge_in_min_words)],
                stop=[TurnAnalyzerUserTurnStopStrategy(turn_analyzer=models.turn)],
            ),
            user_idle_timeout=settings.idle_timeout_secs,
        ),
    )

    @user_agg.event_handler("on_user_turn_idle")
    async def on_idle(_agg) -> None:
        logger.info(f"idle for {settings.idle_timeout_secs}s")
        await sleep()

    interceptors: list[Any] = [QuickIntents()] if settings.quick_intents else []
    for a in abilities:
        interceptors.extend(a.contributions.interceptors)
    slots: dict[Slot, list[Any]] = {}
    for a in abilities:
        for slot, factories in a.contributions.processors.items():
            slots.setdefault(Slot(slot), []).extend(f() for f in factories)

    def last_assistant() -> str:
        return last_assistant_text(context)

    pipeline = Pipeline(
        assemble(
            [
                (Slot.AUDIO_IN, transport.input()),
                (None, models.stt.fork()),
                (
                    None,
                    InterceptorProcessor(
                        interceptors,
                        gate=gate,
                        session_id=session_id,
                        on_sleep=sleep,
                        last_assistant=last_assistant,
                    ),
                ),
                (
                    Slot.AFTER_STT,
                    ConfirmationProcessor(
                        gate, session_id, last_assistant=last_assistant, events=events
                    ),
                ),
                (Slot.BEFORE_LLM, user_agg),
                (Slot.AFTER_LLM, llm),
                (Slot.BEFORE_TTS, ThinkingSoundProcessor(delay_secs=settings.thinking_delay_secs)),
                (Slot.AUDIO_OUT, models.tts),
                (None, transport.output()),
                (None, assistant_agg),
            ],
            slots,
        )
    )
    worker = PipelineWorker(
        pipeline,
        params=PipelineParams(audio_in_sample_rate=16000, enable_metrics=True),
        processor_unusable_policy=ProcessorUnusablePolicy.END,
    )
    runner = WorkerRunner(handle_sigint=False)
    session = GhostSession(session_id=session_id, worker=worker, runner=runner, _ended=ended)

    async def start() -> None:
        await runner.add_workers(worker)
        if first_utterance:
            await worker.queue_frames(
                [
                    ThinkingStartFrame(),
                    LLMMessagesAppendFrame(
                        messages=[{"role": "user", "content": first_utterance}], run_llm=True
                    ),
                ]
            )
        elif greeting:
            from pipecat.frames.frames import TTSSpeakFrame

            await worker.queue_frames([TTSSpeakFrame(greeting)])
        else:
            await worker.queue_frames([LLMRunFrame()])

    session.start = start  # type: ignore[attr-defined]
    return session
