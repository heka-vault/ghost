from __future__ import annotations

from typing import Literal

import pytest
from ghost_domain import RepoPolicy, Snapshot, Store, Target, WriteGate


class FakeWriter:
    """In-memory GitHub. Records calls; can be told to fail a step."""

    def __init__(self) -> None:
        self.issues: dict[Target, dict] = {}
        self.comments: dict[Target, list[tuple[str, str]]] = {}
        self.calls: list[tuple] = []
        self.fail_on: str | None = None

    def seed(self, target: Target, **fields) -> None:
        self.issues[target] = {
            "title": "Add login",
            "state": "open",
            "labels": ["needs-signoff"],
            "updated_at": "2026-10-16T00:00:00Z",
        } | fields

    def _touch(self, target: Target) -> None:
        self.issues[target]["updated_at"] = self.issues[target]["updated_at"] + "+"

    async def fetch(self, target: Target) -> Snapshot:
        self.calls.append(("fetch", target))
        i = self.issues[target]
        return Snapshot(
            title=i["title"],
            state=i["state"],
            labels=tuple(i["labels"]),
            updated_at=i["updated_at"],
        )

    async def set_labels(self, target, add, remove) -> None:
        self.calls.append(("set_labels", target, add, remove))
        if self.fail_on == "set_labels":
            raise RuntimeError("422 label does not exist")
        labels = [x for x in self.issues[target]["labels"] if x not in remove]
        self.issues[target]["labels"] = labels + [a for a in add if a not in labels]
        self._touch(target)

    async def add_comment(self, target, body, marker) -> None:
        self.calls.append(("add_comment", target, body, marker))
        if self.fail_on == "add_comment":
            raise RuntimeError("500 from GitHub")
        existing = self.comments.setdefault(target, [])
        if any(m == marker for _, m in existing):
            return
        existing.append((body, marker))
        self._touch(target)

    async def set_state(self, target, state: Literal["open", "closed"]) -> None:
        self.calls.append(("set_state", target, state))
        self.issues[target]["state"] = state
        self._touch(target)


@pytest.fixture
def target() -> Target:
    return Target(owner="acme", repo="app", number=42)


@pytest.fixture
def writer(target) -> FakeWriter:
    w = FakeWriter()
    w.seed(target)
    return w


@pytest.fixture
def gate(writer) -> WriteGate:
    return WriteGate(Store(), writer, RepoPolicy.from_csv("acme/app"))
