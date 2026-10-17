"""The one interface the gate uses to touch GitHub. Implemented by a skill."""

from __future__ import annotations

from typing import Literal, Protocol

from ghost_domain.models import Snapshot, Target


class IssueWriter(Protocol):
    async def fetch(self, target: Target) -> Snapshot: ...

    async def set_labels(
        self, target: Target, add: tuple[str, ...], remove: tuple[str, ...]
    ) -> None: ...

    async def add_comment(self, target: Target, body: str, marker: str) -> None:
        """``marker`` is a hidden idempotency token. Implementations must embed it in
        the comment and must not post a second comment carrying the same marker."""
        ...

    async def set_state(self, target: Target, state: Literal["open", "closed"]) -> None: ...
