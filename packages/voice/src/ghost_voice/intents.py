"""Fixed intents that need no model: hang up, hold and cancel (the Jev experiment showed
a short exact-match list is nearly as good as a classifier for these, with nothing on
the network path). Ghost's own ``TurnInterceptor``.
"Undo that" is not one: it goes to the model, which proposes the reversing change.

Hold (ADR 0021): "hang on, Ghost" keeps the session but Ghost stops listening until it is
addressed by name again; the room can talk. There is no "go to sleep" any more: a
goodbye hangs up, a wait holds."""

from __future__ import annotations

import random
from datetime import datetime

from ghost_core import Handled, UserTurn
from ghost_core.confirmation import has_negation, normalize
from ghost_core.routes import parse_quiet

_FILLERS = {"ghost", "okay", "ok", "hey", "oh", "um", "so", "please"}

HANG_UP = {
    "that's all",
    "that's all for now",
    "that's all thanks",
    "thanks that's all",
    "thanks that's everything",
    "thanks ghost",
    "thank you ghost",
    "goodnight ghost",
    "good night ghost",
    "bye ghost",
    "we're done",
    "done for now",
    "that's it thanks",
}
# The whole turn (fillers aside) asks Ghost to wait. "Wait" alone is left to the model and
# the confirmation path: with a question pending it is a no, not a hold.
HOLD = {
    "hang on",
    "hang on a sec",
    "hang on a second",
    "hang on a minute",
    "hang on a moment",
    "hold on",
    "hold on a sec",
    "hold on a second",
    "hold on a minute",
    "hold on a moment",
    "hold that thought",
    "one sec",
    "one second",
    "one moment",
    "one minute",
    "just a sec",
    "just a second",
    "just a minute",
    "just a moment",
    "give me a sec",
    "give me a second",
    "give me a minute",
    "give me a moment",
    "wait a sec",
    "wait a second",
    "wait a minute",
    "wait a moment",
    "be right back",
    "stand by",
}
# Two words at least: Kokoro reads a lone "Sure." flat. Short, because the operator has
# already turned to someone else.
HOLD_ACKS = ("Sure, take your time.", "Take your time.", "I'll be here.", "Sure thing.")
# A goodbye at the end of a turn ends the session too, when everything before it is small
# talk: "I'll talk to you later, hang up". Anything else before it ("close it and hang up")
# goes to the model, which does the work first.
GOODBYE_TAILS = (
    "hang up",
    "you can hang up",
    "goodbye",
    "good bye",
    "bye",
    "bye bye",
    "talk to you later",
    "talk later",
    "see you later",
    "see ya",
    "i'm done",
    "we're done",
    "gotta go",
    "got to go",
    "i have to go",
    "i gotta go",
    "have to run",
    "catch you later",
    "later",
    "that's it",
    "all set",
)
_SMALL_TALK = _FILLERS | {
    "alright",
    "all",
    "right",
    "cool",
    "great",
    "thanks",
    "thank",
    "you",
    "i'll",
    "i",
    "will",
    "talk",
    "to",
    "later",
    "that's",
    "it",
    "for",
    "now",
    "uh",
    "yeah",
    "see",
    "ya",
    "good",
    "bye",
    "night",
    "done",
    "i'm",
    "we're",
    "and",
    # "That is all I need, I'll talk to you later"
    "is",
    "that",
    "need",
    "think",
    "today",
    "everything",
    "was",
    "perfect",
    # "Never mind, I'll talk to you later": a dropped thought before the goodbye
    "never",
    "mind",
    "nevermind",
    "anyway",
    "anyways",
    "nothing",
    "else",
    "then",
}
# What Ghost says when the shortcut hangs up. Not a bare "Okay.": Kokoro reads a lone
# word flat and falling, and it came across as sad to be sent away. The goodbye stays
# (it is what makes the end feel human, 2026-10-20); the speed comes from everything
# around it: the instant phrase match, no model round trip, and the phone closing the
# call the moment the goodbye has played.
GOODBYES = ("Talk soon!", "Catch you later!", "Sounds good, talk soon!", "Bye for now!")
CANCEL = {  # only when a change is waiting for a yes
    "never mind",
    "nevermind",
    "forget it",
    "forget that",
    "scratch that",
    "cancel",
    "cancel that",
    "drop it",
    "drop that",
    "leave it",
    "skip it",
}


class QuickIntents:
    name = "quick_intents"

    async def on_user_turn(self, turn: UserTurn) -> Handled | None:
        norm = normalize(turn.text)
        words = norm.split()
        if not words or len(words) > 12:
            return None
        if len(words) > 6:
            return (
                Handled(say=random.choice(GOODBYES), action="hang_up") if _goodbye(words) else None
            )
        # Try the words as heard and with fillers ("ghost", "okay") stripped.
        phrases = {norm, " ".join(w for w in words if w not in _FILLERS)}
        if phrases & HANG_UP or _goodbye(norm.split()):
            return Handled(say=random.choice(GOODBYES), action="hang_up")
        if phrases & HOLD and not has_negation(norm):
            return Handled(say=random.choice(HOLD_ACKS), action="hold")
        if turn.pending and phrases & CANCEL:
            return Handled(say="Dropped it.", action="cancel")
        until = parse_quiet(turn.text, datetime.now().astimezone())
        if until is not None:  # ADR 0031: the operator's words set it; no model
            return Handled(
                say=f"Okay, nothing until {_spoken_time(until)}.", action="quiet", until=until
            )
        return None


def _spoken_time(t: datetime) -> str:
    hour = t.hour % 12 or 12
    half = "in the morning" if t.hour < 12 else ("in the afternoon" if t.hour < 18 else "tonight")
    if t.hour == 12 and t.minute == 0:
        return "noon"
    when = f"{hour}" if t.minute == 0 else f"{hour}:{t.minute:02d}"
    tomorrow = t.date() != datetime.now(t.tzinfo).date()
    return f"{when} {half}" + (" tomorrow" if tomorrow else "")


def _goodbye(words: list[str]) -> bool:
    """Ends with a goodbye, and only small talk comes before it; never with a negation."""
    if has_negation(" ".join(words)) or "don't" in words or "not" in words:
        return False
    while words and words[-1] in _FILLERS:
        words = words[:-1]
    for tail in sorted(GOODBYE_TAILS, key=len, reverse=True):
        t = tail.split()
        if words[-len(t) :] == t:
            return all(w in _SMALL_TALK for w in words[: -len(t)])
    return False
