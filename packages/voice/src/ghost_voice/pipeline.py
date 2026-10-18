"""Build one conversation session."""

from __future__ import annotations

import asyncio
import random
import uuid
from collections.abc import Awaitable, Callable, Sequence
from dataclasses import dataclass
from typing import Any

from anthropic import AsyncAnthropic
from ghost_core import (
    Ability,
    Contributions,
    Events,
    LoadedAbility,
    SimpleInstance,
    Slot,
    WriteGate,
)
from ghost_core.conversations import ABILITY as CONVERSATIONS
from ghost_core.conversations import GHOST, SUMMARY_TEMPLATE, Conversations
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
    LLMAssistantAggregatorParams,
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
from pipecat.utils.context.llm_context_summarization import LLMContextSummarizationConfig
from pipecat.workers.runner import WorkerRunner

from ghost_voice.config import VoiceSettings
from ghost_voice.confirmation import ConfirmationProcessor
from ghost_voice.conversation import ConversationRuntime
from ghost_voice.intents import QuickIntents
from ghost_voice.intercept import InterceptorProcessor
from ghost_voice.parakeet import ParakeetSTTService
from ghost_voice.prompt import build_system_prompt
from ghost_voice.sounds import ensure_thinking_sound
from ghost_voice.tap import AssistantTap, OnPhase, OnTurn, UserTap
from ghost_voice.thinking import ThinkingSoundCut, ThinkingSoundProcessor, ThinkingStartFrame
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
    conversation: ConversationRuntime | None = None

    async def run(self) -> None:
        await self.runner.run()

    async def end(self) -> None:
        if not self._ended.is_set():
            self._ended.set()
            await self.runner.stop_when_done()

    async def inject(self, frames: list[Any]) -> None:
        """Queue frames into the running pipeline (a typed turn, a system note)."""
        await self.worker.queue_frames(frames)

    async def say_text(self, text: str) -> None:
        """A typed user turn: goes through the LLM, the tools and the gate like speech."""
        if self.conversation:
            try:  # a transfer in flight: let its context land first
                await asyncio.wait_for(self.conversation.settled.wait(), 8.0)
            except TimeoutError:
                pass
            self.conversation.record("user", text)
        await self.inject(
            [
                ThinkingStartFrame(),
                LLMMessagesAppendFrame(messages=[{"role": "user", "content": text}], run_llm=True),
            ]
        )

    async def system_note(self, note: str) -> None:
        """Tell the model something happened outside the conversation and have it respond."""
        await self.inject(
            [LLMMessagesAppendFrame(messages=[{"role": "user", "content": note}], run_llm=True)]
        )

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


def conversations_ability(convs: Conversations) -> LoadedAbility:
    """Ghost's own conversation operations, registered like an ability so the gate,
    the policy file and the audit view treat them uniformly (ADR 0014)."""
    c = Contributions(read_tools=convs.read_tools(), operations=convs.operations())
    ab = Ability(
        CONVERSATIONS,
        "1",
        "Saved conversations: persona, voice, memory",
        setup=None,  # type: ignore[arg-type]
    )
    return LoadedAbility(ab, None, SimpleInstance(c), c)


GREETING_VARIED = "<varied>"
COMPACTION_PROMPT = """You are condensing the older part of a spoken conversation between an \
operator and their assistant into the assistant's memory of it.

Keep, as plain factual statements: decisions and agreements; facts, names and numbers the \
operator stated; what was asked for and whether it was done; open questions and action \
items; preferences about how the operator likes things done. Fold in any earlier memory \
that is still true. Drop greetings, small talk, and tool chatter. Do not invent anything. \
Write only the memory, no preamble."""

GREETINGS = (
    "Ghost. What do you need?",
    "Yeah?",
    "I'm here.",
    "What's up?",
    "Go ahead.",
    "Listening.",
    "Hey. What can I do?",
    "Ghost here.",
)


def build_session(
    settings: VoiceSettings,
    *,
    models: Models,
    abilities: Sequence[LoadedAbility],
    gate: WriteGate,
    events: Events | None = None,
    transport: BaseTransport | None = None,
    greeting: str | None = GREETING_VARIED,
    first_utterance: str | None = None,
    on_sleep: Callable[[], Awaitable[None]] | None = None,
    on_turn: OnTurn | None = None,
    on_phase: OnPhase | None = None,
    conversations: Conversations | None = None,
    start_in: str = GHOST,
    on_conversation: Callable[[str], Awaitable[None]] | None = None,
) -> GhostSession:
    """Build one session. ``first_utterance`` is what the operator said right after
    the wake phrase; when given it is the first user turn and there is no greeting.
    With ``conversations`` the session opens in ``start_in`` (normally ``ghost``),
    seeded from its memory, in its voice, and can be transferred (ADR 0014)."""
    session_id = uuid.uuid4().hex
    ended = asyncio.Event()
    abilities = list(abilities)
    runtime: ConversationRuntime | None = None
    fragments = [a.contributions.prompt for a in abilities if a.contributions.prompt]
    if conversations is not None:
        abilities.append(conversations_ability(conversations))

        async def switched(profile) -> None:
            if on_conversation:
                await on_conversation(profile.slug)

        runtime = ConversationRuntime(
            conversations,
            gate=gate,
            session_id=session_id,
            fragments=fragments,
            tts_settings=models.tts.Settings,
            llm_settings=AnthropicLLMService.Settings,
            sample_rate=TTS_SAMPLE_RATE,
            keep_messages=settings.compact_keep_messages,
            start=start_in,
            on_switch=switched,
        )

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
            system_instruction=(
                runtime.system_prompt() if runtime else build_system_prompt(fragments)
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
        llm,
        abilities=abilities,
        gate=gate,
        session_id=session_id,
        on_sleep=sleep,
        events=events,
        conversations=runtime,
        on_call=runtime.record_call if runtime else None,
    )
    context = LLMContext(messages=runtime.initial_messages() if runtime else None, tools=tools)

    assistant_params = LLMAssistantAggregatorParams()
    if runtime and settings.compact_trigger_tokens > 0:
        assistant_params = LLMAssistantAggregatorParams(
            enable_context_summarization=True,
            context_summarization_config=LLMContextSummarizationConfig(
                max_context_tokens=settings.compact_trigger_tokens,
                max_unsummarized_messages=None,
                target_context_tokens=settings.compact_summary_tokens,
                min_messages_after_summary=settings.compact_keep_messages,
                summary_message_template=SUMMARY_TEMPLATE,
                summarization_prompt=COMPACTION_PROMPT,
            ),
        )

    user_agg, assistant_agg = LLMContextAggregatorPair(
        context,
        assistant_params=assistant_params,
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

    if runtime:

        @assistant_agg.event_handler("on_summary_applied")
        async def on_summary(_agg, _summarizer, _event) -> None:
            await runtime.on_summary_applied(context.get_messages())

    interceptors: list[Any] = [QuickIntents()] if settings.quick_intents else []
    for a in abilities:
        interceptors.extend(a.contributions.interceptors)
    slots: dict[Slot, list[Any]] = {}
    for a in abilities:
        for slot, factories in a.contributions.processors.items():
            slots.setdefault(Slot(slot), []).extend(f() for f in factories)

    def last_assistant() -> str:
        return last_assistant_text(context)

    async def _noop_turn(role: str, text: str) -> None: ...

    async def _noop_phase(phase: str) -> None: ...

    outer_turn = on_turn or _noop_turn
    on_phase = on_phase or _noop_phase  # type: ignore[assignment]

    async def on_turn(role: str, text: str) -> None:  # type: ignore[no-redef]
        if runtime and role in ("user", "assistant"):
            runtime.record(role, text)
        await outer_turn(role, text)

    pipeline = Pipeline(
        assemble(
            [
                (Slot.AUDIO_IN, transport.input()),
                (None, models.stt.fork()),
                (None, UserTap(on_turn, on_phase)),
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
                (None, ThinkingSoundProcessor(delay_secs=settings.thinking_delay_secs)),
                (Slot.BEFORE_TTS, AssistantTap(on_turn, runtime.on_marker if runtime else None)),
                (Slot.AUDIO_OUT, models.tts),
                (None, ThinkingSoundCut()),
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
    session = GhostSession(
        session_id=session_id, worker=worker, runner=runner, _ended=ended, conversation=runtime
    )

    async def start() -> None:
        await runner.add_workers(worker)
        if runtime:
            await worker.queue_frames(runtime.start_frames())
        if first_utterance:
            if runtime:
                runtime.record("user", first_utterance)
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

            text = random.choice(GREETINGS) if greeting == GREETING_VARIED else greeting
            await worker.queue_frames([TTSSpeakFrame(text)])
        else:
            await worker.queue_frames([LLMRunFrame()])

    session.start = start  # type: ignore[attr-defined]
    return session
