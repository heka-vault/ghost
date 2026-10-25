"""The phone listener (ADR 0016, ADR 0020): a small signed HTTP app on the configured addresses.

It is not the control API and does not proxy it. Five routes: ``POST /pair``
(unsigned, spends a pairing code), ``GET /hello``, ``GET /audit``, ``POST``/``PATCH
/offer`` (WebRTC signalling) and ``POST /end``. Everything but ``/pair`` is signed both
ways by ``SigningMiddleware``; the wire format is docs/phone-protocol.md.

``GHOST_PHONE_LISTEN`` may list several ``host:port`` addresses, comma separated: the LAN
address and the operator's tunnel address (ADR 0020). The first must bind at start; a later
one whose interface is not up yet is retried until it is.

The daemon is reached through ``PhoneHost``: it owns sessions, so it decides whether a
phone may start one and runs it on the connection the listener negotiated.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import socket
import time
from collections.abc import Callable, Iterator
from typing import Any, Literal, Protocol

import uvicorn
from fastapi import FastAPI, Query, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from ghost_api import AuditPage, PhoneDevice, PhonePairing, PhoneView, Status
from ghost_api.models import ConversationView
from loguru import logger
from pydantic import BaseModel, Field
from starlette.exceptions import HTTPException as StarletteHTTPException
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from ghostd.phone.pairing import PairingCodes, PhoneStore
from ghostd.phone.signing import H_DEVICE, H_NONCE, H_SIGNATURE, H_TIME, Verifier, sign_response

Refusal = Literal["busy", "not_ready", "no_such_line"]
_REFUSAL_STATUS: dict[str, int] = {"busy": 409, "not_ready": 503, "no_such_line": 404}
REBIND_SECS = 5.0  # how often a listen address whose interface isn't up yet is tried again
UNSIGNED_PATHS = frozenset({"/pair"})
_STATUS_CODES = {
    400: "bad_request",
    401: "unauthorized",
    403: "bad_code",
    404: "not_found",
    405: "method_not_allowed",
    409: "busy",
    422: "bad_request",
    500: "internal",
    503: "not_ready",
}


class PhoneHost(Protocol):
    """What the daemon gives the listener."""

    def status(self) -> Status: ...
    def audit(self, limit: int, offset: int) -> AuditPage: ...
    def agents(self) -> list[ConversationView]: ...  # the agents a call can start with
    def admit_phone(self) -> Refusal | None: ...  # a cheap check before negotiating
    async def start_phone(
        self, connection: Any, conversation: str | None = None
    ) -> Refusal | None: ...  # takes ownership
    async def end_phone(self) -> bool: ...


def parse_listen(value: str) -> tuple[str, int]:
    """``host:port`` or ``[v6]:port``."""
    host, sep, port = value.strip().rpartition(":")
    if not sep or not host or not port.isdigit():
        raise ValueError(f"GHOST_PHONE_LISTEN must be host:port, got {value!r}")
    if host.startswith("[") and host.endswith("]"):
        host = host[1:-1]
    return host, int(port)


def parse_listen_list(value: str) -> list[str]:
    """``GHOST_PHONE_LISTEN``: one or more ``host:port``, comma separated, each validated."""
    addresses = [a.strip() for a in value.split(",") if a.strip()]
    if not addresses:
        raise ValueError("GHOST_PHONE_LISTEN is empty")
    for a in addresses:
        parse_listen(a)
    return addresses


def _json(status: int, body: Any, headers: dict[str, str] | None = None) -> JSONResponse:
    return JSONResponse(body, status_code=status, headers=headers)


def _error(status: int, code: str | None = None) -> JSONResponse:
    return _json(status, {"error": code or _STATUS_CODES.get(status, "error")})


# --- request and response bodies -------------------------------------------------------


class PairRequest(BaseModel):
    code: str = Field(max_length=16)
    device_name: str = Field(min_length=1, max_length=64)


class OfferRequest(BaseModel):
    sdp: str = Field(max_length=65536)
    type: Literal["offer"]
    pc_id: str | None = None
    restart_pc: bool | None = None
    # The agent to start with (a slug from /conversations); Ghost when absent.
    conversation: str | None = Field(default=None, max_length=80)


class Candidate(BaseModel):
    candidate: str
    sdp_mid: str
    sdp_mline_index: int


class PatchRequest(BaseModel):
    pc_id: str
    candidates: list[Candidate] = Field(max_length=64)


# --- signing middleware -------------------------------------------------------------------


class SigningMiddleware:
    """Verifies every request outside ``UNSIGNED_PATHS`` and signs every response to one,
    errors included. A pure ASGI middleware so it sees the exact body bytes both ways."""

    def __init__(
        self, app: ASGIApp, *, verifier: Verifier, on_verified: Callable[[], None]
    ) -> None:
        self.app = app
        self.verifier = verifier
        self.on_verified = on_verified

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http" or scope["path"] in UNSIGNED_PATHS:
            await self.app(scope, receive, send)
            return

        body = b""
        more = True
        while more:
            msg = await receive()
            if msg["type"] != "http.request":
                return  # client went away
            body += msg.get("body", b"")
            more = msg.get("more_body", False)
            if len(body) > 256 * 1024:
                await _send_plain(send, 413, {"error": "too_large"}, {})
                return

        headers = {k.decode("latin-1").lower(): v.decode("latin-1") for k, v in scope["headers"]}
        raw_path = scope.get("raw_path") or scope["path"].encode()
        path = raw_path.decode("latin-1")
        if scope.get("query_string"):
            path += "?" + scope["query_string"].decode("latin-1")
        device = headers.get(H_DEVICE)
        nonce = headers.get(H_NONCE)
        ok = self.verifier.verify(
            method=scope["method"],
            path=path,
            body=body,
            device_id=device,
            ts=headers.get(H_TIME),
            nonce=nonce,
            signature=headers.get(H_SIGNATURE),
        )
        secret = self.verifier.secret_for(device)

        def response_headers(status: int, payload: bytes) -> dict[str, str]:
            if secret is None or not nonce:
                return {}  # unknown device: nothing to sign with; the phone drops it
            ts = str(int(time.time()))
            return {
                "X-Ghost-Time": ts,
                "X-Ghost-Signature": sign_response(secret, status, nonce, ts, payload),
            }

        if not ok:
            # What was refused, never the headers: they carry the signature and nonce.
            known = "paired device" if secret is not None else "unknown device"
            logger.info(f"phone: refused {scope['method']} {scope['path']} ({known})")
            payload = json.dumps({"error": "unauthorized"}).encode()
            await _send_plain(send, 401, payload, response_headers(401, payload))
            return
        self.on_verified()

        replayed = False

        async def replay() -> Message:
            nonlocal replayed
            if not replayed:
                replayed = True
                return {"type": "http.request", "body": body, "more_body": False}
            return await receive()

        start: Message | None = None
        chunks: list[bytes] = []

        async def capture(message: Message) -> None:
            nonlocal start
            if message["type"] == "http.response.start":
                start = message
                return
            if message["type"] == "http.response.body":
                chunks.append(message.get("body", b""))
                if message.get("more_body", False):
                    return
                assert start is not None
                payload = b"".join(chunks)
                extra = response_headers(start["status"], payload)
                start["headers"] = list(start.get("headers", [])) + [
                    (k.lower().encode(), v.encode()) for k, v in extra.items()
                ]
                await send(start)
                await send({"type": "http.response.body", "body": payload, "more_body": False})

        try:
            await self.app(scope, replay, capture)
        except Exception:  # noqa: BLE001 - answer with a signed 500, not an unsigned one
            logger.exception("phone: request failed")
            if start is None:
                payload = json.dumps({"error": "internal"}).encode()
                await _send_plain(send, 500, payload, response_headers(500, payload))


async def _send_plain(send: Send, status: int, payload: Any, headers: dict[str, str]) -> None:
    if not isinstance(payload, bytes):
        payload = json.dumps(payload).encode()
    raw = [
        (b"content-type", b"application/json"),
        (b"content-length", str(len(payload)).encode()),
    ] + [(k.lower().encode(), v.encode()) for k, v in headers.items()]
    await send({"type": "http.response.start", "status": status, "headers": raw})
    await send({"type": "http.response.body", "body": payload, "more_body": False})


class _Server(uvicorn.Server):
    """uvicorn without its signal handlers: ghostd owns SIGINT and SIGTERM."""

    @contextlib.contextmanager
    def capture_signals(self) -> Iterator[None]:
        yield


# --- the listener ------------------------------------------------------------------------


def _default_connection() -> Any:
    from pipecat.transports.smallwebrtc.connection import SmallWebRTCConnection

    # No STUN, no TURN: the LAN, or the operator's own tunnel (ADR 0020). Host candidates only.
    return SmallWebRTCConnection(ice_servers=[])


class PhoneListener:
    """Pairing state, the signed app, and one uvicorn server per configured address."""

    def __init__(
        self,
        address: str,
        *,
        store: PhoneStore,
        host: PhoneHost,
        codes: PairingCodes | None = None,
        verifier: Verifier | None = None,
        connection_factory: Callable[[], Any] = _default_connection,
        workstation: str | None = None,
    ) -> None:
        self.addresses = parse_listen_list(address)
        self.address = self.addresses[0]  # the one the desktop shows for pairing
        self.store = store
        self.host = host
        self.codes = codes or PairingCodes()
        self.verifier = verifier or Verifier(store.secret_for)
        self.workstation = workstation or socket.gethostname()
        self._connection_factory = connection_factory
        self._conn: Any = None  # the live phone peer connection, if any
        self._servers: list[tuple[str, uvicorn.Server, asyncio.Task[None]]] = []
        self._rebind: asyncio.Task[None] | None = None
        self.app = self._build_app()

    # --- control API side (Controller delegates here) ---------------------------------

    def new_pairing(self) -> PhonePairing:
        code, expires_at = self.codes.mint()
        logger.info("phone: pairing code issued (valid 2 minutes)")  # never the code
        return PhonePairing(
            code=code, address=self.address, addresses=self.addresses, expires_at=expires_at
        )

    def view(self) -> PhoneView:
        d = self.store.device
        device = (
            PhoneDevice(id=d.id, name=d.name, paired_at=d.paired_at, last_seen=d.last_seen)
            if d
            else None
        )
        return PhoneView(address=self.address, addresses=self.addresses, device=device)

    async def forget(self) -> bool:
        had = self.store.forget()
        if had:
            await self.host.end_phone()
        return had

    # --- the connection --------------------------------------------------------------------

    @property
    def connection(self) -> Any:
        return self._conn

    def _track(self, conn: Any) -> None:
        self._conn = conn

        @conn.event_handler("closed")
        async def _closed(c: Any) -> None:
            if self._conn is c:
                self._conn = None

    async def _close(self, conn: Any) -> None:
        try:
            await conn.disconnect()
        except Exception:  # noqa: BLE001
            logger.debug("phone: error closing a refused connection")

    # --- the app ---------------------------------------------------------------------------

    def _build_app(self) -> FastAPI:
        app = FastAPI(title="Ghost phone listener", docs_url=None, redoc_url=None, openapi_url=None)

        @app.exception_handler(StarletteHTTPException)
        async def http_error(_req: Request, exc: StarletteHTTPException) -> JSONResponse:
            return _error(exc.status_code)

        @app.exception_handler(RequestValidationError)
        async def bad_request(_req: Request, _exc: RequestValidationError) -> JSONResponse:
            return _error(422)

        @app.post("/pair")
        async def pair(req: PairRequest) -> JSONResponse:
            if not self.codes.redeem(req.code.strip()):
                logger.info("phone: pairing refused (wrong or spent code)")
                return _error(403, "bad_code")
            device = self.store.pair(req.device_name.strip())
            return _json(
                200,
                {
                    "device_id": device.id,
                    "secret": device.secret,
                    "workstation": self.workstation,
                    "addresses": self.addresses,
                },
            )

        @app.get("/hello")
        async def hello() -> JSONResponse:
            status = self.host.status()
            return _json(
                200,
                {
                    "workstation": self.workstation,
                    "addresses": self.addresses,
                    "status": status.model_dump(mode="json"),
                },
            )

        @app.get("/conversations")
        async def conversations() -> JSONResponse:
            # Every thread a call may start in: Ghost first, settled ones left out.
            return _json(200, [c.model_dump(mode="json") for c in self.host.agents()])

        @app.get("/audit")
        async def audit(
            limit: int = Query(50, ge=1, le=500), offset: int = Query(0, ge=0)
        ) -> JSONResponse:
            return _json(200, self.host.audit(limit, offset).model_dump(mode="json"))

        @app.post("/offer")
        async def offer(req: OfferRequest) -> JSONResponse:
            if req.pc_id:
                conn = self._conn
                if conn is None or conn.pc_id != req.pc_id:
                    return _error(404)
                await conn.renegotiate(sdp=req.sdp, type=req.type, restart_pc=bool(req.restart_pc))
                return _json(200, conn.get_answer())

            refusal = self.host.admit_phone()
            if refusal:
                return _error(_REFUSAL_STATUS[refusal], refusal)
            conn = self._connection_factory()
            try:
                await conn.initialize(sdp=req.sdp, type=req.type)
            except Exception as e:  # noqa: BLE001 - a malformed offer
                logger.info(f"phone: offer rejected: {type(e).__name__}")
                await self._close(conn)
                return _error(400, "bad_offer")
            answer = conn.get_answer()
            if answer is None:
                await self._close(conn)
                return _error(500)
            refusal = await self.host.start_phone(conn, req.conversation)
            if refusal:
                await self._close(conn)
                return _error(_REFUSAL_STATUS[refusal], refusal)
            self._track(conn)
            logger.info("phone: offer accepted")
            return _json(200, answer)

        @app.patch("/offer")
        async def candidates(req: PatchRequest) -> JSONResponse:
            from aiortc.sdp import candidate_from_sdp

            conn = self._conn
            if conn is None or conn.pc_id != req.pc_id:
                return _error(404)
            for c in req.candidates:
                if c.candidate:  # browsers send "candidate:<foundation> ..."; aiortc wants bare
                    try:
                        cand = candidate_from_sdp(c.candidate.removeprefix("candidate:"))
                    except (AssertionError, ValueError, IndexError):
                        return _error(400, "bad_candidate")
                    cand.sdpMid = c.sdp_mid
                    cand.sdpMLineIndex = c.sdp_mline_index
                else:
                    cand = None  # end of candidates
                await conn.add_ice_candidate(cand)
            return _json(200, {"ok": True})

        @app.post("/end")
        async def end() -> JSONResponse:
            await self.host.end_phone()
            return _json(200, {"ok": True})

        app.add_middleware(SigningMiddleware, verifier=self.verifier, on_verified=self.store.seen)
        return app

    # --- serving ---------------------------------------------------------------------------

    async def start(self) -> None:
        """Bind every configured address and serve. The first address must bind, or this
        raises ``OSError``. A later one whose interface isn't up yet (a tunnel that comes up
        after the daemon) is retried every ``REBIND_SECS`` until it binds or the listener stops."""
        first, *rest = self.addresses
        await self._serve(first)
        late: list[str] = []
        for address in rest:
            try:
                await self._serve(address)
            except OSError as e:
                logger.warning(f"phone: {address} not bound yet ({e.strerror or e}); retrying")
                late.append(address)
        if late:
            self._rebind = asyncio.ensure_future(self._rebind_loop(late))

    async def _serve(self, address: str) -> None:
        host, port = parse_listen(address)
        family = socket.AF_INET6 if ":" in host else socket.AF_INET
        sock = socket.socket(family, socket.SOCK_STREAM)
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            sock.bind((host, port))
        except OSError:
            sock.close()
            raise
        sock.set_inheritable(True)
        config = uvicorn.Config(
            self.app,
            log_level="warning",
            access_log=False,  # paths and devices stay out of the journal
            lifespan="off",
            timeout_graceful_shutdown=2.0,  # the phone's keep-alive polls must not hold the exit
        )
        server = _Server(config)
        task = asyncio.ensure_future(server.serve(sockets=[sock]))
        for _ in range(200):
            if server.started or task.done():
                break
            await asyncio.sleep(0.02)
        if not server.started:
            server.should_exit = True
            await asyncio.gather(task, return_exceptions=True)
            raise RuntimeError(f"phone listener did not start on {address}")
        self._servers.append((address, server, task))
        logger.info(f"phone: listening on {address}")

    async def _rebind_loop(self, late: list[str]) -> None:
        while late:
            await asyncio.sleep(REBIND_SECS)
            for address in list(late):
                try:
                    await self._serve(address)
                except OSError:
                    continue
                except RuntimeError as e:
                    logger.error(str(e))
                late.remove(address)

    @property
    def bound(self) -> list[str]:
        """The addresses serving right now (a late one is missing until it binds)."""
        return [address for address, _, _ in self._servers]

    @property
    def port(self) -> int:
        """The first bound port (useful when the configured port is 0, in tests)."""
        assert self._servers, "not started"
        return self._servers[0][1].servers[0].sockets[0].getsockname()[1]

    async def stop(self) -> None:
        if self._rebind is not None:
            self._rebind.cancel()
            await asyncio.gather(self._rebind, return_exceptions=True)
            self._rebind = None
        if self._conn is not None:
            await self._close(self._conn)
            self._conn = None
        for _, server, _ in self._servers:
            server.should_exit = True
        tasks = [task for _, _, task in self._servers]
        if tasks:
            try:
                await asyncio.wait_for(asyncio.shield(asyncio.gather(*tasks)), 4.0)
            except TimeoutError:
                logger.warning("phone: connections still open at shutdown; closing them")
                for _, server, task in self._servers:
                    server.force_exit = True
                    task.cancel()
                await asyncio.gather(*tasks, return_exceptions=True)
            except Exception:  # noqa: BLE001 - a server's own failure, already logged
                pass
        self._servers = []
        self.store.flush()


__all__ = ["PhoneHost", "PhoneListener", "Refusal", "parse_listen", "parse_listen_list"]
