import asyncio

import httpx
import pytest
import respx
from ghost_core import ConfirmationContext, RegexClassifier, Verdict, has_negation
from ghost_core.jev import (
    OPENROUTER_MODEL,
    OPENROUTER_URL,
    TYPESAFE_URL,
    JevAnswer,
    JevClassifier,
    JevClient,
    JevError,
    confirmation_questions,
)

CTX = ConfirmationContext(pending_action="close acme/app#7", question="Close it?")


def answer(choice="affirm", confidence=0.95, directed=0.99):
    return {
        "model": "jev-1.13.0",
        "answers": {
            "intent": {
                "type": "choice",
                "choice": choice,
                "confidence": confidence,
                "probabilities": {choice: confidence},
            },
            "directed": {"type": "noul", "noul": directed},
        },
        "usage": {"input_tokens": 300, "output_tokens": 0},
    }


class FakeJev:
    def __init__(self, result):
        self.result = result
        self.calls = 0

    async def classify_utterance(self, utterance, ctx):
        self.calls += 1
        if isinstance(self.result, Exception):
            raise self.result
        if self.result == "slow":
            await asyncio.sleep(1)
        return self.result


def ans(choice, confidence=0.95, directed=0.99):
    return JevAnswer(choice, confidence, {choice: confidence}, directed, 0.1, 300, "jev-1.13.0")


async def test_confident_affirm_and_deny():
    assert await JevClassifier(FakeJev(ans("affirm"))).classify("mm hm", CTX) is Verdict.AFFIRMATIVE
    assert await JevClassifier(FakeJev(ans("deny"))).classify("hmm maybe", CTX) is Verdict.NEGATIVE
    assert await JevClassifier(FakeJev(ans("ask"))).classify("what time", CTX) is Verdict.UNCLEAR
    cancel = JevClassifier(FakeJev(ans("cancel")))
    assert await cancel.classify("forget it", CTX) is Verdict.NEGATIVE
    sleep = JevClassifier(FakeJev(ans("sleep")))
    assert await sleep.classify("that's it", CTX) is Verdict.UNCLEAR


async def test_low_confidence_defers_to_regex():
    c = JevClassifier(FakeJev(ans("affirm", confidence=0.85)))  # default threshold is 0.9
    assert await c.classify("confirm", CTX) is Verdict.AFFIRMATIVE  # regex says yes
    assert await c.classify("mm hm", CTX) is Verdict.UNCLEAR  # regex says unclear


async def test_not_directed_defers_to_regex():
    c = JevClassifier(FakeJev(ans("affirm", directed=0.2)))
    assert await c.classify("mm hm", CTX) is Verdict.UNCLEAR


async def test_error_and_timeout_fall_back():
    log = []
    c = JevClassifier(FakeJev(JevError("HTTP 529")), on_decision=log.append)
    assert await c.classify("yes", CTX) is Verdict.AFFIRMATIVE
    assert "falling back" in log[0]
    c = JevClassifier(FakeJev("slow"), deadline=0.01)
    assert await c.classify("yes", CTX) is Verdict.AFFIRMATIVE


async def test_negation_guard_runs_before_jev():
    fake = FakeJev(ans("affirm"))
    c = JevClassifier(fake)
    assert await c.classify("yes but wait", CTX) is Verdict.NEGATIVE
    assert fake.calls == 0


@pytest.mark.parametrize(
    "text,neg", [("no", True), ("yes wait", True), ("yes", False), ("", False)]
)
def test_has_negation(text, neg):
    assert has_negation(text) is neg


def test_from_env_modes():
    assert JevClient.from_env("off", {"TYPESAFE_API_KEY": "k"}) is None
    assert JevClient.from_env("auto", {}) is None
    c = JevClient.from_env("auto", {"OPENROUTER_API_KEY": "k"})
    assert c is not None and c.provider == "openrouter" and c.model == OPENROUTER_MODEL
    c = JevClient.from_env("auto", {"TYPESAFE_API_KEY": "t", "OPENROUTER_API_KEY": "k"})
    assert c.provider == "typesafe" and c.url == TYPESAFE_URL
    with pytest.raises(ValueError, match="GHOST_JEV=on"):
        JevClient.from_env("on", {})
    with pytest.raises(ValueError, match="one of"):
        JevClient.from_env("maybe", {})


@respx.mock
async def test_client_posts_state_and_questions():
    route = respx.post(OPENROUTER_URL).mock(return_value=httpx.Response(200, json=answer()))
    c = JevClient("k", openrouter=True)
    a = await c.classify_utterance("mm hm", CTX)
    body = route.calls[0].request
    import json

    sent = json.loads(body.content)
    assert sent["model"] == OPENROUTER_MODEL
    assert sent["state"]["utterance"] == "mm hm" and sent["state"]["question"] == "Close it?"
    assert set(sent["questions"]) == set(confirmation_questions())
    assert body.headers["Authorization"] == "Bearer k"
    assert a.choice == "affirm" and a.input_tokens == 300 and a.directed == 0.99
    await c.aclose()


@respx.mock
async def test_client_errors_never_include_body():
    respx.post(TYPESAFE_URL).mock(return_value=httpx.Response(422, json={"echo": "secret state"}))
    c = JevClient("k")
    with pytest.raises(JevError) as e:
        await c.ask({"x": 1}, {"q": {"type": "noul", "instructions": "?"}})
    assert "secret" not in str(e.value) and "422" in str(e.value)
    respx.post(TYPESAFE_URL).mock(return_value=httpx.Response(200, json={"answers": {}}))
    with pytest.raises(JevError, match="missing answers"):
        await c.ask({"x": 1}, {"q": {"type": "noul", "instructions": "?"}})
    await c.aclose()


async def test_regex_classifier_matches_function():
    r = RegexClassifier()
    assert await r.classify("go ahead", CTX) is Verdict.AFFIRMATIVE
    assert await r.classify("hold on", CTX) is Verdict.NEGATIVE
