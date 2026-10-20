"""The latency harness's model-free parts and `ghostd latency`."""

from __future__ import annotations

import asyncio
import json

from ghostd.main import latency_command
from ghostd.perf.audio import FRAME_SECS, IN_RATE, Mouth, Utterance
from ghostd.perf.harness import check_line, heard_ratio, thinking_for
from ghostd.perf.report import render
from ghostd.perf.scenarios import DEFAULT, SCENARIOS, Line

N = int(IN_RATE * FRAME_SECS) * 2


def voice_record(turn: int, reply: float, *, tools=(), status=None, cache=(100, 3000, 0)):
    calls = [{"name": t, "secs": 0.01, "status": status} for t in tools]
    return {
        "turn": turn,
        "kind": "voice",
        "origin": "desk",
        "spoke": True,
        "reply": reply,
        "hear": {
            "total": 1.2,
            "vad": 0.2,
            "stt": 0.15,
            "pre": 0.0,
            "turn": 0.05,
            "hold": 0.8,
            "segments": 1,
            "kept_open": 0,
        },
        "llm": {
            "total": reply - 1.6,
            "calls": [
                {
                    "ttfb": 1.0,
                    "dur": 1.1,
                    "in": cache[0],
                    "cache_read": cache[1],
                    "cache_write": cache[2],
                    "out": 40,
                    "tools": list(tools),
                }
            ],
        },
        "tools": {"total": 0.01, "calls": calls} if calls else None,
        "tts": {"total": 0.4, "synth": 0.35},
        "other": 0.0,
    }


def test_every_default_scenario_exists_and_lines_are_scripted():
    assert set(DEFAULT) <= set(SCENARIOS)
    for s in SCENARIOS.values():
        assert s.lines and all(line.text.strip() for line in s.lines)


def test_heard_ratio_scores_words_not_punctuation():
    assert heard_ratio("Close infra 17.", "close infra 17") == 1.0
    assert (
        heard_ratio(
            "What needs my sign-off ... in the app repo?", "What needs my sign off in the app repo"
        )
        > 0.8
    )
    assert heard_ratio("Approve app 151.", "approve at one") < 0.5


def test_check_line_reports_drift_without_text():
    line = Line("x", tools=frozenset({"change"}), status="done:request")
    ok = [voice_record(2, 3.0, tools=("change",), status="done:request")]
    assert check_line(line, ok, ended=False) == []
    drift = [voice_record(2, 3.0, tools=("get_issue",), status="ok")]
    problems = check_line(line, drift, ended=False)
    assert "expected ['change']" in problems[0] and "expected status" in problems[1]
    assert check_line(Line("bye", ends=True), [], ended=False) == [
        "expected the session to end",
        "hung up without a spoken goodbye",
    ]
    silent = voice_record(2, 3.0, tools=("hang_up",)) | {"spoke": False, "reply": None}
    assert check_line(Line("bye", ends=True), [silent], ended=True) == [
        "hung up without a spoken goodbye"
    ]


def test_haiku_runs_without_between_tools():
    assert thinking_for("claude-haiku-5-5", None) == "disabled"
    assert thinking_for("claude-sonnet-5-5", None) is None
    assert thinking_for("claude-haiku-5-5", "adaptive") == "adaptive"


async def test_mouth_plays_lines_then_silence_and_releases_on_stop():
    mouth = Mouth()
    done = mouth.say(Utterance("hi", b"\x01\x00" * (N // 2 * 3)))  # three frames
    frames = [mouth.next_frame(N) for _ in range(3)]
    assert all(f != bytes(N) for f in frames) and not done.is_set()
    assert mouth.next_frame(N) == bytes(N) and done.is_set()  # then silence
    pending = mouth.say(Utterance("bye", b"\x01\x00" * N))
    mouth.release()  # the session ended before the line was sent
    await asyncio.wait_for(pending.wait(), 1)


def test_report_ranks_contributors_and_shows_the_cache():
    runs = [
        {
            "scenario": "lookup",
            "config": "claude-sonnet-5-5 proxy",
            "run": i,
            "wall": 20.0,
            "wake_to_voice": 0.4,
            "problems": [],
            "heard": [1.0],
            "audible": [3.1],
            "records": [
                {"turn": 1, "kind": "opening", "spoke": True, "reply": 0.35, "build": 0.03},
                voice_record(2, 3.0 + i, tools=("list_awaiting_signoff",), status="ok"),
            ],
        }
        for i in range(2)
    ]
    md = render(runs, meta={"date": "2026-10-19"})
    assert "Where a voice turn's time goes" in md
    assert md.index("LLM (all calls on the path)") < md.index("turn hold (grace)")
    assert "| lookup | 2/2 | 2 |" in md
    assert "list_awaiting_signoff" in md and "97%" in md  # 3000 of 3100 input from cache


def test_latency_command_summarizes_a_file(tmp_path, capsys):
    path = tmp_path / "latency.jsonl"
    path.write_text(
        "\n".join(
            json.dumps(voice_record(i, 2.0 + i / 10) | {"ts": "2026-10-19T17:00:00Z"})
            for i in range(5)
        )
    )
    assert latency_command(["--file", str(path), "--last", "2"]) == 0
    out = capsys.readouterr().out
    assert "reply" in out and "p50" in out and "last 2 turns" in out
    assert latency_command(["--file", str(tmp_path / "missing.jsonl")]) == 1
