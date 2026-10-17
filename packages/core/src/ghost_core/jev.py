"""Jev (TypeSafe System One) as a fast classifier for what the operator said.

Jev answers typed questions over JSON state in about 100 ms. Here it judges
*words the operator spoke*; it never decides permission (that is
``ghost_core.policy``) and it is never on the passive path, where audio must
stay on the machine. Every failure, timeout, or low-confidence answer falls back
to the regex classifier, so a missing key or a network blip changes latency, not
behaviour.

Keys: ``TYPESAFE_API_KEY`` for TypeSafe directly, or ``OPENROUTER_API_KEY`` to
reach the same model through OpenRouter. Values are read from the environment
and never logged.
"""

from __future__ import annotations

import asyncio
import time
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any

import httpx

from ghost_core.confirmation import (
    ConfirmationClassifier,
    ConfirmationContext,
    RegexClassifier,
    Verdict,
    has_negation,
)

MODEL = "jev-1.13.0"
TYPESAFE_URL = "https://api.typesafe.ai/v1/systemone"
OPENROUTER_URL = "https://openrouter.ai/api/v1/systemone"
OPENROUTER_MODEL = "typesafe/jev-1.13"  # OpenRouter rejects the bare TypeSafe id

JEV_MODES = ("auto", "on", "off")

# The seven things a short operator turn can mean, shared by the confirmation
# classifier and the intent experiment (docs/benchmarks/scripts/jev_eval.py).
INTENTS: dict[str, str] = {
    "affirm": (
        "explicitly agrees to do pending_action now: yes, yeah, yep, go ahead, do it, "
        "that's right, sure, confirm, close it. A general wrap-up such as 'that's it', "
        "'all done', 'we're good' or 'that'll do' is NOT affirm; it is sleep."
    ),
    "deny": (
        "does not want pending_action done: no, nope, nah, don't, wait, hold on, hold off, "
        "stop, not yet, not that one, leave it open. Anything starting with 'no' is deny."
    ),
    "sleep": (
        "wants to end the conversation: go to sleep, that's all, that's it, all done, "
        "we're done, we're good, that'll do, thanks ghost, goodnight, bye. Still sleep "
        "when a question is pending."
    ),
    "cancel": (
        "drops the current topic or proposal without answering yes or no: never mind, "
        "forget it, scratch that, skip it, drop it, moving on, next one, ignore that."
    ),
    "repeat": (
        "asks to hear the last thing again: what, say that again, repeat that, pardon, "
        "sorry, huh, come again, one more time, didn't catch that, read that again."
    ),
    "ask": (
        "a new request, question, or instruction for the assistant about issues or "
        "anything else, including 'label it bug' or 'what time is it'."
    ),
    "noise": (
        "not addressed to the assistant: song lyrics, talking to another person, a stray "
        "fragment, a filler sound like um or oh, another assistant's name."
    ),
}


class JevError(RuntimeError):
    pass


@dataclass(frozen=True)
class JevAnswer:
    choice: str
    confidence: float
    probabilities: Mapping[str, float]
    directed: float | None  # P(utterance is for the assistant), when asked
    seconds: float
    input_tokens: int
    model: str


def confirmation_questions() -> dict[str, Any]:
    return {
        "intent": {
            "type": "choice",
            "instructions": (
                "The assistant just said `question`; `pending_action` is the change waiting "
                "for a yes, or '(nothing pending)'. The operator then said `utterance`. "
                "Classify what the operator meant by the words of `utterance` alone. Do not "
                "assume agreement: only an explicit yes or go-ahead is affirm. If the words "
                "fit a wrap-up, choose sleep; if they fit a refusal, choose deny."
            ),
            "criteria": dict(INTENTS),
        },
        "directed": {
            "type": "noul",
            "instructions": "Is `utterance` spoken to the assistant at all?",
            "criteria": {
                "true": "a reply, command, or question aimed at the assistant",
                "false": "song lyrics, a fragment of another conversation, or noise",
            },
        },
    }


def confirmation_state(utterance: str, ctx: ConfirmationContext) -> dict[str, Any]:
    return {
        "utterance": utterance,
        "question": ctx.question or f"Shall I {ctx.pending_action}?",
        "pending_action": ctx.pending_action or "(nothing pending)",
    }


class JevClient:
    def __init__(
        self,
        api_key: str,
        *,
        openrouter: bool = False,
        model: str = MODEL,
        timeout: float = 1.0,
        base_url: str | None = None,
        http: httpx.AsyncClient | None = None,
    ) -> None:
        self.openrouter = openrouter
        self.model = OPENROUTER_MODEL if openrouter else model
        self.url = base_url or (OPENROUTER_URL if openrouter else TYPESAFE_URL)
        self._http = http or httpx.AsyncClient(
            timeout=timeout,
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
                "User-Agent": "ghost-voice-triage",
            },
        )

    @classmethod
    def from_env(cls, mode: str, env: Mapping[str, str], **kwargs: Any) -> JevClient | None:
        """``auto``: a client when a key is present, else None. ``on``: a client or a
        config error. ``off``: None."""
        if mode not in JEV_MODES:
            raise ValueError(f"GHOST_JEV must be one of {', '.join(JEV_MODES)}, got {mode!r}")
        if mode == "off":
            return None
        if env.get("TYPESAFE_API_KEY"):
            return cls(env["TYPESAFE_API_KEY"], openrouter=False, **kwargs)
        if env.get("OPENROUTER_API_KEY"):
            return cls(env["OPENROUTER_API_KEY"], openrouter=True, **kwargs)
        if mode == "on":
            raise ValueError(
                "GHOST_JEV=on but neither TYPESAFE_API_KEY nor OPENROUTER_API_KEY is set"
            )
        return None

    @property
    def provider(self) -> str:
        return "openrouter" if self.openrouter else "typesafe"

    async def aclose(self) -> None:
        await self._http.aclose()

    async def ask(self, state: Any, questions: Mapping[str, Any]) -> dict[str, Any]:
        """One request. Returns the raw response with ``seconds`` added, or raises JevError.
        Error bodies are never included: they could echo the submitted state."""
        body = {"model": self.model, "state": state, "questions": dict(questions)}
        started = time.perf_counter()
        try:
            r = await self._http.post(self.url, json=body)
        except httpx.HTTPError as e:
            raise JevError(f"{type(e).__name__}") from None
        if r.status_code != 200:
            raise JevError(f"HTTP {r.status_code}")
        try:
            data = r.json()
        except ValueError:
            raise JevError("malformed JSON") from None
        answers = data.get("answers") if isinstance(data, dict) else None
        if not isinstance(answers, dict) or set(questions) - set(answers):
            raise JevError("missing answers")
        data["seconds"] = time.perf_counter() - started
        return data

    async def classify_utterance(self, utterance: str, ctx: ConfirmationContext) -> JevAnswer:
        data = await self.ask(confirmation_state(utterance, ctx), confirmation_questions())
        intent = data["answers"]["intent"]
        directed = data["answers"].get("directed", {}).get("noul")
        return JevAnswer(
            choice=str(intent.get("choice")),
            confidence=float(intent.get("confidence") or 0.0),
            probabilities=dict(intent.get("probabilities") or {}),
            directed=float(directed) if directed is not None else None,
            seconds=float(data["seconds"]),
            input_tokens=int((data.get("usage") or {}).get("input_tokens") or 0),
            model=str(data.get("model") or self.model),
        )


class JevClassifier:
    """Jev decides affirm / deny; anything else, or any doubt, defers to ``fallback``.

    ``deadline`` is the whole budget including the network: speech is waiting. Measured
    from the workstation through OpenRouter: p50 160 ms, p95 280 ms
    (docs/benchmarks/scripts/jev_eval.py), so 0.5 s rarely falls back.
    ``on_decision`` receives one line per call for the audio debug log; it is never
    given the key and is silent unless GHOST_DEBUG_AUDIO is on.
    """

    name = "jev"

    def __init__(
        self,
        client: JevClient,
        *,
        fallback: ConfirmationClassifier | None = None,
        threshold: float = 0.9,
        deadline: float = 0.5,
        on_decision: Callable[[str], None] | None = None,
    ) -> None:
        self._client = client
        self._fallback = fallback or RegexClassifier()
        self._threshold = threshold
        self._deadline = deadline
        self._log = on_decision or (lambda _msg: None)

    async def classify(self, utterance: str, context: ConfirmationContext) -> Verdict:
        if has_negation(utterance):
            self._log(f"jev: skipped, negation guard -> NEGATIVE for {utterance!r}")
            return Verdict.NEGATIVE
        try:
            a = await asyncio.wait_for(
                self._client.classify_utterance(utterance, context), self._deadline
            )
        except (TimeoutError, JevError, ValueError, KeyError, TypeError) as e:
            self._log(f"jev: {type(e).__name__}, falling back to regex for {utterance!r}")
            return await self._fallback.classify(utterance, context)
        self._log(
            f"jev: {a.choice} conf={a.confidence:.2f} directed={a.directed} "
            f"{a.seconds * 1000:.0f}ms for {utterance!r}"
        )
        if a.confidence < self._threshold or (a.directed is not None and a.directed < 0.5):
            return await self._fallback.classify(utterance, context)
        if a.choice == "affirm":
            return Verdict.AFFIRMATIVE
        if a.choice in {"deny", "cancel"}:  # "never mind" to "close it?" is a no
            return Verdict.NEGATIVE
        return Verdict.UNCLEAR
