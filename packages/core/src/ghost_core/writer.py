"""The one interface the gate uses to touch the world. Provided by an ability."""

from __future__ import annotations

from typing import Protocol

from ghost_core.models import Change, Snapshot, Target


class Writer(Protocol):
    async def fetch(self, target: Target) -> Snapshot: ...

    async def apply(self, target: Target, change: Change, idempotency_key: str) -> None:
        """Apply one change. ``idempotency_key`` is unique per proposal; a writer that
        creates something (a comment, a message) must embed it so a retry never
        creates a second copy, and must be able to find it again for undo."""
        ...
