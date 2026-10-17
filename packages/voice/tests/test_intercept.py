from __future__ import annotations

import pytest
from ghost_core import GateError, Handled, Slot, UserTurn
from ghost_voice.intents import QuickIntents
from ghost_voice.intercept import InterceptorProcessor
from ghost_voice.pipeline import assemble
from pipecat.frames.frames import (
    BotStartedSpeakingFrame,
    TranscriptionFrame,
    TTSSpeakFrame,
)
from pipecat.processors.frame_processor import FrameDirection


@pytest.mark.parametrize(
    "text,pending,expect",
    [
        ("Ghost, go to sleep.", False, "sleep"),
        ("thanks ghost", False, "sleep"),
        ("That's all for now.", True, "sleep"),
        ("never mind", True, "cancel"),
        ("never mind", False, None),  # nothing pending: let the model handle it
        ("undo that", False, "undo"),
        ("go to sleep after you close it", False, None),
        ("what needs my sign off", False, None),
    ],
)
async def test_quick_intents(text, pending, expect):
    h = await QuickIntents().on_user_turn(UserTurn(text, pending, ""))
    assert (h.action if h else None) == expect


class FakeGate:
    def __init__(self, pending=False, undo_ok=True):
        self._pending = pending
        self.cancelled = []
        self.undo_ok = undo_ok

    def pending(self, session_id):
        return object() if self._pending else None

    def cancel(self, session_id, reason=""):
        self.cancelled.append(reason)

    async def undo(self, session_id):
        if not self.undo_ok:
            raise GateError("There is nothing recent to undo.")

        class Outcome:
            def summary(self):
                return "Applied: remove bug."

        return Outcome()


class Always:
    name = "always"

    def __init__(self, handled):
        self.handled = handled

    async def on_user_turn(self, turn):
        return self.handled


def make(interceptors, gate):
    slept = []

    async def on_sleep():
        slept.append(True)

    proc = InterceptorProcessor(interceptors, gate=gate, session_id="s", on_sleep=on_sleep)
    pushed = []

    async def push_frame(frame, direction=FrameDirection.DOWNSTREAM):
        pushed.append(frame)

    proc.push_frame = push_frame  # type: ignore[method-assign]
    return proc, pushed, slept


def said(text):
    return TranscriptionFrame(text=text, user_id="op", timestamp="0")


async def test_pass_through_when_nobody_handles():
    proc, pushed, _ = make([Always(None)], FakeGate())
    await proc.process_frame(said("hello"), FrameDirection.DOWNSTREAM)
    assert any(isinstance(f, TranscriptionFrame) for f in pushed)


async def test_handled_turn_is_dropped_and_spoken():
    proc, pushed, slept = make([Always(Handled(say="Okay.", action="sleep"))], FakeGate())
    await proc.process_frame(said("go to sleep"), FrameDirection.DOWNSTREAM)
    assert not any(isinstance(f, TranscriptionFrame) for f in pushed)
    assert [f.text for f in pushed if isinstance(f, TTSSpeakFrame)] == ["Okay."]
    assert slept == [True]


async def test_cancel_and_undo_actions():
    gate = FakeGate(pending=True)
    proc, pushed, _ = make([Always(Handled(say="Dropped.", action="cancel"))], gate)
    await proc.process_frame(said("never mind"), FrameDirection.DOWNSTREAM)
    assert gate.cancelled == ["operator cancelled"]
    proc, pushed, _ = make([Always(Handled(action="undo"))], FakeGate())
    await proc.process_frame(said("undo"), FrameDirection.DOWNSTREAM)
    assert [f.text for f in pushed if isinstance(f, TTSSpeakFrame)] == ["Applied: remove bug."]
    proc, pushed, _ = make([Always(Handled(action="undo"))], FakeGate(undo_ok=False))
    await proc.process_frame(said("undo"), FrameDirection.DOWNSTREAM)
    assert "nothing recent" in pushed[0].text


async def test_not_evidence_while_bot_speaks():
    proc, pushed, slept = make([Always(Handled(action="sleep"))], FakeGate())
    await proc.process_frame(BotStartedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await proc.process_frame(said("go to sleep"), FrameDirection.DOWNSTREAM)
    assert slept == [] and any(isinstance(f, TranscriptionFrame) for f in pushed)


async def test_broken_interceptor_is_skipped():
    class Boom:
        name = "boom"

        async def on_user_turn(self, turn):
            raise RuntimeError("x")

    proc, pushed, _ = make([Boom(), Always(Handled(say="hi"))], FakeGate())
    await proc.process_frame(said("x"), FrameDirection.DOWNSTREAM)
    assert [f.text for f in pushed if isinstance(f, TTSSpeakFrame)] == ["hi"]


def test_assemble_splices_slots_in_order():
    core = [(Slot.AUDIO_IN, "in"), (None, "stt"), (Slot.AFTER_STT, "confirm"), (None, "out")]
    out = assemble(core, {Slot.AFTER_STT: ["a", "b"], Slot.AUDIO_IN: ["ns"]})
    assert out == ["in", "ns", "stt", "confirm", "a", "b", "out"]
