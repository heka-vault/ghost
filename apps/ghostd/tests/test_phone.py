"""The phone listener (ADR 0016, docs/phone-protocol.md): pairing, signatures both ways,
admission, and a real WebRTC offer/answer from an aiortc client."""

from __future__ import annotations

import asyncio
import json
import stat
import time

import httpx
import pytest
from ghost_api import AuditPage, Phase, Status
from ghostd.phone import PairingCodes, PhoneListener, PhoneStore
from ghostd.phone.signing import (
    NonceCache,
    new_nonce,
    signed_headers,
    verify_response,
)


class Clock:
    def __init__(self) -> None:
        self.t = 1000.0

    def __call__(self) -> float:
        return self.t


class FakeHost:
    def __init__(self) -> None:
        self.refusal: str | None = None
        self.started: list = []
        self.ended = 0

    def status(self) -> Status:
        return Status(
            phase=Phase.PASSIVE,
            session_id=None,
            models_ready=True,
            uptime_secs=1.0,
            version="test",
            wake_mode="file",
            abilities=["fake"],
        )

    def audit(self, limit: int, offset: int) -> AuditPage:
        return AuditPage(items=[], total=0, limit=limit, offset=offset)

    def lines(self):
        return []

    def admit_phone(self):
        return self.refusal

    async def start_phone(self, connection, conversation=None):
        if self.refusal:
            return self.refusal
        self.started.append((connection, conversation))
        return None

    async def end_phone(self):
        self.ended += 1
        return True


@pytest.fixture
def phone(tmp_path):
    clock = Clock()
    host = FakeHost()
    listener = PhoneListener(
        "127.0.0.1:0",
        store=PhoneStore(tmp_path / "phone.json"),
        host=host,
        codes=PairingCodes(clock=clock),
        workstation="desk",
    )
    client = httpx.AsyncClient(
        transport=httpx.ASGITransport(app=listener.app), base_url="http://phone"
    )
    return listener, host, client, clock


async def pair(listener, client, name="Pixel 9"):
    pairing = listener.new_pairing()
    r = await client.post("/pair", json={"code": pairing.code, "device_name": name})
    assert r.status_code == 200, r.text
    return r.json()


async def signed(client, creds, method, path, body=None, **kw):
    raw = json.dumps(body).encode() if body is not None else b""
    headers = signed_headers(creds["secret"], creds["device_id"], method, path, raw, **kw)
    if raw:
        headers["Content-Type"] = "application/json"
    r = await client.request(method, path, content=raw, headers=headers)
    return r, headers["X-Ghost-Nonce"]


def response_ok(r, creds, nonce) -> bool:
    ts, sig = r.headers.get("x-ghost-time"), r.headers.get("x-ghost-signature")
    if not ts or not sig:
        return False
    return verify_response(creds["secret"], r.status_code, nonce, ts, r.content, sig)


# --- signing ------------------------------------------------------------------------------


async def test_pair_then_hello_and_audit_are_signed_both_ways(phone):
    listener, host, client, _ = phone
    creds = await pair(listener, client)
    assert set(creds) == {"device_id", "secret", "workstation", "addresses"}
    assert creds["workstation"] == "desk" and len(creds["secret"]) == 43  # 32 bytes b64url

    r, nonce = await signed(client, creds, "GET", "/hello")
    assert r.status_code == 200
    assert r.json()["workstation"] == "desk"
    assert r.json()["status"]["session_origin"] is None
    assert response_ok(r, creds, nonce)
    assert not response_ok(r, creds, new_nonce())  # bound to the request's nonce

    r, nonce = await signed(client, creds, "GET", "/audit?limit=5&offset=0")
    assert r.status_code == 200 and r.json()["limit"] == 5
    assert response_ok(r, creds, nonce)
    assert listener.store.device.last_seen is not None


async def test_bad_signature_stale_time_and_replay_are_refused(phone):
    listener, _, client, _ = phone
    creds = await pair(listener, client)

    # wrong secret
    bad = dict(creds, secret=creds["secret"][::-1])
    r, nonce = await signed(client, bad, "GET", "/hello")
    assert r.status_code == 401 and r.json() == {"error": "unauthorized"}
    assert response_ok(r, creds, nonce)  # errors to a known device are signed too

    # signed for one path, sent to another
    h = signed_headers(creds["secret"], creds["device_id"], "GET", "/hello")
    r = await client.get("/audit", headers=h)
    assert r.status_code == 401

    # tampered body
    body = json.dumps({"sdp": "x", "type": "offer"}).encode()
    h = signed_headers(creds["secret"], creds["device_id"], "POST", "/offer", body)
    r = await client.post("/offer", content=body + b" ", headers=h)
    assert r.status_code == 401

    # more than 60 s off, either way
    for skew in (-61, 61):
        r, _ = await signed(client, creds, "GET", "/hello", now=time.time() + skew)
        assert r.status_code == 401
    r, _ = await signed(client, creds, "GET", "/hello", now=time.time() - 50)
    assert r.status_code == 200

    # replay: the same signed request twice
    h = signed_headers(creds["secret"], creds["device_id"], "GET", "/hello")
    assert (await client.get("/hello", headers=h)).status_code == 200
    assert (await client.get("/hello", headers=h)).status_code == 401

    # no headers at all, and an unknown device: 401, unsigned (nothing to sign with)
    r = await client.get("/hello")
    assert r.status_code == 401 and "x-ghost-signature" not in r.headers
    r, _ = await signed(client, dict(creds, device_id="nope"), "GET", "/hello")
    assert r.status_code == 401 and "x-ghost-signature" not in r.headers


def test_nonce_cache_forgets_after_ttl():
    clock = Clock()
    cache = NonceCache(ttl=120, clock=clock)
    cache.add("a" * 32)
    assert cache.seen("a" * 32)
    clock.t += 121
    assert not cache.seen("a" * 32) and len(cache) == 0


async def test_error_responses_are_signed(phone):
    listener, _, client, _ = phone
    creds = await pair(listener, client)
    r, nonce = await signed(
        client,
        creds,
        "PATCH",
        "/offer",
        {"pc_id": "nope", "candidates": []},
    )
    assert r.status_code == 404 and r.json() == {"error": "not_found"}
    assert response_ok(r, creds, nonce)

    r, nonce = await signed(client, creds, "POST", "/offer", {"type": "answer"})
    assert r.status_code == 422 and r.json() == {"error": "bad_request"}
    assert response_ok(r, creds, nonce)

    r, nonce = await signed(client, creds, "GET", "/nowhere")
    assert r.status_code == 404 and response_ok(r, creds, nonce)


# --- pairing ------------------------------------------------------------------------------


async def test_code_expires_after_two_minutes(phone):
    listener, _, client, clock = phone
    pairing = listener.new_pairing()
    clock.t += 121
    r = await client.post("/pair", json={"code": pairing.code, "device_name": "p"})
    assert r.status_code == 403 and r.json() == {"error": "bad_code"}
    assert listener.store.device is None


async def test_five_wrong_tries_spend_the_code(phone):
    listener, _, client, _ = phone
    pairing = listener.new_pairing()
    wrong = f"{(int(pairing.code) + 1) % 10**6:06d}"
    for _ in range(5):
        r = await client.post("/pair", json={"code": wrong, "device_name": "p"})
        assert r.status_code == 403
    r = await client.post("/pair", json={"code": pairing.code, "device_name": "p"})
    assert r.status_code == 403  # spent, even though it is right


async def test_code_is_single_use_and_a_new_one_replaces_the_old(phone):
    listener, _, client, _ = phone
    first = listener.new_pairing()
    second = listener.new_pairing()
    r = await client.post("/pair", json={"code": first.code, "device_name": "p"})
    assert r.status_code == 403 or first.code == second.code
    r = await client.post("/pair", json={"code": second.code, "device_name": "p"})
    assert r.status_code == 200
    r = await client.post("/pair", json={"code": second.code, "device_name": "p"})
    assert r.status_code == 403


async def test_pairing_again_replaces_the_device(phone, tmp_path):
    listener, _, client, _ = phone
    old = await pair(listener, client, "old phone")
    new = await pair(listener, client, "new phone")
    assert old["device_id"] != new["device_id"] and old["secret"] != new["secret"]
    assert (await signed(client, old, "GET", "/hello"))[0].status_code == 401
    assert (await signed(client, new, "GET", "/hello"))[0].status_code == 200
    assert listener.view().device.name == "new phone"

    path = tmp_path / "phone.json"
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    saved = json.loads(path.read_text())
    assert saved["id"] == new["device_id"] and saved["secret"] == new["secret"]
    # survives a restart
    assert PhoneStore(path).secret_for(new["device_id"]) == new["secret"]


async def test_forget_revokes_at_once_and_ends_a_phone_session(phone, tmp_path):
    listener, host, client, _ = phone
    creds = await pair(listener, client)
    assert await listener.forget() is True
    assert host.ended == 1
    assert (await signed(client, creds, "GET", "/hello"))[0].status_code == 401
    assert not (tmp_path / "phone.json").exists()
    assert listener.view().device is None
    assert await listener.forget() is False


# --- offers ---------------------------------------------------------------------------


async def test_offer_refused_while_busy_or_not_ready(phone):
    listener, host, client, _ = phone
    creds = await pair(listener, client)
    for refusal, status in (("busy", 409), ("not_ready", 503)):
        host.refusal = refusal
        r, nonce = await signed(client, creds, "POST", "/offer", {"sdp": "v=0", "type": "offer"})
        assert r.status_code == status and r.json() == {"error": refusal}
        assert response_ok(r, creds, nonce)
    assert host.started == []


async def test_lines_are_served_and_a_call_can_start_in_one(phone):
    """ADR 0014 on the phone: the listener lists the threads and an offer may name one."""
    listener, host, client, _ = phone
    creds = await pair(listener, client)
    r, nonce = await signed(client, creds, "GET", "/conversations")
    assert r.status_code == 200 and response_ok(r, creds, nonce) and r.json() == []
    body = {"sdp": "v=0", "type": "offer", "conversation": "heka-workshop"}
    r, _ = await signed(client, creds, "POST", "/offer", body)
    assert r.status_code == 200 and host.started[-1][1] == "heka-workshop"
    host.refusal = "no_such_line"
    r, _ = await signed(client, creds, "POST", "/offer", body)
    assert r.status_code == 404 and r.json() == {"error": "no_such_line"}


async def test_end_asks_the_host(phone):
    listener, host, client, _ = phone
    creds = await pair(listener, client)
    r, nonce = await signed(client, creds, "POST", "/end")
    assert r.json() == {"ok": True} and host.ended == 1 and response_ok(r, creds, nonce)


def cand(text: str) -> dict:
    return {"candidate": text, "sdp_mid": "0", "sdp_mline_index": 0}


async def _client_offer():
    from aiortc import RTCConfiguration, RTCPeerConnection

    pc = RTCPeerConnection(RTCConfiguration(iceServers=[]))
    pc.addTransceiver("audio", direction="sendrecv")
    channel = pc.createDataChannel("chat")
    await pc.setLocalDescription(await pc.createOffer())  # gathers host candidates
    return pc, channel


async def test_offer_returns_a_signed_answer_the_client_can_apply(phone):
    from aiortc import RTCSessionDescription

    listener, host, client, _ = phone
    creds = await pair(listener, client)
    pc, _ = await _client_offer()
    try:
        r, nonce = await signed(
            client,
            creds,
            "POST",
            "/offer",
            {"sdp": pc.localDescription.sdp, "type": "offer"},
        )
        assert r.status_code == 200, r.text
        assert response_ok(r, creds, nonce)  # the phone checks this before applying it
        answer = r.json()
        assert answer["type"] == "answer" and answer["pc_id"] and "a=fingerprint" in answer["sdp"]
        await pc.setRemoteDescription(RTCSessionDescription(answer["sdp"], answer["type"]))
        assert len(host.started) == 1 and listener.connection is host.started[0][0]
        assert host.started[0][0].ice_servers == []

        # A second start is the host's call; the listener only tracks the live one.
        r, _ = await signed(
            client,
            creds,
            "PATCH",
            "/offer",
            {"pc_id": answer["pc_id"], "candidates": []},
        )
        assert r.json() == {"ok": True}
        host_cand = "candidate:1 1 udp 2130706431 127.0.0.1 9 typ host"
        patch = {"pc_id": answer["pc_id"], "candidates": [cand(host_cand), cand("")]}
        r, nonce = await signed(client, creds, "PATCH", "/offer", patch)
        assert r.json() == {"ok": True} and response_ok(r, creds, nonce)
        patch = {"pc_id": answer["pc_id"], "candidates": [cand("candidate:junk")]}
        r, _ = await signed(client, creds, "PATCH", "/offer", patch)
        assert r.status_code == 400 and r.json() == {"error": "bad_candidate"}
    finally:
        await pc.close()
        await listener.stop()


async def test_webrtc_loopback_through_ghosts_rtvi(phone):
    """A real aiortc peer against the listener, with a model-free pipeline standing in
    for the session: client-ready gets bot-ready, send-text is ignored, a server-message
    arrives, and disconnect-bot ends the pipeline."""
    from aiortc import RTCSessionDescription
    from ghost_voice.rtvi import GhostRTVIProcessor, observer_params
    from pipecat.pipeline.pipeline import Pipeline
    from pipecat.pipeline.worker import PipelineParams, PipelineWorker
    from pipecat.transports.base_transport import TransportParams
    from pipecat.transports.smallwebrtc.transport import SmallWebRTCTransport
    from pipecat.workers.runner import WorkerRunner

    listener, host, client, _ = phone
    creds = await pair(listener, client)
    pc, channel = await _client_offer()
    received: asyncio.Queue[dict] = asyncio.Queue()

    @channel.on("message")
    def on_message(message):
        try:
            received.put_nowait(json.loads(message))
        except ValueError:
            pass

    opened = asyncio.Event()
    channel.on("open", opened.set)

    r, _ = await signed(
        client, creds, "POST", "/offer", {"sdp": pc.localDescription.sdp, "type": "offer"}
    )
    answer = r.json()
    await pc.setRemoteDescription(RTCSessionDescription(answer["sdp"], answer["type"]))
    conn = host.started[0][0]

    transport = SmallWebRTCTransport(
        webrtc_connection=conn,
        params=TransportParams(
            audio_in_enabled=True, audio_out_enabled=True, audio_out_sample_rate=24000
        ),
    )
    worker = PipelineWorker(
        Pipeline([transport.input(), transport.output()]),
        params=PipelineParams(audio_in_sample_rate=16000),
        rtvi_processor=GhostRTVIProcessor(),
        rtvi_observer_params=observer_params(),
    )
    runner = WorkerRunner(handle_sigint=False)
    await runner.add_workers(worker)
    run = asyncio.ensure_future(runner.run())

    async def next_of(kind: str) -> dict:
        while True:
            m = await asyncio.wait_for(received.get(), 15)
            if m.get("type") == kind:
                return m

    try:
        await asyncio.wait_for(opened.wait(), 15)
        rtvi = {"label": "rtvi-ai", "id": "1"}
        channel.send(json.dumps({**rtvi, "type": "send-text", "data": {"content": "hi"}}))
        channel.send(json.dumps({**rtvi, "type": "client-ready", "data": {"version": "2.1.0"}}))
        assert (await next_of("bot-ready"))["label"] == "rtvi-ai"

        await worker.rtvi.send_server_message({"type": "pending", "pending": None})
        msg = await next_of("server-message")
        assert msg["data"] == {"type": "pending", "pending": None}

        channel.send(json.dumps({**rtvi, "type": "disconnect-bot"}))
        await asyncio.wait_for(run, 15)
    finally:
        if not run.done():
            await runner.cancel()
            await asyncio.gather(run, return_exceptions=True)
        await pc.close()
        await listener.stop()


async def test_listener_binds_its_address_and_signs_over_real_http(tmp_path):
    host = FakeHost()
    listener = PhoneListener("127.0.0.1:0", store=PhoneStore(tmp_path / "p.json"), host=host)
    await listener.start()
    try:
        base = f"http://127.0.0.1:{listener.port}"
        async with httpx.AsyncClient(base_url=base) as c:
            pairing = listener.new_pairing()
            creds = (await c.post("/pair", json={"code": pairing.code, "device_name": "p"})).json()
            r, nonce = await signed(c, creds, "GET", "/audit?limit=2")
            assert r.status_code == 200 and response_ok(r, creds, nonce)
            assert (await c.get("/status")).status_code == 401  # no control API here
    finally:
        await listener.stop()


def test_parse_listen():
    from ghostd.phone import parse_listen, parse_listen_list

    assert parse_listen("192.168.1.20:8765") == ("192.168.1.20", 8765)
    assert parse_listen("[fe80::1]:8765") == ("fe80::1", 8765)
    for bad in ("", "8765", "host:", "host:port"):
        with pytest.raises(ValueError):
            parse_listen(bad)
    # ADR 0020: the LAN address and the tunnel address, comma separated
    assert parse_listen_list("192.168.1.20:8765, 10.8.0.4:8765") == [
        "192.168.1.20:8765",
        "10.8.0.4:8765",
    ]
    for bad in ("", " , ", "192.168.1.20:8765,nope"):
        with pytest.raises(ValueError):
            parse_listen_list(bad)


async def test_listener_serves_every_address_and_tells_the_phone_all_of_them(tmp_path):
    """Two addresses (ADR 0020): both serve the same signed app, and /pair and /hello carry
    the whole list so the phone can pick the one that answers wherever it is."""
    host = FakeHost()
    listener = PhoneListener(
        "127.0.0.1:0,127.0.0.1:0", store=PhoneStore(tmp_path / "p.json"), host=host
    )
    await listener.start()
    try:
        assert len(listener.bound) == 2
        ports = [s.servers[0].sockets[0].getsockname()[1] for _, s, _ in listener._servers]
        assert len(set(ports)) == 2
        async with httpx.AsyncClient(base_url=f"http://127.0.0.1:{ports[0]}") as c:
            pairing = listener.new_pairing()
            assert pairing.addresses == ["127.0.0.1:0", "127.0.0.1:0"]
            creds = (await c.post("/pair", json={"code": pairing.code, "device_name": "p"})).json()
            assert creds["addresses"] == ["127.0.0.1:0", "127.0.0.1:0"]
        async with httpx.AsyncClient(base_url=f"http://127.0.0.1:{ports[1]}") as c:
            r, nonce = await signed(c, creds, "GET", "/hello")
            assert r.status_code == 200 and response_ok(r, creds, nonce)
            assert r.json()["addresses"] == ["127.0.0.1:0", "127.0.0.1:0"]
        assert listener.view().addresses == ["127.0.0.1:0", "127.0.0.1:0"]
    finally:
        await listener.stop()


async def test_listener_keeps_trying_an_address_whose_interface_is_not_up(tmp_path):
    """The tunnel can come up after the daemon: its address is retried, not given up on,
    while the LAN address serves from the start."""
    from ghostd.phone import listener as mod

    host = FakeHost()
    listener = PhoneListener(
        "127.0.0.1:0,192.0.2.1:0", store=PhoneStore(tmp_path / "p.json"), host=host
    )
    await listener.start()  # 192.0.2.1 (TEST-NET) is not an address of this host
    try:
        assert listener.bound == ["127.0.0.1:0"]
        assert listener.addresses == ["127.0.0.1:0", "192.0.2.1:0"]
        assert listener._rebind is not None and not listener._rebind.done()
        async with httpx.AsyncClient(base_url=f"http://127.0.0.1:{listener.port}") as c:
            pairing = listener.new_pairing()
            r = await c.post("/pair", json={"code": pairing.code, "device_name": "p"})
            assert r.status_code == 200
    finally:
        await listener.stop()
    assert listener._rebind is None and mod.REBIND_SECS > 0


# --- the daemon's side: who may start a session ------------------------------------------


@pytest.fixture
def daemon(tmp_path, monkeypatch):
    from ghost_voice import VoiceSettings
    from ghostd.config import GhostdSettings
    from ghostd.main import Daemon

    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    d = Daemon(GhostdSettings(data_dir=tmp_path), VoiceSettings.model_construct())
    d._manual = type("Manual", (), {"trigger": lambda self, text=None: None})()
    return d


async def test_daemon_admits_a_phone_only_while_passive(daemon):
    d = daemon
    assert d.admit_phone() == "not_ready"  # starting, models loading
    d.state.set_models_ready(True)
    d.state.set_phase(Phase.PASSIVE)
    assert d.admit_phone() == "busy"  # the loop isn't waiting yet
    d._waiting = True
    assert d.admit_phone() is None

    conn = object()
    assert await d.start_phone(conn) is None
    assert d.admit_phone() == "busy"  # one phone at a time
    assert await d.start_phone(object()) == "busy"
    assert await d.wake(None) is False  # the desk can't take it now
    assert await d.end_phone() is True and d._phone_end.is_set()
    assert d._phone_q.get_nowait() == (conn, "ghost")  # Ghost when the offer names no line
    assert await d.start_phone(object(), "no-such-thread") == "no_such_line"


async def test_daemon_refuses_a_phone_during_a_desk_session(daemon):
    d = daemon
    d.state.set_models_ready(True)
    d._waiting = True
    d.state.set_phase(Phase.LISTENING, "s1", origin="desk")
    assert d.admit_phone() == "busy"
    assert d.state.session_origin == "desk"
    assert await d.end_phone() is False  # /end never ends a desk session


async def test_daemon_phone_controller_when_listener_is_off(daemon):
    assert daemon.phone_pairing() is None
    assert daemon.phone_view().address is None
    assert await daemon.forget_phone() is False
