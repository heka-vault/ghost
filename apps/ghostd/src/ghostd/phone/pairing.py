"""The one paired phone and the pairing code that admits it (ADR 0016).

The device record, secret included, lives in ``phone.json`` under the data dir, mode
0600. The pairing code lives only in memory: six digits, 120 s, five tries, single use.
Neither the secret nor the code is ever logged.
"""

from __future__ import annotations

import hmac
import json
import os
import secrets
import time
from collections.abc import Callable
from dataclasses import asdict, dataclass, replace
from datetime import UTC, datetime, timedelta
from pathlib import Path

from loguru import logger

from ghostd.phone.signing import new_secret

CODE_TTL_SECS = 120
CODE_TRIES = 5
LAST_SEEN_PERSIST_SECS = 60  # write last_seen to disk at most this often


def _now() -> datetime:
    return datetime.now(UTC)


@dataclass(frozen=True)
class Device:
    id: str
    name: str
    secret: str
    paired_at: datetime
    last_seen: datetime | None = None

    def to_json(self) -> dict:
        d = asdict(self)
        d["paired_at"] = self.paired_at.isoformat()
        d["last_seen"] = self.last_seen.isoformat() if self.last_seen else None
        return d

    @classmethod
    def from_json(cls, d: dict) -> Device:
        return cls(
            id=str(d["id"]),
            name=str(d["name"]),
            secret=str(d["secret"]),
            paired_at=datetime.fromisoformat(d["paired_at"]),
            last_seen=datetime.fromisoformat(d["last_seen"]) if d.get("last_seen") else None,
        )

    def __repr__(self) -> str:  # never print the secret
        return f"Device(id={self.id!r}, name={self.name!r})"


class PhoneStore:
    """``phone.json``: at most one device. Pairing again replaces it."""

    def __init__(self, path: Path, clock: Callable[[], datetime] = _now) -> None:
        self.path = path
        self._clock = clock
        self._device: Device | None = self._load()
        self._persisted_seen = time.monotonic()

    def _load(self) -> Device | None:
        if not self.path.exists():
            return None
        try:
            return Device.from_json(json.loads(self.path.read_text()))
        except (OSError, ValueError, KeyError, TypeError) as e:
            logger.warning(f"phone: ignoring unreadable {self.path}: {type(e).__name__}")
            return None

    def _save(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        if self._device is None:
            self.path.unlink(missing_ok=True)
            return
        tmp = self.path.with_suffix(".tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(self._device.to_json(), f, indent=2)
        os.chmod(tmp, 0o600)
        os.replace(tmp, self.path)

    @property
    def device(self) -> Device | None:
        return self._device

    def secret_for(self, device_id: str) -> str | None:
        d = self._device
        if d is None or not hmac.compare_digest(d.id.encode(), device_id.encode()):
            return None
        return d.secret

    def pair(self, name: str) -> Device:
        device = Device(
            id=secrets.token_hex(8), name=name, secret=new_secret(), paired_at=self._clock()
        )
        self._device = device
        self._save()
        logger.info(f"phone: paired {name!r} ({device.id})")
        return device

    def forget(self) -> bool:
        had = self._device is not None
        self._device = None
        self._save()
        if had:
            logger.info("phone: forgot the paired phone")
        return had

    def seen(self) -> None:
        """Note a verified request. Kept in memory; written at most once a minute."""
        if self._device is None:
            return
        self._device = replace(self._device, last_seen=self._clock())
        if time.monotonic() - self._persisted_seen >= LAST_SEEN_PERSIST_SECS:
            self._persisted_seen = time.monotonic()
            self._save()

    def flush(self) -> None:
        if self._device is not None:
            self._save()


@dataclass
class _Code:
    code: str
    expires: float  # monotonic
    expires_at: datetime
    tries: int = 0


class PairingCodes:
    """At most one live code. ``mint`` replaces it; ``redeem`` spends it on success,
    and a fifth wrong try spends it too."""

    def __init__(
        self,
        ttl: float = CODE_TTL_SECS,
        tries: int = CODE_TRIES,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._ttl = ttl
        self._tries = tries
        self._clock = clock
        self._code: _Code | None = None

    def mint(self) -> tuple[str, datetime]:
        code = f"{secrets.randbelow(10**6):06d}"
        expires_at = _now() + timedelta(seconds=self._ttl)
        self._code = _Code(code=code, expires=self._clock() + self._ttl, expires_at=expires_at)
        return code, expires_at

    def redeem(self, code: str) -> bool:
        c = self._code
        if c is None:
            return False
        if self._clock() >= c.expires:
            self._code = None
            return False
        if hmac.compare_digest(c.code.encode(), code.encode()):
            self._code = None
            return True
        c.tries += 1
        if c.tries >= self._tries:
            self._code = None
        return False

    @property
    def active(self) -> bool:
        return self._code is not None and self._clock() < self._code.expires
