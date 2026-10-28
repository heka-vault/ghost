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
    hook = parse_hook("jobs", JOBS, "test")
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
        assert hook.path == "/hooks/jobs"  # ADR 0035: by name alone
        assert (await c.post("/hooks/nope", content=b"{}")).status_code == 404
        assert (await c.post("/status", content=b"{}")).status_code == 404
        assert (await c.post("/hooks/ghost/jobs", content=b"{}")).status_code == 404
        assert (await c.post("/hooks/", content=b"{}")).status_code == 404
        bad = await c.post(
            hook.path, content=body, headers={**signed(body, "d2"), H_SIG: "sha256=00"}
        )
        assert bad.status_code == 401 and bad.json() == {"error": "refused"}  # no reason, no echo
        big = await c.post(hook.path, content=b"x" * (MAX_BODY + 1), headers=signed(b""))
        assert big.status_code == 413


async def test_a_signature_header_with_a_high_byte_is_refused_not_a_crash():
    intake, hook = wired()
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(hooks_app(intake)), base_url="http://x"
    ) as c:
        body = json.dumps({"job": "backup", "ok": False}).encode()
        headers = {**signed(body), H_SIG: b"sha256=\xff"}
        r = await c.post(hook.path, content=body, headers=headers)
        assert r.status_code == 401 and r.json() == {"error": "refused"}


async def test_an_unknown_hook_is_not_found_before_its_body_is_read():
    intake, _ = wired()
    app = hooks_app(intake)
    sent: list[dict] = []

    async def receive():
        raise AssertionError("the body was read")

    async def send(msg):
        sent.append(msg)

    scope = {"type": "http", "method": "POST", "path": "/hooks/nope", "headers": []}
    await app(scope, receive, send)
    assert sent[0]["status"] == 404


def test_push_with_the_listener_down_says_so_in_one_line(tmp_path, monkeypatch, capsys):
    import socket

    from ghostd.main import push_cli

    with socket.socket() as s:  # a port nothing listens on
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    (tmp_path / "hooks.json").write_text(json.dumps({"jobs": JOBS}))
    monkeypatch.setenv("GHOST_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("GHOST_HOOKS_LISTEN", f"127.0.0.1:{port}")
    monkeypatch.setenv("JOB_SECRET", "k")
    assert push_cli(["jobs", "done", '{"job": "backup", "ok": false}']) == 1
    err = capsys.readouterr().err.strip()
    assert "\n" not in err and "could not reach" in err
    # the older <department>/<hook> form is not a hook name
    assert push_cli(["ghost/jobs", "done"]) == 2
    assert "usage: ghostd push <hook>" in capsys.readouterr().err


def test_the_journal_never_prints_variable_values_in_a_traceback(monkeypatch):
    # loguru's diagnose shows each frame's variables: once, a raw body and os.environ.
    import pytest
    from ghostd import main

    added: list[dict] = []
    monkeypatch.setattr(main.logger, "remove", lambda *a: None)
    monkeypatch.setattr(main.logger, "add", lambda *a, **kw: added.append(kw))
    with pytest.raises(SystemExit):
        main.cli(["no-such-command"])
    assert added and all(kw.get("diagnose") is False for kw in added)


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
