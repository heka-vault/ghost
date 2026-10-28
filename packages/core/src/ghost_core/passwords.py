"""Passwords (ADR 0036): every API key, token and account password Ghost holds.

The operator's word is *password*; the code says secret where it always has. They are kept
in Ghost's own vault, one file (``~/.local/share/ghost/passwords.vault``, 0600), sealed with
AES-256-GCM. The vault's key is in the operator's keyring through Secret Service (GNOME
Keyring), never on disk. The daemon opens the vault at start and serves the values from
memory; nothing writes a value anywhere else.

A value goes in and never comes back out: there is no method here that hands a value to a
caller except at the point of use (``value``, and the mapping ``environ`` that
``ghost_core.apps.fill_secrets`` reads when an app connects), and every value is in the
redactor, so text from outside (an app's result, notice or error) and what Ghost stores or
logs is checked against it before anything reads it (``redact``).

Where the values live while the daemon runs:

- In its process environment, so Ghost's own settings (the model connection, Jev, the
  ElevenLabs voice, the Hugging Face download) read them where they always did. Nothing
  Ghost spawns inherits them: ``child_env`` leaves every password out.
- An app that runs on this computer gets the ones its ``env`` names as environment
  variables. That is the one place a value leaves the vault.

A locked keyring is not a failure: ``open`` says ``locked``, and the daemon keeps trying.
"""

from __future__ import annotations

import base64
import fcntl
import json
import os
import re
import tempfile
import threading
from collections.abc import Iterable, Iterator, Mapping, MutableMapping
from contextlib import contextmanager
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any, Literal, Protocol

from loguru import logger

VAULT_FILE = "passwords.vault"
NAME_RE = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")
MAX_VALUE_CHARS = 16_000
# A value shorter than this is not redacted: "1234" would turn every page number into a
# password. Real keys are far longer; a short one is still kept, only not looked for.
MIN_REDACT_CHARS = 8
# last_used is written back at most this often per password, so a busy app is not a write
# per call.
LAST_USED_EVERY = timedelta(minutes=10)
_AAD = b"ghost-passwords-v1"
_SCHEMES = ("Bearer ", "Basic ", "Token ", "token ", "bearer ")

State = Literal["open", "locked", "unavailable", "broken"]

# The passwords Ghost itself reads, and what for (shown as "used by"). Every other name is
# an app's (``${NAME}`` in apps.json) or a webhook's (``hooks.json``), or one the operator
# added by hand.
GHOST_USES: dict[str, str] = {
    # The model connection's keys, one per provider (ADR 0037); GHOST_LLM_PASSWORD may
    # name another, which the Passwords page shows as the model's too.
    "ANTHROPIC_AUTH_TOKEN": "The model connection",
    "ANTHROPIC_API_KEY": "The model connection",
    "OPENAI_API_KEY": "The model connection",
    "LLM_API_KEY": "The model connection",
    "OPENROUTER_API_KEY": "Jev, and the model when it goes through OpenRouter",
    "TYPESAFE_API_KEY": "Jev",
    "ELEVENLABS_API_KEY": "The ElevenLabs voice",
    "HF_TOKEN": "Voice model downloads",
    "GITHUB_TOKEN": "An app you added by hand that names ${GITHUB_TOKEN}",
}
_SECRET_SUFFIXES = ("_TOKEN", "_KEY", "_SECRET", "_PASSWORD", "_PAT", "_API_KEY")


def is_password_name(name: str, referenced: Iterable[str] = ()) -> bool:
    """Whether a KEY in a plaintext env file is a password: one Ghost reads, one an app or
    a webhook names, a key Ghost kept for an app, or a name shaped like one. Ghost's own
    settings (``GHOST_*``) are not, except an app's key (``GHOST_APP_*``)."""
    if name in GHOST_USES or name in set(referenced) or name.startswith("GHOST_APP_"):
        return True
    if name.startswith("GHOST_"):
        return False
    return name.endswith(_SECRET_SUFFIXES)


class KeyringLocked(Exception):
    """The keyring is there but locked (before login, or headless)."""


class KeyringUnavailable(Exception):
    """No Secret Service on this session bus (no keyring daemon, no bus)."""


class VaultBroken(Exception):
    """The vault file can't be read with the key the keyring holds. Never overwritten."""


class VaultClosed(Exception):
    """A change while the vault isn't open (locked, no keyring, or broken)."""


# --- the keyring -----------------------------------------------------------------------


class Keyring(Protocol):
    def load_key(self) -> bytes | None:
        """The vault's key, or None when the keyring has none yet. Raises
        ``KeyringLocked`` or ``KeyringUnavailable``."""
        ...

    def store_key(self, key: bytes) -> None: ...

    def unlock(self) -> bool:
        """Ask the keyring to unlock (it may show its own prompt). True when it is."""
        ...


class SecretServiceKeyring:
    """The vault's key in the default collection of Secret Service (GNOME Keyring's login
    keyring), as one item Ghost owns."""

    ATTRIBUTES = {"application": "ghost", "ghost": "passwords-vault-key"}
    LABEL = "Ghost passwords vault key"

    def _open(self) -> tuple[Any, Any]:
        try:
            import secretstorage
            from secretstorage.exceptions import SecretServiceNotAvailableException
        except ImportError as e:  # pragma: no cover - a dependency of ghost-core
            raise KeyringUnavailable("secretstorage is not installed") from e
        try:
            conn = secretstorage.dbus_init()
        except (SecretServiceNotAvailableException, OSError, ValueError) as e:
            raise KeyringUnavailable(f"no keyring on the session bus ({type(e).__name__})") from e
        try:
            collection = secretstorage.get_default_collection(conn)
        except Exception as e:  # noqa: BLE001 - the keyring daemon's own error
            conn.close()
            raise KeyringUnavailable(f"no default keyring ({type(e).__name__})") from e
        return conn, collection

    def load_key(self) -> bytes | None:
        conn, collection = self._open()
        try:
            if collection.is_locked():
                raise KeyringLocked("the keyring is locked")
            for item in collection.search_items(self.ATTRIBUTES):
                return base64.b64decode(item.get_secret())
            return None
        finally:
            conn.close()

    def store_key(self, key: bytes) -> None:
        conn, collection = self._open()
        try:
            if collection.is_locked():
                raise KeyringLocked("the keyring is locked")
            collection.create_item(
                self.LABEL,
                self.ATTRIBUTES,
                base64.b64encode(key),
                replace=True,
                content_type="text/plain",
            )
        finally:
            conn.close()

    def unlock(self) -> bool:
        conn, collection = self._open()
        try:
            if collection.is_locked():
                collection.unlock()  # the keyring's own prompt, in the operator's session
            return not collection.is_locked()
        finally:
            conn.close()


class MemoryKeyring:
    """A keyring for tests and tools: holds the key in memory, can be locked or absent."""

    def __init__(self, key: bytes | None = None, *, locked: bool = False, available: bool = True):
        self.key = key
        self.locked = locked
        self.available = available
        self.unlock_works = True

    def _check(self) -> None:
        if not self.available:
            raise KeyringUnavailable("no keyring")
        if self.locked:
            raise KeyringLocked("the keyring is locked")

    def load_key(self) -> bytes | None:
        self._check()
        return self.key

    def store_key(self, key: bytes) -> None:
        self._check()
        self.key = key

    def unlock(self) -> bool:
        if self.available and self.unlock_works:
            self.locked = False
        return self.available and not self.locked


# --- the file --------------------------------------------------------------------------


def _b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


def seal(key: bytes, payload: Mapping[str, Any]) -> bytes:
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM

    nonce = os.urandom(12)
    data = AESGCM(key).encrypt(nonce, json.dumps(payload).encode(), _AAD)
    envelope = {"ghost_passwords": 1, "cipher": "AES-256-GCM", "nonce": _b64(nonce)}
    return json.dumps({**envelope, "data": _b64(data)}).encode() + b"\n"


def unseal(key: bytes, raw: bytes) -> dict[str, Any]:
    from cryptography.exceptions import InvalidTag
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM

    try:
        envelope = json.loads(raw)
        if envelope.get("ghost_passwords") != 1 or envelope.get("cipher") != "AES-256-GCM":
            raise VaultBroken("not a vault Ghost can read")
        nonce = base64.b64decode(envelope["nonce"])
        data = base64.b64decode(envelope["data"])
    except (ValueError, KeyError, TypeError, AttributeError) as e:
        raise VaultBroken("the vault file is damaged") from e
    try:
        plain = AESGCM(key).decrypt(nonce, data, _AAD)
    except (InvalidTag, ValueError) as e:
        raise VaultBroken("the key in your keyring doesn't open this vault") from e
    payload = json.loads(plain)
    if not isinstance(payload, dict):
        raise VaultBroken("the vault file is damaged")
    return payload


def _when(raw: object) -> datetime | None:
    if not isinstance(raw, str) or not raw:
        return None
    try:
        return datetime.fromisoformat(raw)
    except ValueError:
        return None


@dataclass
class _Entry:
    value: str = field(repr=False)
    added: datetime
    updated: datetime
    last_used: datetime | None = None
    saved_use: datetime | None = None  # the last_used the file holds

    def dump(self) -> dict[str, str | None]:
        return {
            "value": self.value,
            "added": self.added.isoformat(),
            "updated": self.updated.isoformat(),
            "last_used": self.last_used.isoformat() if self.last_used else None,
        }


@dataclass(frozen=True)
class PasswordMeta:
    """What may be said about a password: never its value."""

    name: str
    added: datetime
    updated: datetime
    last_used: datetime | None


def _now() -> datetime:
    return datetime.now(UTC)


class Passwords:
    """The vault, open in memory.

    ``environ``: an environment kept in step with the vault (the daemon passes
    ``os.environ``, so Ghost's own settings read the values); None for a tool that only
    edits the file.

    More than one process may have the vault open (the daemon, ``ghostd check`` during a
    deploy, ``ghostd passwords``, ``ghostd perf``), and the daemon changes it from more than
    one thread (the import runs off the event loop). Every change holds the object's lock
    and an ``flock`` on ``passwords.vault.lock`` beside the file, reads the file again, then
    changes and saves, so no process writes over what another saved."""

    def __init__(
        self,
        path: Path,
        keyring: Keyring,
        *,
        environ: MutableMapping[str, str] | None = None,
    ) -> None:
        self.path = path
        self._keyring = keyring
        self._environ = environ
        self._entries: dict[str, _Entry] = {}
        self._key: bytes | None = None
        self.state: State = "locked"
        self.problem: str | None = "not opened yet"
        self._lock = threading.RLock()

    def __repr__(self) -> str:
        return f"Passwords({self.path}, {self.state}, {len(self._entries)} kept)"

    # --- opening -----------------------------------------------------------------------

    @property
    def is_open(self) -> bool:
        return self.state == "open"

    @property
    def keyring(self) -> Keyring:
        return self._keyring

    def open(self) -> bool:
        """Read the vault. True when it is open; a vault that doesn't exist yet is open and
        empty, and needs the keyring only when the first password is saved. On False,
        ``state`` and ``problem`` say why; nothing was read and nothing is written."""
        with self._lock:
            if not self.path.exists():
                self._entries = {}
                return self._opened()
            try:
                key = self._keyring.load_key()
                if key is None:
                    raise VaultBroken("your keyring has no key for this vault")
                self._entries = self._read(key)
            except KeyringLocked:
                return self._closed("locked", "Your keyring is locked")
            except KeyringUnavailable as e:
                return self._closed("unavailable", f"Ghost can't reach your keyring: {e}")
            except VaultBroken as e:
                return self._closed("broken", str(e))
            self._key = key
            return self._opened()

    def unlock(self) -> bool:
        """Ask the keyring to unlock, then open. True when the vault is open."""
        try:
            self._keyring.unlock()
        except (KeyringLocked, KeyringUnavailable) as e:
            logger.info(f"passwords: the keyring didn't unlock ({type(e).__name__})")
        except Exception as e:  # noqa: BLE001 - a dismissed prompt and the like
            logger.info(f"passwords: the keyring didn't unlock ({type(e).__name__})")
        return self.open()

    def _opened(self) -> bool:
        self.state, self.problem = "open", None
        self._publish()
        return True

    def _closed(self, state: State, problem: str) -> bool:
        if (state, problem) != (self.state, self.problem):  # once, not every retry
            logger.warning(f"passwords: vault not open: {problem}")
        self.state, self.problem = state, problem
        return False

    def _read(self, key: bytes) -> dict[str, _Entry]:
        payload = unseal(key, self.path.read_bytes())
        out: dict[str, _Entry] = {}
        for name, raw in (payload.get("passwords") or {}).items():
            if not isinstance(raw, dict) or not isinstance(raw.get("value"), str):
                continue
            added = _when(raw.get("added")) or _now()
            used = _when(raw.get("last_used"))
            out[name] = _Entry(
                value=raw["value"],
                added=added,
                updated=_when(raw.get("updated")) or added,
                last_used=used,
                saved_use=used,
            )
        return out

    # --- reading -----------------------------------------------------------------------

    def __contains__(self, name: object) -> bool:
        return name in self._entries

    def __len__(self) -> int:
        return len(self._entries)

    def names(self) -> list[str]:
        with self._lock:
            return sorted(self._entries)

    def info(self) -> list[PasswordMeta]:
        with self._lock:
            return [
                PasswordMeta(n, e.added, e.updated, e.last_used)
                for n, e in sorted(self._entries.items())
            ]

    def holds(self, name: str, value: str) -> bool:
        """Whether the vault holds exactly ``value`` under ``name``: a check (the import's
        read-back), not a use, and never the value itself."""
        import hmac

        with self._lock:
            e = self._entries.get(name)
        return e is not None and hmac.compare_digest(e.value.encode(), value.encode())

    def value(self, name: str) -> str | None:
        """The value, at its point of use: an app's header or environment, a webhook's
        check. Notes the use."""
        e = self._entries.get(name)
        if e is None:
            return None
        self.note_used(name)
        return e.value

    def note_used(self, *names: str) -> None:
        """Ghost filled these in just now. Kept for "last used"; written back now and then,
        over the file as it is now (another process may have changed it)."""
        now = _now()
        due = False
        with self._lock:
            for name in names:
                e = self._entries.get(name)
                if e is None:
                    continue
                e.last_used = now
                if e.saved_use is None or now - e.saved_use >= LAST_USED_EVERY:
                    due = True
            if not (due and self.is_open and self._key is not None):
                return
            try:
                with self._file_lock():
                    self._refresh()
                    self._save()
            except Exception as e:  # noqa: BLE001 - "last used" is never worth a failure
                logger.debug(f"passwords: last used not saved ({type(e).__name__})")

    def environ(self, base: Mapping[str, str] | None = None) -> Mapping[str, str]:
        """An environment for ``fill_secrets`` and the webhook checks: the vault's values
        first (each read noted as a use), then ``base`` (``os.environ``) for the rest."""
        return _VaultEnviron(self, os.environ if base is None else base)

    # --- changing ----------------------------------------------------------------------

    def set(self, name: str, value: str) -> None:
        self.set_many({name: value})

    def set_many(self, values: Mapping[str, str]) -> None:
        """Save passwords, replacing any with the same name. One write for all of them."""
        for name, value in values.items():
            check_name(name)
            check_value(value)
        with self._lock:
            self._require_open()
            with self._file_lock():
                self._refresh()
                now = _now()
                for name, value in values.items():
                    e = self._entries.get(name)
                    if e is None:
                        self._entries[name] = _Entry(value=value, added=now, updated=now)
                    elif e.value != value:
                        e.value, e.updated = value, now
                self._save()
            if self._environ is not None:
                self._environ.update(values)

    def delete(self, *names: str) -> list[str]:
        """Forget passwords. Returns the names that were there."""
        with self._lock:
            self._require_open()
            with self._file_lock():
                self._refresh()
                gone = [n for n in names if n in self._entries]
                if not gone:
                    return []
                for n in gone:
                    del self._entries[n]
                self._save()
            if self._environ is not None:
                for n in gone:
                    self._environ.pop(n, None)
            return gone

    def _require_open(self) -> None:
        if not self.is_open:
            raise VaultClosed(self.problem or "the vault isn't open")

    @contextmanager
    def _file_lock(self) -> Iterator[None]:
        """An exclusive ``flock`` on ``<vault>.lock``, held across a read, a change and a
        save, so two processes' changes don't write over each other."""
        lock = self.path.with_name(self.path.name + ".lock")
        lock.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd = os.open(lock, os.O_RDWR | os.O_CREAT | os.O_CLOEXEC, 0o600)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX)
            yield
        finally:
            os.close(fd)  # closing releases the lock

    def _refresh(self) -> None:
        """Read the file again before a change (under ``_file_lock``): another process may
        have written it since it was read here. The uses noted here are kept; a password
        another process forgot is forgotten here too."""
        if not self.path.exists():
            return
        try:
            key = self._key or self._keyring.load_key()
            if key is None:
                raise VaultBroken("your keyring has no key for this vault")
            fresh = self._read(key)
        except KeyringLocked as e:
            raise VaultClosed("Your keyring is locked") from e
        except (KeyringUnavailable, VaultBroken) as e:
            raise VaultClosed(str(e)) from e
        self._key = key
        for name, e in fresh.items():
            mine = self._entries.get(name)
            if (
                mine is not None
                and mine.last_used
                and (e.last_used is None or mine.last_used > e.last_used)
            ):
                e.last_used = mine.last_used
        if self._environ is not None:
            for name in set(self._entries) - set(fresh):
                self._environ.pop(name, None)
        self._entries = fresh

    def _key_for_write(self) -> bytes:
        if self._key is not None:
            return self._key
        try:
            key = self._keyring.load_key()
            if key is None:
                if self.path.exists():  # never seal over a vault another key opens
                    raise VaultClosed("your keyring has no key for this vault")
                from cryptography.hazmat.primitives.ciphers.aead import AESGCM

                key = AESGCM.generate_key(bit_length=256)
                self._keyring.store_key(key)
        except KeyringLocked as e:
            self._closed("locked", "Your keyring is locked")
            raise VaultClosed("Your keyring is locked") from e
        except KeyringUnavailable as e:
            self._closed("unavailable", f"Ghost can't reach your keyring: {e}")
            raise VaultClosed(str(e)) from e
        self._key = key
        return key

    def _save(self) -> None:
        key = self._key_for_write()
        payload = {"passwords": {n: e.dump() for n, e in sorted(self._entries.items())}}
        sealed = seal(key, payload)
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd, tmp = tempfile.mkstemp(dir=self.path.parent, prefix=f".{self.path.name}.")
        try:
            os.fchmod(fd, 0o600)
            with os.fdopen(fd, "wb") as f:
                f.write(sealed)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp, self.path)
        except BaseException:
            Path(tmp).unlink(missing_ok=True)
            raise
        for e in self._entries.values():
            e.saved_use = e.last_used
        self._publish()

    # --- what the rest of the process knows --------------------------------------------

    def _publish(self) -> None:
        """Every value into the redactor, and into the kept environment."""
        values = {n: e.value for n, e in self._entries.items()}
        _REDACTOR.use("vault", values)
        if self._environ is not None:
            self._environ.update(values)


class _VaultEnviron(Mapping[str, str]):
    def __init__(self, vault: Passwords, base: Mapping[str, str]) -> None:
        self._vault = vault
        self._base = base

    def __getitem__(self, key: str) -> str:
        v = self._vault.value(key)
        if v is not None:
            return v
        return self._base[key]

    def __contains__(self, key: object) -> bool:
        return key in self._vault or key in self._base

    def get(self, key: str, default: Any = None) -> Any:
        return self[key] if key in self else default

    def __iter__(self) -> Iterator[str]:
        seen = set(self._vault.names())
        yield from seen
        yield from (k for k in self._base if k not in seen)

    def __len__(self) -> int:
        return len(set(self._vault.names()) | set(self._base))


def check_name(name: str) -> None:
    if not NAME_RE.match(name):
        raise ValueError(
            "a password's name is capital letters, digits and _, starting with a letter"
        )


def check_value(value: str) -> None:
    if not value or not value.strip():
        raise ValueError("a password can't be empty")
    if len(value) > MAX_VALUE_CHARS:
        raise ValueError(f"a password is at most {MAX_VALUE_CHARS} characters")
    if "\x00" in value or "\n" in value:
        raise ValueError("a password is one line")


# --- redaction -------------------------------------------------------------------------


class _Redactor:
    """Every password value this process holds, longest first, so a value that contains
    another is replaced whole. Deterministic: plain substring matches, no model."""

    def __init__(self) -> None:
        self._sets: dict[str, dict[str, str]] = {}
        self._pairs: tuple[tuple[str, str], ...] = ()

    def use(self, source: str, values: Mapping[str, str]) -> None:
        self._sets[source] = dict(values)
        pairs: dict[str, str] = {}
        for vals in self._sets.values():
            for name, value in vals.items():
                for v in _forms(value):
                    if len(v) >= MIN_REDACT_CHARS:
                        pairs.setdefault(v, name)
        self._pairs = tuple(sorted(pairs.items(), key=lambda p: -len(p[0])))

    def names(self) -> set[str]:
        return {n for vals in self._sets.values() for n in vals}

    def text(self, s: str) -> str:
        for value, name in self._pairs:
            if value in s:
                s = s.replace(value, f"[password: {name}]")
        return s

    def __call__(self, obj: Any) -> Any:
        if not self._pairs:
            return obj
        return self._walk(obj)

    def _walk(self, obj: Any) -> Any:
        if isinstance(obj, str):
            return self.text(obj)
        if isinstance(obj, Mapping):
            return {
                self._walk(k) if isinstance(k, str) else k: self._walk(v) for k, v in obj.items()
            }
        if isinstance(obj, list):
            return [self._walk(v) for v in obj]
        if isinstance(obj, tuple):
            return tuple(self._walk(v) for v in obj)
        return obj


def _forms(value: str) -> list[str]:
    """The value, and the token inside one kept with its scheme (``Bearer abc…`` is also
    looked for as ``abc…``, which is what an app would echo)."""
    out = [value]
    for scheme in _SCHEMES:
        if value.startswith(scheme):
            out.append(value[len(scheme) :].strip())
    return out


_REDACTOR = _Redactor()


def redact(obj: Any) -> Any:
    """``obj`` with every password value this process holds replaced by
    ``[password: NAME]``: strings, and the strings inside dicts, lists and tuples."""
    return _REDACTOR(obj)


def redact_text(s: str) -> str:
    return _REDACTOR.text(s) if _REDACTOR._pairs else s


def also_redact(source: str, values: Mapping[str, str]) -> None:
    """Values that are passwords but aren't in the vault (a plaintext file not imported
    yet): redacted, and left out of ``child_env``, all the same."""
    _REDACTOR.use(source, values)


def password_names() -> set[str]:
    """The names of every password this process holds."""
    return _REDACTOR.names()


def child_env(base: Mapping[str, str] | None = None) -> dict[str, str]:
    """An environment for a process Ghost starts: its own, less every password."""
    held = password_names()
    return {k: v for k, v in (os.environ if base is None else base).items() if k not in held}


__all__ = [
    "GHOST_USES",
    "KeyringLocked",
    "KeyringUnavailable",
    "MemoryKeyring",
    "PasswordMeta",
    "Passwords",
    "SecretServiceKeyring",
    "VAULT_FILE",
    "VaultBroken",
    "VaultClosed",
    "also_redact",
    "check_name",
    "child_env",
    "is_password_name",
    "password_names",
    "redact",
    "redact_text",
]
