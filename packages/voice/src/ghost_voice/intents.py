"""Fixed intents that need no model: sleep and cancel (the Jev experiment showed a
short exact-match list is nearly as good as a classifier for these, with nothing on
the network path). An in-tree ``TurnInterceptor``; abilities can add their own.
"Undo that" is not one: it goes to the model, which proposes the reversing change."""

from __future__ import annotations

import random

from ghost_core import Handled, UserTurn
from ghost_core.confirmation import has_negation, normalize

_FILLERS = {"ghost", "okay", "ok", "hey", "oh", "um", "so", "please"}

SLEEP = {
    "go to sleep",
    "sleep",
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
    "you can go to sleep",
}
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
}
# What Ghost says when the shortcut hangs up. Not a bare "Okay.": Kokoro reads a lone
# word flat and falling, and it came across as sad to be sent away.
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
            return Handled(say=random.choice(GOODBYES), action="sleep") if _goodbye(words) else None
        # Try the words as heard and with fillers ("ghost", "okay") stripped.
        phrases = {norm, " ".join(w for w in words if w not in _FILLERS)}
        if phrases & SLEEP or _goodbye(norm.split()):
            return Handled(say=random.choice(GOODBYES), action="sleep")
        if turn.pending and phrases & CANCEL:
            return Handled(say="Dropped it.", action="cancel")
        return None


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
