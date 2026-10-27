"""The webhook listener (ADR 0027): one route, the body capped before it is read, the
reply never echoes it."""

from __future__ import annotations

import json

import httpx
from ghost_core import Store
from ghost_core.hooks import H_EVENT, H_ID, H_SIG, H_TIME, HookIntake, parse_hook, sign_hmac
from ghost_core.models import utcnow
from ghost_core.waiting import Items
from ghostd.hooks import MAX_BODY, HooksListener, hooks_app

JOBS = {
    "verify": "hmac",
    "secret": "${JOB_SECRET}",
    "events": {
        "done": {
            "fields": {
                "job": {"at": "job", "pattern": r"^[\w-]+$"},
                "ok": {"at": "ok", "type": "bool"},
            },
            "when": [{"field": "ok", "op": "==", "value": False}],
            "key": "job:{job}",
            "title": "{job} failed",
        }
    },
}


def wired():
    store = Store()
    intake = HookIntake(Items(store), store, environ={"JOB_SECRET": "k"})
    hook = parse_hook("jobs", JOBS, "ghost", "test")
    intake.hooks[hook.path] = hook
    return intake, hook


def signed(body: bytes, delivery="d1"):
    ts = str(int(utcnow().timestamp()))
    return {H_TIME: ts, H_ID: delivery, H_EVENT: "done", H_SIG: sign_hmac("k", ts, delivery, body)}


async def test_only_post_to_a_declared_hook_reaches_the_intake():
    intake, hook = wired()
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(hooks_app(intake)), base_url="http://x"
    ) as c:
        body = json.dumps({"job": "backup", "ok": False}).encode()
        r = await c.post(hook.path, content=body, headers=signed(body))
        assert r.status_code == 202 and r.json() == {"ok": True}
        assert [i.title for i in intake._items.open()] == ["backup failed"]
        assert (await c.get(hook.path)).status_code == 405
        assert (await c.post("/hooks/ghost", content=b"{}")).status_code == 404
        assert (await c.post("/status", content=b"{}")).status_code == 404
        assert (await c.post("/hooks/ghost/nope", content=b"{}")).status_code == 404
        bad = await c.post(
            hook.path, content=body, headers={**signed(body, "d2"), H_SIG: "sha256=00"}
        )
        assert bad.status_code == 401 and bad.json() == {"error": "refused"}  # no reason, no echo
        big = await c.post(hook.path, content=b"x" * (MAX_BODY + 1), headers=signed(b""))
        assert big.status_code == 413


async def test_the_listener_binds_its_own_address():
    intake, hook = wired()
    listener = HooksListener("127.0.0.1:0", intake)
    await listener.start()
    try:
        body = json.dumps({"job": "render", "ok": False}).encode()
        async with httpx.AsyncClient() as c:
            r = await c.post(
                f"http://127.0.0.1:{listener.port}{hook.path}", content=body, headers=signed(body)
            )
        assert r.status_code == 202
    finally:
        await listener.stop()
