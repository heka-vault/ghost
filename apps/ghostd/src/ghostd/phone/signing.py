"""Request and response signatures for the phone listener (docs/phone-protocol.md).

Requests are HMAC-SHA256 under the device secret over::

    ghost-v1 / METHOD / path?query / time / nonce / sha256(body)

and every response to a signed request is signed back over::

    ghost-v1-response / status / request nonce / time / sha256(body)

each joined by newlines. Signatures are base64url without padding.
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import re
import secrets
import time
from collections.abc import Callable

REQUEST_PREFIX = "ghost-v1"
RESPONSE_PREFIX = "ghost-v1-response"
CLOCK_WINDOW_SECS = 60
NONCE_TTL_SECS = 120

H_DEVICE = "x-ghost-device"
H_TIME = "x-ghost-time"
H_NONCE = "x-ghost-nonce"
H_SIGNATURE = "x-ghost-signature"

_NONCE_RE = re.compile(r"^[0-9a-f]{32}$")


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def b64url_decode(text: str) -> bytes:
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def new_secret() -> str:
    """32 random bytes, base64url without padding."""
    return b64url(secrets.token_bytes(32))


def new_nonce() -> str:
    return secrets.token_hex(16)


def body_hash(body: bytes) -> str:
    return hashlib.sha256(body).hexdigest()


def _mac(secret: str, text: str) -> str:
    return b64url(hmac.new(b64url_decode(secret), text.encode(), hashlib.sha256).digest())


def _same(a: str, b: str) -> bool:
    """Constant-time compare that tolerates non-ASCII input (header values)."""
    return hmac.compare_digest(a.encode(), b.encode())


def request_string(method: str, path: str, ts: str, nonce: str, body: bytes) -> str:
    return "\n".join((REQUEST_PREFIX, method.upper(), path, ts, nonce, body_hash(body)))


def response_string(status: int, nonce: str, ts: str, body: bytes) -> str:
    return "\n".join((RESPONSE_PREFIX, str(status), nonce, ts, body_hash(body)))


def sign_request(secret: str, method: str, path: str, ts: str, nonce: str, body: bytes) -> str:
    return _mac(secret, request_string(method, path, ts, nonce, body))


def sign_response(secret: str, status: int, nonce: str, ts: str, body: bytes) -> str:
    return _mac(secret, response_string(status, nonce, ts, body))


def signed_headers(
    secret: str,
    device_id: str,
    method: str,
    path: str,
    body: bytes = b"",
    *,
    now: float | None = None,
    nonce: str | None = None,
) -> dict[str, str]:
    """The four request headers, as the phone sends them. Used by tests and tools."""
    ts = str(int(time.time() if now is None else now))
    nonce = nonce or new_nonce()
    return {
        "X-Ghost-Device": device_id,
        "X-Ghost-Time": ts,
        "X-Ghost-Nonce": nonce,
        "X-Ghost-Signature": sign_request(secret, method, path, ts, nonce, body),
    }


def verify_response(
    secret: str, status: int, nonce: str, ts: str, body: bytes, signature: str
) -> bool:
    return _same(sign_response(secret, status, nonce, ts, body), signature)


class NonceCache:
    """Nonces seen in the last ``ttl`` seconds. A nonce is recorded only once its
    request verified, so junk can't fill it with entries that block real requests."""

    def __init__(
        self, ttl: float = NONCE_TTL_SECS, clock: Callable[[], float] = time.monotonic
    ) -> None:
        self._ttl = ttl
        self._clock = clock
        self._seen: dict[str, float] = {}

    def seen(self, nonce: str) -> bool:
        self._prune()
        return nonce in self._seen

    def add(self, nonce: str) -> None:
        self._prune()
        self._seen[nonce] = self._clock() + self._ttl

    def _prune(self) -> None:
        now = self._clock()
        for n in [n for n, exp in self._seen.items() if exp <= now]:
            del self._seen[n]

    def __len__(self) -> int:
        return len(self._seen)


class Verifier:
    """Checks one signed request against the paired device. ``lookup`` returns the
    secret for a device id, or None when it isn't the paired phone."""

    def __init__(
        self,
        lookup: Callable[[str], str | None],
        *,
        window: float = CLOCK_WINDOW_SECS,
        nonces: NonceCache | None = None,
        clock: Callable[[], float] = time.time,
    ) -> None:
        self._lookup = lookup
        self._window = window
        self._nonces = nonces or NonceCache()
        self._clock = clock

    def secret_for(self, device_id: str | None) -> str | None:
        return self._lookup(device_id) if device_id else None

    def verify(
        self,
        *,
        method: str,
        path: str,
        body: bytes,
        device_id: str | None,
        ts: str | None,
        nonce: str | None,
        signature: str | None,
    ) -> bool:
        secret = self.secret_for(device_id)
        if secret is None or not ts or not nonce or not signature:
            return False
        try:
            t = int(ts)
        except ValueError:
            return False
        if abs(self._clock() - t) > self._window:
            return False
        if not _NONCE_RE.match(nonce) or self._nonces.seen(nonce):
            return False
        expected = sign_request(secret, method, path, ts, nonce, body)
        if not _same(expected, signature):
            return False
        self._nonces.add(nonce)
        return True
