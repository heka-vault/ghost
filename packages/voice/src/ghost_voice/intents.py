"""Fixed intents that need no model: sleep, cancel, undo (the Jev experiment showed a
short exact-match list is nearly as good as a classifier for these, with nothing on
the network path). An in-tree ``TurnInterceptor``; abilities can add their own."""

from __future__ import annotations

from ghost_core import Handled, UserTurn
from ghost_core.confirmation import normalize

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
UNDO = {"undo", "undo that", "undo the last change", "take that back", "revert that"}


class QuickIntents:
    name = "quick_intents"

    async def on_user_turn(self, turn: UserTurn) -> Handled | None:
        norm = normalize(turn.text)
        words = norm.split()
        if not words or len(words) > 6:
            return None
        # Try the words as heard and with fillers ("ghost", "okay") stripped.
        phrases = {norm, " ".join(w for w in words if w not in _FILLERS)}
        if phrases & SLEEP:
            return Handled(say="Okay.", action="sleep")
        if phrases & UNDO:
            return Handled(action="undo")
        if turn.pending and phrases & CANCEL:
            return Handled(say="Okay, dropped.", action="cancel")
        return None
