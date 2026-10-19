"""Ghost hearing itself is dropped; the operator repeating a few of its words is not."""

from __future__ import annotations

from ghost_voice.echo import EchoFilter, echo_share


class Clock:
    def __init__(self) -> None:
        self.t = 100.0

    def __call__(self) -> float:
        return self.t


def stopped(f: EchoFilter, clock: Clock, at: float) -> None:
    f._speaking = False  # noqa: SLF001
    f._stopped_at = at  # noqa: SLF001
    clock.t = at


def test_echo_share_counts_runs_of_spoken_words() -> None:
    assert echo_share("Hey, what can I do?", "Hey. What can I do?") == 1.0
    assert echo_share("yes close it", "Close it with a note saying so?") < 0.8
    assert echo_share("anything", "") == 0.0


def test_late_echo_of_the_greeting_is_dropped() -> None:
    clock = Clock()
    f = EchoFilter(clock=clock)
    f.spoke("Hey. What can I do?")
    stopped(f, clock, 100.0)
    clock.t = 101.5  # back over the network, after Ghost stopped
    assert f.is_echo("Hey, what can I do?")


def test_echo_outside_the_window_is_kept() -> None:
    clock = Clock()
    f = EchoFilter(clock=clock, window_secs=3.0)
    f.spoke("Hey. What can I do?")
    stopped(f, clock, 100.0)
    clock.t = 104.0
    assert not f.is_echo("Hey, what can I do?")


def test_short_answers_always_pass() -> None:
    clock = Clock()
    f = EchoFilter(clock=clock)
    f.spoke("Infra 77 looks fixed. Close it with a note saying so?")
    f._speaking = True  # noqa: SLF001
    assert not f.is_echo("yes")
    assert not f.is_echo("close it")
    assert not f.is_echo("yeah close it")


def test_new_words_are_not_echo() -> None:
    clock = Clock()
    f = EchoFilter(clock=clock)
    f.spoke("Approved 142. Want to move on to the next one?")
    stopped(f, clock, 100.0)
    clock.t = 100.5
    assert not f.is_echo("no let's look at the dark mode one")
