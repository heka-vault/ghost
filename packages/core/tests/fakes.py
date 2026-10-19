"""An issue-like write model for core tests, independent of any ability."""

from __future__ import annotations

from typing import Literal

from ghost_core import (
    Change,
    ChangeSet,
    Operation,
    Prepared,
    Snapshot,
    Target,
    WriteGate,
    number_mentions,
)


class Issue(Target, frozen=True):
    owner: str
    repo: str
    number: int

    @property
    def full_name(self) -> str:
        return f"{self.owner}/{self.repo}"

    def key(self) -> str:
        return f"fake:{self.full_name}#{self.number}"

    def scope(self) -> str:
        return self.full_name

    def spoken(self) -> str:
        return f"{self.full_name}#{self.number}"

    def mentions(self) -> set[str]:
        return number_mentions(self.number)


class IssueSnap(Snapshot, frozen=True):
    title: str
    state: Literal["open", "closed"]
    labels: tuple[str, ...]

    def spoken(self) -> str:
        return f"{self.state}, labels {', '.join(self.labels) or 'none'}"


class SetLabels(Change, frozen=True):
    add: tuple[str, ...] = ()
    remove: tuple[str, ...] = ()

    def describe(self) -> str:
        parts = []
        if self.add:
            parts.append("add " + ", ".join(self.add))
        if self.remove:
            parts.append("remove " + ", ".join(self.remove))
        return "labels: " + "; ".join(parts)


class AddComment(Change, frozen=True):
    body: str

    def describe(self) -> str:
        return f"comment: {self.body}"


class SetState(Change, frozen=True):
    state: Literal["open", "closed"]

    def describe(self) -> str:
        return f"{self.state} the issue"


class FakeWriter:
    """In-memory issues. Records calls; can be told to fail a step."""

    def __init__(self) -> None:
        self.issues: dict[Issue, dict] = {}
        self.comments: dict[Issue, list[tuple[str, str]]] = {}
        self.calls: list[tuple] = []
        self.fail_on: str | None = None

    def seed(self, target: Issue, **fields) -> None:
        self.issues[target] = {
            "title": "Add login",
            "state": "open",
            "labels": ["needs-signoff"],
            "updated_at": "2026-10-16T00:00:00Z",
        } | fields

    def _touch(self, target: Issue) -> None:
        self.issues[target]["updated_at"] += "+"

    async def fetch(self, target):
        self.calls.append(("fetch", target))
        i = self.issues[target]
        return IssueSnap(
            version=i["updated_at"], title=i["title"], state=i["state"], labels=tuple(i["labels"])
        )

    async def apply(self, target, change, idempotency_key):
        match change:
            case SetLabels(add=add, remove=remove):
                self.calls.append(("set_labels", target, add, remove))
                if self.fail_on == "set_labels":
                    raise RuntimeError("422 label does not exist")
                labels = [x for x in self.issues[target]["labels"] if x not in remove]
                self.issues[target]["labels"] = labels + [a for a in add if a not in labels]
            case AddComment(body=body):
                self.calls.append(("add_comment", target, body, idempotency_key))
                if self.fail_on == "add_comment":
                    raise RuntimeError("500 from GitHub")
                existing = self.comments.setdefault(target, [])
                if any(m == idempotency_key for _, m in existing):
                    return
                existing.append((body, idempotency_key))
            case SetState(state=state):
                self.calls.append(("set_state", target, state))
                self.issues[target]["state"] = state
        self._touch(target)


RISKS = {
    "approve": "low",
    "set_labels": "low",
    "comment": "low",
    "close": "high",
    "reopen": "medium",
}


ASKED_BY = {"close": ("close", "shut")}  # approve declares none: it always asks


def register_fake(gate: WriteGate, writer: FakeWriter, allowed_repo: str = "acme/app") -> None:
    async def prepare(args):
        return Prepared(target=args["target"], changes=args["changes"])

    gate.register(
        "github_issues",
        [
            Operation(
                name=name,
                description=name,
                properties={},
                required=[],
                prepare=prepare,
                writer=writer,
                risk=risk,  # type: ignore[arg-type]
                allowed=lambda t, r=allowed_repo: isinstance(t, Issue) and t.full_name == r,
                asked_by=ASKED_BY.get(name, ()),
            )
            for name, risk in RISKS.items()
        ],
    )


def changeset(*steps) -> ChangeSet:
    return ChangeSet(steps=tuple(steps))
