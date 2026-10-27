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
        ("thanks ghost", False, "hang_up"),
        ("That's all for now.", True, "hang_up"),
        ("Ghost, go to sleep.", False, None),  # no such phrase any more (ADR 0021)
        # hold: the whole turn asks Ghost to wait
        ("hang on, Ghost", False, "hold"),
        ("Hold on a sec.", True, "hold"),
        ("okay ghost one second", False, "hold"),
        ("don't hold on", False, None),
        ("hold on, close it first", False, None),
        ("never mind", True, "cancel"),
        ("never mind", False, None),  # nothing pending: let the model handle it
        # no undo intent: taking a change back is a turn for the model
        ("undo that", False, None),
        ("Ghost, undo that.", False, None),
        ("take that back", False, None),
        ("go to sleep after you close it", False, None),
        ("what needs my sign off", False, None),
        # a goodbye at the end, after small talk
        ("I'll talk to you later. Hang up.", False, "hang_up"),
        ("Okay, thanks, bye.", False, "hang_up"),
        ("Alright, I'm done.", False, "hang_up"),
        ("That is all I need. I will talk to you later.", False, "hang_up"),
        ("I think that is everything for today. Bye.", False, "hang_up"),
        ("goodbye", True, "hang_up"),
        ("Uh never mind. I will talk to you later.", False, "hang_up"),
        ("Nothing else, gotta go.", True, "hang_up"),
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
        self.set_aside_for = []

    def pending(self, session_id):
        return object() if self._pending else None

    def cancel(self, session_id, reason=""):
        self.cancelled.append(reason)

    def set_aside(self, session_id, reason=""):
        self.set_aside_for.append(reason)


class Always:
    name = "always"

    def __init__(self, handled):
        self.handled = handled

    async def on_user_turn(self, turn):
        return self.handled


def make(interceptors, gate, dropped=None):
    slept = []

    async def on_sleep():
        slept.append(True)

    async def on_handled():
        if dropped is not None:
            dropped.append(True)

    proc = InterceptorProcessor(
        interceptors, gate=gate, session_id="s", on_hang_up=on_sleep, on_handled=on_handled
    )
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
    proc, pushed, slept = make([Always(Handled(say="Okay.", action="hang_up"))], FakeGate())
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
    proc, pushed, slept = make([Always(Handled(action="hang_up"))], FakeGate())
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

    h = await QuickIntents().on_user_turn(UserTurn("I'll talk to you later. Hang up.", False, ""))
    from ghost_voice.intents import GOODBYES

    assert h is not None and h.say in GOODBYES and h.say != "Okay." and h.action == "hang_up"


async def test_a_goodbye_in_pieces_is_caught_and_the_earlier_pieces_are_taken_back():
    """STT hands "All right. Good." and then "I will talk to you later." as two frames. The
    first went down to the aggregator; the goodbye takes it back (on_handled) so the model
    never runs on it, and the whole turn is judged together ("I will talk to you" + "later")."""
    from pipecat.frames.frames import UserStartedSpeakingFrame

    dropped: list = []
    proc, pushed, slept = make([QuickIntents()], FakeGate(), dropped)
    await proc.process_frame(UserStartedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await proc.process_frame(said("All right. Good."), FrameDirection.DOWNSTREAM)
    assert any(isinstance(f, TranscriptionFrame) for f in pushed) and not slept
    await proc.process_frame(said("I will talk to you later."), FrameDirection.DOWNSTREAM)
    assert slept == [True] and dropped == [True]

    dropped.clear()
    proc, pushed, slept = make([QuickIntents()], FakeGate(), dropped)
    await proc.process_frame(UserStartedSpeakingFrame(), FrameDirection.DOWNSTREAM)
    await proc.process_frame(said("I will talk to you"), FrameDirection.DOWNSTREAM)
    await proc.process_frame(said("later."), FrameDirection.DOWNSTREAM)
    assert slept == [True] and dropped == [True]  # neither piece alone is a goodbye

    # a lone piece that is not a goodbye still passes, and nothing is taken back
    dropped.clear()
    proc, pushed, slept = make([QuickIntents()], FakeGate(), dropped)
    await proc.process_frame(said("what needs my sign off"), FrameDirection.DOWNSTREAM)
    assert not slept and not dropped and isinstance(pushed[-1], TranscriptionFrame)


async def test_do_not_disturb_is_a_fixed_intent_with_its_time():
    from ghost_voice.intents import QuickIntents

    h = await QuickIntents().on_user_turn(
        UserTurn(text="Ghost, nothing for an hour", pending=False, last_assistant="")
    )
    assert h is not None and h.action == "quiet" and h.until is not None
    assert h.say.startswith("Okay, nothing until")
    assert (
        await QuickIntents().on_user_turn(
            UserTurn(text="nothing until later", pending=False, last_assistant="")
        )
        is None
    )
