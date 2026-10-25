"""Build one conversation session."""

from __future__ import annotations

import asyncio
import random
import uuid
from collections.abc import Awaitable, Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ghost_core import (
    Events,
    Slot,
    ToolSource,
    WriteGate,
)
from ghost_core.conversations import GHOST, SUMMARY_TEMPLATE, Conversations, conversations_source
from loguru import logger
from pipecat.audio.mixers.base_audio_mixer import BaseAudioMixer
from pipecat.audio.mixers.soundfile_mixer import SoundfileMixer
from pipecat.audio.turn.smart_turn.base_smart_turn import SmartTurnParams
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
from pipecat.transports.base_transport import BaseTransport
from pipecat.turns.empty_user_turn import EmptyUserTurnConfig
from pipecat.turns.user_turn_strategies import UserTurnStrategies
from pipecat.utils.context.llm_context_summarization import LLMContextSummarizationConfig
from pipecat.workers.runner import WorkerRunner

from ghost_voice.config import VoiceSettings
from ghost_voice.confirmation import ConfirmationProcessor
from ghost_voice.conversation import ConversationRuntime
from ghost_voice.echo import EchoFilter
from ghost_voice.elevenlabs import session_tts
from ghost_voice.hold import HoldGate
from ghost_voice.intents import QuickIntents
from ghost_voice.intercept import InterceptorProcessor
from ghost_voice.kokoro import KokoroTTS
from ghost_voice.latency import LatencyObserver
from ghost_voice.llm import GhostAnthropicLLMService, shared_client
from ghost_voice.narration import NarrationCut
from ghost_voice.parakeet import ParakeetSTTService
from ghost_voice.prompt import build_system_prompt
from ghost_voice.rtvi import GhostRTVIProcessor, observer_params
from ghost_voice.sounds import ensure_thinking_sound
from ghost_voice.tap import AssistantTap, OnPhase, OnTurn, UserTap
from ghost_voice.thinking import (
    ThinkingSoundCut,
    ThinkingSoundProcessor,
    ThinkingStartFrame,
    ThinkingStopFrame,
)
from ghost_voice.tools import register_tools
from ghost_voice.transport import SoundDeviceTransport, SoundDeviceTransportParams
from ghost_voice.turns import BargeInUserTurnStartStrategy, PatientTurnStopStrategy, SmartTurn
from ghost_voice.wake import DEFAULT_WAKE_PHRASES

TTS_SAMPLE_RATE = 24000  # Kokoro's native rate; the output transport runs at it

# Builds the session's transport, given the thinking-sound mixer (or None when the
# thinking sound is off) to attach to its audio output.
TransportFactory = Callable[[BaseAudioMixer | None], BaseTransport]


@dataclass
class Models:
    """Loaded once by the daemon and shared across sessions (about 3 GB).

    Each is a template: pipelines use ``fork()`` so each gets its own processor (or
    turn analyzer) over the one loaded model. Never put a template in a pipeline:
    Pipecat leaves a processor that saw a CancelFrame refusing frames for good."""

    stt: ParakeetSTTService
    tts: KokoroTTS
    turn: SmartTurn

    @classmethod
    def load(cls, s: VoiceSettings) -> Models:
        stt = ParakeetSTTService(
            model=s.stt_model, quantization=s.stt_quantization, threads=s.stt_threads
        )
        stt.load()
        # Prefer files already in Ghost's cache; otherwise Pipecat downloads them.
        model_path = s.tts_model_path or s.cache_dir / "kokoro" / "kokoro-v1.0.onnx"
        voices_path = s.tts_voices_path or s.cache_dir / "kokoro" / "voices-v1.0.bin"
        tts = KokoroTTS.load(
            model_path=model_path if model_path.exists() else None,
            voices_path=voices_path if voices_path.exists() else None,
            settings=KokoroTTS.Settings(voice=s.tts_voice, speed=s.tts_speed),
        )
        turn = SmartTurn(
            cpu_count=s.smart_turn_threads, params=SmartTurnParams(stop_secs=s.turn_stop_secs)
        )
        return cls(stt=stt, tts=tts, turn=turn)


@dataclass
class GhostSession:
    session_id: str
    worker: PipelineWorker
    runner: WorkerRunner
    _ended: asyncio.Event
    conversation: ConversationRuntime | None = None
    latency: LatencyObserver | None = None
    # Set by build_session: a typed turn during a hold ends the hold first (ADR 0021).
    before_text: Callable[[], Awaitable[None]] | None = None
    # Set by build_session: stops the hold timer however the session ended, so a session
    # ended from the desktop while on hold can't "hang up" whichever one runs 30 min later.
    after_run: Callable[[], None] | None = None

    async def run(self) -> None:
        try:
            await self.runner.run()
        finally:
            if self.after_run:
                self.after_run()
            if self.latency:
                self.latency.flush()  # the turn that was open when the session ended

    async def end(self) -> None:
        if not self._ended.is_set():
            self._ended.set()
            await self.runner.stop_when_done()

    async def inject(self, frames: list[Any]) -> None:
        """Queue frames into the running pipeline (a typed turn, a system note)."""
        await self.worker.queue_frames(frames)

    async def say_text(self, text: str) -> None:
        """A typed user turn: goes through the LLM, the tools and the gate like speech."""
        if self.before_text:
            await self.before_text()
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

    async def send_server_message(self, data: Any) -> None:
        """Send ``data`` to a connected client as an RTVI ``server-message``."""
        await self.worker.rtvi.send_server_message(data)

    def on_client_ready(self, handler: Callable[[], Awaitable[None]]) -> None:
        """Run ``handler`` when an RTVI client says ``client-ready``."""

        @self.worker.rtvi.event_handler("on_client_ready")
        async def _ready(_rtvi) -> None:
            await handler()


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
    """Splice processors into the fixed pipeline. ``core`` is the ordered list of
    Ghost's own processors, each tagged with the slot that comes right after it."""
    out: list[Any] = []
    for slot, proc in core:
        out.append(proc)
        if slot is not None:
            out.extend(slots.get(slot, []))
    return out


GREETING_VARIED = "<varied>"
COMPACTION_PROMPT = """You are condensing the older part of a spoken conversation between an \
operator and their assistant into the assistant's memory of it.

Keep, as plain factual statements: decisions and agreements; facts, names and numbers the \
operator stated; what was asked for and whether it was done; open questions and action \
items; preferences about how the operator likes things done. Fold in any earlier memory \
that is still true. Drop greetings, small talk, and tool chatter. Do not invent anything. \
Write only the memory, no preamble."""

# An agent called directly (ADR 0014; the phone can start in one) answers as itself.
THREAD_GREETINGS = ("{name} here.", "{name}. Go ahead.", "{name}, what do you need?")
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


# Spoken when the operator comes back from hold with only Ghost's name ("Ghost?").
BACK_LINES = ("I'm here.", "Go ahead.", "Yes?")


def build_session(
    settings: VoiceSettings,
    *,
    models: Models,
    sources: Sequence[ToolSource],
    gate: WriteGate,
    events: Events | None = None,
    transport: BaseTransport | None = None,
    transport_factory: TransportFactory | None = None,
    greeting: str | None = GREETING_VARIED,
    first_utterance: str | None = None,
    on_hang_up: Callable[[], Awaitable[None]] | None = None,
    on_turn: OnTurn | None = None,
    wake_phrases: tuple[str, ...] | None = None,
    wake_fuzzy: float = 0.0,
    on_phase: OnPhase | None = None,
    conversations: Conversations | None = None,
    start_in: str = GHOST,
    on_conversation: Callable[[str], Awaitable[None]] | None = None,
    origin: str = "desk",
    latency_log: Path | None = None,
    on_latency: Callable[[dict[str, Any]], None] | None = None,
    reach: Callable[[Any], list[ToolSource]] | None = None,
    on_enter: Callable[[Any], Awaitable[None]] | None = None,
    on_leave: Callable[[Any], None] | None = None,
) -> GhostSession:
    """Build one session. ``first_utterance`` is what the operator said right after
    the wake phrase; when given it is the first user turn and there is no greeting.
    With ``conversations`` the session opens in ``start_in`` (normally ``ghost``),
    seeded from its memory, in its voice, and can be transferred (ADR 0014).

    The transport is the workstation's sound devices unless ``transport`` or
    ``transport_factory`` is given; the factory receives the thinking-sound mixer so a
    phone's WebRTC track gets it too (ADR 0016). ``start(open=False)`` runs the pipeline
    without the greeting or first turn; ``open()`` sends them later, for a transport
    whose far end connects after the pipeline starts.

    Every turn's timeline (ghost_voice.latency) is logged and, with ``latency_log``,
    appended there; ``on_latency`` gets each record too. ``origin`` (desk, phone) is
    only a label on those records.

    ``on_hang_up`` is called when the session is ending (a goodbye, the idle timeout, a
    hold that ran out). ``wake_phrases`` are what brings a session back from hold (ADR
    0021); the desk's wake phrases by default. ``reach``, ``on_enter`` and ``on_leave``
    are the departments (ADR 0024): what an agent's session reaches, and the hooks that
    start a department's servers on a transfer and release them after."""
    session_id = uuid.uuid4().hex
    target = settings.llm_target()  # GHOST_LLM_ENDPOINT: the proxy or OpenRouter
    latency = (
        LatencyObserver(
            session_id=session_id,
            origin=origin,
            model=target.model,
            log_path=latency_log,
            max_bytes=int(settings.latency_max_mb * 1024 * 1024),
            sink=on_latency,
        )
        if settings.latency
        else None
    )
    ended = asyncio.Event()
    sources = list(sources)
    runtime: ConversationRuntime | None = None
    fragments = [a.prompt for a in sources if a.prompt]
    if conversations is not None:
        sources.append(conversations_source(conversations))

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
            routing=settings.department_routing,
            reach=reach,  # ADR 0024: what the agent's department reaches
            on_enter=on_enter,
            on_leave=on_leave,
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
    if transport is None and transport_factory is not None:
        transport = transport_factory(mixer)
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

    llm = GhostAnthropicLLMService(
        api_key=target.api_key,
        client=shared_client(target),
        settings=AnthropicLLMService.Settings(
            model=target.model,
            system_instruction=(
                runtime.system_prompt() if runtime else build_system_prompt(fragments)
            ),
            max_tokens=settings.llm_max_tokens,
            enable_prompt_caching=True,
            thinking=AnthropicLLMService.ThinkingConfig(type=settings.llm_thinking),
        ),
    )

    session: GhostSession | None = None

    narration = NarrationCut()
    hold_timer: asyncio.Task[None] | None = None

    async def hang_up(*, keep_current: bool = True) -> None:
        """The model called hang_up: its goodbye in this response is spoken, then the
        session ends. The shortcut (``hang_up_now``) has queued its own goodbye already;
        a hold that ran out or the idle timeout hangs up with nothing said."""
        nonlocal hold_timer
        logger.info("hang_up")
        if hold_timer:
            hold_timer.cancel()
            hold_timer = None
        narration.close(keep_current=keep_current)
        if on_hang_up:
            await on_hang_up()
        if session:
            await session.end()

    async def hang_up_now() -> None:
        await hang_up(keep_current=False)

    # --- hold (ADR 0021) -----------------------------------------------------------

    async def hold_ran_out() -> None:
        await asyncio.sleep(settings.hold_timeout_secs)
        logger.info(f"hold: nothing for {settings.hold_timeout_secs:.0f}s; hanging up quietly")
        await hang_up(keep_current=False)

    async def hold() -> None:
        """Ghost stops listening until addressed by name; the context, the thread and a
        pending question keep. Hangs up by itself after ``hold_timeout_secs``."""
        nonlocal hold_timer
        if hold_gate.on_hold:
            return
        logger.info("hold")
        hold_gate.hold()
        await worker.queue_frames([ThinkingStopFrame()])  # no loop while waiting
        if hold_timer:
            hold_timer.cancel()
        hold_timer = asyncio.ensure_future(hold_ran_out())
        await on_phase("hold")

    def stop_hold_timer() -> None:
        nonlocal hold_timer
        if hold_timer:
            hold_timer.cancel()
            hold_timer = None

    async def unhold() -> None:
        """Back to listening; nothing said yet."""
        nonlocal hold_timer
        if hold_timer:
            hold_timer.cancel()
            hold_timer = None
        if not hold_gate.on_hold:
            return
        hold_gate.release()
        logger.info("hold: resumed")
        await on_phase("listening")

    async def resume(remainder: str) -> None:
        """Addressed by name again: back to listening, with the rest of what was said as
        the next turn, or a word that Ghost is here when the name was all."""
        hold_gate.on_hold = True  # the gate cleared itself; unhold expects to do it
        await unhold()
        if remainder.strip():
            await on_turn("user", remainder)
            await worker.queue_frames(
                [
                    ThinkingStartFrame(),
                    LLMMessagesAppendFrame(
                        messages=[{"role": "user", "content": remainder}], run_llm=True
                    ),
                ]
            )
        else:
            from pipecat.frames.frames import TTSSpeakFrame

            await worker.queue_frames([TTSSpeakFrame(random.choice(BACK_LINES))])

    hold_gate = HoldGate(
        on_resume=resume, phrases=wake_phrases or DEFAULT_WAKE_PHRASES, fuzzy=wake_fuzzy
    )

    tools = register_tools(
        llm,
        sources=sources,
        gate=gate,
        session_id=session_id,
        on_hang_up=hang_up,
        on_hold=hold,
        events=events,
        conversations=runtime,
        on_call=runtime.record_call if runtime else None,
        last_request=lambda: last_request(),  # defined below with the turn taps
        probe=latency,
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

    turn_stop = PatientTurnStopStrategy(
        turn_analyzer=models.turn.fork(),
        grace_secs=settings.turn_grace_secs,
        speculate=settings.turn_speculate,
        probe=latency,
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
                stop=[turn_stop],
            ),
            user_idle_timeout=settings.idle_timeout_secs,
            # A turn that opens on voice activity and ends with no words is echo or noise,
            # never a missed interruption: barge-in needs two heard words. Pipecat's default
            # answers such a turn with "sorry, I think I got cut off"; leave it unanswered.
            empty_user_turn=EmptyUserTurnConfig(interrupted_prompt=None, idle_prompt=None),
        ),
    )

    turn_stop.turn_text = user_agg.aggregation_string  # what a speculative answer answers

    @user_agg.event_handler("on_user_turn_idle")
    async def on_idle(_agg) -> None:
        if hold_gate.on_hold:
            return  # a hold has its own, longer clock
        logger.info(f"idle for {settings.idle_timeout_secs}s")
        await hang_up()

    if runtime:

        @assistant_agg.event_handler("on_summary_applied")
        async def on_summary(_agg, _summarizer, _event) -> None:
            await runtime.on_summary_applied(context.get_messages())

    interceptors: list[Any] = [QuickIntents()] if settings.quick_intents else []
    for a in sources:
        interceptors.extend(a.interceptors)
    slots: dict[Slot, list[Any]] = {}
    for a in sources:
        for slot, factories in a.processors.items():
            slots.setdefault(Slot(slot), []).extend(f() for f in factories)

    def last_assistant() -> str:
        return last_assistant_text(context)

    async def _noop_turn(role: str, text: str) -> None: ...

    async def _noop_phase(phase: str) -> None: ...

    outer_turn = on_turn or _noop_turn
    on_phase = on_phase or _noop_phase  # type: ignore[assignment]
    echo = EchoFilter(window_secs=settings.echo_window_secs)
    # The operator's words since Ghost last spoke, and that reply: what a change was asked
    # for in, read by the gate as a possible yes (ghost_core.request).
    heard: list[str] = []
    replied = [""]

    def last_request() -> tuple[str, str]:
        return " ".join(heard), replied[0]

    async def on_turn(role: str, text: str) -> None:  # type: ignore[no-redef]
        if role == "assistant":
            echo.spoke(text)
            replied[0] = text
            heard.clear()
        elif role == "user":
            heard.append(text)
        if runtime and role in ("user", "assistant"):
            runtime.record(role, text)
        await outer_turn(role, text)

    pipeline = Pipeline(
        assemble(
            [
                (Slot.AUDIO_IN, transport.input()),
                (
                    None,
                    models.stt.fork(
                        interim_secs=settings.barge_in_interim_secs
                        if settings.barge_in_min_words > 0
                        else 0.0
                    ),
                ),
                (None, hold_gate),  # on hold, nothing below hears the room (ADR 0021)
                (None, echo),
                (None, UserTap(on_turn, on_phase, held=lambda: hold_gate.on_hold)),
                (
                    None,
                    InterceptorProcessor(
                        interceptors,
                        gate=gate,
                        session_id=session_id,
                        on_hang_up=hang_up_now,
                        on_hold=hold,
                        on_handled=user_agg.reset,
                        last_assistant=last_assistant,
                        probe=latency,
                    ),
                ),
                (
                    Slot.AFTER_STT,
                    ConfirmationProcessor(
                        gate,
                        session_id,
                        last_assistant=last_assistant,
                        events=events,
                        probe=latency,
                    ),
                ),
                (Slot.BEFORE_LLM, user_agg),
                (Slot.AFTER_LLM, llm),
                (None, narration),
                (
                    None,
                    ThinkingSoundProcessor(
                        delay_secs=settings.thinking_delay_secs, held=lambda: hold_gate.on_hold
                    ),
                ),
                (Slot.BEFORE_TTS, AssistantTap(on_turn, runtime.on_marker if runtime else None)),
                (
                    Slot.AUDIO_OUT,
                    # Kokoro, or ElevenLabs with Kokoro behind it (ghost_voice.elevenlabs)
                    session_tts(
                        settings,
                        models.tts,
                        clause_min_words=settings.tts_clause_min_words
                        if settings.tts_clauses
                        else None,
                        line=(lambda: runtime.active.slug) if runtime else None,
                    ),
                ),
                (None, ThinkingSoundCut()),
                (None, transport.output()),
                (None, assistant_agg),
            ],
            slots,
        )
    )
    worker = PipelineWorker(
        pipeline,
        # Usage metrics carry the token counts and prompt-cache reads per model call.
        params=PipelineParams(
            audio_in_sample_rate=16000, enable_metrics=True, enable_usage_metrics=True
        ),
        observers=[latency] if latency else None,
        processor_unusable_policy=ProcessorUnusablePolicy.END,
        rtvi_processor=GhostRTVIProcessor(),
        rtvi_observer_params=observer_params(),
    )
    runner = WorkerRunner(handle_sigint=False)
    session = GhostSession(
        session_id=session_id,
        worker=worker,
        runner=runner,
        _ended=ended,
        conversation=runtime,
        latency=latency,
    )

    async def start(open: bool = True) -> None:
        await runner.add_workers(worker)
        if open:
            await opening()

    async def opening() -> None:
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

            if greeting == GREETING_VARIED and runtime and not runtime.active.is_ghost:
                text = random.choice(THREAD_GREETINGS).format(name=runtime.active.name)
            elif greeting == GREETING_VARIED:
                text = random.choice(GREETINGS)
            else:
                text = greeting
            await worker.queue_frames([TTSSpeakFrame(text)])
        else:
            await worker.queue_frames([LLMRunFrame()])

    session.start = start  # type: ignore[attr-defined]
    session.open = opening  # type: ignore[attr-defined]
    session.before_text = unhold
    session.after_run = stop_hold_timer
    return session
