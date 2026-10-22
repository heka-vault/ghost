from __future__ import annotations

import pytest
from ghost_voice.turns import BargeInUserTurnStartStrategy
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    BotStoppedSpeakingFrame,
    InterimTranscriptionFrame,
    TranscriptionFrame,
    VADUserStartedSpeakingFrame,
)
from pipecat.turns.types import ProcessFrameResult


def strategy(min_words: int = 2):
    s = BargeInUserTurnStartStrategy(min_words=min_words)
    events: list[str] = []

    @s.event_handler("on_user_turn_started")
    async def _started(_s, _params):
        events.append("started")

    @s.event_handler("on_reset_aggregation")
    async def _reset(_s):
        events.append("reset")

    return s, events


def vad_start():
    return VADUserStartedSpeakingFrame(start_secs=0.2, timestamp=0.0)


def said(text: str):
    return TranscriptionFrame(text=text, user_id="op", timestamp="0")


@pytest.mark.asyncio
async def test_vad_starts_turn_while_bot_silent():
    s, events = strategy()
    assert await s.process_frame(vad_start()) is ProcessFrameResult.STOP
    assert events == ["started"]


@pytest.mark.asyncio
async def test_vad_ignored_while_bot_speaking():
    s, events = strategy()
    await s.process_frame(BotStartedSpeakingFrame())
    assert await s.process_frame(vad_start()) is ProcessFrameResult.CONTINUE
    assert events == []


@pytest.mark.asyncio
async def test_short_fragment_while_bot_speaking_is_dropped():
    s, events = strategy(min_words=2)
    await s.process_frame(BotStartedSpeakingFrame())
    assert await s.process_frame(said("the")) is ProcessFrameResult.CONTINUE
    assert events == ["reset"]


@pytest.mark.asyncio
async def test_enough_words_while_bot_speaking_interrupts():
    s, events = strategy(min_words=2)
    await s.process_frame(BotStartedSpeakingFrame())
    assert await s.process_frame(said("stop that")) is ProcessFrameResult.STOP
    assert events == ["started"]


@pytest.mark.asyncio
async def test_two_word_interim_while_bot_speaking_interrupts():
    s, events = strategy(min_words=2)
    await s.process_frame(BotStartedSpeakingFrame())
    partial = InterimTranscriptionFrame(text="wait", user_id="op", timestamp="0")
    assert await s.process_frame(partial) is ProcessFrameResult.CONTINUE
    assert events == []  # nothing was aggregated to drop; the next interim may say more
    more = InterimTranscriptionFrame(text="wait stop", user_id="op", timestamp="0")
    assert await s.process_frame(more) is ProcessFrameResult.STOP
    assert events == ["started"]


@pytest.mark.asyncio
async def test_zero_min_words_disables_barge_in():
    s, events = strategy(min_words=0)
    await s.process_frame(BotStartedSpeakingFrame())
    assert await s.process_frame(said("please stop talking now")) is ProcessFrameResult.CONTINUE
    assert events == ["reset"]


@pytest.mark.asyncio
async def test_single_word_starts_turn_once_bot_stopped():
    s, events = strategy()
    await s.process_frame(BotStartedSpeakingFrame())
    await s.process_frame(BotStoppedSpeakingFrame())
    assert await s.process_frame(said("yes")) is ProcessFrameResult.STOP
    assert events == ["started"]


@pytest.mark.asyncio
async def test_turn_start_clears_bot_speaking():
    s, _ = strategy()
    await s.process_frame(BotStartedSpeakingFrame())
    await s.handle_user_turn_started()
    assert s.bot_speaking is False


class _Tasks:
    """Just enough of a task manager for the grace timer."""

    def create_task(self, coro, name):
        import asyncio

        return asyncio.ensure_future(coro)

    async def cancel_task(self, task):
        task.cancel()


def patient(grace: float):
    from unittest.mock import MagicMock

    from ghost_voice.turns import PatientTurnStopStrategy

    class Testable(PatientTurnStopStrategy):
        task_manager = _Tasks()  # type: ignore[assignment]

    s = Testable(turn_analyzer=MagicMock(), grace_secs=grace)
    stops: list[str] = []

    async def stopped(**_kwargs):
        stops.append("stopped")

    # what Smart Turn's verdict would do without the hold
    from pipecat.turns.user_stop.turn_analyzer_user_turn_stop_strategy import (
        TurnAnalyzerUserTurnStopStrategy,
    )

    return s, stops, stopped, TurnAnalyzerUserTurnStopStrategy


@pytest.mark.asyncio
async def test_end_of_turn_waits_out_the_grace(monkeypatch):
    import asyncio

    s, stops, stopped, base = patient(0.05)
    monkeypatch.setattr(base, "trigger_user_turn_stopped", lambda self, **kw: stopped(**kw))
    await s.trigger_user_turn_stopped()
    assert stops == []
    await asyncio.sleep(0.1)
    assert stops == ["stopped"]


@pytest.mark.asyncio
async def test_speaking_again_keeps_the_turn_open(monkeypatch):
    import asyncio

    s, stops, stopped, base = patient(0.05)
    monkeypatch.setattr(base, "trigger_user_turn_stopped", lambda self, **kw: stopped(**kw))
    await s.trigger_user_turn_stopped()
    await s._release_hold()  # noqa: SLF001 - what VADUserStartedSpeaking does
    await asyncio.sleep(0.1)
    assert stops == []


# --- speculative answer during the grace -------------------------------------------


def speculating(grace: float, text: str = "what needs my sign off"):
    """A patient strategy that speculates, with the turn's text already heard and each
    trigger recorded instead of reaching an aggregator."""
    s, _stops, _stopped, base = patient(grace)
    s._speculate = True  # noqa: SLF001
    s.heard = [text]  # the user aggregator's turn so far
    s.turn_text = lambda: " ".join(s.heard)
    events: list[str] = []

    async def inference(*, speculation=None):
        events.append(f"answer:{speculation.text}" if speculation else "answer")

    async def finalized(*, confirms_speculation=False, **_kw):
        events.append("confirmed" if confirms_speculation else "finalized")

    async def withdrawn():
        events.append("withdrawn")

    s.trigger_user_turn_inference_triggered = inference
    s.trigger_user_turn_finalized = finalized
    s.trigger_user_turn_speculation_cancelled = withdrawn
    return s, events, base


def rerun_into(events: list[str]):
    """Smart Turn's own end of turn: the inference a confirmed turn runs."""

    async def rerun(self, **_kw):
        events.append("rerun")

    return rerun


@pytest.mark.asyncio
async def test_the_answer_starts_at_the_verdict_and_is_kept(monkeypatch):
    import asyncio

    s, events, base = speculating(0.05)
    monkeypatch.setattr(base, "trigger_user_turn_stopped", rerun_into(events))
    await s.trigger_user_turn_stopped()
    assert events == ["answer:what needs my sign off"]
    await asyncio.sleep(0.1)
    # Confirmed without a second inference: the held answer is the answer.
    assert events == ["answer:what needs my sign off", "confirmed"]


@pytest.mark.asyncio
async def test_going_on_withdraws_the_answer_and_keeps_the_turn_open(monkeypatch):
    import asyncio

    from pipecat.frames.frames import VADUserStartedSpeakingFrame

    s, events, base = speculating(0.05)
    monkeypatch.setattr(base, "trigger_user_turn_stopped", rerun_into(events))
    await s.trigger_user_turn_stopped()
    await s.process_frame(VADUserStartedSpeakingFrame(start_secs=0.2))
    await asyncio.sleep(0.1)
    assert events == ["answer:what needs my sign off", "withdrawn"]


@pytest.mark.asyncio
async def test_an_answer_that_reached_a_tool_runs_again_once_confirmed(monkeypatch):
    """The LLM service withdraws a speculative answer that wants a tool, so no tool runs
    for a turn that hadn't ended; the confirmed turn makes the call."""
    import asyncio

    from pipecat.frames.frames import EagerEndOfTurnCancelFrame

    s, events, base = speculating(0.05)
    monkeypatch.setattr(base, "trigger_user_turn_stopped", rerun_into(events))
    await s.trigger_user_turn_stopped()
    await s.process_frame(EagerEndOfTurnCancelFrame())
    await asyncio.sleep(0.1)
    assert events == ["answer:what needs my sign off", "rerun"]


@pytest.mark.asyncio
async def test_changed_words_discard_the_answer(monkeypatch):
    import asyncio

    s, events, base = speculating(0.05)
    monkeypatch.setattr(base, "trigger_user_turn_stopped", rerun_into(events))
    await s.trigger_user_turn_stopped()
    s.heard.append("and the comments")  # a late transcript, no new speech
    await asyncio.sleep(0.1)
    assert events == ["answer:what needs my sign off", "withdrawn", "rerun"]


@pytest.mark.asyncio
async def test_no_words_no_speculation(monkeypatch):
    import asyncio

    s, events, base = speculating(0.05, text="  ")
    monkeypatch.setattr(base, "trigger_user_turn_stopped", rerun_into(events))
    await s.trigger_user_turn_stopped()
    await asyncio.sleep(0.1)
    assert events == ["rerun"]


@pytest.mark.asyncio
async def test_a_turn_that_ends_another_way_withdraws_the_answer():
    s, events, _base = speculating(10)
    await s.trigger_user_turn_stopped()
    await s.handle_user_turn_started()  # a barge-in or a typed turn
    assert events == ["answer:what needs my sign off", "withdrawn"]


@pytest.mark.asyncio
async def test_a_turn_kept_open_is_answered_whole(monkeypatch):
    """The analyzer keeps only the latest transcript; the answer must be to the whole turn
    the aggregator holds, or "what's waiting for triage ... in the app repo" is answered
    as "in the app repo"."""
    from pipecat.frames.frames import VADUserStartedSpeakingFrame

    s, events, base = speculating(10, text="What's waiting for triage?")
    s._text = "What's waiting for triage?"  # noqa: SLF001
    monkeypatch.setattr(base, "trigger_user_turn_stopped", rerun_into(events))
    await s.trigger_user_turn_stopped()
    await s.process_frame(VADUserStartedSpeakingFrame(start_secs=0.2))  # the pause ends
    s.heard.append("In the app repo.")
    s._text = "In the app repo."  # noqa: SLF001 - what the analyzer keeps
    await s.trigger_user_turn_stopped()
    assert events == [
        "answer:What's waiting for triage?",
        "withdrawn",
        "answer:What's waiting for triage? In the app repo.",
    ]


@pytest.mark.asyncio
async def test_words_an_interceptor_took_back_discard_the_answer(monkeypatch):
    import asyncio

    s, events, base = speculating(0.05, text="all right good")
    monkeypatch.setattr(base, "trigger_user_turn_stopped", rerun_into(events))
    await s.trigger_user_turn_stopped()
    s.heard.clear()  # a quick intent handled the turn and reset the aggregator
    await asyncio.sleep(0.1)
    assert events == ["answer:all right good", "withdrawn", "rerun"]
