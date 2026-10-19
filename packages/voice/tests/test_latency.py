"""The per-turn latency timeline: the arithmetic, and that no words ever leave it."""

from __future__ import annotations

import json

from ghost_voice.kokoro import KokoroTTS
from ghost_voice.latency import (
    LatencyObserver,
    append_record,
    cache_share,
    format_line,
    percentile,
    read_records,
    summarize,
)
from ghost_voice.parakeet import ParakeetSTTService
from loguru import logger
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    EndFrame,
    FunctionCallFromLLM,
    FunctionCallInProgressFrame,
    FunctionCallResultFrame,
    FunctionCallsStartedFrame,
    LLMFullResponseEndFrame,
    LLMFullResponseStartFrame,
    LLMTextFrame,
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
from pipecat.metrics.metrics import (
    LLMTokenUsage,
    LLMUsageMetricsData,
    TTFBMetricsData,
    TurnMetricsData,
)
from pipecat.observers.base_observer import FramePushed
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor
from pipecat.services.anthropic.llm import AnthropicLLMService

SECRET = "zebra quartz"  # stands in for anything the operator or the model said

# Processors only need the right type: the observer tells services apart by class.
LLM = AnthropicLLMService.__new__(AnthropicLLMService)
STT = ParakeetSTTService.__new__(ParakeetSTTService)
TTS = KokoroTTS.__new__(KokoroTTS)
OTHER = FrameProcessor.__new__(FrameProcessor)


class Feed:
    def __init__(self, obs: LatencyObserver) -> None:
        self.obs = obs

    async def __call__(self, t: float, frame, src=OTHER, direction=FrameDirection.DOWNSTREAM):
        await self.obs.on_push_frame(
            FramePushed(
                source=src,
                destination=OTHER,
                frame=frame,
                direction=direction,
                timestamp=int(t * 1e9),
            )
        )


def observer(tmp_path, **kw):
    records: list[dict] = []
    obs = LatencyObserver(
        session_id="s1",
        origin="phone",
        model="claude-test",
        log_path=tmp_path / "latency.jsonl",
        sink=records.append,
        **kw,
    )
    return obs, Feed(obs), records


def said(text: str = SECRET) -> TranscriptionFrame:
    return TranscriptionFrame(text=text, user_id="op", timestamp="0", finalized=True)


def usage(inp: int, out: int, read: int, write: int) -> MetricsFrame:
    return MetricsFrame(
        data=[
            LLMUsageMetricsData(
                processor="llm",
                model="claude-test",
                value=LLMTokenUsage(
                    prompt_tokens=inp,
                    completion_tokens=out,
                    total_tokens=inp + out + read + write,
                    cache_read_input_tokens=read,
                    cache_creation_input_tokens=write,
                ),
            )
        ]
    )


def ttfb(value: float) -> MetricsFrame:
    return MetricsFrame(data=[TTFBMetricsData(processor="p", model="claude-test", value=value)])


def call(name: str, cid: str) -> FunctionCallFromLLM:
    return FunctionCallFromLLM(
        function_name=name, tool_call_id=cid, arguments={"q": SECRET}, context=None
    )


async def a_tool_turn(feed: Feed, obs: LatencyObserver) -> None:
    """The operator asks, the model calls get_issue (narrating first), then answers."""
    await feed(0.0, TTSSpeakFrame("Yeah?"))  # the greeting
    await feed(0.1, TTSStartedFrame(), TTS)
    await feed(0.3, TTSAudioRawFrame(b"\0\0" * 240, 24000, 1), TTS)
    await feed(0.35, BotStartedSpeakingFrame())
    await feed(0.9, BotStoppedSpeakingFrame())

    await feed(1.0, VADUserStartedSpeakingFrame(start_secs=0.2))
    await feed(1.0, UserStartedSpeakingFrame())
    await feed(1.01, UserStartedSpeakingFrame())  # the other broadcast copy
    await feed(2.0, VADUserStoppedSpeakingFrame(stop_secs=0.2))  # operator silent at 1.8
    await feed(2.01, VADUserStoppedSpeakingFrame(stop_secs=0.2))
    await feed(2.15, ttfb(0.12), STT)
    await feed(2.2, said(), STT)
    obs.interceptors(0.01)
    await feed(
        2.25,
        MetricsFrame(
            data=[
                TurnMetricsData(
                    processor="turn", is_complete=True, probability=0.9, e2e_processing_time_ms=40.0
                )
            ]
        ),
    )
    obs.held(0.8)
    await feed(3.05, UserStoppedSpeakingFrame())

    await feed(3.1, LLMFullResponseStartFrame(), LLM)
    await feed(3.7, ttfb(0.6), LLM)
    await feed(3.72, LLMTextFrame(f"Let me look, {SECRET}."), LLM)
    await feed(3.9, FunctionCallsStartedFrame(function_calls=[call("get_issue", "c1")]), LLM)
    await feed(
        3.91,
        FunctionCallInProgressFrame(
            function_name="get_issue", tool_call_id="c1", arguments={"q": SECRET}
        ),
        LLM,
    )
    await feed(3.92, LLMFullResponseEndFrame(), LLM)
    await feed(3.93, usage(100, 40, 3000, 0), LLM)
    obs.tool_step("c1", "read", 0.19)
    obs.tool_status("c1", "ok")
    await feed(
        4.11,
        FunctionCallResultFrame(
            function_name="get_issue", tool_call_id="c1", arguments={}, result={"body": SECRET}
        ),
        LLM,
    )

    await feed(4.2, LLMFullResponseStartFrame(), LLM)
    await feed(4.7, ttfb(0.5), LLM)
    await feed(4.75, LLMTextFrame(f"It is {SECRET}."), LLM)
    await feed(4.9, LLMFullResponseEndFrame(), LLM)
    await feed(4.91, usage(50, 20, 3100, 200), LLM)
    await feed(4.95, TTSStartedFrame(), TTS)
    await feed(5.15, TTSAudioRawFrame(b"\0\0" * 240, 24000, 1), TTS)
    await feed(5.2, BotStartedSpeakingFrame())
    await feed(5.21, BotStartedSpeakingFrame(), direction=FrameDirection.UPSTREAM)
    await feed(7.0, BotStoppedSpeakingFrame())


async def test_a_tool_turn_adds_up(tmp_path):
    obs, feed, records = observer(tmp_path)
    await a_tool_turn(feed, obs)
    opening, turn = records
    assert opening["kind"] == "opening" and opening["reply"] == 0.35

    assert turn["kind"] == "voice" and turn["origin"] == "phone"
    assert turn["reply"] == 3.4  # 5.2 - 1.8, from when the operator stopped talking
    hear = turn["hear"]
    assert hear["total"] == 1.25  # 3.05 - 1.8
    assert hear["vad"] == 0.2 and hear["stt"] == 0.2 and hear["stt_model"] == 0.12
    assert hear["hold"] == 0.8 and hear["pre"] == 0.01
    assert hear["turn"] == 0.05  # 3.05 - 2.2 - 0.8
    assert hear["segments"] == 1
    assert hear["smart_turn"] == [{"complete": True, "p": 0.9, "ms": 40.0}]

    llm = turn["llm"]
    assert [c["ttfb"] for c in llm["calls"]] == [0.6, 0.5]
    assert llm["calls"][0]["tools"] == ["get_issue"] and llm["calls"][0]["first_tool"] == 0.8
    assert llm["calls"][0]["cache_read"] == 3000 and llm["calls"][1]["cache_write"] == 200
    assert llm["total"] == 1.52  # 0.82 + 0.7
    assert turn["tools"]["total"] == 0.19  # 4.11 - 3.92, past the end of the first call
    assert turn["tools"]["calls"][0]["steps"] == {"read": 0.19}
    tts = turn["tts"]
    assert tts["total"] == 0.3 and tts["synth"] == 0.2 and tts["out"] == 0.05
    assert turn["speech"] == 1.8
    parts = hear["total"] + llm["total"] + turn["tools"]["total"] + tts["total"]
    assert abs(turn["reply"] - parts - turn["other"]) < 0.002
    assert turn["flags"] == ["narration_dropped"]


async def test_no_words_reach_the_record_the_file_or_the_log(tmp_path):
    lines: list[str] = []
    sink = logger.add(lambda m: lines.append(str(m)), level="INFO")
    try:
        obs, feed, records = observer(tmp_path)
        await a_tool_turn(feed, obs)
    finally:
        logger.remove(sink)
    on_disk = (tmp_path / "latency.jsonl").read_text()
    assert len(on_disk.splitlines()) == 2
    for blob in (json.dumps(records), on_disk, "\n".join(lines)):
        for word in SECRET.split():
            assert word not in blob
        assert "Yeah?" not in blob
    assert any("latency: turn 2 phone voice reply 3.40s = hear 1.25" in ln for ln in lines)


async def test_format_line_names_the_parts(tmp_path):
    obs, feed, records = observer(tmp_path)
    await a_tool_turn(feed, obs)
    line = format_line(records[1])
    assert line.startswith("latency: turn 2 phone voice reply 3.40s = hear 1.25 (stt 0.20")
    assert "hold 0.80" in line
    assert "llm 1.52 (ttfb 0.60, 2 calls, cache 95%)" in line
    assert "tools 0.19 (get_issue 0.20)" in line  # 0.01 of it overlaps the model call
    assert "other 0.14" in line  # hops: 0.05 to the model, 0.09 from the result back
    assert "tts 0.30" in line
    assert line.endswith("[narration_dropped]")


async def test_an_intercepted_goodbye_is_timed_without_the_model(tmp_path):
    obs, feed, records = observer(tmp_path)
    await feed(0.0, UserStartedSpeakingFrame())
    await feed(1.0, VADUserStoppedSpeakingFrame(stop_secs=0.2))
    await feed(1.15, said(), STT)
    obs.intercepted("quick_intents", 0.002)
    await feed(1.16, TTSSpeakFrame("Talk soon!"))
    await feed(1.2, TTSStartedFrame(), TTS)
    await feed(1.4, TTSAudioRawFrame(b"\0\0" * 240, 24000, 1), TTS)
    await feed(1.42, BotStartedSpeakingFrame())
    await feed(1.43, EndFrame())  # first seen at the source, before the goodbye ends
    assert records == []
    obs.flush()  # the session is over
    [turn] = records
    assert turn["intercepted"] == "quick_intents"
    assert turn["reply"] == 0.62 and turn["llm"] is None
    assert turn["hear"]["total"] == 0.352  # transcript at 1.15 plus the interceptor
    assert turn["tts"]["direct"] is True


async def test_a_pause_mid_sentence_counts_segments_and_times_from_the_last(tmp_path):
    obs, feed, records = observer(tmp_path)
    await feed(0.0, VADUserStartedSpeakingFrame(start_secs=0.2))
    await feed(0.0, UserStartedSpeakingFrame())
    await feed(1.0, VADUserStoppedSpeakingFrame(stop_secs=0.2))
    await feed(1.2, said("first half"), STT)
    obs.kept_open()
    await feed(1.5, VADUserStartedSpeakingFrame(start_secs=0.2))
    await feed(2.5, VADUserStoppedSpeakingFrame(stop_secs=0.2))
    await feed(2.7, said("second half"), STT)
    obs.held(0.8)
    await feed(3.6, UserStoppedSpeakingFrame())
    await feed(3.6, LLMFullResponseStartFrame(), LLM)
    await feed(4.0, LLMFullResponseEndFrame(), LLM)
    await feed(4.3, BotStartedSpeakingFrame())
    obs.flush()
    [turn] = records
    assert turn["hear"]["segments"] == 2 and turn["hear"]["kept_open"] == 1
    assert turn["reply"] == 2.0  # from 2.3, the end of the second segment
    assert turn["hear"]["stt"] == 0.2


async def test_a_typed_turn_is_timed_from_its_start(tmp_path):
    from ghost_voice.thinking import ThinkingStartFrame

    obs, feed, records = observer(tmp_path)
    await feed(0.0, TTSSpeakFrame("Yeah?"))
    await feed(0.2, BotStartedSpeakingFrame())
    await feed(0.6, BotStoppedSpeakingFrame())
    await feed(10.0, ThinkingStartFrame())
    await feed(10.01, LLMFullResponseStartFrame(), LLM)
    await feed(10.5, LLMFullResponseEndFrame(), LLM)
    await feed(10.8, BotStartedSpeakingFrame())
    await feed(11.5, BotStoppedSpeakingFrame())
    opening, turn = records
    assert opening["kind"] == "opening" and opening["reply"] == 0.2
    assert turn["kind"] == "text" and turn["reply"] == 0.8 and "hear" not in turn


async def test_a_new_turn_before_ghost_answered_marks_the_old_one_interrupted(tmp_path):
    obs, feed, records = observer(tmp_path)
    await feed(0.0, UserStartedSpeakingFrame())
    await feed(1.0, VADUserStoppedSpeakingFrame(stop_secs=0.2))
    await feed(1.5, UserStoppedSpeakingFrame())
    await feed(1.6, LLMFullResponseStartFrame(), LLM)
    await feed(2.0, UserStartedSpeakingFrame())  # the operator talks over the wait
    [turn] = records
    assert turn["flags"] == ["interrupted"] and turn["reply"] is None


def test_rotation_keeps_one_generation(tmp_path):
    path = tmp_path / "latency.jsonl"
    for i in range(30):
        append_record(path, {"turn": i, "pad": "x" * 100}, max_bytes=1000)
    assert path.with_name("latency.jsonl.1").exists()
    assert path.stat().st_size < 1200
    turns = [r["turn"] for r in read_records(path)]
    assert turns == sorted(turns) and turns[-1] == 29 and len(turns) < 30


def test_percentiles_and_summary():
    assert percentile([1, 2, 3, 4], 50) == 2.5
    assert percentile([5], 95) == 5
    assert percentile([], 50) is None
    recs = [{"reply": r, "hear": {"total": r / 2}} for r in (1.0, 2.0, 3.0)]
    s = summarize(recs)
    assert s["reply"]["p50"] == 2.0 and s["hear"]["n"] == 3
    assert cache_share([{"in": 10, "cache_read": 90, "cache_write": 0}]) == 0.9
    assert cache_share([]) is None
