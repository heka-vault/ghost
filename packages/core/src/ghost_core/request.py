"""The operator's own request as the yes for a ``confirm``-level change.

"Close infra 17" already says yes to closing infra 17; asking "Okay to close infra 17?"
after it is ceremony. When the gate holds a confirm-level proposal, it reads the operator's
words that led to it (the pipeline's transcript, never the model's text) and executes at
once when they plainly ask for exactly that:

- a word for the operation (``Operation.asked_by``: "close", "shut" for close; an app's
  come from the tool's name and short arguments, ADR 0034). One with none always asks;
- the target, named in the request ("17", "seventeen", ``Target.mentions``), or "it",
  "this", "that one" right after Ghost's last reply named it. What names it is what the
  change is about: the repository or the branch it happens in never does;
- nothing hedged or negated: no "no", "don't", "not", "wait", "should", "maybe", "if";
- not a question, except a polite request ("can you reopen seventeen?").

The operation word must be said as is: "close 17" asks, "is 17 closed" does not.

Otherwise Ghost asks once, as before. This is the operator's request, observed: the model
proposing the change is never evidence that the operator asked for it.
"""

from __future__ import annotations

import re
from collections.abc import Iterable

_WORD = re.compile(r"[a-z0-9']+")
_HEDGES = {
    "no",
    "not",
    "don't",
    "dont",
    "never",
    "wait",
    "stop",
    "cancel",
    "should",
    "shouldn't",
    "maybe",
    "might",
    "if",
    "whether",
    "think",
    "probably",
}
_POINTERS = {"it", "this", "that", "one"}
_FILLERS = {"hey", "ghost", "um", "uh", "so", "okay", "ok", "yeah", "and", "please"}
# "Can you close 17?" is a request, however the transcript punctuates it.
_POLITE = {("can", "you"), ("could", "you"), ("would", "you"), ("will", "you"), ("can", "we")}

_ONES = (
    "zero one two three four five six seven eight nine ten eleven twelve thirteen "
    "fourteen fifteen sixteen seventeen eighteen nineteen"
).split()
_TENS = "_ _ twenty thirty forty fifty sixty seventy eighty ninety".split()


def words(text: str) -> list[str]:
    return _WORD.findall(text.lower())


def _under_100(n: int) -> str:
    if n < 20:
        return _ONES[n]
    tens, ones = divmod(n, 10)
    return _TENS[tens] + (f" {_ONES[ones]}" if ones else "")


def number_mentions(n: int) -> set[str]:
    """How a number can come back from STT: "142", "one forty two", "one hundred forty two"."""
    out = {str(n)}
    if 0 <= n < 100:
        out.add(_under_100(n))
    elif 100 <= n < 1000:
        hundreds, rest = divmod(n, 100)
        h = _ONES[hundreds]
        if rest:
            r = _under_100(rest)
            out |= {f"{h} hundred {r}", f"{h} hundred and {r}", f"{h} {r}"}
            if rest < 10:
                out.add(f"{h} oh {r}")
        else:
            out.add(f"{h} hundred")
    return out


def _contains(haystack: list[str], phrase: str) -> bool:
    needle = phrase.split()
    n = len(needle)
    return any(haystack[i : i + n] == needle for i in range(len(haystack) - n + 1))


def asked_for(
    request: str,
    *,
    verbs: Iterable[str],
    mentions: Iterable[str],
    context: str = "",
) -> bool:
    """Whether ``request`` plainly asks for this operation on this target."""
    said = words(request)
    verbs = [v.lower() for v in verbs]
    # Compared word by word, as the transcript is: "README.md" is "readme md".
    mentions = [said_as for m in mentions if (said_as := " ".join(words(m)))]
    if not said or not verbs or not mentions:
        return False
    if any(w in _HEDGES for w in said):
        return False
    if not any(w in verbs for w in said):
        return False
    # Judge the sentence that asks: "Okay, that's fine. Can we close seventeen, please?"
    asking = next(
        (
            s
            for s in re.split(r"(?<=[.!?])\s+", request.strip())
            if any(w in verbs for w in words(s))
        ),
        request.strip(),
    )
    if asking.endswith("?"):
        lead = [w for w in words(asking) if w not in _FILLERS][:2]
        if tuple(lead) not in _POLITE:
            return False
    if any(_contains(said, m) for m in mentions):
        return True
    ghost = words(context)
    return any(w in _POINTERS for w in said) and any(_contains(ghost, m) for m in mentions)
