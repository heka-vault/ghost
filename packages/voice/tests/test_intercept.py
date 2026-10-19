from __future__ import annotations

import pytest
from ghost_core import Handled, Slot, UserTurn
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
        # no undo intent: taking a change back is a turn for the model
        ("undo that", False, None),
        ("Ghost, undo that.", False, None),
        ("take that back", False, None),
        ("go to sleep after you close it", False, None),
        ("what needs my sign off", False, None),
        # a goodbye at the end, after small talk
        ("I'll talk to you later. Hang up.", False, "sleep"),
        ("Okay, thanks, bye.", False, "sleep"),
        ("Alright, I'm done.", False, "sleep"),
        ("That is all I need. I will talk to you later.", False, "sleep"),
        ("I think that is everything for today. Bye.", False, "sleep"),
        ("goodbye", True, "sleep"),
        # anything else first goes to the model, which does the work
        ("close it and then hang up", False, None),
        ("label it bug, bye", False, None),
        ("don't hang up", False, None),
        ("not yet, don't hang up", False, None),
        ("how do I hang up", False, None),
    ],
)
async def test_quick_intents(text, pending, expect):
    h = await QuickIntents().on_user_turn(UserTurn(text, pending, ""))
    assert (h.action if h else None) == expect


class FakeGate:
    def __init__(self, pending=False):
        self._pending = pending
        self.cancelled = []

    def pending(self, session_id):
        return object() if self._pending else None

    def cancel(self, session_id, reason=""):
        self.cancelled.append(reason)


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


async def test_cancel_action():
    gate = FakeGate(pending=True)
    proc, pushed, _ = make([Always(Handled(say="Dropped.", action="cancel"))], gate)
    await proc.process_frame(said("never mind"), FrameDirection.DOWNSTREAM)
    assert gate.cancelled == ["operator cancelled"]
    assert [f.text for f in pushed if isinstance(f, TTSSpeakFrame)] == ["Dropped."]


async def test_undo_that_reaches_the_model():
    gate = FakeGate()
    proc, pushed, slept = make([QuickIntents()], gate)
    await proc.process_frame(said("Undo that."), FrameDirection.DOWNSTREAM)
    assert [f.text for f in pushed if isinstance(f, TranscriptionFrame)] == ["Undo that."]
    assert not any(isinstance(f, TTSSpeakFrame) for f in pushed)
    assert gate.cancelled == [] and slept == []


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


async def test_the_shortcut_says_a_warm_goodbye_not_a_bare_okay():
    from ghost_voice.intents import GOODBYES

    h = await QuickIntents().on_user_turn(UserTurn("I'll talk to you later. Hang up.", False, ""))
    assert h is not None and h.say in GOODBYES and h.say != "Okay."
