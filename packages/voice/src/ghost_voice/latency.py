"""Where a turn's time goes: one timeline per turn, timings only, never text.

``LatencyObserver`` is a Pipecat observer attached to every session. It watches frames
on their first push (Pipecat dedupes hops for an observer created with
``observe_every_push=False``), keeps the moments of the turn on the pipeline clock, and
takes a few durations straight from Ghost's own processors through ``LatencyProbe``
(the turn hold, interceptors, the confirmation classifier, the steps inside a tool).
When the turn is over it logs one INFO line and appends one JSON record to
``latency.jsonl``.

A voice turn, end to end::

    user stops talking (VAD stop minus its stop_secs)
      hear   VAD silence, STT, interceptors + confirmation, Smart Turn, our grace hold
      llm    each model call: start, first byte, first tool call, end, tokens, cache
      tools  each function call by name (read tools hit the GitHub client)
      tts    last model call ends -> narration released -> Kokoro first audio -> transport
    Ghost's voice starts (BotStartedSpeaking)

A reply the operator talked over and cut off carries ``barge_in``: from when they began
(the VAD start minus its start_secs) to the interruption.

What is never recorded: transcripts, model text, tool arguments or results, persona or
conversation names. The record holds numbers, counts, model ids, tool and operation
names (identifiers in Ghost's own code), classifier names and verdicts. Passive audio
never reaches a session, so it never reaches this module.
"""

from __future__ import annotations

import json
import os
import time
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from loguru import logger
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    ClientConnectedFrame,
    FunctionCallInProgressFrame,
    FunctionCallResultFrame,
    FunctionCallsStartedFrame,
    LLMFullResponseEndFrame,
    LLMFullResponseStartFrame,
    LLMTextFrame,
    LLMThoughtStartFrame,
    MetricsFrame,
    TranscriptionFrame,
    TTSAudioRawFrame,
    TTSSpeakFrame,
    TTSStartedFrame,
    UserStartedSpeakingFrame,
    UserStoppedSpeakingFrame,
    VADUserStartedSpeakingFrame,
    VADUserStoppedSpeakingFrame,
)
from pipecat.metrics.metrics import LLMUsageMetricsData, TTFBMetricsData, TurnMetricsData
from pipecat.observers.base_observer import BaseObserver, FramePushed
from pipecat.services.llm_service import LLMService
from pipecat.services.stt_service import STTService
from pipecat.services.tts_service import TTSService

RECORD_VERSION = 1
DEFAULT_MAX_BYTES = 5 * 1024 * 1024
# A response that calls only these still speaks its text (ghost_voice.narration).
_SPOKEN_WITH = frozenset({"hang_up", "hold"})
# The thinking sound processor's frame; matched by name to keep this module free of
# a dependency on ghost_voice.thinking.
_THINKING_START = "ThinkingStartFrame"


# --- what Ghost's own processors report -------------------------------------------


class LatencyProbe:
    """Durations Ghost's processors measure themselves. The base class is a no-op, so a
    processor built without a session (tests, the wake listener) needs no checks."""

    def held(self, secs: float) -> None:
        """The turn model said done and the grace hold ran ``secs`` before the turn ended."""

    def kept_open(self) -> None:
        """The operator went on during the grace hold; the turn stayed open."""

    def intercepted(self, name: str, secs: float) -> None:
        """An interceptor (``name`` is its code name) handled the turn in ``secs``."""

    def interceptors(self, secs: float) -> None:
        """The interceptors looked at a transcript and let it through, taking ``secs``."""

    def confirmation(
        self, *, classifier: str, classify_secs: float, verdict: str, total_secs: float
    ) -> None:
        """A pending change met a transcript: which classifier decided, how long it took,
        and the whole call including any execution."""

    def tool_step(self, call_id: str, step: str, secs: float) -> None:
        """A step inside a function call (``prepare``, ``submit``, ``request``, ``read``)."""

    def tool_status(self, call_id: str, status: str) -> None:
        """How a function call ended: ``done``, ``needs_confirmation``, ``error``, ``ok``."""

    def tool_operation(self, call_id: str, operation: str) -> None:
        """The operation a ``change`` call named, as ``ability.operation``."""


NULL_PROBE = LatencyProbe()


# --- one turn -----------------------------------------------------------------------


@dataclass
class _LLMCall:
    start: float
    ttfb: float | None = None
    first_text: float | None = None
    first_tool: float | None = None
    end: float | None = None
    model: str | None = None
    tools: list[str] = field(default_factory=list)
    thinking: bool = False
    text: bool = False
    usage: dict[str, int] | None = None


@dataclass
class _ToolCall:
    name: str
    start: float
    end: float | None = None


@dataclass
class _Turn:
    index: int
    kind: str  # voice | text | opening | system
    opened: float
    anchor: float | None = None  # when the operator stopped talking, or the turn's start
    vad_stop: float | None = None
    stop_secs: float = 0.0
    transcript: float | None = None
    stt_model: float | None = None
    segments: int = 0
    released: float | None = None
    llm: list[_LLMCall] = field(default_factory=list)
    tools: dict[str, _ToolCall] = field(default_factory=dict)
    tts_start: float | None = None
    tts_ttfb: float | None = None
    first_audio: float | None = None
    bot_started: float | None = None
    bot_stopped: float | None = None
    spoken_direct: bool = False  # TTSSpeakFrame: a greeting, an interceptor's line
    over_at: float | None = None  # the operator began talking over this reply (VAD)
    barge_in: float | None = None  # from then until the interruption
    smart_turn: list[dict[str, Any]] = field(default_factory=list)
    flags: set[str] = field(default_factory=set)
    # From the probe
    hold: float = 0.0
    kept_open: int = 0
    pre: float = 0.0
    intercepted: str | None = None
    confirm: dict[str, Any] | None = None
    steps: dict[str, dict[str, float]] = field(default_factory=dict)
    status: dict[str, str] = field(default_factory=dict)
    operations: dict[str, str] = field(default_factory=dict)

    def open_llm(self) -> _LLMCall | None:
        return next((c for c in reversed(self.llm) if c.end is None), None)

    def busy(self) -> bool:
        return self.open_llm() is not None or any(t.end is None for t in self.tools.values())

    def happened(self) -> bool:
        return bool(self.llm or self.bot_started is not None or self.intercepted or self.tools)


def _speaking(turn: _Turn) -> bool:
    """Ghost is saying this turn's reply."""
    return turn.bot_started is not None and turn.bot_stopped is None


# --- the observer -------------------------------------------------------------------


class LatencyObserver(BaseObserver, LatencyProbe):
    """Builds one timeline per turn from the frames and the probe; see the module doc.

    ``sink`` gets each finished record (the harness keeps them); ``log_path`` is where
    the JSONL goes (None: nowhere). Nothing here may raise into the pipeline."""

    def __init__(
        self,
        *,
        session_id: str,
        origin: str = "desk",
        model: str | None = None,
        log_path: Path | None = None,
        max_bytes: int = DEFAULT_MAX_BYTES,
        sink: Callable[[dict[str, Any]], None] | None = None,
        log_line: bool = True,
        clock: Callable[[], float] = time.monotonic,
        **kwargs: Any,
    ) -> None:
        super().__init__(observe_every_push=False, **kwargs)
        self.session_id = session_id
        self.origin = origin
        self.model = model
        self._path = log_path
        self._max_bytes = max_bytes
        self._sink = sink
        self._log_line = log_line
        self._clock = clock
        self._turn: _Turn | None = None
        self._count = 0
        self._last_t = 0.0  # the latest pipeline time seen, for probe-only events
        self._opening = True  # no turn yet: the first thing said is the opening
        self._built = clock()
        self._pipeline_t0: float | None = None  # monotonic time of pipeline clock zero
        self.records: list[dict[str, Any]] = []

    # --- frames -------------------------------------------------------------------

    async def on_push_frame(self, data: FramePushed) -> None:
        try:
            self._on_frame(data)
        except Exception:  # noqa: BLE001 - measuring must never break a session
            logger.exception("latency: observer error")

    def _on_frame(self, data: FramePushed) -> None:
        frame = data.frame
        t = data.timestamp / 1e9
        self._last_t = max(self._last_t, t)
        src = data.source
        if self._pipeline_t0 is None:
            self._pipeline_t0 = _pipeline_zero(src)

        if self._opening and self._turn is None:
            self._turn = self._new("opening", t)

        if isinstance(frame, ClientConnectedFrame):
            turn = self._turn
            if turn and turn.kind == "opening" and turn.bot_started is None:
                turn.anchor = t  # the phone's opening is timed from the peer connecting
        elif isinstance(frame, VADUserStartedSpeakingFrame):
            turn = self._turn
            if turn and turn.kind == "voice" and turn.released is None:
                turn.flags.add("_segment_open")
            if turn and _speaking(turn) and "_over" not in turn.flags:
                # A segment over Ghost's voice; the latest one is what a barge-in cut in
                # with. Timed from when the speech began, as a turn is from when it ended.
                turn.flags.add("_over")
                turn.over_at = t - frame.start_secs
        elif isinstance(frame, VADUserStoppedSpeakingFrame):
            if self._turn and _speaking(self._turn):
                self._turn.flags.discard("_over")
            self._vad_stop(t, frame.stop_secs)
        elif isinstance(frame, UserStartedSpeakingFrame):
            self._user_started(t)
        elif isinstance(frame, TranscriptionFrame) and isinstance(src, STTService):
            # Only the open voice turn's own words; a transcript while Ghost speaks is
            # echo or a barge-in, and a barge-in opens its own turn.
            turn = self._turn
            if turn and turn.kind == "voice" and turn.released is None:
                turn.transcript = t
        elif isinstance(frame, UserStoppedSpeakingFrame):
            turn = self._turn
            if turn and turn.kind == "voice" and turn.released is None:
                turn.released = t
        elif type(frame).__name__ == _THINKING_START:
            if self._turn is None or self._turn.bot_started is not None:
                self._replace(self._new("text", t))
        elif isinstance(frame, LLMFullResponseStartFrame) and isinstance(src, LLMService):
            turn = self._turn
            if turn is None or (turn.bot_started is not None and not turn.busy()):
                self._replace(self._new("system", t))
                turn = self._turn
            assert turn is not None
            turn.llm.append(_LLMCall(start=t, model=self.model))
        elif isinstance(frame, LLMThoughtStartFrame) and isinstance(src, LLMService):
            call = self._turn.open_llm() if self._turn else None
            if call:
                call.thinking = True
        elif isinstance(frame, LLMTextFrame) and isinstance(src, LLMService):
            call = self._turn.open_llm() if self._turn else None
            if call:
                call.text = True
                if call.first_text is None:
                    call.first_text = t
        elif isinstance(frame, FunctionCallsStartedFrame) and isinstance(src, LLMService):
            call = self._turn.open_llm() if self._turn else None
            if call and call.first_tool is None:
                call.first_tool = t
                call.tools = [c.function_name for c in frame.function_calls]
                if call.text and not set(call.tools) <= _SPOKEN_WITH:
                    self._turn.flags.add("narration_dropped")  # type: ignore[union-attr]
        elif isinstance(frame, LLMFullResponseEndFrame) and isinstance(src, LLMService):
            call = self._turn.open_llm() if self._turn else None
            if call:
                call.end = t
        elif isinstance(frame, FunctionCallInProgressFrame):
            turn = self._turn
            if turn and frame.tool_call_id not in turn.tools:
                turn.tools[frame.tool_call_id] = _ToolCall(name=frame.function_name, start=t)
        elif isinstance(frame, FunctionCallResultFrame):
            call = self._turn.tools.get(frame.tool_call_id) if self._turn else None
            if call and call.end is None:
                call.end = t
        elif isinstance(frame, MetricsFrame):
            self._metrics(frame, src)
        elif isinstance(frame, TTSSpeakFrame):
            if self._turn:
                self._turn.spoken_direct = True
        elif isinstance(frame, TTSStartedFrame) and isinstance(src, TTSService):
            turn = self._turn
            if turn and turn.tts_start is None and turn.bot_started is None:
                turn.tts_start = t
        elif isinstance(frame, TTSAudioRawFrame) and isinstance(src, TTSService):
            turn = self._turn
            if turn and turn.first_audio is None and turn.bot_started is None:
                turn.first_audio = t
        elif isinstance(frame, BotStartedSpeakingFrame):
            turn = self._turn
            if turn and turn.bot_started is None:
                turn.bot_started = t
        elif isinstance(frame, BotStoppedSpeakingFrame):
            turn = self._turn
            if turn and turn.bot_started is not None and turn.bot_stopped is None:
                turn.bot_stopped = t
                if not turn.busy():
                    self._finish()
        # An EndFrame is first pushed at the pipeline's source, before the goodbye queued
        # ahead of it has played; the session's end calls flush() instead.

    def _metrics(self, frame: MetricsFrame, src: Any) -> None:
        turn = self._turn
        if turn is None:
            return
        for d in frame.data:
            if isinstance(d, TTFBMetricsData):
                if isinstance(src, LLMService):
                    call = turn.open_llm() or (turn.llm[-1] if turn.llm else None)
                    if call and call.ttfb is None:
                        call.ttfb = d.value
                        call.model = d.model or call.model
                elif isinstance(src, TTSService):
                    if turn.tts_ttfb is None and turn.bot_started is None:
                        turn.tts_ttfb = d.value
                elif isinstance(src, STTService):
                    turn.stt_model = d.value
            elif isinstance(d, LLMUsageMetricsData):
                # Reported just after the response's end frame: the latest call
                # without usage is the one it belongs to.
                call = next((c for c in reversed(turn.llm) if c.usage is None), None)
                if call:
                    u = d.value
                    call.usage = {
                        "in": u.prompt_tokens,
                        "out": u.completion_tokens,
                        "cache_read": u.cache_read_input_tokens or 0,
                        "cache_write": u.cache_creation_input_tokens or 0,
                    }
                    call.model = d.model or call.model
            elif isinstance(d, TurnMetricsData):
                turn.smart_turn.append(
                    {
                        "complete": d.is_complete,
                        "p": round(d.probability, 3),
                        "ms": round(d.e2e_processing_time_ms, 1),
                    }
                )

    # --- voice bookkeeping -----------------------------------------------------------

    def _user_started(self, t: float) -> None:
        turn = self._turn
        if turn and turn.kind == "voice" and turn.released is None and turn.bot_started is None:
            return  # the same turn, seen again (both broadcast copies)
        if turn is not None:
            if turn.bot_started is None and turn.happened():
                turn.flags.add("interrupted")
            elif _speaking(turn):
                turn.flags.add("barged_in")
                if turn.over_at is not None:
                    turn.barge_in = t - turn.over_at
        self._replace(self._new("voice", t))

    def _vad_stop(self, t: float, stop_secs: float) -> None:
        turn = self._turn
        if turn is None or turn.kind != "voice" or turn.released is not None:
            return
        if "_segment_open" in turn.flags or turn.vad_stop is None:
            turn.flags.discard("_segment_open")
            turn.segments += 1
            turn.vad_stop = t
        else:  # the other broadcast copy of the same stop
            turn.vad_stop = min(turn.vad_stop, t)
        turn.stop_secs = stop_secs

    # --- probe --------------------------------------------------------------------------

    def held(self, secs: float) -> None:
        if self._turn:
            self._turn.hold += secs

    def kept_open(self) -> None:
        if self._turn:
            self._turn.kept_open += 1

    def intercepted(self, name: str, secs: float) -> None:
        if self._turn:
            self._turn.intercepted = name
            self._turn.pre += secs

    def interceptors(self, secs: float) -> None:
        if self._turn:
            self._turn.pre += secs

    def confirmation(
        self, *, classifier: str, classify_secs: float, verdict: str, total_secs: float
    ) -> None:
        if self._turn:
            self._turn.pre += total_secs
            self._turn.confirm = {
                "classifier": classifier,
                "classify": round(classify_secs, 4),
                "verdict": verdict,
                "total": round(total_secs, 4),
            }

    def tool_step(self, call_id: str, step: str, secs: float) -> None:
        if self._turn:
            self._turn.steps.setdefault(call_id, {})[step] = round(secs, 4)

    def tool_status(self, call_id: str, status: str) -> None:
        if self._turn:
            self._turn.status[call_id] = status

    def tool_operation(self, call_id: str, operation: str) -> None:
        if self._turn:
            self._turn.operations[call_id] = operation

    # --- turn lifecycle ----------------------------------------------------------------

    def _new(self, kind: str, t: float) -> _Turn:
        self._opening = False
        self._count += 1
        return _Turn(index=self._count, kind=kind, opened=t)

    def _replace(self, turn: _Turn) -> None:
        self._finish()
        self._turn = turn

    def _finish(self) -> None:
        turn, self._turn = self._turn, None
        if turn is None or not turn.happened():
            return
        record = build_record(
            turn,
            session_id=self.session_id,
            origin=self.origin,
            model=self.model,
            build_secs=(
                self._pipeline_t0 - self._built
                if turn.kind == "opening" and self._pipeline_t0 is not None
                else None
            ),
        )
        self.records.append(record)
        if self._log_line:
            logger.info(format_line(record))
        if self._sink:
            try:
                self._sink(record)
            except Exception:  # noqa: BLE001
                logger.exception("latency: sink failed")
        if self._path is not None:
            append_record(self._path, record, max_bytes=self._max_bytes)

    def flush(self) -> None:
        """Finish whatever turn is open (the session is over)."""
        try:
            self._finish()
        except Exception:  # noqa: BLE001
            logger.exception("latency: flush failed")


# --- the record -----------------------------------------------------------------------


def _pipeline_zero(src: Any) -> float | None:
    """Monotonic seconds at the pipeline clock's zero, when the clock says (Pipecat's
    ``SystemClock``); used only to report how long a session took to start."""
    try:
        started = getattr(src.get_clock(), "_time", 0)
    except Exception:  # noqa: BLE001 - not set up yet, or another clock
        return None
    return started / 1e9 if isinstance(started, int) and started > 0 else None


def _r(x: float | None, nd: int = 3) -> float | None:
    return None if x is None else round(x, nd)


def _union(intervals: Iterable[tuple[float, float]]) -> float:
    total, end = 0.0, float("-inf")
    for s, e in sorted(intervals):
        if e <= end:
            continue
        total += e - max(s, end)
        end = e
    return total


def build_record(
    turn: _Turn,
    *,
    session_id: str,
    origin: str,
    model: str | None,
    build_secs: float | None = None,
) -> dict[str, Any]:
    """The JSON record of one finished turn. Times are seconds; ``at`` fields are from
    the turn's anchor (the moment the operator stopped talking, for a voice turn)."""
    voice = turn.kind == "voice"
    if voice and turn.vad_stop is not None:
        anchor = turn.vad_stop - turn.stop_secs
    elif turn.anchor is not None:
        anchor = turn.anchor
    else:
        anchor = turn.opened
    end = turn.bot_started
    # Model calls and tools on the way to Ghost's voice (all of them if it never spoke).
    horizon = end if end is not None else float("inf")
    calls = [c for c in turn.llm if c.start < horizon]
    tools = [t for t in turn.tools.values() if t.start < horizon]

    def rel(x: float | None) -> float | None:
        return None if x is None else round(x - anchor, 3)

    rec: dict[str, Any] = {
        "v": RECORD_VERSION,
        "ts": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%S.%fZ")[:-4] + "Z",
        "session": session_id,
        "origin": origin,
        "turn": turn.index,
        "kind": turn.kind,
        "model": next((c.model for c in turn.llm if c.model), model),
        "reply": _r(end - anchor) if end is not None else None,
        "spoke": end is not None,
    }
    if build_secs is not None:
        rec["build"] = _r(build_secs)

    hear_total = None
    if voice:
        released = turn.released
        if released is None and turn.intercepted and turn.transcript is not None:
            # The interceptor took the transcript, so the turn never ended the usual
            # way: Ghost heard it once the interceptor had decided.
            released = turn.transcript + turn.pre
        hear_total = (released - anchor) if released is not None else None
        stt = (
            turn.transcript - turn.vad_stop
            if turn.transcript is not None
            and turn.vad_stop is not None
            and turn.transcript >= turn.vad_stop
            else None
        )
        turn_wait = None
        if released is not None and turn.transcript is not None:
            turn_wait = max(0.0, released - max(turn.transcript, turn.vad_stop or 0) - turn.hold)
        rec["hear"] = {
            "total": _r(hear_total),
            "vad": _r(turn.stop_secs),
            "stt": _r(stt),
            "stt_model": _r(turn.stt_model),
            "pre": _r(turn.pre),
            "turn": _r(turn_wait),
            "hold": _r(turn.hold),
            "segments": turn.segments,
            "kept_open": turn.kept_open,
        }
        if turn.smart_turn:
            rec["hear"]["smart_turn"] = turn.smart_turn
    if turn.confirm:
        rec["confirm"] = turn.confirm
    if turn.intercepted:
        rec["intercepted"] = turn.intercepted

    llm_calls = []
    for c in turn.llm:
        entry: dict[str, Any] = {
            "at": rel(c.start),
            "ttfb": _r(c.ttfb),
            "dur": _r(c.end - c.start) if c.end is not None else None,
            "first_tool": _r(c.first_tool - c.start) if c.first_tool is not None else None,
            "first_text": _r(c.first_text - c.start) if c.first_text is not None else None,
            "tools": c.tools,
            "text": c.text,
            "thinking": c.thinking,
        }
        if c.usage:
            entry.update(c.usage)
        llm_calls.append(entry)

    def clip(s: float, e: float | None) -> tuple[float, float]:
        return s, min(e if e is not None else horizon, horizon)

    llm_spans = [clip(c.start, c.end) for c in calls if c.end is not None or end is not None]
    llm_total = _union(llm_spans)
    rec["llm"] = {"total": _r(llm_total), "calls": llm_calls} if llm_calls else None

    tool_calls = []
    for cid, tc in turn.tools.items():
        entry = {
            "name": tc.name,
            "at": rel(tc.start),
            "secs": _r(tc.end - tc.start) if tc.end is not None else None,
        }
        if cid in turn.operations:
            entry["op"] = turn.operations[cid]
        if cid in turn.status:
            entry["status"] = turn.status[cid]
        if cid in turn.steps:
            entry["steps"] = turn.steps[cid]
        tool_calls.append(entry)
    # Tool time on the path is what tools add beyond the model calls they overlap.
    tool_spans = [clip(tc.start, tc.end) for tc in tools if tc.end is not None or end is not None]
    tools_total = _union(llm_spans + tool_spans) - llm_total
    rec["tools"] = {"total": _r(tools_total), "calls": tool_calls} if tool_calls else None

    tts_total = None
    if end is not None:
        last_llm_end = max((c.end for c in calls if c.end is not None), default=None)
        if last_llm_end is not None:
            start_tts = last_llm_end
        elif voice and hear_total is not None:
            start_tts = anchor + hear_total
        else:
            start_tts = turn.anchor if turn.anchor is not None else turn.opened
        if start_tts <= end:
            tts_total = end - start_tts
        rec["tts"] = {
            "total": _r(tts_total),
            "wait": _r(turn.tts_start - start_tts) if turn.tts_start is not None else None,
            "synth": _r(turn.first_audio - turn.tts_start)
            if turn.first_audio is not None and turn.tts_start is not None
            else None,
            "ttfb": _r(turn.tts_ttfb),
            "out": _r(end - turn.first_audio) if turn.first_audio is not None else None,
            "direct": turn.spoken_direct,
        }
        if turn.bot_stopped is not None:
            rec["speech"] = _r(turn.bot_stopped - end)
    if end is not None:
        parts = [hear_total or 0.0, llm_total or 0.0, tools_total or 0.0, tts_total or 0.0]
        rec["other"] = _r((end - anchor) - sum(parts))
    if turn.barge_in is not None:
        rec["barge_in"] = _r(turn.barge_in)
    flags = sorted(f for f in turn.flags if not f.startswith("_"))
    if flags:
        rec["flags"] = flags
    return rec


def cache_share(calls: Sequence[dict[str, Any]]) -> float | None:
    """Share of input tokens read from the prompt cache across ``calls``."""
    read = sum(int(c.get("cache_read") or 0) for c in calls)
    total = sum(
        int(c.get("in") or 0) + int(c.get("cache_read") or 0) + int(c.get("cache_write") or 0)
        for c in calls
    )
    return read / total if total else None


def format_line(rec: dict[str, Any]) -> str:
    """One log line per turn, timings only."""
    head = f"latency: turn {rec['turn']} {rec['origin']} {rec['kind']}"
    reply = rec.get("reply")
    if reply is None:
        head += " no reply"
    else:
        head += f" reply {reply:.2f}s"
    parts: list[str] = []
    hear = rec.get("hear")
    if hear and hear.get("total") is not None:
        sub = [f"stt {hear['stt']:.2f}" if hear.get("stt") is not None else None]
        if hear.get("pre"):
            sub.append(f"pre {hear['pre']:.2f}")
        if hear.get("turn"):
            sub.append(f"turn {hear['turn']:.2f}")
        if hear.get("hold"):
            sub.append(f"hold {hear['hold']:.2f}")
        parts.append(f"hear {hear['total']:.2f} ({', '.join(s for s in sub if s)})")
    if rec.get("intercepted"):
        parts.append(f"intercepted {rec['intercepted']}")
    if rec.get("confirm"):
        c = rec["confirm"]
        parts.append(f"confirm {c['total']:.2f} ({c['classifier']} {c['classify']:.2f})")
    llm = rec.get("llm")
    if llm:
        calls = llm["calls"]
        sub = []
        first = calls[0].get("ttfb")
        if first is not None:
            sub.append(f"ttfb {first:.2f}")
        sub.append(f"{len(calls)} call{'s' if len(calls) != 1 else ''}")
        share = cache_share(calls)
        if share is not None:
            sub.append(f"cache {share:.0%}")
        parts.append(f"llm {llm['total']:.2f} ({', '.join(sub)})")
    tools = rec.get("tools")
    if tools:
        names = ", ".join(
            f"{t['name']} {t['secs']:.2f}" if t.get("secs") is not None else t["name"]
            for t in tools["calls"]
        )
        parts.append(f"tools {tools['total']:.2f} ({names})")
    tts = rec.get("tts")
    if tts and tts.get("total") is not None:
        parts.append(f"tts {tts['total']:.2f}")
    other = rec.get("other")
    if other is not None and abs(other) >= 0.01:
        parts.append(f"other {other:.2f}")
    line = head + (" = " + " + ".join(parts) if parts and reply is not None else "")
    if reply is None and parts:
        line += ": " + ", ".join(parts)
    if rec.get("barge_in") is not None:
        line += f", cut off {rec['barge_in']:.2f}s after the operator began"
    if rec.get("flags"):
        line += f" [{', '.join(rec['flags'])}]"
    return line


def append_record(
    path: Path, record: dict[str, Any], *, max_bytes: int = DEFAULT_MAX_BYTES
) -> None:
    """Append one line; past ``max_bytes`` the file moves to ``.1`` (one generation kept)."""
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        try:
            if path.stat().st_size >= max_bytes:
                os.replace(path, path.with_name(path.name + ".1"))
        except FileNotFoundError:
            pass
        with path.open("a", encoding="utf-8") as f:
            f.write(json.dumps(record, separators=(",", ":")) + "\n")
    except OSError as e:
        logger.warning(f"latency: could not write {path}: {e.strerror}")


def read_records(path: Path) -> list[dict[str, Any]]:
    """Every record in ``path`` and its rotated generation, oldest first."""
    out: list[dict[str, Any]] = []
    for p in (path.with_name(path.name + ".1"), path):
        if not p.exists():
            continue
        for line in p.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                out.append(json.loads(line))
            except ValueError:
                continue
    return out


# --- summaries -------------------------------------------------------------------------


def percentile(values: Sequence[float], q: float) -> float | None:
    """Nearest-rank-with-interpolation percentile, ``q`` in [0, 100]."""
    xs = sorted(v for v in values if v is not None)
    if not xs:
        return None
    if len(xs) == 1:
        return xs[0]
    k = (len(xs) - 1) * q / 100
    lo = int(k)
    hi = min(lo + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def stage_values(rec: dict[str, Any]) -> dict[str, float | None]:
    """The flat stages a summary reports for one record."""
    hear = rec.get("hear") or {}
    llm = rec.get("llm") or {}
    calls = llm.get("calls") or []
    tts = rec.get("tts") or {}
    tools = rec.get("tools") or {}
    confirm = rec.get("confirm") or {}
    return {
        "reply": rec.get("reply"),
        "hear": hear.get("total"),
        "stt": hear.get("stt"),
        "turn": hear.get("turn"),
        "hold": hear.get("hold"),
        "pre": hear.get("pre"),
        "confirm": confirm.get("total"),
        "llm": llm.get("total"),
        "llm_ttfb": calls[0].get("ttfb") if calls else None,
        "llm_calls": float(len(calls)) if calls else None,
        "tools": tools.get("total"),
        "tts": tts.get("total"),
        "tts_synth": tts.get("synth"),
        "other": rec.get("other"),
        "barge_in": rec.get("barge_in"),
    }


STAGES = (
    "reply",
    "hear",
    "stt",
    "turn",
    "hold",
    "pre",
    "confirm",
    "llm",
    "llm_ttfb",
    "llm_calls",
    "tools",
    "tts",
    "tts_synth",
    "other",
    "barge_in",
)


def summarize(records: Sequence[dict[str, Any]]) -> dict[str, dict[str, float | int | None]]:
    """p50, p95 and count per stage."""
    out: dict[str, dict[str, float | int | None]] = {}
    for stage in STAGES:
        vals = [v for r in records if (v := stage_values(r).get(stage)) is not None]
        if not vals:
            continue
        out[stage] = {
            "n": len(vals),
            "p50": percentile(vals, 50),
            "p95": percentile(vals, 95),
        }
    return out


def summary_table(records: Sequence[dict[str, Any]], title: str) -> list[str]:
    s = summarize(records)
    lines = [f"{title} ({len(records)} turns)", f"  {'stage':<10} {'n':>4} {'p50':>7} {'p95':>7}"]
    for stage, v in s.items():
        p50, p95 = v["p50"], v["p95"]
        fmt = "{:7.0f}" if stage == "llm_calls" else "{:7.2f}"
        lines.append(
            f"  {stage:<10} {v['n']:>4} "
            + (fmt.format(p50) if p50 is not None else "      -")
            + " "
            + (fmt.format(p95) if p95 is not None else "      -")
        )
    calls = [c for r in records for c in ((r.get("llm") or {}).get("calls") or [])]
    share = cache_share(calls)
    if share is not None:
        lines.append(f"  prompt cache: {share:.0%} of input tokens read from cache")
    return lines
