"""What Ghost hands an ability at setup. Attributes, not a container: for one operator
and a dozen abilities, a dataclass is enough and is greppable."""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

import httpx
from loguru import logger

from ghost_core.models import utcnow
from ghost_core.store import Store


class MissingSecret(KeyError):
    pass


class Secrets:
    """The one door to secrets. Values come from the environment, which the operator's
    secrets store renders; they are never logged."""

    def __init__(self, env: Mapping[str, str]) -> None:
        self._env = env

    def has(self, name: str) -> bool:
        return bool(self._env.get(name))

    def get(self, name: str) -> str:
        value = self._env.get(name)
        if not value:
            raise MissingSecret(f"{name} is not set in the environment")
        return value


class AbilityStore:
    """A key/value namespace in Ghost's SQLite for one ability."""

    def __init__(self, store: Store, ability: str) -> None:
        self._store = store
        self._ability = ability

    def get(self, key: str) -> str | None:
        return self._store.kv_get(self._ability, key)

    def set(self, key: str, value: str) -> None:
        self._store.kv_set(self._ability, key, value)

    def delete(self, key: str) -> None:
        self._store.kv_delete(self._ability, key)

    def items(self) -> dict[str, str]:
        return self._store.kv_items(self._ability)


@dataclass(frozen=True)
class GhostContext:
    ability: str
    data_dir: Path  # per-ability, created
    cache_dir: Path  # per-ability, created
    store: AbilityStore
    secrets: Secrets
    http: Callable[[], httpx.AsyncClient]  # shared timeouts and user agent
    log: Any  # loguru logger bound with ability=name
    debug_audio: Callable[[str], None]  # ghost_voice.debug.audio: silent unless the flag is on
    now: Callable[[], datetime] = utcnow

    @classmethod
    def build(
        cls,
        ability: str,
        *,
        data_root: Path,
        cache_root: Path,
        store: Store,
        env: Mapping[str, str],
        debug_audio: Callable[[str], None] | None = None,
        http_timeout: float = 10.0,
    ) -> GhostContext:
        data_dir = data_root / ability
        cache_dir = cache_root / ability
        data_dir.mkdir(parents=True, exist_ok=True)
        cache_dir.mkdir(parents=True, exist_ok=True)

        def http() -> httpx.AsyncClient:
            return httpx.AsyncClient(
                timeout=http_timeout, headers={"User-Agent": f"ghost/{ability}"}
            )

        return cls(
            ability=ability,
            data_dir=data_dir,
            cache_dir=cache_dir,
            store=AbilityStore(store, ability),
            secrets=Secrets(env),
            http=http,
            log=logger.bind(ability=ability),
            debug_audio=debug_audio or (lambda _m: None),
        )
